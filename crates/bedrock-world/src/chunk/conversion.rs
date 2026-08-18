//! Historical chunk conversion into an authoritative paletted target representation.
//!
//! Physical format decoding, semantic BlockState migration and final palette validation are separate
//! stages. Unknown/future records are never rewritten implicitly, while mixed historical/current
//! chunks prefer explicit SubChunk records over overlapping `LegacyTerrain` block arrays.

mod historical;

pub use historical::{
    LegacyBlockMapping, LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource,
    ResolvedHistoricalSubChunk, ResolvedLegacyTerrain, resolve_legacy_subchunk,
    resolve_legacy_terrain,
};

use crate::biome::{Biome2d, Biome2dLegacy, promote_data2d_to_data3d};
use crate::block::{BlockPalette, BlockState, BlockStateMigrator};
use crate::chunk::encoding::encode_paletted_subchunk_from_palettes;
use crate::chunk::legacy::{LegacySubChunk, LegacyTerrain};
use crate::chunk::{
    ChunkKey, ChunkPos, ChunkRecordTag, ChunkVersion, SubChunkDecodeMode, SubChunkFormat,
    parse_subchunk_with_mode,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::integrity::SubChunkCodecKind;
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Explicit target schema for destructive historical chunk migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoricalChunkMigrationOptions {
    /// Target chunk version byte written to the `Version` record.
    pub target_chunk_version: u8,
    /// Target vertical generation used when expanding 2D biomes into 3D biome sections.
    pub target_chunk_generation: ChunkVersion,
    /// Target BlockState storage version required for every emitted palette entry.
    pub target_block_state_version: i32,
    /// Target paletted SubChunk codec version. Versions 1, 8 and 9 are supported.
    pub target_subchunk_version: u8,
    /// Convert legacy `Data2D`/`Data2DLegacy` or `LegacyTerrain` biome columns into `Data3D`.
    pub migrate_biomes_to_3d: bool,
}

/// Summary of records changed by one historical chunk migration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoricalChunkMigrationReport {
    /// Number of subchunks rewritten to the target paletted representation.
    pub subchunks_migrated: usize,
    /// Whether a `LegacyTerrain` record was consumed and deleted.
    pub legacy_terrain_removed: bool,
    /// Whether a 2D/legacy biome source was promoted to `Data3D`.
    pub biome_data_promoted: bool,
    /// Number of unique canonical BlockState permutations validated before write.
    pub block_states_validated: usize,
    /// Number of source palette entries eliminated after semantic migration produced duplicates.
    pub palette_entries_compacted: usize,
}

/// Converts one historical or mixed-version chunk to a validated target representation in one batch.
///
/// Historical numeric blocks must resolve through `resolver`. Every old BlockState must migrate
/// through `migrator`, and every final state must pass `target_palette_contains`. SubChunk versions
/// newer than this library understands and unknown chunk record tags abort before mutation. Explicit
/// SubChunk records take precedence over overlapping `LegacyTerrain` blocks because partially upgraded
/// Bedrock worlds can legitimately retain both generations.
pub fn migrate_historical_chunk_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    resolver: &dyn LegacyBlockResolver,
    migrator: &dyn BlockStateMigrator,
    target_palette_contains: &dyn Fn(&BlockState) -> bool,
    options: HistoricalChunkMigrationOptions,
) -> Result<HistoricalChunkMigrationReport> {
    if !matches!(options.target_subchunk_version, 1 | 8 | 9) {
        return Err(BedrockWorldError::Validation(format!(
            "target subchunk version must be 1, 8 or 9, got {}",
            options.target_subchunk_version
        )));
    }

    let prefix = chunk_prefix(pos);
    let mut legacy_terrain = None::<Bytes>;
    let mut source_subchunks = BTreeMap::<i8, Bytes>::new();
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
                        SubChunkCodecKind::LegacyV0
                        | SubChunkCodecKind::PalettedV1
                        | SubChunkCodecKind::LegacyV2ToV7(_)
                        | SubChunkCodecKind::PalettedV8
                        | SubChunkCodecKind::PalettedV9 => {
                            source_subchunks.insert(y, value.clone());
                        }
                        SubChunkCodecKind::UnknownFuture(version)
                        | SubChunkCodecKind::UnknownLegacy(version) => {
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
    if legacy_terrain.is_none() && source_subchunks.is_empty() && data2d.is_none() {
        return Ok(HistoricalChunkMigrationReport::default());
    }

    let mut layers = BTreeMap::<i8, Vec<BlockPalette>>::new();
    let mut terrain_biome_fallback = None::<Biome2d>;

    if let Some(raw) = &legacy_terrain {
        let terrain = LegacyTerrain::parse(raw.clone())?;
        let resolved = resolve_legacy_terrain(&terrain, resolver)?;
        terrain_biome_fallback = Some(Biome2d::new(
            resolved.heightmap.iter().copied().map(i16::from).collect(),
            resolved.biomes.iter().map(|sample| sample.biome_id).collect(),
        )?);
        for subchunk in resolved.subchunks {
            if !source_subchunks.contains_key(&subchunk.y) {
                layers.insert(subchunk.y, vec![subchunk.palette]);
            }
        }
    }

    for (y, raw) in &source_subchunks {
        let version = raw.first().copied();
        match SubChunkCodecKind::from_version(version) {
            SubChunkCodecKind::LegacyV0 | SubChunkCodecKind::LegacyV2ToV7(_) => {
                let legacy = LegacySubChunk::parse(raw.clone())?;
                let resolved = resolve_legacy_subchunk(*y, &legacy, resolver)?;
                layers.insert(*y, vec![resolved.palette]);
            }
            SubChunkCodecKind::PalettedV1
            | SubChunkCodecKind::PalettedV8
            | SubChunkCodecKind::PalettedV9 => {
                let parsed = parse_subchunk_with_mode(
                    *y,
                    raw.clone(),
                    SubChunkDecodeMode::FullIndices,
                )?;
                let SubChunkFormat::Paletted { storages, .. } = parsed.format else {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "known paletted subchunk at y={y} could not be decoded losslessly"
                    )));
                };
                layers.insert(*y, storages);
            }
            SubChunkCodecKind::UnknownFuture(_)
            | SubChunkCodecKind::UnknownLegacy(_)
            | SubChunkCodecKind::Unknown => unreachable!("filtered before migration"),
        }
    }

    let mut unique_states = BTreeSet::<Vec<u8>>::new();
    let mut compacted = 0usize;
    for storages in layers.values_mut() {
        for palette in storages {
            for state in &mut palette.states {
                *state = migrate_state(
                    state,
                    migrator,
                    options.target_block_state_version,
                    target_palette_contains,
                )?;
            }
            compacted = compacted.saturating_add(compact_palette(palette)?);
            for state in &palette.states {
                unique_states.insert(state.canonical_bytes()?);
            }
        }
    }

    let mut batch = StorageBatch::new();
    let mut report = HistoricalChunkMigrationReport {
        block_states_validated: unique_states.len(),
        palette_entries_compacted: compacted,
        ..HistoricalChunkMigrationReport::default()
    };
    for (y, storages) in &layers {
        let storage_refs = storages.iter().collect::<Vec<_>>();
        let encoded = encode_paletted_subchunk_from_palettes(
            options.target_subchunk_version,
            *y,
            &storage_refs,
        )?;
        batch.put(ChunkKey::subchunk(pos, *y).encode(), encoded);
        report.subchunks_migrated = report.subchunks_migrated.saturating_add(1);
    }

    if legacy_terrain.is_some() {
        batch.delete(ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode());
        report.legacy_terrain_removed = true;
    }

    if options.migrate_biomes_to_3d && !has_data3d {
        let source_biome = match data2d.as_ref() {
            Some((ChunkRecordTag::Data2D, raw)) => Some(Biome2d::parse(raw)?),
            Some((ChunkRecordTag::Data2DLegacy, raw)) => {
                Some(Biome2dLegacy::parse(raw)?.to_data2d()?)
            }
            Some(_) => unreachable!("only legacy 2D biome tags are collected"),
            None => terrain_biome_fallback,
        };
        if let Some(source_biome) = source_biome {
            let (min_section, max_section) =
                pos.subchunk_index_range(options.target_chunk_generation);
            let modern = promote_data2d_to_data3d(&source_biome, min_section..=max_section)?;
            batch.put(
                ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
                Bytes::from(modern.encode()?),
            );
            if let Some((source_tag, _)) = data2d {
                batch.delete(ChunkKey::new(pos, source_tag).encode());
            }
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
    migrator: &dyn BlockStateMigrator,
    target_version: i32,
    validator: &dyn Fn(&BlockState) -> bool,
) -> Result<BlockState> {
    // Always delegate, even when the source version already equals the selected target version.
    // Mojang has shipped schema changes without incrementing the BlockState version; authoritative
    // migrators must be allowed to apply those same-version schema groups.
    let migrated = migrator.migrate_to(state, target_version)?;
    if migrated.version != Some(target_version) {
        return Err(BedrockWorldError::Validation(format!(
            "BlockState migrator returned version {:?} for {}, expected {target_version}",
            migrated.version, migrated.name
        )));
    }
    if !validator(&migrated) {
        return Err(BedrockWorldError::Validation(format!(
            "historical block state {} is not registered in target authoritative palette",
            migrated.name
        )));
    }
    Ok(migrated)
}

fn compact_palette(palette: &mut BlockPalette) -> Result<usize> {
    let indices = palette.indices.as_mut().ok_or_else(|| {
        BedrockWorldError::Validation(
            "historical migration requires full palette indices".to_string(),
        )
    })?;
    if indices.len() != 4096 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "subchunk palette has {} indices instead of 4096",
            indices.len()
        )));
    }

    let old_states = std::mem::take(&mut palette.states);
    let old_len = old_states.len();
    let mut canonical = BTreeMap::<Vec<u8>, u16>::new();
    let mut remap = Vec::<u16>::with_capacity(old_len);
    let mut states = Vec::<BlockState>::with_capacity(old_len);
    for state in old_states {
        let mut key = state.canonical_bytes()?;
        if let Some(version) = state.version {
            key.extend_from_slice(&version.to_le_bytes());
        }
        let target = if let Some(index) = canonical.get(&key).copied() {
            index
        } else {
            let index = u16::try_from(states.len()).map_err(|_| {
                BedrockWorldError::Validation("palette exceeds u16".to_string())
            })?;
            canonical.insert(key, index);
            states.push(state);
            index
        };
        remap.push(target);
    }
    for index in indices.iter_mut() {
        *index = *remap.get(usize::from(*index)).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "palette index {} exceeds source palette length {}",
                *index, old_len
            ))
        })?;
    }
    let mut counts = vec![0_u16; states.len()];
    for index in indices.iter().copied() {
        counts[usize::from(index)] = counts[usize::from(index)].saturating_add(1);
    }
    palette.states = states;
    palette.counts = Some(counts);
    Ok(old_len.saturating_sub(palette.states.len()))
}

fn chunk_prefix(pos: ChunkPos) -> Vec<u8> {
    let encoded = ChunkKey::new(pos, ChunkRecordTag::Version).encode();
    encoded[..encoded.len().saturating_sub(1)].to_vec()
}
