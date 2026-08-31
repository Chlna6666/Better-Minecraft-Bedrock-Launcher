//! Authoritative BlockState upgrade for paletted Minecraft Bedrock SubChunks.
//!
//! Historical numeric/fixed-array SubChunks are intentionally not handled here; they require the
//! legacy numeric block tables before they can enter the BlockState upgrade path.

use crate::block::{AuthoritativeBlockStateCatalog, BlockState, VanillaBlockStatePalette};
use crate::chunk::{
    BedrockDbKey, ChunkRecordTag, SubChunk, SubChunkDecodeMode, SubChunkFormat, SubChunkVersion,
};
use crate::storage::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;

/// Summary of staging authoritative BlockState upgrades across all paletted SubChunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubChunkUpgradeWriteReport {
    /// Requested persisted SubChunk version.
    pub target: SubChunkVersion,
    /// Target BlockState storage version produced by the authoritative target data.
    pub target_block_state_version: i32,
    /// Number of SubChunk records inspected.
    pub records: usize,
    /// Number of records already identical to the target representation.
    pub unchanged: usize,
    /// Number of SubChunk records staged for rewriting.
    pub rewritten: usize,
    /// Number of palette entries changed by authoritative BlockState upgrade/target-palette binding.
    pub block_states_rewritten: usize,
    /// Number of repeated persisted BlockStates served from the operation-local upgrade cache.
    pub block_state_cache_hits: usize,
    /// Total encoded value bytes staged in the atomic storage batch.
    pub staged_bytes: usize,
}

impl SubChunkUpgradeWriteReport {
    fn new(target: SubChunkVersion, target_palette: &VanillaBlockStatePalette) -> Self {
        Self {
            target,
            target_block_state_version: target_palette.storage_version().raw(),
            records: 0,
            unchanged: 0,
            rewritten: 0,
            block_states_rewritten: 0,
            block_state_cache_hits: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights every paletted SubChunk, upgrades every unique persisted BlockState through the supplied
/// authoritative schema catalog, validates the result against the target game's real vanilla palette,
/// and stages all resulting records in one batch.
///
/// The schema catalog and target palette must end at the same persisted BlockState storage version.
/// No database mutation occurs until every affected SubChunk has decoded, upgraded, target-validated
/// and encoded successfully. A future BlockState, missing storage version, legacy numeric SubChunk,
/// missing target permutation, or target encoding failure therefore aborts the complete operation.
pub(crate) fn stage_paletted_subchunks_for_upgrade(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
) -> Result<(StorageBatch, SubChunkUpgradeWriteReport)> {
    if let SubChunkVersion::Unknown(version) = target {
        return Err(BedrockWorldError::Validation(format!(
            "SubChunk upgrade cannot target unknown V{version}"
        )));
    }
    if catalog.output_version() != target_palette.storage_version() {
        return Err(BedrockWorldError::Validation(format!(
            "BlockState upgrade catalog ends at storage version {}, but target Bedrock {} palette uses {}",
            catalog.output_version().raw(),
            target_palette.game_version(),
            target_palette.storage_version().raw()
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = SubChunkUpgradeWriteReport::new(target, target_palette);
    let mut upgraded_states = BTreeMap::<Vec<u8>, BlockState>::new();

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
        let y = key.subchunk_y.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunkPrefix key {key:?} has no subchunk Y byte"
            ))
        })?;
        let mut subchunk = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
        let SubChunkFormat::Paletted { storages, .. } = &mut subchunk.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "authoritative BlockState upgrade requires a paletted source SubChunk; {key:?} is {source:?}"
            )));
        };

        for storage_layer in storages {
            for state in &mut storage_layer.states {
                let cache_key = persisted_state_key(state)?;
                let target_state = if let Some(target_state) = upgraded_states.get(&cache_key) {
                    report.block_state_cache_hits = report.block_state_cache_hits.saturating_add(1);
                    target_state.clone()
                } else {
                    let upgraded = catalog.upgrade(state)?;
                    let target_state = target_palette.target_state(&upgraded).ok_or_else(|| {
                        BedrockWorldError::UnsupportedChunkFormat(format!(
                            "upgraded BlockState {} {:?} from SubChunk {:?} does not exist in target Bedrock {} vanilla palette",
                            upgraded.name,
                            upgraded.states,
                            key,
                            target_palette.game_version()
                        ))
                    })?;
                    let target_state = target_state.clone();
                    upgraded_states.insert(cache_key, target_state.clone());
                    target_state
                };
                if *state != target_state {
                    *state = target_state;
                    report.block_states_rewritten =
                        report.block_states_rewritten.saturating_add(1);
                }
            }
        }

        let encoded = subchunk.write_as_version(target).map_err(|error| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "cannot write upgraded SubChunk {key:?} from {source:?} as {target:?}: {error}"
            ))
        })?;
        if encoded.as_ref() == value.as_ref() {
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

fn persisted_state_key(state: &BlockState) -> Result<Vec<u8>> {
    let version = state.version.ok_or_else(|| {
        BedrockWorldError::UnsupportedChunkFormat(format!(
            "BlockState {} has no persisted storage version",
            state.name
        ))
    })?;
    let mut key = state.canonical_bytes()?;
    key.extend_from_slice(&version.to_le_bytes());
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockStateSchemaSource;
    use crate::chunk::{BlockPalette, ChunkKey, ChunkPos, Dimension};
    use crate::storage::MemoryStorage;
    use crate::version::GameVersion;
    use std::collections::BTreeMap;

    fn catalog() -> AuthoritativeBlockStateCatalog {
        AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{
                "maxVersionMajor":1,
                "maxVersionMinor":18,
                "maxVersionPatch":0,
                "maxVersionRevision":20,
                "renamedIds":{"minecraft:old_test":"minecraft:new_test"}
            }"#,
        }])
        .expect("catalog")
    }

    fn source_subchunk() -> SubChunk {
        let state = BlockState {
            name: "minecraft:old_test".to_string(),
            states: BTreeMap::new(),
            version: Some(0x0111_2800),
        };
        SubChunk {
            y: 0,
            format: SubChunkFormat::Paletted {
                version: 8,
                storages: vec![BlockPalette::with_unpacked_indices(
                    vec![state],
                    vec![0; 4096],
                    Some(vec![4096_u16]),
                )],
            },
        }
    }

    fn target_palette(catalog: &AuthoritativeBlockStateCatalog) -> VanillaBlockStatePalette {
        VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 18, 0, 20]).unwrap(),
            vec![BlockState {
                name: "minecraft:new_test".to_string(),
                states: BTreeMap::new(),
                version: Some(catalog.output_version().raw()),
            }],
        )
        .expect("target palette")
    }

    #[test]
    fn authoritative_upgrade_is_staged_before_commit() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let source = source_subchunk().write_v8().expect("source V8");
        storage.put(&key, &source).expect("seed source");

        let catalog = catalog();
        let target_palette = target_palette(&catalog);
        let (batch, report) = stage_paletted_subchunks_for_upgrade(
            &storage,
            SubChunkVersion::V9,
            &catalog,
            &target_palette,
        )
        .expect("stage upgrade");
        assert_eq!(report.block_states_rewritten, 1);
        assert_eq!(
            storage
                .get(&key)
                .expect("read before commit")
                .expect("record")
                .first()
                .copied(),
            Some(8)
        );

        storage.write_batch(&batch).expect("commit upgrade");
        let value = storage.get(&key).expect("read V9").expect("record");
        let parsed = SubChunk::read(0, value, SubChunkDecodeMode::FullIndices).expect("parse V9");
        let state = parsed.block_state_at(0, 0, 0).expect("state");
        assert_eq!(state.name, "minecraft:new_test");
        assert_eq!(state.version, Some(catalog.output_version().raw()));
    }

    #[test]
    fn mismatched_catalog_and_target_palette_are_rejected() {
        let storage = MemoryStorage::new();
        let catalog = catalog();
        let wrong_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![BlockState {
                name: "minecraft:new_test".to_string(),
                states: BTreeMap::new(),
                version: Some(0x0111_2800),
            }],
        )
        .expect("wrong palette");
        assert!(
            stage_paletted_subchunks_for_upgrade(
                &storage,
                SubChunkVersion::V9,
                &catalog,
                &wrong_palette,
            )
            .is_err()
        );
    }
}
