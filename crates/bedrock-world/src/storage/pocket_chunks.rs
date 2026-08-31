//! Read-only pre-LevelDB Pocket Edition `chunks.dat` terrain backend.
//!
//! This backend exists for the generic world opener. It deliberately preserves the historical
//! 82,176-byte terrain core instead of padding it to the later 83,200-byte LevelDB `LegacyTerrain`
//! shape. Full Pocket world opening, including `entities.dat`, is composed by the world layer.

use super::{
    StorageBatch, StorageReadOptions, StorageScanOutcome, StorageScanProgress, StorageVisitorControl,
    WorldStorage,
};
use crate::chunk::{
    ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LEGACY_TERRAIN_VALUE_LEN,
    POCKET_TERRAIN_VALUE_LEN,
};
use crate::error::{BedrockWorldError, Result};
use crate::level::read_level_dat_document;
use crate::nbt::NbtTag;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

const LOCATION_TABLE_LEN: usize = 4 * 32 * 32;
const SECTOR_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub(crate) struct PocketChunksDatStorage {
    values: Arc<BTreeMap<Vec<u8>, Bytes>>,
    origin_chunk_x: i32,
    origin_chunk_z: i32,
}

impl PocketChunksDatStorage {
    pub(crate) fn open(world_path: impl AsRef<Path>) -> Result<Self> {
        let world_path = world_path.as_ref();
        let chunks_path = world_path.join("chunks.dat");
        let bytes = fs::read(&chunks_path)?;
        let (origin_chunk_x, origin_chunk_z) = read_limited_world_origin(world_path);
        let values = parse_chunks_dat(&bytes, origin_chunk_x, origin_chunk_z)?;
        log::debug!(
            "opened Pocket chunks.dat without format synthesis (chunks={}, origin=({}, {}), path={})",
            values.len(),
            origin_chunk_x,
            origin_chunk_z,
            chunks_path.display()
        );
        Ok(Self {
            values: Arc::new(values),
            origin_chunk_x,
            origin_chunk_z,
        })
    }

    #[must_use]
    pub(crate) const fn origin_chunk_x(&self) -> i32 {
        self.origin_chunk_x
    }

    #[must_use]
    pub(crate) const fn origin_chunk_z(&self) -> i32 {
        self.origin_chunk_z
    }
}

impl WorldStorage for PocketChunksDatStorage {
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
            check_cancelled(&options)?;
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
            check_cancelled(&options)?;
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

fn parse_chunks_dat(
    bytes: &[u8],
    origin_chunk_x: i32,
    origin_chunk_z: i32,
) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    if bytes.len() < LOCATION_TABLE_LEN {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "chunks.dat is too small for its 32x32 location table: {} bytes",
            bytes.len()
        )));
    }

    let mut values = BTreeMap::new();
    for index in 0..(32 * 32) {
        let entry_offset = index * 4;
        let entry = &bytes[entry_offset..entry_offset + 4];
        if entry == [0, 0, 0, 0] {
            continue;
        }

        let sector_count = usize::from(entry[0]);
        let sector_offset =
            usize::from(entry[1]) | (usize::from(entry[2]) << 8) | (usize::from(entry[3]) << 16);
        if sector_count == 0 || sector_offset == 0 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "chunks.dat entry {index} has invalid sector offset/count ({sector_offset}, {sector_count})"
            )));
        }
        let byte_offset = sector_offset.checked_mul(SECTOR_BYTES).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "chunks.dat entry {index} sector offset overflows byte offset"
            ))
        })?;
        let payload = chunk_payload(bytes, byte_offset, sector_count).map_err(|message| {
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

fn chunk_payload(
    bytes: &[u8],
    byte_offset: usize,
    sector_count: usize,
) -> std::result::Result<&[u8], String> {
    let sector_bytes = sector_count
        .checked_mul(SECTOR_BYTES)
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

    if available.len() >= 4 {
        let declared_len = u32::from_le_bytes(
            available[..4]
                .try_into()
                .expect("four-byte prefix already checked"),
        ) as usize;
        if matches!(
            declared_len,
            POCKET_TERRAIN_VALUE_LEN | LEGACY_TERRAIN_VALUE_LEN
        ) {
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

fn check_cancelled(options: &StorageReadOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        Err(BedrockWorldError::Cancelled {
            operation: "Pocket chunks.dat scan",
        })
    } else {
        Ok(())
    }
}

fn emit_progress(options: &StorageReadOptions, outcome: StorageScanOutcome) {
    if let Some(progress) = &options.progress {
        let interval = options.pipeline.progress_interval.max(1);
        if outcome.visited.is_multiple_of(interval) {
            progress.emit(StorageScanProgress {
                entries_seen: outcome.visited,
                bytes_read: outcome.bytes_read,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::LegacyTerrain;

    #[test]
    fn raw_pocket_payload_remains_short_and_biome_less() {
        let mut sectors = vec![0_u8; 21 * SECTOR_BYTES];
        sectors[..POCKET_TERRAIN_VALUE_LEN].fill(0x2A);
        let payload = chunk_payload(&sectors, 0, 21).unwrap();
        assert_eq!(payload.len(), POCKET_TERRAIN_VALUE_LEN);
        let terrain = LegacyTerrain::parse(Bytes::copy_from_slice(payload)).unwrap();
        assert!(!terrain.has_biome_samples());
        assert!(terrain.biomes().is_empty());
    }

    #[test]
    fn location_table_maps_multiple_chunks_from_limited_world_origin() {
        let mut bytes = vec![0_u8; 43 * SECTOR_BYTES];
        bytes[0..4].copy_from_slice(&[21, 1, 0, 0]);
        let second_entry = (32 + 1) * 4;
        bytes[second_entry..second_entry + 4].copy_from_slice(&[21, 22, 0, 0]);
        bytes[SECTOR_BYTES..SECTOR_BYTES + POCKET_TERRAIN_VALUE_LEN].fill(0x11);
        let second_offset = 22 * SECTOR_BYTES;
        bytes[second_offset..second_offset + POCKET_TERRAIN_VALUE_LEN].fill(0x22);

        let values = parse_chunks_dat(&bytes, 10, -5).unwrap();

        let first = ChunkKey::new(
            ChunkPos {
                x: 10,
                z: -5,
                dimension: Dimension::Overworld,
            },
            ChunkRecordTag::LegacyTerrain,
        )
        .encode();
        let second = ChunkKey::new(
            ChunkPos {
                x: 11,
                z: -4,
                dimension: Dimension::Overworld,
            },
            ChunkRecordTag::LegacyTerrain,
        )
        .encode();
        assert_eq!(values.len(), 2);
        assert_eq!(values[&first.to_vec()][0], 0x11);
        assert_eq!(values[&second.to_vec()][0], 0x22);
    }

    #[test]
    fn location_table_rejects_sector_range_outside_file() {
        let mut bytes = vec![0_u8; LOCATION_TABLE_LEN];
        bytes[0..4].copy_from_slice(&[21, 99, 0, 0]);
        assert!(parse_chunks_dat(&bytes, 0, 0).is_err());
    }

    #[test]
    fn location_table_rejects_zero_sector_count() {
        let mut bytes = vec![0_u8; LOCATION_TABLE_LEN];
        bytes[0..4].copy_from_slice(&[0, 1, 0, 0]);
        assert!(parse_chunks_dat(&bytes, 0, 0).is_err());
    }

    #[test]
    fn declared_terrain_length_must_fit_allocated_sectors() {
        let mut sector = vec![0_u8; SECTOR_BYTES];
        sector[..4].copy_from_slice(&(POCKET_TERRAIN_VALUE_LEN as u32).to_le_bytes());
        assert!(chunk_payload(&sector, 0, 1).is_err());
    }
}
