//! Whole-world Minecraft Bedrock SubChunk persisted-version writes.
//!
//! This module changes only the SubChunk container version selected by the caller. It does not infer
//! a game version, upgrade BlockState schemas, or manufacture legacy numeric block IDs.

use crate::chunk::{BedrockDbKey, ChunkRecordTag, SubChunk, SubChunkDecodeMode, SubChunkVersion};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};

/// Summary of staging every SubChunk for one explicitly selected persisted version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubChunkStorageWriteReport {
    /// Requested persisted SubChunk version.
    pub target: SubChunkVersion,
    /// Number of SubChunk records inspected.
    pub records: usize,
    /// Number of records already using the requested version and left byte-for-byte unchanged.
    pub unchanged: usize,
    /// Number of records staged for rewriting.
    pub rewritten: usize,
    /// Total encoded value bytes staged in the atomic storage batch.
    pub staged_bytes: usize,
}

impl SubChunkStorageWriteReport {
    fn new(target: SubChunkVersion) -> Self {
        Self {
            target,
            records: 0,
            unchanged: 0,
            rewritten: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights every SubChunk record and stages all representable changes in one storage batch.
///
/// No mutation occurs before the complete scan succeeds. A single unsupported source record therefore
/// rejects the entire operation instead of leaving a mixed partially rewritten world.
pub(crate) fn stage_subchunks_as_version(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
) -> Result<(StorageBatch, SubChunkStorageWriteReport)> {
    if let SubChunkVersion::Unknown(version) = target {
        return Err(BedrockWorldError::Validation(format!(
            "whole-world SubChunk writes cannot target unknown V{version}"
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = SubChunkStorageWriteReport::new(target);
    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }

        report.records = report.records.saturating_add(1);
        let source = SubChunkVersion::detect(value).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunk record {key:?} has an empty payload"
            ))
        })?;
        if source == target {
            report.unchanged = report.unchanged.saturating_add(1);
            return Ok(StorageVisitorControl::Continue);
        }

        let y = key.subchunk_y.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunkPrefix key {key:?} has no subchunk Y byte"
            ))
        })?;
        let subchunk = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
        let encoded = subchunk.write_as_version(target).map_err(|error| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "cannot write SubChunk {:?} from {:?} as {:?}: {error}",
                key, source, target
            ))
        })?;
        report.rewritten = report.rewritten.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(key.encode(), encoded);
        Ok(StorageVisitorControl::Continue)
    })?;

    Ok((batch, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockPalette, BlockState, ChunkKey, ChunkPos, Dimension, SubChunkFormat};
    use crate::database::MemoryStorage;
    use std::collections::BTreeMap;

    fn paletted(version: u8, y: i8) -> SubChunk {
        let air = BlockState {
            name: "minecraft:air".to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        SubChunk {
            y,
            format: SubChunkFormat::Paletted {
                version,
                storages: vec![BlockPalette::with_unpacked_indices(
                    vec![air],
                    vec![0; 4096],
                    Some(vec![4096_u16]),
                )],
            },
        }
    }

    #[test]
    fn world_subchunk_stage_is_all_preflight_before_commit() {
        let storage = MemoryStorage::new();
        let positions = [
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 1,
                z: 0,
                dimension: Dimension::Overworld,
            },
        ];
        for (index, pos) in positions.into_iter().enumerate() {
            let y = i8::try_from(index).expect("test y");
            let key = ChunkKey::subchunk(pos, y).encode();
            let value = paletted(8, y).write_v8().expect("encode V8");
            storage.put(&key, &value).expect("seed V8");
        }

        let (batch, report) =
            stage_subchunks_as_version(&storage, SubChunkVersion::V9).expect("stage V9");
        assert_eq!(report.records, 2);
        assert_eq!(report.rewritten, 2);
        assert_eq!(report.unchanged, 0);
        for (index, pos) in positions.into_iter().enumerate() {
            let y = i8::try_from(index).expect("test y");
            let value = storage
                .get(&ChunkKey::subchunk(pos, y).encode())
                .expect("read before commit")
                .expect("record exists");
            assert_eq!(value.first().copied(), Some(8));
        }

        storage.write_batch(&batch).expect("commit V9");
        for (index, pos) in positions.into_iter().enumerate() {
            let y = i8::try_from(index).expect("test y");
            let value = storage
                .get(&ChunkKey::subchunk(pos, y).encode())
                .expect("read after commit")
                .expect("record exists");
            assert_eq!(value.first().copied(), Some(9));
        }
    }

    #[test]
    fn unsupported_source_aborts_before_any_storage_write() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 3,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let legacy = crate::chunk::LegacySubChunkBuilder::new(7)
            .expect("builder")
            .build()
            .expect("legacy V7");
        storage.put(&key, &legacy).expect("seed V7");

        assert!(stage_subchunks_as_version(&storage, SubChunkVersion::V9).is_err());
        let value = storage
            .get(&key)
            .expect("read V7")
            .expect("record exists");
        assert_eq!(value.first().copied(), Some(7));
    }
}
