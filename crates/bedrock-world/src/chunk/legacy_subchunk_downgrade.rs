//! Explicit paletted Minecraft Bedrock SubChunk writes to historical fixed-array numeric versions.
//!
//! This module is a physical SubChunk representation write. It does not infer a Minecraft game
//! version. Modern palette entries are matched through a caller-supplied
//! [`crate::block::LegacyNumericBlockUpgradeTable`], whose historical candidates were first run
//! forward through authoritative BlockState upgrade rules. Reverse writes therefore do not invert
//! rename/property rules heuristically.

use crate::block::{BlockState, LegacyNumericBlockMatch, LegacyNumericBlockUpgradeTable};
use crate::chunk::{
    BedrockDbKey, ChunkPos, ChunkRecordTag, ChunkVersion, Dimension, LegacyBlockExtraDataBuilder,
    LegacySubChunkBuilder, SubChunk, SubChunkDecodeMode, SubChunkFormat, SubChunkVersion,
};
use crate::storage::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::surface::is_air_block_name;
use std::collections::{BTreeMap, BTreeSet};

/// Summary of explicitly writing paletted SubChunks as historical numeric fixed-array payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyNumericSubChunkWriteReport {
    /// Requested fixed-array target SubChunk version.
    pub target: SubChunkVersion,
    /// Number of all SubChunk records inspected.
    pub records: usize,
    /// Number of records already using the exact requested fixed-array version and retained unchanged.
    pub unchanged: usize,
    /// Number of paletted SubChunks staged for numeric rewriting.
    pub rewritten: usize,
    /// Number of rewritten SubChunks that contained a second paletted storage layer.
    pub second_storage_records: usize,
    /// Number of chunk-scoped `BlockExtraData` records generated from second storage layers.
    pub block_extra_data_written: usize,
    /// Number of non-air second-layer blocks encoded into generated `BlockExtraData` records.
    pub block_extra_entries_written: usize,
    /// Number of palette entries resolved through the forward-verified historical numeric table.
    pub palette_entries_resolved: usize,
    /// Number of rewritten primary fixed-array payloads intentionally emitted without historical
    /// sky/block light arrays because paletted sources do not contain those values to preserve.
    pub target_without_light_arrays: usize,
    /// Total encoded SubChunk plus generated `BlockExtraData` value bytes staged before commit.
    pub staged_bytes: usize,
}

impl LegacyNumericSubChunkWriteReport {
    fn new(target: SubChunkVersion) -> Self {
        Self {
            target,
            records: 0,
            unchanged: 0,
            rewritten: 0,
            second_storage_records: 0,
            block_extra_data_written: 0,
            block_extra_entries_written: 0,
            palette_entries_resolved: 0,
            target_without_light_arrays: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights all SubChunks and stages exact paletted -> fixed-array numeric writes in one batch.
///
/// Target versions are V0 or V2-V7. Source SubChunks must lie inside the historical build-height
/// range for their Bedrock dimension. A source already using the exact target version is preserved
/// byte-for-byte. A paletted source may have one or two storage layers: the primary layer becomes the
/// fixed-array block ID/four-bit metadata payload, while non-air blocks from an optional second layer
/// become chunk-scoped `BlockExtraData` entries using the historical full-Y coordinate index. More
/// than two layers are rejected.
///
/// Every non-air palette state that needs a numeric representation must have exactly one match in the
/// forward-verified `numeric` table. Primary metadata must fit four bits; second-layer
/// `BlockExtraData` metadata may use the full historical `u8`. Existing `BlockExtraData` beside a
/// paletted source is rejected rather than merged with generated entries. Paletted source records do
/// not persist historical sky/block light arrays, so rewritten fixed-array targets use the valid short
/// form instead of inventing light.
pub(crate) fn stage_paletted_subchunks_as_legacy_numeric(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    numeric: &LegacyNumericBlockUpgradeTable,
) -> Result<(StorageBatch, LegacyNumericSubChunkWriteReport)> {
    let target_byte = legacy_target_byte(target)?;

    let mut existing_block_extra = BTreeSet::<ChunkPos>::new();
    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        if let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key)
            && key.tag == ChunkRecordTag::BlockExtraData
        {
            existing_block_extra.insert(key.pos);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    let mut batch = StorageBatch::new();
    let mut report = LegacyNumericSubChunkWriteReport::new(target);
    let mut generated_block_extra = BTreeMap::<ChunkPos, LegacyBlockExtraDataBuilder>::new();

    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }

        report.records = report.records.saturating_add(1);
        let y = key.subchunk_y.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunkPrefix key {key:?} has no SubChunk Y byte"
            ))
        })?;
        validate_historical_subchunk_y(key.pos, y)?;

        let source_version = SubChunkVersion::detect(value).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunk record {key:?} has an empty payload"
            ))
        })?;
        if source_version == target {
            let parsed = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
            if !matches!(parsed.format, SubChunkFormat::LegacySubChunk(_)) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "SubChunk {key:?} reports target {target:?} but is not a fixed-array numeric payload"
                )));
            }
            report.unchanged = report.unchanged.saturating_add(1);
            return Ok(StorageVisitorControl::Continue);
        }

        if matches!(
            source_version,
            SubChunkVersion::V0
                | SubChunkVersion::V2
                | SubChunkVersion::V3
                | SubChunkVersion::V4
                | SubChunkVersion::V5
                | SubChunkVersion::V6
                | SubChunkVersion::V7
        ) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "SubChunk {key:?} already uses historical fixed-array {source_version:?}; this explicit paletted writer refuses to reinterpret it as {target:?}"
            )));
        }
        if existing_block_extra.contains(&key.pos) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "paletted SubChunk {key:?} coexists with historical BlockExtraData; refusing to merge generated second-layer entries with an existing record"
            )));
        }

        let source = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
        let SubChunkFormat::Paletted { storages, .. } = source.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "SubChunk {key:?} is not a supported paletted source for numeric target {target:?}"
            )));
        };
        if storages.is_empty() || storages.len() > 2 {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "SubChunk {key:?} has {} paletted storage layers; historical numeric representation supports one primary layer plus at most one BlockExtraData layer",
                storages.len()
            )));
        }

        let primary = &storages[0];
        let primary_indices = primary.surface_indices().ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunk {key:?} primary storage does not expose exactly 4096 valid palette indices"
            ))
        })?;
        let primary_numeric = resolve_primary_palette(primary, &key, numeric, &mut report)?;

        let mut builder = LegacySubChunkBuilder::zeroed(target_byte, false)?;
        for local_x in 0_u8..16 {
            for local_z in 0_u8..16 {
                for local_y in 0_u8..16 {
                    let block_index = storage_index(local_x, local_y, local_z);
                    let palette_index = usize::from(primary_indices[block_index]);
                    let (numeric_id, metadata) = *primary_numeric.get(palette_index).ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(format!(
                            "SubChunk {key:?} primary block index {block_index} references missing palette entry {palette_index}"
                        ))
                    })?;
                    builder.set_block(local_x, local_y, local_z, numeric_id, metadata)?;
                }
            }
        }

        if let Some(secondary) = storages.get(1) {
            report.second_storage_records = report.second_storage_records.saturating_add(1);
            stage_secondary_storage(
                secondary,
                key.pos,
                y,
                &key,
                numeric,
                &mut generated_block_extra,
                &mut report,
            )?;
        }

        let encoded = builder.build()?.into_raw();
        report.rewritten = report.rewritten.saturating_add(1);
        report.target_without_light_arrays =
            report.target_without_light_arrays.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(key.encode(), encoded);
        Ok(StorageVisitorControl::Continue)
    })?;

    for (pos, builder) in generated_block_extra {
        if builder.is_empty() {
            continue;
        }
        let value = builder.build()?.into_raw();
        report.block_extra_data_written = report.block_extra_data_written.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(value.len());
        batch.put(
            crate::chunk::ChunkKey::new(pos, ChunkRecordTag::BlockExtraData).encode(),
            value,
        );
    }

    Ok((batch, report))
}

fn resolve_primary_palette(
    palette: &crate::chunk::BlockPalette,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockUpgradeTable,
    report: &mut LegacyNumericSubChunkWriteReport,
) -> Result<Vec<(u8, u8)>> {
    let mut resolved = Vec::with_capacity(palette.states.len());
    for state in &palette.states {
        let value = unique_numeric_match(state, key, numeric, report)?;
        let numeric_id = u8::try_from(value.numeric_id).map_err(|_| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical numeric ID {} for BlockState {} exceeds fixed-array u8 storage",
                value.numeric_id, state.name
            ))
        })?;
        let metadata = u8::try_from(value.metadata).map_err(|_| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical metadata {} for BlockState {} exceeds u8",
                value.metadata, state.name
            ))
        })?;
        if metadata >= 16 {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical metadata {metadata} for primary BlockState {} cannot fit fixed-array four-bit block data",
                state.name
            )));
        }
        resolved.push((numeric_id, metadata));
    }
    Ok(resolved)
}

#[allow(clippy::too_many_arguments)]
fn stage_secondary_storage(
    palette: &crate::chunk::BlockPalette,
    pos: ChunkPos,
    subchunk_y: i8,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockUpgradeTable,
    generated: &mut BTreeMap<ChunkPos, LegacyBlockExtraDataBuilder>,
    report: &mut LegacyNumericSubChunkWriteReport,
) -> Result<()> {
    let indices = palette.surface_indices().ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!(
            "SubChunk {key:?} second storage does not expose exactly 4096 valid palette indices"
        ))
    })?;

    let mut resolved = Vec::<Option<(u8, u8)>>::with_capacity(palette.states.len());
    for state in &palette.states {
        if is_air_block_name(&state.name) {
            resolved.push(None);
            continue;
        }
        let value = unique_numeric_match(state, key, numeric, report)?;
        let numeric_id = u8::try_from(value.numeric_id).map_err(|_| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical second-layer numeric ID {} for BlockState {} exceeds BlockExtraData u8 storage",
                value.numeric_id, state.name
            ))
        })?;
        let metadata = u8::try_from(value.metadata).map_err(|_| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical second-layer metadata {} for BlockState {} exceeds BlockExtraData u8 storage",
                value.metadata, state.name
            ))
        })?;
        resolved.push(Some((numeric_id, metadata)));
    }

    let builder = generated.entry(pos).or_default();
    for local_x in 0_u8..16 {
        for local_z in 0_u8..16 {
            for local_y in 0_u8..16 {
                let block_index = storage_index(local_x, local_y, local_z);
                let palette_index = usize::from(indices[block_index]);
                let Some(mapping) = resolved.get(palette_index) else {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "SubChunk {key:?} second-layer block index {block_index} references missing palette entry {palette_index}"
                    )));
                };
                let Some((numeric_id, metadata)) = *mapping else {
                    continue;
                };
                let absolute_y = i32::from(subchunk_y) * 16 + i32::from(local_y);
                let absolute_y = u8::try_from(absolute_y).map_err(|_| {
                    BedrockWorldError::UnsupportedChunkFormat(format!(
                        "SubChunk {key:?} second-layer Y {absolute_y} cannot fit historical BlockExtraData full-Y u8 coordinate"
                    ))
                })?;
                builder
                    .push_chunk_coordinates(local_x, absolute_y, local_z, numeric_id, metadata)?;
                report.block_extra_entries_written =
                    report.block_extra_entries_written.saturating_add(1);
            }
        }
    }
    Ok(())
}

fn unique_numeric_match(
    state: &BlockState,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockUpgradeTable,
    report: &mut LegacyNumericSubChunkWriteReport,
) -> Result<crate::block::LegacyNumericBlock> {
    report.palette_entries_resolved = report.palette_entries_resolved.saturating_add(1);
    match numeric.match_numeric(state) {
        LegacyNumericBlockMatch::Unique(value) => Ok(value),
        LegacyNumericBlockMatch::Missing => {
            Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "BlockState {} {:?} from SubChunk {key:?} has no representation in the forward-verified historical numeric table ending at BlockState version {}",
                state.name,
                state.states,
                numeric.output_version().raw()
            )))
        }
        LegacyNumericBlockMatch::Ambiguous {
            first,
            second,
            matches,
        } => Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "BlockState {} {:?} from SubChunk {key:?} has {matches} historical numeric aliases after authoritative upgrade; first two are {}:{} and {}:{}, refusing to choose silently",
            state.name,
            state.states,
            first.numeric_id,
            first.metadata,
            second.numeric_id,
            second.metadata
        ))),
    }
}

fn validate_historical_subchunk_y(pos: ChunkPos, y: i8) -> Result<()> {
    if let Dimension::Unknown(id) = pos.dimension {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "cannot select historical fixed-array vertical range for unknown Bedrock dimension {id} at chunk ({}, {})",
            pos.x, pos.z
        )));
    }
    let (min_y, max_y) = pos.subchunk_index_range(ChunkVersion::Old);
    if y < min_y || y > max_y {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "SubChunk ({}, {}, {:?}) Y {y} lies outside historical fixed-array range {min_y}..={max_y}",
            pos.x, pos.z, pos.dimension
        )));
    }
    Ok(())
}

#[inline]
fn storage_index(local_x: u8, local_y: u8, local_z: u8) -> usize {
    usize::from(local_x) * 256 + usize::from(local_z) * 16 + usize::from(local_y)
}

fn legacy_target_byte(target: SubChunkVersion) -> Result<u8> {
    match target {
        SubChunkVersion::V0 => Ok(0),
        SubChunkVersion::V2 => Ok(2),
        SubChunkVersion::V3 => Ok(3),
        SubChunkVersion::V4 => Ok(4),
        SubChunkVersion::V5 => Ok(5),
        SubChunkVersion::V6 => Ok(6),
        SubChunkVersion::V7 => Ok(7),
        other => Err(BedrockWorldError::Validation(format!(
            "historical numeric fixed-array target must be SubChunk V0 or V2-V7, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{
        AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
        LegacyNumericBlockStateTable, LegacyNumericBlockUpgradeTable,
    };
    use crate::chunk::{BlockPalette, ChunkKey};
    use crate::storage::MemoryStorage;
    use crate::nbt::{NbtTag, serialize_root_nbt};
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

    fn numeric_table() -> LegacyNumericBlockStateTable {
        let version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let entries = [
            ("minecraft:test", 5_i64, 2_u32),
            ("minecraft:extra", 6_i64, 20_u32),
        ];
        let mut table = Vec::new();
        put_var_u32(entries.len() as u32, &mut table);
        let mut ids = BTreeMap::<String, i64>::new();
        for (name, numeric_id, metadata) in entries {
            ids.insert(name.to_string(), numeric_id);
            put_var_u32(name.len() as u32, &mut table);
            table.extend_from_slice(name.as_bytes());
            put_var_u32(1, &mut table);
            put_var_u32(metadata, &mut table);
            let root = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(version)),
            ]));
            table.extend(serialize_root_nbt(&root).unwrap());
        }
        LegacyNumericBlockStateTable::parse(&table, &serde_json::to_string(&ids).unwrap()).unwrap()
    }

    fn numeric_reverse() -> LegacyNumericBlockUpgradeTable {
        let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_identity.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":12,"maxVersionPatch":0,"maxVersionRevision":1}"#,
        }])
        .unwrap();
        LegacyNumericBlockUpgradeTable::build(&numeric_table(), &catalog).unwrap()
    }

    fn renamed_numeric_reverse() -> LegacyNumericBlockUpgradeTable {
        let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_rename.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":0,"renamedIds":{"minecraft:test":"minecraft:modern_test"}}"#,
        }])
        .unwrap();
        LegacyNumericBlockUpgradeTable::build(&numeric_table(), &catalog).unwrap()
    }

    fn block(name: &str) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::new(),
            version: Some(20_000_000),
        }
    }

    fn paletted_subchunk(second_storage: bool) -> SubChunk {
        let mut storages = vec![BlockPalette::with_unpacked_indices(
            vec![block("minecraft:test")],
            vec![0; 4096],
            Some(vec![4096]),
        )];
        if second_storage {
            let air = BlockState {
                name: "minecraft:air".to_string(),
                states: BTreeMap::new(),
                version: Some(20_000_000),
            };
            let mut indices = vec![0_u16; 4096];
            indices[storage_index(1, 2, 3)] = 1;
            storages.push(BlockPalette::with_unpacked_indices(
                vec![air, block("minecraft:extra")],
                indices,
                Some(vec![4095, 1]),
            ));
        }
        SubChunk {
            y: 0,
            format: SubChunkFormat::Paletted {
                version: 9,
                storages,
            },
        }
    }

    #[test]
    fn single_storage_paletted_subchunk_writes_unique_numeric_v7() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 2,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let source = paletted_subchunk(false).write_v9().unwrap();
        storage.put(&key, &source).unwrap();

        let (batch, report) = stage_paletted_subchunks_as_legacy_numeric(
            &storage,
            SubChunkVersion::V7,
            &numeric_reverse(),
        )
        .unwrap();
        assert_eq!(report.rewritten, 1);
        assert_eq!(report.palette_entries_resolved, 1);
        assert_eq!(report.target_without_light_arrays, 1);
        assert_eq!(
            storage.get(&key).unwrap().unwrap().first().copied(),
            Some(9)
        );

        storage.write_batch(&batch).unwrap();
        let target = storage.get(&key).unwrap().unwrap();
        let parsed = SubChunk::read(0, target, SubChunkDecodeMode::FullIndices).unwrap();
        let SubChunkFormat::LegacySubChunk(legacy) = parsed.format else {
            panic!("target must be fixed-array numeric");
        };
        assert_eq!(legacy.block_id_at(0, 0, 0), Some(5));
        assert_eq!(legacy.block_data_at(0, 0, 0), Some(2));
        assert!(!legacy.has_light_arrays());
    }

    #[test]
    fn writer_matches_modern_state_after_authoritative_rename() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 1,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let source = SubChunk {
            y: 0,
            format: SubChunkFormat::Paletted {
                version: 9,
                storages: vec![BlockPalette::with_unpacked_indices(
                    vec![block("minecraft:modern_test")],
                    vec![0; 4096],
                    Some(vec![4096]),
                )],
            },
        };
        storage.put(&key, &source.write_v9().unwrap()).unwrap();

        let (batch, _) = stage_paletted_subchunks_as_legacy_numeric(
            &storage,
            SubChunkVersion::V7,
            &renamed_numeric_reverse(),
        )
        .unwrap();
        storage.write_batch(&batch).unwrap();
        let parsed = SubChunk::read(
            0,
            storage.get(&key).unwrap().unwrap(),
            SubChunkDecodeMode::FullIndices,
        )
        .unwrap();
        let SubChunkFormat::LegacySubChunk(legacy) = parsed.format else {
            panic!("target must be fixed-array numeric");
        };
        assert_eq!(legacy.block_id_at(0, 0, 0), Some(5));
        assert_eq!(legacy.block_data_at(0, 0, 0), Some(2));
    }

    #[test]
    fn second_storage_becomes_block_extra_data() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: -3,
            z: 7,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        storage
            .put(&key, &paletted_subchunk(true).write_v9().unwrap())
            .unwrap();

        let (batch, report) = stage_paletted_subchunks_as_legacy_numeric(
            &storage,
            SubChunkVersion::V7,
            &numeric_reverse(),
        )
        .unwrap();
        assert_eq!(report.second_storage_records, 1);
        assert_eq!(report.block_extra_entries_written, 1);
        assert_eq!(report.block_extra_data_written, 1);
        storage.write_batch(&batch).unwrap();

        let extra_key = ChunkKey::new(pos, ChunkRecordTag::BlockExtraData).encode();
        let extra =
            crate::chunk::LegacyBlockExtraData::parse(storage.get(&extra_key).unwrap().unwrap())
                .unwrap();
        let entry = extra.entries().next().unwrap();
        assert_eq!(entry.chunk_coordinates(), Some((1, 2, 3)));
        assert_eq!(entry.block_id, 6);
        assert_eq!(entry.block_data, 20);
    }

    #[test]
    fn third_storage_is_never_silently_discarded() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 4,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        let mut source = paletted_subchunk(true);
        let SubChunkFormat::Paletted { storages, .. } = &mut source.format else {
            unreachable!();
        };
        storages.push(storages[0].clone());
        storage.put(&key, &source.write_v9().unwrap()).unwrap();

        assert!(
            stage_paletted_subchunks_as_legacy_numeric(
                &storage,
                SubChunkVersion::V7,
                &numeric_reverse(),
            )
            .is_err()
        );
        assert_eq!(
            storage.get(&key).unwrap().unwrap().first().copied(),
            Some(9)
        );
    }
}