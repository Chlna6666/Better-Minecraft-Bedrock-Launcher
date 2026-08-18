//! Whole-world Minecraft Bedrock SubChunk persisted-version writes.
//!
//! This module changes only explicitly selected SubChunk data. Upgrade storage writes preserve the
//! decoded BlockStates; exact downgrade writes additionally require the target game's real vanilla
//! BlockState palette so unavailable old-game states are rejected rather than guessed.

use crate::block::VanillaBlockStatePalette;
use crate::chunk::{
    BedrockDbKey, ChunkRecordTag, SubChunk, SubChunkDecodeMode, SubChunkFormat, SubChunkVersion,
};
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

/// Summary of an exact target-palette SubChunk downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubChunkDowngradeWriteReport {
    /// Requested persisted SubChunk version.
    pub target: SubChunkVersion,
    /// Target palette BlockState storage version written into matched palette entries.
    pub target_block_state_version: i32,
    /// Number of SubChunk records inspected.
    pub records: usize,
    /// Number of records already identical to the exact target representation.
    pub unchanged: usize,
    /// Number of SubChunk records staged for rewriting.
    pub rewritten: usize,
    /// Number of SubChunk palette entries replaced with the target game's exact BlockState entry.
    pub block_states_rewritten: usize,
    /// Total encoded value bytes staged in the atomic storage batch.
    pub staged_bytes: usize,
}

impl SubChunkDowngradeWriteReport {
    fn new(target: SubChunkVersion, palette: &VanillaBlockStatePalette) -> Self {
        Self {
            target,
            target_block_state_version: palette.storage_version().raw(),
            records: 0,
            unchanged: 0,
            rewritten: 0,
            block_states_rewritten: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights every SubChunk record and stages all representable storage-version changes in one batch.
///
/// No mutation occurs before the complete scan succeeds. A single unsupported source record therefore
/// rejects the entire operation instead of leaving a mixed partially rewritten world.
pub(crate) fn stage_subchunks_as_version(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
) -> Result<(StorageBatch, SubChunkStorageWriteReport)> {
    reject_unknown_world_target(target)?;

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
        let source = subchunk_source_version(&key, value)?;
        if source == target {
            report.unchanged = report.unchanged.saturating_add(1);
            return Ok(StorageVisitorControl::Continue);
        }

        let y = subchunk_y(&key)?;
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

/// Preflights a lossless SubChunk downgrade against one real target-game vanilla BlockState palette.
///
/// Every decoded palette entry must exist with the same semantic name/states in the supplied target
/// palette. Matching entries are replaced by the target palette entry so the persisted BlockState
/// `version` also comes from the target game. Renamed, removed, or otherwise unavailable states abort
/// the entire operation before any database write.
pub(crate) fn stage_subchunks_for_exact_downgrade(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    target_palette: &VanillaBlockStatePalette,
) -> Result<(StorageBatch, SubChunkDowngradeWriteReport)> {
    reject_unknown_world_target(target)?;

    let mut batch = StorageBatch::new();
    let mut report = SubChunkDowngradeWriteReport::new(target, target_palette);
    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }

        report.records = report.records.saturating_add(1);
        let source = subchunk_source_version(&key, value)?;
        let y = subchunk_y(&key)?;
        let mut subchunk = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
        let SubChunkFormat::Paletted { storages, .. } = &mut subchunk.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "exact target-palette downgrade requires a paletted source SubChunk; {:?} is {:?}",
                key, source
            )));
        };

        for storage_layer in storages {
            for state in &mut storage_layer.states {
                let target_state = target_palette.target_state(state).ok_or_else(|| {
                    BedrockWorldError::UnsupportedChunkFormat(format!(
                        "BlockState {} {:?} from SubChunk {:?} does not exist in target Bedrock {} vanilla palette",
                        state.name,
                        state.states,
                        key,
                        target_palette.game_version()
                    ))
                })?;
                if state != target_state {
                    *state = target_state.clone();
                    report.block_states_rewritten =
                        report.block_states_rewritten.saturating_add(1);
                }
            }
        }

        let encoded = subchunk.write_as_version(target).map_err(|error| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "cannot downgrade SubChunk {:?} from {:?} as {:?}: {error}",
                key, source, target
            ))
        })?;
        if encoded == *value {
            report.unchanged = report.unchanged.saturating_add(1);
        } else {
            report.rewritten = report.rewritten.saturating_add(1);
            report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
            batch.put(key.encode(), encoded);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    Ok((batch, report))
}

fn reject_unknown_world_target(target: SubChunkVersion) -> Result<()> {
    if let SubChunkVersion::Unknown(version) = target {
        return Err(BedrockWorldError::Validation(format!(
            "whole-world SubChunk writes cannot target unknown V{version}"
        )));
    }
    Ok(())
}

fn subchunk_source_version(
    key: &crate::chunk::ChunkKey,
    value: &[u8],
) -> Result<SubChunkVersion> {
    SubChunkVersion::detect(value).ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!("SubChunk record {key:?} has an empty payload"))
    })
}

fn subchunk_y(key: &crate::chunk::ChunkKey) -> Result<i8> {
    key.subchunk_y.ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!(
            "SubChunkPrefix key {key:?} has no subchunk Y byte"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::VanillaBlockStatePalette;
    use crate::chunk::{BlockPalette, BlockState, ChunkKey, ChunkPos, Dimension};
    use crate::database::MemoryStorage;
    use crate::nbt::NbtTag;
    use crate::version::GameVersion;
    use std::collections::BTreeMap;

    fn block_state(name: &str, facing: i32, version: i32) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("facing_direction".to_string(), NbtTag::Int(facing))]),
            version: Some(version),
        }
    }

    fn paletted(version: u8, y: i8, state: BlockState) -> SubChunk {
        SubChunk {
            y,
            format: SubChunkFormat::Paletted {
                version,
                storages: vec![BlockPalette::with_unpacked_indices(
                    vec![state],
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
            let value = paletted(
                8,
                y,
                block_state("minecraft:air", 0, 18_168_865),
            )
            .write_v8()
            .expect("encode V8");
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

    #[test]
    fn exact_downgrade_uses_target_palette_storage_version() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 9,
            z: -1,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let source = block_state("minecraft:test", 2, 18_168_865);
        let value = paletted(9, 0, source).write_v9().expect("encode V9");
        storage.put(&key, &value).expect("seed V9");

        let target_state = block_state("minecraft:test", 2, 17_000_001);
        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![target_state.clone()],
        )
        .unwrap();
        let (batch, report) = stage_subchunks_for_exact_downgrade(
            &storage,
            SubChunkVersion::V8,
            &palette,
        )
        .expect("stage exact downgrade");
        assert_eq!(report.block_states_rewritten, 1);
        assert_eq!(report.rewritten, 1);
        assert_eq!(
            storage
                .get(&key)
                .expect("read before commit")
                .expect("record")
                .first()
                .copied(),
            Some(9)
        );

        storage.write_batch(&batch).expect("commit downgrade");
        let downgraded = storage.get(&key).expect("read V8").expect("record");
        let parsed = SubChunk::read(0, downgraded, SubChunkDecodeMode::FullIndices)
            .expect("parse downgraded V8");
        let state = parsed.block_state_at(0, 0, 0).expect("target state");
        assert_eq!(state, &target_state);
    }

    #[test]
    fn unavailable_target_state_aborts_exact_downgrade() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 10,
            z: -1,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let value = paletted(
            9,
            0,
            block_state("minecraft:new_block", 0, 18_168_865),
        )
        .write_v9()
        .expect("encode V9");
        storage.put(&key, &value).expect("seed V9");

        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![block_state("minecraft:stone", 0, 17_000_001)],
        )
        .unwrap();
        assert!(
            stage_subchunks_for_exact_downgrade(&storage, SubChunkVersion::V8, &palette)
                .is_err()
        );
        assert_eq!(
            storage
                .get(&key)
                .expect("read source")
                .expect("record")
                .first()
                .copied(),
            Some(9)
        );
    }
}
