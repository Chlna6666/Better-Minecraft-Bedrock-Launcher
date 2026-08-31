//! Whole-world lossless `Data3D` to `Data2D` writes.

use crate::biome::{Biome3d, data3d_to_data2d};
use crate::chunk::{BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag};
use crate::storage::{
    StorageBatch, StorageOp, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
use crate::error::{BedrockWorldError, Result};
use crate::world::{BedrockWorld, WorldStorageHandle};
use std::collections::BTreeSet;

/// Summary of losslessly collapsing modern `Data3D` biome records to `Data2D`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BiomeData2dDowngradeReport {
    /// Number of `Data3D` records inspected and rewritten.
    pub data3d_records: usize,
    /// Number of `Data2D` records staged.
    pub data2d_written: usize,
    /// Number of old `Data3D` source records staged for deletion.
    pub source_records_removed: usize,
    /// Total encoded `Data2D` value bytes staged before commit.
    pub staged_bytes: usize,
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Losslessly collapses every `Data3D` biome record to `Data2D` in one world transaction.
    ///
    /// Every vertical sample in each `(x,z)` biome column must be identical and every biome id must
    /// fit the historical `u8` `Data2D` representation. The complete world is preflighted first: one
    /// vertically varying or otherwise unrepresentable chunk aborts the operation before any write.
    /// Existing `Data2D` or `Data2DLegacy` records are treated as conflicts rather than overwritten.
    pub fn downgrade_biomes_to_data2d_blocking(&self) -> Result<BiomeData2dDowngradeReport> {
        let (batch, report) = stage_biomes_to_data2d(self.storage())?;
        if batch.is_empty() {
            return Ok(report);
        }
        let mut transaction = self.transaction();
        for op in batch.ops() {
            match op {
                StorageOp::Put { key, value } => {
                    transaction.put_raw_key(key.clone(), value.clone());
                }
                StorageOp::Delete { key } => {
                    transaction.delete_raw_key(key.clone());
                }
            }
        }
        transaction.commit()?;
        Ok(report)
    }
}

fn stage_biomes_to_data2d(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, BiomeData2dDowngradeReport)> {
    let mut data3d = BTreeSet::<ChunkPos>::new();
    let mut old_biomes = BTreeSet::<ChunkPos>::new();

    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        match key.tag {
            ChunkRecordTag::Data3D => {
                data3d.insert(key.pos);
            }
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                old_biomes.insert(key.pos);
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if let Some(pos) = data3d.iter().find(|pos| old_biomes.contains(pos)) {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "chunk {pos:?} already contains Data3D together with Data2D/Data2DLegacy"
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = BiomeData2dDowngradeReport::default();
    for pos in data3d {
        let source_key = ChunkKey::new(pos, ChunkRecordTag::Data3D).encode();
        let raw = storage.get(&source_key)?.ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite(format!(
                "Data3D at chunk {pos:?} disappeared after the preflight key scan"
            ))
        })?;
        let source = Biome3d::parse(&raw)?;
        let encoded = data3d_to_data2d(&source)?.encode()?;
        report.data3d_records = report.data3d_records.saturating_add(1);
        report.data2d_written = report.data2d_written.saturating_add(1);
        report.source_records_removed = report.source_records_removed.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(), encoded);
        batch.delete(source_key);
    }

    Ok((batch, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::{Biome2d, data2d_to_data3d};
    use crate::chunk::{ChunkVersion, Dimension};
    use crate::storage::MemoryStorage;

    #[test]
    fn uniform_data3d_world_downgrades_only_after_preflight() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: -8,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let (min_y, max_y) = pos.subchunk_index_range(ChunkVersion::New);
        let source = data2d_to_data3d(
            &Biome2d::new(vec![80; 256], vec![6; 256]).unwrap(),
            min_y..=max_y,
        )
        .unwrap();
        let source_key = ChunkKey::new(pos, ChunkRecordTag::Data3D).encode();
        storage.put(&source_key, &source.encode().unwrap()).unwrap();

        let (batch, report) = stage_biomes_to_data2d(&storage).unwrap();
        assert_eq!(report.data3d_records, 1);
        assert!(storage.get(&source_key).unwrap().is_some());
        storage.write_batch(&batch).unwrap();
        assert!(storage.get(&source_key).unwrap().is_none());
        let target = storage
            .get(&ChunkKey::new(pos, ChunkRecordTag::Data2D).encode())
            .unwrap()
            .unwrap();
        let target = Biome2d::parse(&target).unwrap();
        assert_eq!(target.height_map[0], 80);
        assert_eq!(target.biomes[0], 6);
    }

    #[test]
    fn vertical_variation_aborts_without_mutation() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 1,
            dimension: Dimension::Overworld,
        };
        let mut source =
            data2d_to_data3d(&Biome2d::new(vec![64; 256], vec![1; 256]).unwrap(), -4..=-3).unwrap();
        source.storages[1].palette = vec![2];
        source.storages[1].indices = Some(vec![0; 4096]);
        source.storages[1].counts = vec![4096];
        let source_key = ChunkKey::new(pos, ChunkRecordTag::Data3D).encode();
        storage.put(&source_key, &source.encode().unwrap()).unwrap();

        assert!(stage_biomes_to_data2d(&storage).is_err());
        assert!(storage.get(&source_key).unwrap().is_some());
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Data2D).encode())
                .unwrap()
                .is_none()
        );
    }
}
