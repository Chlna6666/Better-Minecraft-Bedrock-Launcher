//! Explicit paletted Minecraft Bedrock SubChunk writes to historical fixed-array numeric versions.
//!
//! This module is a physical SubChunk representation write. It does not infer a Minecraft game
//! version and it never reverses BlockState upgrade rules. A palette entry is writable only when the
//! caller-supplied historical numeric table contains exactly one semantically identical `(id, meta)`.

use crate::block::{
    BlockState, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
};
use crate::chunk::{
    BedrockDbKey, ChunkPos, ChunkRecordTag, LegacySubChunkBuilder, SubChunk, SubChunkDecodeMode,
    SubChunkFormat, SubChunkVersion,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
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
    /// Number of distinct semantic BlockStates resolved through the historical numeric table.
    pub unique_block_states: usize,
    /// Number of repeated semantic BlockState resolutions served from the operation cache.
    pub match_cache_hits: usize,
    /// Number of rewritten target payloads intentionally emitted in the historical short form without
    /// sky/block light arrays, because paletted source SubChunks do not contain those values to preserve.
    pub target_without_light_arrays: usize,
    /// Total encoded value bytes staged in the atomic storage batch.
    pub staged_bytes: usize,
}

impl LegacyNumericSubChunkWriteReport {
    fn new(target: SubChunkVersion) -> Self {
        Self {
            target,
            records: 0,
            unchanged: 0,
            rewritten: 0,
            unique_block_states: 0,
            match_cache_hits: 0,
            target_without_light_arrays: 0,
            staged_bytes: 0,
        }
    }
}

#[derive(Default)]
struct NumericMatchCache {
    by_name: BTreeMap<String, Vec<(BTreeMap<String, NbtTag>, LegacyNumericBlockMatch)>>,
}

impl NumericMatchCache {
    fn resolve(
        &mut self,
        table: &LegacyNumericBlockStateTable,
        state: &BlockState,
        report: &mut LegacyNumericSubChunkWriteReport,
    ) -> LegacyNumericBlockMatch {
        if let Some(permutations) = self.by_name.get(state.name.as_str())
            && let Some((_, result)) = permutations
                .iter()
                .find(|(states, _)| states == &state.states)
        {
            report.match_cache_hits = report.match_cache_hits.saturating_add(1);
            return *result;
        }

        let result = table.match_numeric(state);
        self.by_name
            .entry(state.name.clone())
            .or_default()
            .push((state.states.clone(), result));
        report.unique_block_states = report.unique_block_states.saturating_add(1);
        result
    }
}

/// Preflights all SubChunks and stages exact paletted -> fixed-array numeric writes in one batch.
///
/// Target versions are V0 or V2-V7. A source already using the exact target version is preserved
/// byte-for-byte. A paletted source must have exactly one storage layer. Every palette state must have
/// one and only one semantic match in `numeric`, with numeric ID <= 255 and metadata < 16. Existing
/// `BlockExtraData` beside a paletted source is rejected because this operation does not guess how a
/// mixed historical second layer relates to the modern storage.
///
/// Paletted source records do not persist the historical sky/block light nibble arrays. Rewritten
/// targets therefore use the valid short fixed-array form (`id + metadata`) instead of manufacturing
/// lighting values. This is an explicit representation write, not a claim that a complete older game
/// version conversion has finished.
pub(crate) fn stage_paletted_subchunks_as_legacy_numeric(
    storage: &dyn WorldStorage,
    target: SubChunkVersion,
    numeric: &LegacyNumericBlockStateTable,
) -> Result<(StorageBatch, LegacyNumericSubChunkWriteReport)> {
    let target_byte = legacy_target_byte(target)?;

    let mut block_extra_chunks = BTreeSet::<ChunkPos>::new();
    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        if let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key)
            && key.tag == ChunkRecordTag::BlockExtraData
        {
            block_extra_chunks.insert(key.pos);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    let mut batch = StorageBatch::new();
    let mut report = LegacyNumericSubChunkWriteReport::new(target);
    let mut cache = NumericMatchCache::default();

    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }

        report.records = report.records.saturating_add(1);
        let source_version = SubChunkVersion::detect(value).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunk record {key:?} has an empty payload"
            ))
        })?;
        if source_version == target {
            let parsed = SubChunk::read(
                key.subchunk_y.ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(format!(
                        "SubChunkPrefix key {key:?} has no SubChunk Y byte"
                    ))
                })?,
                value.clone(),
                SubChunkDecodeMode::FullIndices,
            )?;
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
        if block_extra_chunks.contains(&key.pos) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "paletted SubChunk {key:?} coexists with historical BlockExtraData; refusing numeric rewrite until the second-layer relationship is explicit"
            )));
        }

        let y = key.subchunk_y.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunkPrefix key {key:?} has no SubChunk Y byte"
            ))
        })?;
        let source = SubChunk::read(y, value.clone(), SubChunkDecodeMode::FullIndices)?;
        let SubChunkFormat::Paletted { storages, .. } = source.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "SubChunk {key:?} is not a supported paletted source for numeric target {target:?}"
            )));
        };
        if storages.len() != 1 {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "SubChunk {key:?} has {} paletted storage layers; fixed-array numeric write currently requires exactly one so a second layer is never discarded",
                storages.len()
            )));
        }
        let storage_layer = &storages[0];
        let indices = storage_layer.surface_indices().ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "SubChunk {key:?} does not expose exactly 4096 valid palette indices"
            ))
        })?;

        let mut numeric_palette = Vec::<(u8, u8)>::with_capacity(storage_layer.states.len());
        for state in &storage_layer.states {
            let matched = cache.resolve(numeric, state, &mut report);
            let value = match matched {
                LegacyNumericBlockMatch::Unique(value) => value,
                LegacyNumericBlockMatch::Missing => {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "BlockState {} {:?} from SubChunk {key:?} has no representation in the supplied historical numeric table",
                        state.name, state.states
                    )));
                }
                LegacyNumericBlockMatch::Ambiguous {
                    first,
                    second,
                    matches,
                } => {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "BlockState {} {:?} from SubChunk {key:?} has {matches} historical numeric aliases; first two are {}:{} and {}:{}, refusing to choose silently",
                        state.name,
                        state.states,
                        first.numeric_id,
                        first.metadata,
                        second.numeric_id,
                        second.metadata
                    )));
                }
            };
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
                    "historical metadata {metadata} for BlockState {} cannot fit fixed-array four-bit block data",
                    state.name
                )));
            }
            numeric_palette.push((numeric_id, metadata));
        }

        let mut builder = LegacySubChunkBuilder::zeroed(target_byte, false)?;
        for local_x in 0_u8..16 {
            for local_z in 0_u8..16 {
                for local_y in 0_u8..16 {
                    let block_index = usize::from(local_x) * 256
                        + usize::from(local_z) * 16
                        + usize::from(local_y);
                    let palette_index = usize::from(indices[block_index]);
                    let (numeric_id, metadata) = *numeric_palette.get(palette_index).ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(format!(
                            "SubChunk {key:?} block index {block_index} references missing palette entry {palette_index}"
                        ))
                    })?;
                    builder.set_block(
                        local_x,
                        local_y,
                        local_z,
                        numeric_id,
                        metadata,
                    )?;
                }
            }
        }

        let encoded = builder.build()?.into_raw();
        report.rewritten = report.rewritten.saturating_add(1);
        report.target_without_light_arrays =
            report.target_without_light_arrays.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(encoded.len());
        batch.put(key.encode(), encoded);
        Ok(StorageVisitorControl::Continue)
    })?;

    Ok((batch, report))
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
    use crate::block::BlockStateStorageVersion;
    use crate::chunk::{BlockPalette, ChunkKey, Dimension};
    use crate::database::MemoryStorage;
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

    fn numeric_table(aliases: &[u32]) -> LegacyNumericBlockStateTable {
        let version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let root = NbtTag::Compound(IndexMap::from([
            (
                "name".to_string(),
                NbtTag::String("minecraft:test".to_string()),
            ),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(version)),
        ]));
        let nbt = serialize_root_nbt(&root).unwrap();
        let mut table = Vec::new();
        put_var_u32(1, &mut table);
        put_var_u32("minecraft:test".len() as u32, &mut table);
        table.extend_from_slice(b"minecraft:test");
        put_var_u32(aliases.len() as u32, &mut table);
        for &metadata in aliases {
            put_var_u32(metadata, &mut table);
            table.extend_from_slice(&nbt);
        }
        LegacyNumericBlockStateTable::parse(&table, r#"{"minecraft:test":5}"#).unwrap()
    }

    fn paletted_subchunk(storages: usize) -> SubChunk {
        let state = BlockState {
            name: "minecraft:test".to_string(),
            states: BTreeMap::new(),
            version: Some(20_000_000),
        };
        SubChunk {
            y: 0,
            format: SubChunkFormat::Paletted {
                version: 9,
                storages: (0..storages)
                    .map(|_| {
                        BlockPalette::with_unpacked_indices(
                            vec![state.clone()],
                            vec![0; 4096],
                            Some(vec![4096]),
                        )
                    })
                    .collect(),
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
        let source = paletted_subchunk(1).write_v9().unwrap();
        storage.put(&key, &source).unwrap();

        let (batch, report) = stage_paletted_subchunks_as_legacy_numeric(
            &storage,
            SubChunkVersion::V7,
            &numeric_table(&[2]),
        )
        .unwrap();
        assert_eq!(report.rewritten, 1);
        assert_eq!(report.target_without_light_arrays, 1);
        assert_eq!(storage.get(&key).unwrap().unwrap().first().copied(), Some(9));

        storage.write_batch(&batch).unwrap();
        let target = storage.get(&key).unwrap().unwrap();
        assert_eq!(target.first().copied(), Some(7));
        let parsed = SubChunk::read(0, target, SubChunkDecodeMode::FullIndices).unwrap();
        let SubChunkFormat::LegacySubChunk(legacy) = parsed.format else {
            panic!("target must be fixed-array numeric");
        };
        assert_eq!(legacy.block_id_at(0, 0, 0), Some(5));
        assert_eq!(legacy.block_data_at(0, 0, 0), Some(2));
        assert!(!legacy.has_light_arrays());
    }

    #[test]
    fn ambiguous_numeric_alias_aborts_before_write() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 3,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        storage.put(&key, &paletted_subchunk(1).write_v9().unwrap()).unwrap();

        assert!(
            stage_paletted_subchunks_as_legacy_numeric(
                &storage,
                SubChunkVersion::V7,
                &numeric_table(&[0, 1]),
            )
            .is_err()
        );
        assert_eq!(storage.get(&key).unwrap().unwrap().first().copied(), Some(9));
    }

    #[test]
    fn second_storage_is_never_silently_discarded() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 4,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0).encode();
        storage.put(&key, &paletted_subchunk(2).write_v9().unwrap()).unwrap();

        assert!(
            stage_paletted_subchunks_as_legacy_numeric(
                &storage,
                SubChunkVersion::V7,
                &numeric_table(&[2]),
            )
            .is_err()
        );
        assert_eq!(storage.get(&key).unwrap().unwrap().first().copied(), Some(9));
    }
}
