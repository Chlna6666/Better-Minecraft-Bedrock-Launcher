//! Whole-world `Data2D` / `Data2DLegacy` to `Data3D` writes.

use crate::biome::{Biome2d, Biome2dLegacy, data2d_to_data3d};
use crate::chunk::{BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag, ChunkVersion, Dimension};
use crate::database::{
    StorageBatch, StorageOp, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
use crate::error::{BedrockWorldError, Result};
use crate::world::{BedrockWorld, WorldStorageHandle};
use std::collections::{BTreeMap, BTreeSet};

/// Summary of promoting all two-dimensional Bedrock biome records to `Data3D`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BiomeData3dUpgradeReport {
    /// Number of `Data2D` records converted without dropping persisted fields.
    pub data2d_records: usize,
    /// Number of `Data2DLegacy` records converted.
    pub data2d_legacy_records: usize,
    /// Number of historical saved biome RGB samples discarded because `Data3D` has no RGB field.
    pub saved_rgb_samples_discarded: usize,
    /// Number of `Data3D` records staged.
    pub data3d_written: usize,
    /// Number of old `Data2D`/`Data2DLegacy` source records staged for deletion.
    pub source_records_removed: usize,
    /// Total encoded `Data3D` value bytes staged before commit.
    pub staged_bytes: usize,
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Promotes every `Data2D` and `Data2DLegacy` biome record to modern `Data3D` in one transaction.
    ///
    /// `Data2D` is expanded losslessly by repeating each horizontal biome through the dimension's
    /// modern vertical SubChunk range. `Data2DLegacy` contains an additional saved RGB triplet for
    /// every column; `Data3D` has no field for those bytes. If such records exist and
    /// `allow_saved_rgb_loss` is `false`, the complete operation is rejected before any write. Passing
    /// `true` explicitly acknowledges that those historical RGB samples will be discarded while biome
    /// IDs and height values are preserved.
    pub fn upgrade_biomes_to_data3d_blocking(
        &self,
        allow_saved_rgb_loss: bool,
    ) -> Result<BiomeData3dUpgradeReport> {
        let (batch, report) =
            stage_biomes_to_data3d(self.storage(), allow_saved_rgb_loss)?;
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

fn stage_biomes_to_data3d(
    storage: &dyn WorldStorage,
    allow_saved_rgb_loss: bool,
) -> Result<(StorageBatch, BiomeData3dUpgradeReport)> {
    let mut sources = BTreeMap::<ChunkPos, ChunkRecordTag>::new();
    let mut data3d = BTreeSet::<ChunkPos>::new();

    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        match key.tag {
            tag @ (ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy) => {
                if let Some(previous) = sources.insert(key.pos, tag) {
                    if previous != tag {
                        return Err(BedrockWorldError::CorruptWorld(format!(
                            "chunk {:?} contains both {previous:?} and {tag:?} biome records",
                            key.pos
                        )));
                    }
                }
            }
            ChunkRecordTag::Data3D => {
                data3d.insert(key.pos);
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if !allow_saved_rgb_loss
        && sources
            .values()
            .any(|tag| *tag == ChunkRecordTag::Data2DLegacy)
    {
        return Err(BedrockWorldError::UnsupportedChunkFormat(
            "Data2DLegacy -> Data3D would discard saved historical biome RGB samples; rerun with allow_saved_rgb_loss=true to acknowledge that loss"
                .to_string(),
        ));
    }

    if let Some(pos) = sources.keys().find(|pos| data3d.contains(pos)) {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "chunk {pos:?} already contains Data3D together with a two-dimensional biome record"
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = BiomeData3dUpgradeReport::default();
    for (pos, source_tag) in sources {
        let (min_section, max_section) = match pos.dimension {
            Dimension::Unknown(id) => {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot select Data3D vertical range for unknown Bedrock dimension {id} at chunk ({}, {})",
                    pos.x, pos.z
                )));
            }
            _ => pos.subchunk_index_range(ChunkVersion::New),
        };
        let source_key = ChunkKey::new(pos, source_tag).encode();
        let raw = storage.get(&source_key)?.ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite(format!(
                "{source_tag:?} at chunk {pos:?} disappeared after the preflight key scan"
            ))
        })?;

        let data2d = match source_tag {
            ChunkRecordTag::Data2D => {
                report.data2d_records = report.data2d_records.saturating_add(1);
                Biome2d::parse(&raw)?
            }
            ChunkRecordTag::Data2DLegacy => {
                report.data2d_legacy_records = report.data2d_legacy_records.saturating_add(1);
                report.saved_rgb_samples_discarded =
                    report.saved_rgb_samples_discarded.saturating_add(256);
                Biome2dLegacy::parse(&raw)?.to_data2d()?
            }
            _ => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "biome staging lost source tag {source_tag:?}"
                )));
            }
        };
        let encoded = data2d_to_data3d(&data2d, min_section..=max_section)?.encode()?;
        report.data3d_written = report.data3d_written.saturating_add(1);
        report.source_records_removed = report.source_records_removed.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(), encoded);
        batch.delete(source_key);
    }

    Ok((batch, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::{Biome2dLegacy, LegacyBiomeSample};
    use crate::chunk::Dimension;
    use crate::database::MemoryStorage;

    #[test]
    fn data2d_promotes_to_full_modern_overworld_range() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 3,
            z: -6,
            dimension: Dimension::Overworld,
        };
        let source_key = ChunkKey::new(pos, ChunkRecordTag::Data2D).encode();
        let source = Biome2d::new(vec![72; 256], vec![4; 256]).unwrap();
        storage.put(&source_key, &source.encode().unwrap()).unwrap();

        let (batch, report) = stage_biomes_to_data3d(&storage, false).unwrap();
        assert_eq!(report.data2d_records, 1);
        assert_eq!(report.saved_rgb_samples_discarded, 0);
        assert!(storage.get(&source_key).unwrap().is_some());
        storage.write_batch(&batch).unwrap();
        assert!(storage.get(&source_key).unwrap().is_none());

        let value = storage
            .get(&ChunkKey::new(pos, ChunkRecordTag::Data3D).encode())
            .unwrap()
            .unwrap();
        let parsed = crate::biome::Biome3d::parse(&value).unwrap();
        assert_eq!(parsed.storages.len(), 24);
        assert_eq!(parsed.storages.first().unwrap().y, Some(-4));
        assert_eq!(parsed.storages.last().unwrap().y, Some(19));
        assert_eq!(parsed.height_map[0], 72);
    }

    #[test]
    fn data2d_legacy_requires_explicit_saved_rgb_loss() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let source_key = ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode();
        let legacy = Biome2dLegacy::new(
            vec![64; 256],
            vec![
                LegacyBiomeSample {
                    biome_id: 1,
                    red: 2,
                    green: 3,
                    blue: 4,
                };
                256
            ],
        )
        .unwrap();
        storage.put(&source_key, &legacy.encode().unwrap()).unwrap();

        assert!(stage_biomes_to_data3d(&storage, false).is_err());
        assert!(storage.get(&source_key).unwrap().is_some());
        let (batch, report) = stage_biomes_to_data3d(&storage, true).unwrap();
        assert_eq!(report.data2d_legacy_records, 1);
        assert_eq!(report.saved_rgb_samples_discarded, 256);
        storage.write_batch(&batch).unwrap();
        assert!(storage.get(&source_key).unwrap().is_none());
    }
}
