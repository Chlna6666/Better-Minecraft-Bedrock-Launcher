//! Combined read-only storage for pre-LevelDB Pocket Edition world folders.
//!
//! `chunks.dat` terrain is exposed exactly as persisted: the historical 82,176-byte core remains
//! 82,176 bytes and is never padded with invented biome/RGB samples. The `Entities` and
//! `TileEntities` lists from `entities.dat` are exposed as virtual legacy `Entity` and `BlockEntity`
//! chunk records so normal [`crate::world::BedrockWorld`] query APIs can inspect the complete old
//! world without caller-side file plumbing.

use super::pocket_entities_dat::read_pocket_entities_dat;
use crate::chunk::{
    ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LEGACY_TERRAIN_VALUE_LEN,
    POCKET_TERRAIN_VALUE_LEN,
};
use crate::database::{
    StorageBatch, StorageReadOptions, StorageScanOutcome, StorageVisitorControl, WorldStorage,
};
use crate::error::{BedrockWorldError, Result};
use crate::level::read_level_dat_document;
use crate::nbt::{NbtTag, serialize_root_nbt};
use bytes::Bytes;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

const CONFIRMED_ENTITIES_DAT_VERSION: i32 = 1;
const CHUNKS_DAT_LOCATION_TABLE_LEN: usize = 4 * 32 * 32;
const CHUNKS_DAT_SECTOR_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub(crate) struct PocketWorldStorage {
    values: Arc<BTreeMap<Vec<u8>, Bytes>>,
}

impl PocketWorldStorage {
    pub(crate) fn open(world_path: impl AsRef<Path>) -> Result<Self> {
        let world_path = world_path.as_ref();
        let mut values = read_terrain_records(world_path)?;
        if world_path.join("entities.dat").is_file() {
            for (key, value) in build_entity_records(world_path)? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "Pocket world sidecar record collides with an existing chunks.dat key: {key:02x?}"
                    )));
                }
            }
        }
        log::debug!(
            "opened pre-LevelDB Pocket world storage (records={}, path={})",
            values.len(),
            world_path.display()
        );
        Ok(Self {
            values: Arc::new(values),
        })
    }
}

impl WorldStorage for PocketWorldStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        Ok(self.values.get(key).cloned())
    }

    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        Ok(keys
            .iter()
            .map(|key| self.values.get(key.as_ref()).cloned())
            .collect())
    }

    fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn delete(&self, _key: &[u8]) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn for_each_key(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in self.values.iter() {
            check_cancelled(&options, "Pocket world key scan")?;
            outcome.record(value.len());
            outcome.worker_threads = outcome.worker_threads.max(1);
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                break;
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn for_each_prefix(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in self
            .values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options, "Pocket world prefix scan")?;
            outcome.record(value.len());
            outcome.worker_threads = outcome.worker_threads.max(1);
            if visitor(key, value)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                break;
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }

    fn compact(&self) -> Result<()> {
        Ok(())
    }
}

fn read_terrain_records(world_path: &Path) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    let chunks_path = world_path.join("chunks.dat");
    let bytes = fs::read(&chunks_path)?;
    if bytes.len() < CHUNKS_DAT_LOCATION_TABLE_LEN {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "chunks.dat is too small for its 32x32 location table: {} bytes",
            bytes.len()
        )));
    }

    let (origin_chunk_x, origin_chunk_z) = read_limited_world_origin(world_path);
    let mut values = BTreeMap::new();
    for index in 0..(32 * 32) {
        let entry_offset = index * 4;
        let entry = &bytes[entry_offset..entry_offset + 4];
        if entry == [0, 0, 0, 0] {
            continue;
        }

        let sector_count = usize::from(entry[0]);
        let sector_offset = usize::from(entry[1])
            | (usize::from(entry[2]) << 8)
            | (usize::from(entry[3]) << 16);
        if sector_count == 0 || sector_offset == 0 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "chunks.dat entry {index} has invalid sector offset/count ({sector_offset}, {sector_count})"
            )));
        }
        let byte_offset = sector_offset
            .checked_mul(CHUNKS_DAT_SECTOR_BYTES)
            .ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "chunks.dat entry {index} sector offset overflows byte offset"
                ))
            })?;
        let payload = pocket_chunk_payload(&bytes, byte_offset, sector_count).map_err(|message| {
            BedrockWorldError::CorruptWorld(format!("chunks.dat entry {index}: {message}"))
        })?;

        let local_x = i32::try_from(index % 32).expect("0..31 fits i32");
        let local_z = i32::try_from(index / 32).expect("0..31 fits i32");
        let pos = ChunkPos {
            x: origin_chunk_x.saturating_add(local_x),
            z: origin_chunk_z.saturating_add(local_z),
            dimension: Dimension::Overworld,
        };
        values.insert(
            ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain)
                .encode()
                .to_vec(),
            Bytes::copy_from_slice(payload),
        );
    }
    Ok(values)
}

fn pocket_chunk_payload(
    bytes: &[u8],
    byte_offset: usize,
    sector_count: usize,
) -> std::result::Result<&[u8], String> {
    let sector_bytes = sector_count
        .checked_mul(CHUNKS_DAT_SECTOR_BYTES)
        .ok_or_else(|| "sector byte length overflows usize".to_string())?;
    let max_end = byte_offset
        .checked_add(sector_bytes)
        .ok_or_else(|| "sector end offset overflows usize".to_string())?;
    if byte_offset >= bytes.len() || max_end > bytes.len() {
        return Err(format!(
            "sector range {byte_offset}..{max_end} exceeds file length {}",
            bytes.len()
        ));
    }
    let available = &bytes[byte_offset..max_end];

    // Some historical writers prefix the terrain payload with a little-endian byte length. Preserve
    // exactly the declared representation when it is one of the two layouts the library understands.
    if available.len() >= 4 {
        let declared_len = u32::from_le_bytes(
            available[..4]
                .try_into()
                .expect("four-byte prefix already checked"),
        ) as usize;
        if matches!(declared_len, POCKET_TERRAIN_VALUE_LEN | LEGACY_TERRAIN_VALUE_LEN) {
            let end = 4usize
                .checked_add(declared_len)
                .ok_or_else(|| "declared terrain length overflows sector range".to_string())?;
            if end > available.len() {
                return Err(format!(
                    "declares {declared_len} terrain bytes but only {} bytes follow the length prefix",
                    available.len().saturating_sub(4)
                ));
            }
            return Ok(&available[4..end]);
        }
    }

    // The confirmed pre-LevelDB Pocket layout is an unprefixed 82,176-byte terrain core. Sector
    // padding is not data and is deliberately not exposed as a synthetic LevelDB biome tail.
    if available.len() < POCKET_TERRAIN_VALUE_LEN {
        return Err(format!(
            "terrain payload is shorter than the confirmed {POCKET_TERRAIN_VALUE_LEN}-byte core: {} bytes available",
            available.len()
        ));
    }
    Ok(&available[..POCKET_TERRAIN_VALUE_LEN])
}

fn read_limited_world_origin(world_path: &Path) -> (i32, i32) {
    let Ok(document) = read_level_dat_document(&world_path.join("level.dat")) else {
        return (0, 0);
    };
    let NbtTag::Compound(root) = document.root else {
        return (0, 0);
    };
    (
        nbt_i32(root.get("LimitedWorldOriginX")).unwrap_or(0),
        nbt_i32(root.get("LimitedWorldOriginZ")).unwrap_or(0),
    )
}

fn nbt_i32(tag: Option<&NbtTag>) -> Option<i32> {
    match tag {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn build_entity_records(world_path: &Path) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    let document = read_pocket_entities_dat(world_path)?;
    if document.version() != CONFIRMED_ENTITIES_DAT_VERSION {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "entities.dat version {} is not the confirmed Pocket Edition version {}",
            document.version(),
            CONFIRMED_ENTITIES_DAT_VERSION
        )));
    }

    let mut entities = BTreeMap::<ChunkPos, Vec<&NbtTag>>::new();
    let mut block_entities = BTreeMap::<ChunkPos, Vec<&NbtTag>>::new();
    for (index, entity) in document.entities()?.iter().enumerate() {
        let pos = entity_chunk_pos(entity)?.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "entities.dat Entities[{index}] has no usable Pos"
            ))
        })?;
        entities.entry(pos).or_default().push(entity);
    }
    for (index, block_entity) in document.tile_entities()?.iter().enumerate() {
        let pos = block_entity_chunk_pos(block_entity)?.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "entities.dat TileEntities[{index}] has no usable x/z"
            ))
        })?;
        block_entities.entry(pos).or_default().push(block_entity);
    }

    let mut values = BTreeMap::new();
    for (pos, roots) in entities {
        values.insert(
            ChunkKey::new(pos, ChunkRecordTag::Entity).encode().to_vec(),
            serialize_consecutive_roots(&roots)?,
        );
    }
    for (pos, roots) in block_entities {
        values.insert(
            ChunkKey::new(pos, ChunkRecordTag::BlockEntity)
                .encode()
                .to_vec(),
            serialize_consecutive_roots(&roots)?,
        );
    }
    Ok(values)
}

fn entity_chunk_pos(entity: &NbtTag) -> Result<Option<ChunkPos>> {
    let NbtTag::Compound(root) = entity else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat entity is not an NBT compound".to_string(),
        ));
    };
    let Some(NbtTag::List(pos)) = root.get("Pos") else {
        return Ok(None);
    };
    if pos.len() < 3 {
        return Ok(None);
    }
    let Some(x) = numeric_value(&pos[0]).and_then(floor_i32) else {
        return Ok(None);
    };
    let Some(z) = numeric_value(&pos[2]).and_then(floor_i32) else {
        return Ok(None);
    };
    Ok(Some(ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
        dimension: Dimension::Overworld,
    }))
}

fn block_entity_chunk_pos(block_entity: &NbtTag) -> Result<Option<ChunkPos>> {
    let NbtTag::Compound(root) = block_entity else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat tile entity is not an NBT compound".to_string(),
        ));
    };
    let Some(x) = integer_value(root.get("x")) else {
        return Ok(None);
    };
    let Some(z) = integer_value(root.get("z")) else {
        return Ok(None);
    };
    Ok(Some(ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
        dimension: Dimension::Overworld,
    }))
}

fn numeric_value(tag: &NbtTag) -> Option<f64> {
    let value = match tag {
        NbtTag::Byte(value) => f64::from(*value),
        NbtTag::Short(value) => f64::from(*value),
        NbtTag::Int(value) => f64::from(*value),
        NbtTag::Long(value) => *value as f64,
        NbtTag::Float(value) => f64::from(*value),
        NbtTag::Double(value) => *value,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn floor_i32(value: f64) -> Option<i32> {
    let value = value.floor();
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        None
    } else {
        Some(value as i32)
    }
}

fn integer_value(tag: Option<&NbtTag>) -> Option<i32> {
    match tag? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn serialize_consecutive_roots(roots: &[&NbtTag]) -> Result<Bytes> {
    let mut raw = Vec::new();
    for root in roots {
        raw.extend_from_slice(&serialize_root_nbt(root)?);
    }
    Ok(Bytes::from(raw))
}

fn check_cancelled(options: &StorageReadOptions, operation: &'static str) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        Err(BedrockWorldError::Cancelled { operation })
    } else {
        Ok(())
    }
}

fn emit_progress(options: &StorageReadOptions, outcome: StorageScanOutcome) {
    if let Some(progress) = &options.progress {
        let interval = options.pipeline.progress_interval.max(1);
        if outcome.visited.is_multiple_of(interval) {
            progress.emit(crate::database::StorageScanProgress {
                entries_seen: outcome.visited,
                bytes_read: outcome.bytes_read,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprefixed_pocket_terrain_is_not_padded_with_fake_biomes() {
        let mut sector = vec![0_u8; POCKET_TERRAIN_VALUE_LEN + 32];
        sector[POCKET_TERRAIN_VALUE_LEN..].fill(0xCC);
        let payload = pocket_chunk_payload(&sector, 0, 21).unwrap_err();
        assert!(payload.contains("exceeds file length"));

        let mut sectors = vec![0_u8; 21 * CHUNKS_DAT_SECTOR_BYTES];
        sectors[..POCKET_TERRAIN_VALUE_LEN].fill(7);
        let payload = pocket_chunk_payload(&sectors, 0, 21).unwrap();
        assert_eq!(payload.len(), POCKET_TERRAIN_VALUE_LEN);
        let terrain = crate::chunk::LegacyTerrain::parse(Bytes::copy_from_slice(payload)).unwrap();
        assert!(!terrain.has_biome_samples());
        assert!(terrain.biomes().is_empty());
    }

    #[test]
    fn length_prefixed_full_legacy_terrain_keeps_real_biome_tail() {
        let mut sectors = vec![0_u8; 21 * CHUNKS_DAT_SECTOR_BYTES];
        sectors[..4].copy_from_slice(&(LEGACY_TERRAIN_VALUE_LEN as u32).to_le_bytes());
        sectors[4..4 + LEGACY_TERRAIN_VALUE_LEN].fill(9);
        let payload = pocket_chunk_payload(&sectors, 0, 21).unwrap();
        assert_eq!(payload.len(), LEGACY_TERRAIN_VALUE_LEN);
        let terrain = crate::chunk::LegacyTerrain::parse(Bytes::copy_from_slice(payload)).unwrap();
        assert!(terrain.has_biome_samples());
    }
}
