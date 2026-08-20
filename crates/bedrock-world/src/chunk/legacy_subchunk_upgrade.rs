//! Historical numeric Minecraft Bedrock SubChunk upgrade into paletted BlockStates.

use crate::block::{
    AuthoritativeBlockStateCatalog, BlockState, LegacyNumericBlockStateTable,
    VanillaBlockStatePalette,
};
use crate::chunk::{
    BedrockDbKey, BlockPalette, ChunkPos, ChunkRecordTag, LegacyBlockExtraData,
    LegacyBlockExtraDataEntry, SubChunk, SubChunkFormat, SubChunkVersion, block_storage_index,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

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
    /// Number of historical chunk-scoped `BlockExtraData` records inspected.
    pub block_extra_data_records: usize,
    /// Number of second-layer block entries found in those records.
    pub block_extra_entries: usize,
    /// Number of target SubChunks that received a second paletted storage from `BlockExtraData`.
    pub block_extra_storage_layers: usize,
    /// Number of `BlockExtraData` records removed after every contained entry was merged successfully.
    ///
    /// This stays zero for V1 targets, where `BlockExtraData` remains part of the historical target
    /// representation and is therefore preserved byte-for-byte.
    pub block_extra_data_records_removed: usize,
    /// Number of block positions resolved from numeric id/metadata pairs, including second-layer entries.
    pub blocks_resolved: usize,
    /// Number of distinct legacy `(numeric id, metadata)` pairs resolved across all converted records.
    pub unique_numeric_states: usize,
    /// Number of resulting target palette entries across all converted storage layers after semantic deduplication.
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
            block_extra_data_records: 0,
            block_extra_entries: 0,
            block_extra_storage_layers: 0,
            block_extra_data_records_removed: 0,
            blocks_resolved: 0,
            unique_numeric_states: 0,
            target_palette_entries: 0,
            numeric_cache_hits: 0,
            staged_bytes: 0,
        }
    }
}

#[derive(Debug, Default)]
struct NumericResolutionCache {
    dense: Vec<Option<BlockState>>,
    wide: BTreeMap<u16, BlockState>,
}

impl NumericResolutionCache {
    fn new() -> Self {
        Self {
            dense: vec![None; NUMERIC_ID_META_SLOTS],
            wide: BTreeMap::new(),
        }
    }
}

struct BlockExtraDataIndex {
    by_subchunk: BTreeMap<(ChunkPos, i8), Vec<LegacyBlockExtraDataEntry>>,
    record_keys: BTreeMap<ChunkPos, Bytes>,
}

/// Preflights and stages every legacy numeric V0/V2-V7 SubChunk as a target paletted SubChunk.
///
/// The numeric table resolves historical `(id, meta)` values, the schema catalog applies authoritative
/// ordered BlockState upgrades, and the target vanilla palette proves that the upgraded semantic state
/// exists in the requested game while supplying its exact persisted BlockState version. For V8/V9
/// targets, chunk-scoped `BlockExtraData` is merged into a second paletted storage layer and removed
/// only after all entries have been assigned to concrete legacy SubChunks. V1 targets preserve the
/// historical `BlockExtraData` record instead. All records are converted before any storage mutation occurs.
pub(crate) fn stage_legacy_subchunks_for_upgrade(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
) -> Result<(StorageBatch, LegacySubChunkUpgradeWriteReport)> {
    if !matches!(
        target,
        SubChunkVersion::V1 | SubChunkVersion::V8 | SubChunkVersion::V9
    ) {
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

    let mut report = LegacySubChunkUpgradeWriteReport::new(target, target_palette);
    let extra_data = index_block_extra_data(storage, &mut report)?;
    let merge_extra_data = matches!(target, SubChunkVersion::V8 | SubChunkVersion::V9);

    let mut batch = StorageBatch::new();
    let mut resolved_numeric = NumericResolutionCache::new();
    let mut consumed_extra_subchunks = BTreeSet::<(ChunkPos, i8)>::new();

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
            report.source_light_array_records = report.source_light_array_records.saturating_add(1);
        }

        let primary = build_primary_storage(
            &legacy,
            &key,
            numeric,
            catalog,
            target_palette,
            &mut resolved_numeric,
            &mut report,
        )?;
        report.target_palette_entries = report
            .target_palette_entries
            .saturating_add(primary.states.len());

        let mut storages = Vec::with_capacity(2);
        storages.push(primary);
        if merge_extra_data {
            let extra_key = (key.pos, y);
            if let Some(entries) = extra_data.by_subchunk.get(&extra_key) {
                if let Some(secondary) = build_extra_storage(
                    entries,
                    y,
                    &key,
                    numeric,
                    catalog,
                    target_palette,
                    &mut resolved_numeric,
                    &mut report,
                )? {
                    report.block_extra_storage_layers =
                        report.block_extra_storage_layers.saturating_add(1);
                    report.target_palette_entries = report
                        .target_palette_entries
                        .saturating_add(secondary.states.len());
                    storages.push(secondary);
                }
                consumed_extra_subchunks.insert(extra_key);
            }
        }

        report.records = report.records.saturating_add(1);
        let target_subchunk = SubChunk {
            y,
            format: SubChunkFormat::Paletted {
                version: target.byte(),
                storages,
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

    if merge_extra_data {
        if let Some(((pos, y), entries)) = extra_data
            .by_subchunk
            .iter()
            .find(|(key, _)| !consumed_extra_subchunks.contains(key))
        {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "BlockExtraData for chunk ({}, {}, {:?}) references {} second-layer blocks in SubChunk Y {y}, but no legacy V0/V2-V7 SubChunk exists there",
                pos.x,
                pos.z,
                pos.dimension,
                entries.len()
            )));
        }
        for raw_key in extra_data.record_keys.values() {
            batch.delete(raw_key.clone());
            report.block_extra_data_records_removed =
                report.block_extra_data_records_removed.saturating_add(1);
        }
    }

    Ok((batch, report))
}

fn index_block_extra_data(
    storage: &dyn WorldStorage,
    report: &mut LegacySubChunkUpgradeWriteReport,
) -> Result<BlockExtraDataIndex> {
    let mut by_subchunk = BTreeMap::<(ChunkPos, i8), Vec<LegacyBlockExtraDataEntry>>::new();
    let mut record_keys = BTreeMap::<ChunkPos, Bytes>::new();

    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::BlockExtraData {
            return Ok(StorageVisitorControl::Continue);
        }
        if key.subchunk_y.is_some() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "chunk-scoped BlockExtraData key unexpectedly contains a SubChunk Y byte: {key:?}"
            )));
        }
        let data = LegacyBlockExtraData::parse(value.clone())?;
        if record_keys.insert(key.pos, key.encode()).is_some() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "duplicate BlockExtraData record for chunk {:?}",
                key.pos
            )));
        }
        report.block_extra_data_records = report.block_extra_data_records.saturating_add(1);
        report.block_extra_entries = report.block_extra_entries.saturating_add(data.len());

        for entry in data.entries() {
            let (_, y, _) = entry.chunk_coordinates().ok_or_else(|| {
                BedrockWorldError::UnsupportedChunkFormat(format!(
                    "BlockExtraData entry in chunk {:?} has unsupported high index bits: 0x{:08x}",
                    key.pos, entry.raw_index
                ))
            })?;
            let subchunk_y = i8::try_from(y / 16).map_err(|_| {
                BedrockWorldError::CorruptWorld(format!(
                    "BlockExtraData Y {y} cannot be represented as a legacy SubChunk index"
                ))
            })?;
            by_subchunk
                .entry((key.pos, subchunk_y))
                .or_default()
                .push(entry);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    Ok(BlockExtraDataIndex {
        by_subchunk,
        record_keys,
    })
}

fn build_primary_storage(
    legacy: &crate::chunk::LegacySubChunk,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
    resolved_numeric: &mut NumericResolutionCache,
    report: &mut LegacySubChunkUpgradeWriteReport,
) -> Result<BlockPalette> {
    let mut numeric_to_palette = [u16::MAX; NUMERIC_ID_META_SLOTS];
    let mut target_identity_to_palette = BTreeMap::<Vec<u8>, u16>::new();
    let mut states = Vec::<BlockState>::new();
    let mut indices = vec![0_u16; BLOCKS_PER_SUBCHUNK];
    let mut counts = Vec::<u16>::new();

    for block_index in 0..BLOCKS_PER_SUBCHUNK {
        let numeric_id = legacy.block_ids()[block_index];
        let metadata = nibble_at(legacy.block_data(), block_index);
        let numeric_slot = usize::from(numeric_id) * 16 + usize::from(metadata);

        let palette_index = if numeric_to_palette[numeric_slot] != u16::MAX {
            report.numeric_cache_hits = report.numeric_cache_hits.saturating_add(1);
            numeric_to_palette[numeric_slot]
        } else {
            let target_state = resolve_numeric_target(
                numeric_id,
                metadata,
                key,
                numeric,
                catalog,
                target_palette,
                resolved_numeric,
                report,
            )?;
            let palette_index = palette_index_for_state(
                target_state,
                &mut target_identity_to_palette,
                &mut states,
                &mut counts,
            )?;
            numeric_to_palette[numeric_slot] = palette_index;
            palette_index
        };

        indices[block_index] = palette_index;
        increment_count(
            &mut counts,
            palette_index,
            "primary legacy SubChunk palette",
        )?;
        report.blocks_resolved = report.blocks_resolved.saturating_add(1);
    }

    Ok(BlockPalette::with_unpacked_indices(
        states,
        indices,
        Some(counts),
    ))
}

fn build_extra_storage(
    entries: &[LegacyBlockExtraDataEntry],
    subchunk_y: i8,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
    resolved_numeric: &mut NumericResolutionCache,
    report: &mut LegacySubChunkUpgradeWriteReport,
) -> Result<Option<BlockPalette>> {
    if entries.is_empty() {
        return Ok(None);
    }

    let air = resolve_numeric_target(
        0,
        0,
        key,
        numeric,
        catalog,
        target_palette,
        resolved_numeric,
        report,
    )?;
    let mut states = vec![air.clone()];
    let mut target_identity_to_palette = BTreeMap::<Vec<u8>, u16>::new();
    target_identity_to_palette.insert(air.canonical_bytes()?, 0);
    let mut indices = vec![0_u16; BLOCKS_PER_SUBCHUNK];
    let mut counts = vec![u16::try_from(BLOCKS_PER_SUBCHUNK).map_err(|_| {
        BedrockWorldError::Validation("SubChunk block count exceeds u16".to_string())
    })?];
    let mut seen = [false; BLOCKS_PER_SUBCHUNK];

    for entry in entries {
        let (local_x, absolute_y, local_z) = entry.chunk_coordinates().ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "BlockExtraData entry in chunk {:?} has unsupported high index bits: 0x{:08x}",
                key.pos, entry.raw_index
            ))
        })?;
        let actual_subchunk_y = i8::try_from(absolute_y / 16).map_err(|_| {
            BedrockWorldError::CorruptWorld(format!(
                "BlockExtraData Y {absolute_y} cannot be represented as a SubChunk index"
            ))
        })?;
        if actual_subchunk_y != subchunk_y {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "BlockExtraData entry Y {absolute_y} was grouped under SubChunk {subchunk_y}, expected {actual_subchunk_y}"
            )));
        }
        let local_y = absolute_y & 0x0f;
        let block_index = block_storage_index(local_x, local_y, local_z);
        if seen[block_index] {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "BlockExtraData contains duplicate second-layer entry at ({local_x}, {absolute_y}, {local_z}) in chunk {:?}",
                key.pos
            )));
        }
        seen[block_index] = true;

        let target_state = resolve_numeric_target(
            entry.block_id,
            entry.block_data,
            key,
            numeric,
            catalog,
            target_palette,
            resolved_numeric,
            report,
        )?;
        let palette_index = if target_state.semantic_eq(&air) {
            0
        } else {
            palette_index_for_state(
                target_state,
                &mut target_identity_to_palette,
                &mut states,
                &mut counts,
            )?
        };
        indices[block_index] = palette_index;
        if palette_index != 0 {
            counts[0] = counts[0].checked_sub(1).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "BlockExtraData second-layer air count underflowed".to_string(),
                )
            })?;
            increment_count(
                &mut counts,
                palette_index,
                "BlockExtraData second-layer palette",
            )?;
        }
        report.blocks_resolved = report.blocks_resolved.saturating_add(1);
    }

    if states.len() == 1 {
        return Ok(None);
    }
    Ok(Some(BlockPalette::with_unpacked_indices(
        states,
        indices,
        Some(counts),
    )))
}

#[allow(clippy::too_many_arguments)]
fn resolve_numeric_target(
    numeric_id: u8,
    metadata: u8,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
    cache: &mut NumericResolutionCache,
    report: &mut LegacySubChunkUpgradeWriteReport,
) -> Result<BlockState> {
    if metadata < 16 {
        let slot = usize::from(numeric_id) * 16 + usize::from(metadata);
        if let Some(state) = &cache.dense[slot] {
            report.numeric_cache_hits = report.numeric_cache_hits.saturating_add(1);
            return Ok(state.clone());
        }
        let state =
            upgrade_numeric_state(numeric_id, metadata, key, numeric, catalog, target_palette)?;
        cache.dense[slot] = Some(state.clone());
        report.unique_numeric_states = report.unique_numeric_states.saturating_add(1);
        return Ok(state);
    }

    let wide_key = (u16::from(numeric_id) << 8) | u16::from(metadata);
    if let Some(state) = cache.wide.get(&wide_key) {
        report.numeric_cache_hits = report.numeric_cache_hits.saturating_add(1);
        return Ok(state.clone());
    }
    let state = upgrade_numeric_state(numeric_id, metadata, key, numeric, catalog, target_palette)?;
    cache.wide.insert(wide_key, state.clone());
    report.unique_numeric_states = report.unique_numeric_states.saturating_add(1);
    Ok(state)
}

fn upgrade_numeric_state(
    numeric_id: u8,
    metadata: u8,
    key: &crate::chunk::ChunkKey,
    numeric: &LegacyNumericBlockStateTable,
    catalog: &AuthoritativeBlockStateCatalog,
    target_palette: &VanillaBlockStatePalette,
) -> Result<BlockState> {
    let source_state = numeric
        .get(u32::from(numeric_id), u32::from(metadata))
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
    Ok(target_state.clone())
}

fn palette_index_for_state(
    state: BlockState,
    identity_to_palette: &mut BTreeMap<Vec<u8>, u16>,
    states: &mut Vec<BlockState>,
    counts: &mut Vec<u16>,
) -> Result<u16> {
    let identity = state.canonical_bytes()?;
    if let Some(index) = identity_to_palette.get(&identity) {
        return Ok(*index);
    }
    let index = u16::try_from(states.len()).map_err(|_| {
        BedrockWorldError::Validation("SubChunk target palette exceeds u16 entries".to_string())
    })?;
    states.push(state);
    counts.push(0);
    identity_to_palette.insert(identity, index);
    Ok(index)
}

fn increment_count(counts: &mut [u16], palette_index: u16, context: &str) -> Result<()> {
    let count = counts.get_mut(usize::from(palette_index)).ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!(
            "{context} index {palette_index} lost its count slot"
        ))
    })?;
    *count = count.checked_add(1).ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!("{context} usage count overflowed u16"))
    })?;
    Ok(())
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
    use crate::chunk::{
        ChunkKey, ChunkPos, Dimension, LegacyBlockExtraDataBuilder, LegacySubChunkBuilder,
        SubChunkDecodeMode,
    };
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
            (
                "minecraft:air",
                0_u32,
                numeric_state("minecraft:air", source_version),
            ),
            (
                "minecraft:old_test",
                2_u32,
                numeric_state("minecraft:old_test", source_version),
            ),
            (
                "minecraft:old_extra",
                20_u32,
                numeric_state("minecraft:old_extra", source_version),
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
            r#"{"minecraft:air":0,"minecraft:old_test":1,"minecraft:old_extra":2}"#,
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
                "renamedIds":{
                    "minecraft:old_test":"minecraft:new_test",
                    "minecraft:old_extra":"minecraft:new_extra"
                }
            }"#,
        }])
        .expect("catalog")
    }

    fn target_palette(catalog: &AuthoritativeBlockStateCatalog) -> VanillaBlockStatePalette {
        let target_version = catalog.output_version().raw();
        VanillaBlockStatePalette::new(
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
                BlockState {
                    name: "minecraft:new_extra".to_string(),
                    states: BTreeMap::new(),
                    version: Some(target_version),
                },
            ],
        )
        .expect("target palette")
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
        builder.set_block(1, 2, 3, 1, 2).expect("set legacy block");
        let source = builder.build().expect("V7").into_raw();
        storage.put(&key, &source).expect("seed V7");

        let catalog = catalog();
        let palette = target_palette(&catalog);
        let target_version = catalog.output_version().raw();
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
        assert_eq!(report.block_extra_entries, 0);
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

    #[test]
    fn block_extra_data_becomes_second_v8_storage_and_is_deleted_atomically() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: -4,
            z: 7,
            dimension: Dimension::Overworld,
        };
        let subchunk_key = ChunkKey::subchunk(pos, 0).encode();
        let mut builder = LegacySubChunkBuilder::zeroed(7, true).expect("V7 builder");
        builder
            .set_block(1, 2, 3, 1, 2)
            .expect("set primary legacy block");
        storage
            .put(&subchunk_key, &builder.build().expect("V7").into_raw())
            .expect("seed V7");

        let mut extra = LegacyBlockExtraDataBuilder::new();
        extra
            .push_chunk_coordinates(1, 2, 3, 2, 20)
            .expect("second-layer entry");
        let extra_key = ChunkKey::new(pos, ChunkRecordTag::BlockExtraData).encode();
        storage
            .put(&extra_key, extra.build().expect("extra data").raw())
            .expect("seed BlockExtraData");

        let catalog = catalog();
        let palette = target_palette(&catalog);
        let (batch, report) = stage_legacy_subchunks_for_upgrade(
            &storage,
            SubChunkVersion::V8,
            &numeric_table(),
            &catalog,
            &palette,
        )
        .expect("stage legacy + extra upgrade");
        assert_eq!(report.records, 1);
        assert_eq!(report.source_light_array_records, 1);
        assert_eq!(report.block_extra_data_records, 1);
        assert_eq!(report.block_extra_entries, 1);
        assert_eq!(report.block_extra_storage_layers, 1);
        assert_eq!(report.block_extra_data_records_removed, 1);
        assert!(
            storage
                .get(&extra_key)
                .expect("extra before commit")
                .is_some()
        );

        storage
            .write_batch(&batch)
            .expect("commit legacy + extra upgrade");
        assert!(
            storage
                .get(&extra_key)
                .expect("extra after commit")
                .is_none()
        );
        let value = storage
            .get(&subchunk_key)
            .expect("read upgraded SubChunk")
            .expect("upgraded SubChunk exists");
        let parsed = SubChunk::read(0, value, SubChunkDecodeMode::FullIndices).expect("parse V8");
        assert_eq!(
            parsed
                .block_state_at(1, 2, 3)
                .map(|state| state.name.as_str()),
            Some("minecraft:new_test")
        );
        assert_eq!(
            parsed
                .visible_block_state_at(1, 2, 3)
                .map(|state| state.name.as_str()),
            Some("minecraft:new_extra")
        );
    }

    #[test]
    fn v1_target_preserves_block_extra_data_record() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 1,
            dimension: Dimension::Overworld,
        };
        let subchunk_key = ChunkKey::subchunk(pos, 0).encode();
        storage
            .put(
                &subchunk_key,
                &LegacySubChunkBuilder::zeroed(7, false)
                    .expect("builder")
                    .build()
                    .expect("V7")
                    .into_raw(),
            )
            .expect("seed V7");
        let mut extra = LegacyBlockExtraDataBuilder::new();
        extra.push_chunk_coordinates(1, 2, 3, 2, 20).expect("extra");
        let extra_key = ChunkKey::new(pos, ChunkRecordTag::BlockExtraData).encode();
        storage
            .put(&extra_key, extra.build().expect("extra data").raw())
            .expect("seed extra");

        let catalog = catalog();
        let palette = target_palette(&catalog);
        let (batch, report) = stage_legacy_subchunks_for_upgrade(
            &storage,
            SubChunkVersion::V1,
            &numeric_table(),
            &catalog,
            &palette,
        )
        .expect("stage V1");
        assert_eq!(report.block_extra_data_records, 1);
        assert_eq!(report.block_extra_data_records_removed, 0);
        storage.write_batch(&batch).expect("commit V1");
        assert!(storage.get(&extra_key).expect("extra after V1").is_some());
    }
}
