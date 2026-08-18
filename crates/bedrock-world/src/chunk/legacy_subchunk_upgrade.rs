//! Historical numeric Minecraft Bedrock SubChunk upgrade into paletted BlockStates.

use crate::block::{
    AuthoritativeBlockStateCatalog, BlockState, LegacyNumericBlockStateTable,
    VanillaBlockStatePalette,
};
use crate::chunk::{
    BedrockDbKey, BlockPalette, ChunkRecordTag, SubChunk, SubChunkFormat, SubChunkVersion,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;

const NUMERIC_ID_META_SLOTS: usize = 256 * 16;
const BLOCKS_PER_SUBCHUNK: usize = 4096;

/// Summary of converting legacy numeric SubChunks into a target paletted representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacySubChunkUpgradeWriteReport {
    /// Requested target persisted SubChunk version.
    pub target: SubChunkVersion,
    /// Target BlockState storage version written into the resulting palette entries.
    pub target_block_state_version: i32,
    /// Number of legacy V0/V2-V7 SubChunk records converted.
    pub records: usize,
    /// Number of converted source SubChunks that carried legacy sky/block light arrays.
    ///
    /// Paletted V1/V8/V9 SubChunk payloads do not carry these arrays. They are derived lighting data
    /// and are therefore omitted from the target payload for the game to recalculate, but the count is
    /// reported explicitly rather than hiding the persisted-format difference.
    pub source_light_array_records: usize,
    /// Number of block positions resolved from numeric id/metadata pairs.
    pub blocks_resolved: usize,
    /// Number of distinct legacy `(numeric id, metadata)` pairs resolved across all converted records.
    pub unique_numeric_states: usize,
    /// Number of resulting target palette entries across all converted records after semantic deduplication.
    pub target_palette_entries: usize,
    /// Number of repeated numeric id/metadata lookups served from the operation caches.
    pub numeric_cache_hits: usize,
    /// Total encoded value bytes staged in the atomic storage batch.
    pub staged_bytes: usize,
}

impl LegacySubChunkUpgradeWriteReport {
    fn new(target: SubChunkVersion, target_palette: &VanillaBlockStatePalette) -> Self {
        Self {
            target,
            target_block_state_version: target_palette.storage_version().raw(),
            records: 0,
            source_light_array_records: 0,
            blocks_resolved: 0,
            unique_numeric_states: 0,
            target_palette_entries: 0,
            numeric_cache_hits: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights and stages every legacy numeric V0/V2-V7 SubChunk as a target paletted SubChunk.
///
/// The numeric table resolves historical `(id, meta)` values, the schema catalog applies authoritative
/// ordered BlockState upgrades, and the target vanilla palette proves that the upgraded semantic state
/// exists in the requested game while supplying its exact persisted BlockState version. All records
/// are converted before any storage mutation occurs.
pub(crate) fn stage_legacy_subchunks_for_upgrade(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
) -> Result<(StorageBatch, LegacySubChunkUpgradeWriteReport)> {
    if !matches!(target, SubChunkVersion::V1 | SubChunkVersion::V8 | SubChunkVersion::V9) {
        return Err(BedrockWorldError::Validation(format!(
            "legacy numeric SubChunk upgrade requires a paletted V1/V8/V9 target, got {target:?}"
        )));
    }
    if catalog.output_version() != target_palette.storage_version() {
        return Err(BedrockWorldError::Validation(format!(
            "BlockState upgrade catalog ends at {}, target Bedrock {} palette uses {}",
            catalog.output_version().raw(),
            target_palette.game_version(),
            target_palette.storage_version().raw()
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = LegacySubChunkUpgradeWriteReport::new(target, target_palette);
    let mut resolved_numeric = vec![None::<BlockState>; NUMERIC_ID_META_SLOTS];

    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }
        let Some(source_version) = SubChunkVersion::detect(value) else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "SubChunk record {key:?} has an empty payload"
            )));
        };
        if !matches!(
            source_version,
            SubChunkVersion::V0
                | SubChunkVersion::V2
                | SubChunkVersion::V3
                | SubChunkVersion::V4
                | SubChunkVersion::V5
                | SubChunkVersion::V6
                | SubChunkVersion::V7
        ) {
            return Ok(StorageVisitorControl::Continue);
        }

        let y = key.subchunk_y.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "legacy SubChunkPrefix key {key:?} has no subchunk Y byte"
            ))
        })?;
        let source = SubChunk::read(
            y,
            value.clone(),
            crate::chunk::SubChunkDecodeMode::FullIndices,
        )?;
        let SubChunkFormat::LegacySubChunk(legacy) = source.format else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "legacy SubChunk {key:?} decoded into a non-legacy representation"
            )));
        };
        if legacy.has_light_arrays() {
            report.source_light_array_records =
                report.source_light_array_records.saturating_add(1);
        }

        let mut numeric_to_palette = [u16::MAX; NUMERIC_ID_META_SLOTS];
        let mut target_identity_to_palette = BTreeMap::<Vec<u8>, u16>::new();
        let mut states = Vec::<BlockState>::new();
        let mut indices = vec![0_u16; BLOCKS_PER_SUBCHUNK];
        let mut counts = Vec::<u16>::new();

        for block_index in 0..BLOCKS_PER_SUBCHUNK {
            let numeric_id = usize::from(legacy.block_ids()[block_index]);
            let metadata = usize::from(nibble_at(legacy.block_data(), block_index));
            let numeric_slot = numeric_id * 16 + metadata;

            let palette_index = if numeric_to_palette[numeric_slot] != u16::MAX {
                report.numeric_cache_hits = report.numeric_cache_hits.saturating_add(1);
                numeric_to_palette[numeric_slot]
            } else {
                let target_state = if let Some(target_state) = &resolved_numeric[numeric_slot] {
                    report.numeric_cache_hits = report.numeric_cache_hits.saturating_add(1);
                    target_state.clone()
                } else {
                    let source_state = numeric
                        .get(numeric_id as u32, metadata as u32)
                        .ok_or_else(|| {
                            BedrockWorldError::UnsupportedChunkFormat(format!(
                                "legacy SubChunk {key:?} uses unmapped numeric block {numeric_id}:{metadata}"
                            ))
                        })?;
                    let upgraded = catalog.upgrade(source_state)?;
                    let target_state = target_palette.target_state(&upgraded).ok_or_else(|| {
                        BedrockWorldError::UnsupportedChunkFormat(format!(
                            "upgraded legacy block {numeric_id}:{metadata} -> {} {:?} does not exist in target Bedrock {} vanilla palette",
                            upgraded.name,
                            upgraded.states,
                            target_palette.game_version()
                        ))
                    })?;
                    let target_state = target_state.clone();
                    resolved_numeric[numeric_slot] = Some(target_state.clone());
                    report.unique_numeric_states = report.unique_numeric_states.saturating_add(1);
                    target_state
                };

                let identity = target_state.canonical_bytes()?;
                let palette_index = if let Some(index) = target_identity_to_palette.get(&identity) {
                    *index
                } else {
                    let index = u16::try_from(states.len()).map_err(|_| {
                        BedrockWorldError::Validation(
                            "legacy SubChunk target palette exceeds u16 entries".to_string(),
                        )
                    })?;
                    states.push(target_state);
                    counts.push(0);
                    target_identity_to_palette.insert(identity, index);
                    index
                };
                numeric_to_palette[numeric_slot] = palette_index;
                palette_index
            };

            indices[block_index] = palette_index;
            let count = counts.get_mut(usize::from(palette_index)).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "legacy SubChunk target palette index lost its count slot".to_string(),
                )
            })?;
            *count = count.saturating_add(1);
            report.blocks_resolved = report.blocks_resolved.saturating_add(1);
        }

        report.records = report.records.saturating_add(1);
        report.target_palette_entries = report
            .target_palette_entries
            .saturating_add(states.len());
        let target_subchunk = SubChunk {
            y,
            format: SubChunkFormat::Paletted {
                version: target.byte(),
                storages: vec![BlockPalette::with_unpacked_indices(
                    states,
                    indices,
                    Some(counts),
                )],
            },
        };
        let encoded = target_subchunk.write_as_version(target).map_err(|error| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "cannot encode upgraded legacy SubChunk {key:?} as {target:?}: {error}"
            ))
        })?;
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(key.encode(), encoded);
        Ok(StorageVisitorControl::Continue)
    })?;

    Ok((batch, report))
}

#[inline]
fn nibble_at(bytes: &[u8], index: usize) -> u8 {
    let byte = bytes[index / 2];
    if index.is_multiple_of(2) {
        byte & 0x0f
    } else {
        byte >> 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockStateSchemaSource, BlockStateStorageVersion};
    use crate::chunk::{ChunkKey, ChunkPos, Dimension, LegacySubChunkBuilder, SubChunkDecodeMode};
    use crate::database::MemoryStorage;
    use crate::nbt::{NbtTag, serialize_root_nbt};
    use crate::version::GameVersion;
    use indexmap::IndexMap;

    fn put_var_u32(mut value: u32, output: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            output.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn numeric_state(name: &str, version: i32) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(name.to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(version)),
        ]))
    }

    fn numeric_table() -> LegacyNumericBlockStateTable {
        let source_version = BlockStateStorageVersion::from_components(1, 9, 0, 0).raw();
        let entries = [
            ("minecraft:air", 0_u32, numeric_state("minecraft:air", source_version)),
            (
                "minecraft:old_test",
                2_u32,
                numeric_state("minecraft:old_test", source_version),
            ),
        ];
        let mut table = Vec::new();
        put_var_u32(entries.len() as u32, &mut table);
        for (name, meta, root) in entries {
            put_var_u32(name.len() as u32, &mut table);
            table.extend_from_slice(name.as_bytes());
            put_var_u32(1, &mut table);
            put_var_u32(meta, &mut table);
            table.extend(serialize_root_nbt(&root).expect("serialize numeric state"));
        }
        LegacyNumericBlockStateTable::parse(
            &table,
            r#"{"minecraft:air":0,"minecraft:old_test":1}"#,
        )
        .expect("numeric table")
    }

    fn catalog() -> AuthoritativeBlockStateCatalog {
        AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_1.9.0_to_1.10.0.json",
            json: r#"{
                "maxVersionMajor":1,
                "maxVersionMinor":10,
                "maxVersionPatch":0,
                "maxVersionRevision":0,
                "renamedIds":{"minecraft:old_test":"minecraft:new_test"}
            }"#,
        }])
        .expect("catalog")
    }

    #[test]
    fn legacy_v7_numeric_blocks_upgrade_to_target_palette_before_commit() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 2,
            z: 3,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let mut builder = LegacySubChunkBuilder::zeroed(7, false).expect("V7 builder");
        builder
            .set_block(1, 2, 3, 1, 2)
            .expect("set legacy block");
        let source = builder.build().expect("V7").into_raw();
        storage.put(&key, &source).expect("seed V7");

        let catalog = catalog();
        let target_version = catalog.output_version().raw();
        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 10, 0, 0]).unwrap(),
            vec![
                BlockState {
                    name: "minecraft:air".to_string(),
                    states: BTreeMap::new(),
                    version: Some(target_version),
                },
                BlockState {
                    name: "minecraft:new_test".to_string(),
                    states: BTreeMap::new(),
                    version: Some(target_version),
                },
            ],
        )
        .expect("target palette");

        let (batch, report) = stage_legacy_subchunks_for_upgrade(
            &storage,
            SubChunkVersion::V8,
            &numeric_table(),
            &catalog,
            &palette,
        )
        .expect("stage legacy upgrade");
        assert_eq!(report.records, 1);
        assert_eq!(report.blocks_resolved, 4096);
        assert_eq!(report.source_light_array_records, 0);
        assert_eq!(
            storage
                .get(&key)
                .expect("read before commit")
                .expect("source exists")
                .first()
                .copied(),
            Some(7)
        );

        storage.write_batch(&batch).expect("commit upgrade");
        let value = storage.get(&key).expect("read V8").expect("record");
        let parsed = SubChunk::read(0, value, SubChunkDecodeMode::FullIndices).expect("parse V8");
        let state = parsed.block_state_at(1, 2, 3).expect("upgraded block");
        assert_eq!(state.name, "minecraft:new_test");
        assert_eq!(state.version, Some(target_version));
    }
}
