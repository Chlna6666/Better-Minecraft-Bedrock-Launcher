//! Combined read-only storage for pre-LevelDB Pocket Edition world folders.
//!
//! Terrain remains sourced from `chunks.dat` through the preservation-first Pocket backend; the
//! `Entities` and `TileEntities` lists from `entities.dat` are exposed as virtual legacy `Entity` and
//! `BlockEntity` chunk records so normal World query APIs can inspect the complete old world
//! without caller-side file plumbing.

use super::entities::read_pocket_entities_dat;
use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag, Dimension};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, serialize_root_nbt};
use crate::storage::{
    PocketChunksDatStorage, StorageBatch, StorageReadOptions, StorageScanOutcome,
    StorageVisitorControl, WorldStorage,
};
use bytes::Bytes;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const CONFIRMED_ENTITIES_DAT_VERSION: i32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct PocketWorldStorage {
    terrain: PocketChunksDatStorage,
    sidecars: Arc<BTreeMap<Vec<u8>, Bytes>>,
}

impl PocketWorldStorage {
    pub(crate) fn open(world_path: impl AsRef<Path>) -> Result<Self> {
        let world_path = world_path.as_ref();
        let terrain = PocketChunksDatStorage::open(world_path)?;
        let sidecars = if world_path.join("entities.dat").is_file() {
            build_entity_records(world_path)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            terrain,
            sidecars: Arc::new(sidecars),
        })
    }
}

impl WorldStorage for PocketWorldStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        if let Some(value) = self.sidecars.get(key) {
            return Ok(Some(value.clone()));
        }
        self.terrain.get(key)
    }

    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        keys.iter().map(|key| self.get(key.as_ref())).collect()
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
        let mut outcome = self.terrain.for_each_key(options.clone(), visitor)?;
        if outcome.stopped {
            return Ok(outcome);
        }
        for (key, value) in self.sidecars.iter() {
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
        let mut outcome = self
            .terrain
            .for_each_prefix(prefix, options.clone(), visitor)?;
        if outcome.stopped {
            return Ok(outcome);
        }
        for (key, value) in self
            .sidecars
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
            progress.emit(crate::storage::StorageScanProgress {
                entries_seen: outcome.visited,
                bytes_read: outcome.bytes_read,
            });
        }
    }
}
