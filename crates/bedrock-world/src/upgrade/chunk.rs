//! Historical chunk conversion into modern paletted Bedrock records.

use crate::chunk::key::{ChunkKey, ChunkRecordTag};
use crate::chunk::legacy::{LegacySubChunk, LegacyTerrain};
use crate::chunk::model::{ChunkPos, ChunkVersion};
use crate::chunk::palette::BlockState;
use crate::chunk::subchunk_write::encode_paletted_subchunk;
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::integrity::{CompatibilityLevel, SubChunkCodecKind};
use crate::parsed::Biome2d;
use crate::upgrade::{
    BlockStateMigrationGraph, LegacyBlockResolver, promote_data2d_to_data3d,
    resolve_legacy_subchunk, resolve_legacy_terrain,
};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Explicit target schema for destructive historical chunk migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoricalChunkMigrationOptions {
    /// Target chunk version byte written to the modern `Version` record.
    pub target_chunk_version: u8,
    /// Target BlockState storage version required for every emitted palette entry.
    pub target_block_state_version: i32,
    /// Target modern SubChunk codec version. Only 8 or 9 are supported.
    pub target_subchunk_version: u8,
    /// Convert legacy `Data2D`/`Data2DLegacy` into modern `Data3D`.
    pub migrate_biomes_to_3d: bool,
}

/// Summary of records changed by one historical chunk migration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoricalChunkMigrationReport {
    /// Number of historical subchunks rewritten as modern palettes.
    pub subchunks_migrated: usize,
    /// Whether a `LegacyTerrain` record was consumed and deleted.
    pub legacy_terrain_removed: bool,
    /// Whether a 2D biome record was promoted to `Data3D`.
    pub biome_data_promoted: bool,
    /// Number of unique canonical BlockState permutations validated before write.
    pub block_states_validated: usize,
}

/// Converts one historical chunk to modern palette records in a single storage batch.
///
/// The function never guesses target schema constants. Historical numeric blocks must resolve through
/// `resolver`; non-target BlockState versions must have a path in `graph`; every final state must pass
/// `target_palette_contains`. Unknown/future subchunks and ambiguous overlapping legacy sources abort
/// without mutating storage.
pub fn migrate_historical_chunk_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    resolver: &dyn LegacyBlockResolver,
    graph: &BlockStateMigrationGraph,
    target_palette_contains: &dyn Fn(&BlockState) -> bool,
    options: HistoricalChunkMigrationOptions,
) -> Result<HistoricalChunkMigrationReport> {
    if !matches!(options.target_subchunk_version, 8 | 9) {
        return Err(BedrockWorldError::Validation(format!(
            "target subchunk version must be 8 or 9, got {}",
            options.target_subchunk_version
        )));
    }

    let prefix = chunk_prefix(pos);
    let mut legacy_terrain = None::<Bytes>;
    let mut legacy_subchunks = BTreeMap::<i8, Bytes>::new();
    let mut data2d = None::<(ChunkRecordTag, Bytes)>;
    let mut has_data3d = false;
    let mut future_or_unknown = Vec::<(i8, Option<u8>)>::new();

    storage.for_each_prefix(
        &prefix,
        StorageReadOptions::default(),
        &mut |key, value| {
            let Ok(decoded) = ChunkKey::decode(key) else {
                return Ok(StorageVisitorControl::Continue);
            };
            if decoded.pos != pos {
                return Ok(StorageVisitorControl::Continue);
            }
            match decoded.tag {
                ChunkRecordTag::LegacyTerrain => legacy_terrain = Some(value.clone()),
                ChunkRecordTag::SubChunkPrefix => {
                    let y = decoded.subchunk_y.ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(
                            "SubChunkPrefix key is missing subchunk Y".to_string(),
                        )
                    })?;
                    match SubChunkCodecKind::from_version(value.first().copied()) {
                        SubChunkCodecKind::LegacyV0 | SubChunkCodecKind::LegacyV2ToV7(_) => {
                            legacy_subchunks.insert(y, value.clone());
                        }
                        SubChunkCodecKind::PalettedV1 => {
                            return Err(BedrockWorldError::UnsupportedChunkFormat(
                                "paletted v1 migration is not a numeric legacy conversion; preserve it until an explicit v1 upgrader is selected".to_string(),
                            ));
                        }
                        SubChunkCodecKind::PalettedV8 | SubChunkCodecKind::PalettedV9 => {}
                        SubChunkCodecKind::UnknownFuture(version) => {
                            future_or_unknown.push((y, Some(version)));
                        }
                        SubChunkCodecKind::UnknownLegacy(version) => {
                            future_or_unknown.push((y, Some(version)));
                        }
                        SubChunkCodecKind::Unknown => future_or_unknown.push((y, None)),
                    }
                }
                ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                    if data2d.is_some() {
                        return Err(BedrockWorldError::CorruptWorld(
                            "chunk contains multiple legacy 2D biome records".to_string(),
                        ));
                    }
                    data2d = Some((decoded.tag, value.clone()));
                }
                ChunkRecordTag::Data3D => has_data3d = true,
                ChunkRecordTag::Unknown(tag) => {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "chunk contains unknown record tag 0x{tag:02x}; migration refuses destructive rewrite"
                    )));
                }
                _ => {}
            }
            Ok(StorageVisitorControl::Continue)
        },
    )?;

    if !future_or_unknown.is_empty() {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "chunk contains {} unknown/future subchunk payloads; raw records were preserved",
            future_or_unknown.len()
        )));
    }
    if legacy_terrain.is_none() && legacy_subchunks.is_empty() && data2d.is_none() {
        return Ok(HistoricalChunkMigrationReport::default());
    }

    let mut modern_layers = BTreeMap::<i8, Vec<BlockState>>::new();
    if let Some(raw) = &legacy_terrain {
        let terrain = LegacyTerrain::parse(raw.clone())?;
        let resolved = resolve_legacy_terrain(&terrain, resolver)?;
        for subchunk in resolved.subchunks {
            if legacy_subchunks.contains_key(&subchunk.y) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "chunk has both LegacyTerrain and legacy SubChunkPrefix for y={}",
                    subchunk.y
                )));
            }
            modern_layers.insert(subchunk.y, subchunk.blocks);
        }
    }
    for (y, raw) in &legacy_subchunks {
        let legacy = LegacySubChunk::parse(raw.clone())?;
        let resolved = resolve_legacy_subchunk(*y, &legacy, resolver)?;
        modern_layers.insert(*y, resolved.blocks);
    }

    let mut unique_states = BTreeSet::<Vec<u8>>::new();
    for blocks in modern_layers.values_mut() {
        for state in blocks.iter_mut() {
            *state = migrate_state(
                state,
                graph,
                options.target_block_state_version,
                target_palette_contains,
            )?;
            unique_states.insert(state.canonical_bytes()?);
        }
    }

    let mut batch = StorageBatch::new();
    let mut report = HistoricalChunkMigrationReport {
        block_states_validated: unique_states.len(),
        ..HistoricalChunkMigrationReport::default()
    };
    for (y, blocks) in &modern_layers {
        let encoded = encode_paletted_subchunk(
            options.target_subchunk_version,
            *y,
            &[blocks.as_slice()],
        )?;
        batch.put(ChunkKey::subchunk(pos, *y).encode(), encoded);
        report.subchunks_migrated = report.subchunks_migrated.saturating_add(1);
    }

    if legacy_terrain.is_some() {
        batch.delete(ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode());
        report.legacy_terrain_removed = true;
    }

    if options.migrate_biomes_to_3d {
        if let Some((source_tag, raw)) = data2d {
            if has_data3d {
                return Err(BedrockWorldError::CorruptWorld(
                    "chunk contains Data3D and legacy Data2D simultaneously; migration requires explicit reconciliation"
                        .to_string(),
                ));
            }
            let old = Biome2d::parse(&raw)?;
            let (min_section, max_section) = pos.subchunk_index_range(ChunkVersion::New);
            let modern = promote_data2d_to_data3d(&old, min_section..=max_section)?;
            batch.put(
                ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
                Bytes::from(modern.encode()?),
            );
            batch.delete(ChunkKey::new(pos, source_tag).encode());
            report.biome_data_promoted = true;
        }
    }

    batch.put(
        ChunkKey::new(pos, ChunkRecordTag::Version).encode(),
        Bytes::from(vec![options.target_chunk_version]),
    );
    storage.write_batch(&batch)?;
    Ok(report)
}

fn migrate_state(
    state: &BlockState,
    graph: &BlockStateMigrationGraph,
    target_version: i32,
    validator: &dyn Fn(&BlockState) -> bool,
) -> Result<BlockState> {
    let migrated = if state.version == Some(target_version) {
        state.clone()
    } else {
        graph.migrate_to(state, target_version)?
    };
    if !validator(&migrated) {
        return Err(BedrockWorldError::Validation(format!(
            "historical block state {} is not registered in target authoritative palette",
            migrated.name
        )));
    }
    Ok(migrated)
}

fn chunk_prefix(pos: ChunkPos) -> Vec<u8> {
    let encoded = ChunkKey::new(pos, ChunkRecordTag::Version).encode();
    encoded[..encoded.len().saturating_sub(1)].to_vec()
}

#[allow(dead_code)]
const fn _migration_compatibility_marker() -> CompatibilityLevel {
    CompatibilityLevel::MigrationRequired
}
