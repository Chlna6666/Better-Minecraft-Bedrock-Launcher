//! Compact business-level 2D surface-map queries.
//!
//! This API intentionally does not expose `SubChunk`, full 3D block indices, block entities or
//! general `ChunkData`. The public contract is always exact. Modern paletted terrain is read through
//! one exact multi-get, retained as packed palette words and projected directly into a fixed 16x16
//! column plane plus a chunk-local material table. Full `BlockState` values are cloned only once per
//! unique material, never once per output column.
//!
//! Historical `LegacyTerrain` and numeric SubChunk formats deliberately fall back to the canonical
//! compatibility loader. That keeps old-world correctness independent from the modern 2D fast path
//! and does not affect the 3D `ChunkData`/`BlockState` contract.

use super::{
    BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority, ChunkLoadStats,
    ExactSurfaceSubchunkPolicy, TerrainColumnBiome, TerrainSampleSource, World,
    WorldPipelineOptions, StorageBackend, WorldThreadingOptions,
};
use crate::chunk::{
    BlockState, ChunkKey, ChunkPos, ChunkRecordTag, ChunkVersion, SubChunk, SubChunkDecodeMode,
    SubChunkFormat,
};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::scan::{
    BiomeData, BiomeStorage, parse_data2d_legacy, parse_data3d, parse_legacy_data2d,
};
use crate::storage::{
    StorageCachePolicy, StorageKeyBatchBuilder, StoragePipelineOptions, StorageReadOptions,
    StorageScanMode, StorageThreadingOptions,
};
use crate::surface::{TerrainSurfaceRole, terrain_surface_role};
use bytes::Bytes;
use rayon::{ThreadPool, ThreadPoolBuilder, prelude::*};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const SURFACE_COLUMN_COUNT: usize = 16 * 16;
const NO_MATERIAL: u16 = u16::MAX;
const NO_HEIGHT: i16 = i16::MIN;
const ESTIMATED_CHUNK_KEY_BYTES: usize = 14;

/// Compact material referenced by one or more exact 2D map columns.
///
/// `version` from the general `BlockState` representation is intentionally omitted because it does
/// not affect 2D palette selection. State properties are retained because render palettes may define
/// state-specific color overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMapMaterial {
    /// Canonical Bedrock block identifier.
    pub name: String,
    /// State properties needed for exact state-sensitive palette selection.
    pub states: BTreeMap<String, NbtTag>,
}

impl SurfaceMapMaterial {
    fn from_state(state: &BlockState) -> Self {
        Self {
            name: state.name.clone(),
            states: state.states.clone(),
        }
    }

    fn matches_state(&self, state: &BlockState) -> bool {
        self.name == state.name && self.states == state.states
    }
}

/// One compact exact 2D terrain column in local `z * 16 + x` order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMapColumn {
    /// Y coordinate of the visible surface block.
    pub surface_y: i16,
    /// Material id of the visible surface block.
    pub surface_material: u16,
    /// Y coordinate of the relief/support block.
    pub relief_y: i16,
    /// Material id of the relief/support block.
    pub relief_material: u16,
    /// Internal thin-overlay Y coordinate; use [`Self::overlay_y`] for the optional view.
    overlay_y: i16,
    /// Material id of a thin overlay, or an internal sentinel when absent.
    overlay_material: u16,
    /// Water depth above the underwater support block.
    pub water_depth: u8,
    /// Material id of visible water, or an internal sentinel when absent.
    water_material: u16,
    /// Internal underwater-support Y coordinate; use [`Self::underwater_y`] for the optional view.
    underwater_y: i16,
    /// Material id of the underwater support block, or an internal sentinel when absent.
    underwater_material: u16,
    /// Biome context used by the 2D palette.
    pub biome: Option<TerrainColumnBiome>,
    /// Storage family that produced the visible surface.
    pub source: TerrainSampleSource,
}

impl SurfaceMapColumn {
    /// Returns the optional overlay Y coordinate.
    #[must_use]
    pub const fn overlay_y(self) -> Option<i16> {
        if self.overlay_y == NO_HEIGHT {
            None
        } else {
            Some(self.overlay_y)
        }
    }

    /// Returns the optional overlay material id.
    #[must_use]
    pub const fn overlay_material(self) -> Option<u16> {
        if self.overlay_material == NO_MATERIAL {
            None
        } else {
            Some(self.overlay_material)
        }
    }

    /// Returns the optional visible-water material id.
    #[must_use]
    pub const fn water_material(self) -> Option<u16> {
        if self.water_material == NO_MATERIAL {
            None
        } else {
            Some(self.water_material)
        }
    }

    /// Returns the optional underwater-support Y coordinate.
    #[must_use]
    pub const fn underwater_y(self) -> Option<i16> {
        if self.underwater_y == NO_HEIGHT {
            None
        } else {
            Some(self.underwater_y)
        }
    }

    /// Returns the optional underwater-support material id.
    #[must_use]
    pub const fn underwater_material(self) -> Option<u16> {
        if self.underwater_material == NO_MATERIAL {
            None
        } else {
            Some(self.underwater_material)
        }
    }
}

/// Compact exact 16x16 surface plane for one requested chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMapChunk {
    /// Chunk position represented by this plane.
    pub pos: ChunkPos,
    /// Deduplicated render materials referenced by the 256 columns.
    pub materials: Vec<SurfaceMapMaterial>,
    /// Columns in `z * 16 + x` order. Missing columns stay `None`.
    columns: Box<[Option<SurfaceMapColumn>; SURFACE_COLUMN_COUNT]>,
}

impl SurfaceMapChunk {
    /// Returns one local 2D map column.
    #[must_use]
    pub fn column(&self, local_x: u8, local_z: u8) -> Option<&SurfaceMapColumn> {
        if local_x >= 16 || local_z >= 16 {
            return None;
        }
        self.columns[usize::from(local_z) * 16 + usize::from(local_x)].as_ref()
    }

    /// Returns the fixed 256-column plane in `z * 16 + x` order.
    #[must_use]
    pub fn columns(&self) -> &[Option<SurfaceMapColumn>; SURFACE_COLUMN_COUNT] {
        &self.columns
    }

    /// Resolves a compact material id.
    #[must_use]
    pub fn material(&self, id: u16) -> Option<&SurfaceMapMaterial> {
        self.materials.get(usize::from(id))
    }
}

/// Controls an exact compact 2D surface-map batch query.
///
/// There is deliberately no public `HintThenVerify`/`Full` correctness switch. A call to
/// [`World::query_surface_map_many`] always means an exact persisted-world surface.
/// The modern path currently issues the complete dimension SubChunk key range through exact batch
/// reads; future hinting is permitted only when it can prove the same result before returning.
#[derive(Debug, Clone)]
pub struct SurfaceMapQueryOptions {
    /// Threading policy for independent chunk decoding and projection.
    pub threading: WorldThreadingOptions,
    /// Bounded chunk/decode pipeline settings.
    pub pipeline: WorldPipelineOptions,
    /// Chunk ordering policy.
    pub priority: ChunkLoadPriority,
    /// Backend cache policy for exact storage reads.
    pub storage_cache_policy: StorageCachePolicy,
}

impl Default for SurfaceMapQueryOptions {
    fn default() -> Self {
        Self {
            threading: WorldThreadingOptions::Auto,
            pipeline: WorldPipelineOptions::default(),
            priority: ChunkLoadPriority::RowMajor,
            storage_cache_policy: StorageCachePolicy::Use,
        }
    }
}

/// Diagnostics returned by an exact compact 2D surface-map batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceMapBatchStats {
    /// Underlying exact storage/decode statistics.
    pub load: ChunkLoadStats,
    /// Number of compact chunks returned.
    pub chunks: usize,
    /// Number of populated surface columns returned.
    pub columns: usize,
    /// Number of unique per-chunk materials retained after compaction.
    pub materials: usize,
}

#[derive(Debug)]
struct RawSurfaceChunk {
    pos: ChunkPos,
    biome_record: Option<(ChunkRecordTag, Bytes)>,
    subchunks: BTreeMap<i8, Bytes>,
    legacy_terrain: Option<Bytes>,
    found_any: bool,
}

impl RawSurfaceChunk {
    fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            biome_record: None,
            subchunks: BTreeMap::new(),
            legacy_terrain: None,
            found_any: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SurfaceRecordKind {
    LegacyTerrain,
    Data3D,
    Data2D,
    Data2DLegacy,
    Subchunk(i8),
}

#[derive(Debug, Clone, Copy)]
struct SurfaceRecordRequest {
    chunk_index: usize,
    kind: SurfaceRecordKind,
}

#[derive(Debug, Clone, Copy, Default)]
struct DirectSurfaceTiming {
    biome_us: u128,
    subchunk_us: u128,
    surface_us: u128,
}

#[derive(Debug)]
struct DirectSurfaceChunk {
    chunk: SurfaceMapChunk,
    timing: DirectSurfaceTiming,
    subchunks_decoded: usize,
    raw_height_mismatches: usize,
    missing_columns: usize,
}

#[derive(Debug)]
enum SurfaceDecodeOutput {
    Direct(DirectSurfaceChunk),
    CompatibilityFallback(ChunkPos),
}

#[derive(Debug, Clone, Copy)]
struct PendingOverlay {
    y: i16,
    material: u16,
}

#[derive(Debug, Clone, Copy)]
struct PendingWater {
    y: i16,
    material: u16,
    source: TerrainSampleSource,
}

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Loads compact exact 2D map data for explicit chunk positions.
    ///
    /// Modern V1/V8/V9 paletted SubChunks are decoded with [`SubChunkDecodeMode::SurfaceColumns`]
    /// and projected directly from packed words into material ids. The fast path never constructs
    /// `ChunkData`, `TerrainColumnSamples`, full 4096-entry index arrays, or per-column owned
    /// `BlockState` values.
    ///
    /// Historical numeric terrain automatically falls back to the canonical exact compatibility
    /// loader so this optimization cannot change old-world semantics.
    pub fn query_surface_map_many(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: SurfaceMapQueryOptions,
    ) -> Result<(Vec<SurfaceMapChunk>, SurfaceMapBatchStats)> {
        let started = Instant::now();
        let mut positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok((Vec::new(), SurfaceMapBatchStats::default()));
        }
        sort_surface_positions(&mut positions, options.priority);
        let worker_count = options.threading.resolve_checked(positions.len())?;

        let mut raw_chunks = positions
            .iter()
            .copied()
            .map(RawSurfaceChunk::new)
            .collect::<Vec<_>>();
        let estimated_key_count = positions.len().saturating_mul(28);
        let mut key_builder = StorageKeyBatchBuilder::with_capacity(
            estimated_key_count.saturating_mul(ESTIMATED_CHUNK_KEY_BYTES),
            estimated_key_count,
        );
        let mut requests = Vec::with_capacity(estimated_key_count);
        for (chunk_index, pos) in positions.iter().copied().enumerate() {
            push_surface_key(
                &mut key_builder,
                &mut requests,
                chunk_index,
                ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain),
                SurfaceRecordKind::LegacyTerrain,
            );
            push_surface_key(
                &mut key_builder,
                &mut requests,
                chunk_index,
                ChunkKey::new(pos, ChunkRecordTag::Data3D),
                SurfaceRecordKind::Data3D,
            );
            push_surface_key(
                &mut key_builder,
                &mut requests,
                chunk_index,
                ChunkKey::new(pos, ChunkRecordTag::Data2D),
                SurfaceRecordKind::Data2D,
            );
            push_surface_key(
                &mut key_builder,
                &mut requests,
                chunk_index,
                ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy),
                SurfaceRecordKind::Data2DLegacy,
            );
            let (min_subchunk_y, max_subchunk_y) = pos.subchunk_index_range(ChunkVersion::New);
            for y in min_subchunk_y..=max_subchunk_y {
                push_surface_key(
                    &mut key_builder,
                    &mut requests,
                    chunk_index,
                    ChunkKey::subchunk(pos, y),
                    SurfaceRecordKind::Subchunk(y),
                );
            }
        }
        let keys = key_builder.finish();
        let storage_options = surface_storage_read_options(&options);
        let db_started = Instant::now();
        let values = self
            .storage()
            .get_many_ordered(keys.keys(), storage_options)?;
        let db_read_ms = db_started.elapsed().as_millis();
        if values.len() != requests.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "surface exact batch returned {} values for {} keys",
                values.len(),
                requests.len()
            )));
        }
        let keys_found = apply_surface_values(&mut raw_chunks, &requests, values)?;
        let loaded_chunks = raw_chunks.iter().filter(|raw| raw.found_any).count();

        let decode_started = Instant::now();
        let decoded = if worker_count == 1 {
            raw_chunks
                .into_iter()
                .map(decode_direct_surface_chunk)
                .collect::<Result<Vec<_>>>()?
        } else {
            let pool = surface_decode_pool(worker_count)?;
            pool.install(|| {
                raw_chunks
                    .into_par_iter()
                    .map(decode_direct_surface_chunk)
                    .collect::<Result<Vec<_>>>()
            })?
        };
        let direct_decode_ms = decode_started.elapsed().as_millis();

        let fallback_positions = decoded
            .iter()
            .filter_map(|decoded| match decoded {
                SurfaceDecodeOutput::CompatibilityFallback(pos) => Some(*pos),
                SurfaceDecodeOutput::Direct(_) => None,
            })
            .collect::<Vec<_>>();
        let mut fallback_chunks = BTreeMap::<ChunkPos, SurfaceMapChunk>::new();
        let mut fallback_stats = ChunkLoadStats::default();
        if !fallback_positions.is_empty() {
            let mut load_options = ChunkLoadOptions::for_data_request(
                ChunkDataRequest::new()
                    .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                    .biome(BiomeDataRequirement::SurfaceColumns),
            );
            load_options.subchunk_decode = SubChunkDecodeMode::SurfaceColumns;
            load_options.threading = options.threading;
            load_options.pipeline = options.pipeline;
            load_options.priority = options.priority;
            load_options.storage_cache_policy = options.storage_cache_policy;
            let (chunks, stats) =
                self.query_chunk_data_with_stats(fallback_positions, load_options)?;
            fallback_stats = stats;
            for chunk in chunks {
                fallback_chunks.insert(chunk.pos, compact_surface_chunk(&chunk)?);
            }
        }

        let mut output = Vec::with_capacity(decoded.len());
        let mut direct_timing = DirectSurfaceTiming::default();
        let mut direct_subchunks = 0usize;
        let mut direct_mismatches = 0usize;
        let mut direct_missing = 0usize;
        for decoded in decoded {
            match decoded {
                SurfaceDecodeOutput::Direct(decoded) => {
                    direct_timing.biome_us = direct_timing
                        .biome_us
                        .saturating_add(decoded.timing.biome_us);
                    direct_timing.subchunk_us = direct_timing
                        .subchunk_us
                        .saturating_add(decoded.timing.subchunk_us);
                    direct_timing.surface_us = direct_timing
                        .surface_us
                        .saturating_add(decoded.timing.surface_us);
                    direct_subchunks = direct_subchunks.saturating_add(decoded.subchunks_decoded);
                    direct_mismatches =
                        direct_mismatches.saturating_add(decoded.raw_height_mismatches);
                    direct_missing = direct_missing.saturating_add(decoded.missing_columns);
                    output.push(decoded.chunk);
                }
                SurfaceDecodeOutput::CompatibilityFallback(pos) => {
                    output.push(
                        fallback_chunks
                            .remove(&pos)
                            .unwrap_or_else(|| SurfaceMapChunk {
                                pos,
                                materials: Vec::new(),
                                columns: Box::new(std::array::from_fn(|_| None)),
                            }),
                    );
                }
            }
        }

        let columns = output
            .iter()
            .map(|chunk| {
                chunk
                    .columns
                    .iter()
                    .filter(|column| column.is_some())
                    .count()
            })
            .sum::<usize>();
        let materials = output
            .iter()
            .map(|chunk| chunk.materials.len())
            .sum::<usize>();
        let source_subchunk = output
            .iter()
            .flat_map(|chunk| chunk.columns.iter().filter_map(Option::as_ref))
            .filter(|column| column.source == TerrainSampleSource::Subchunk)
            .count();
        let source_legacy = columns.saturating_sub(source_subchunk);

        let mut load = ChunkLoadStats {
            requested_chunks: positions.len(),
            loaded_chunks,
            subchunks_decoded: direct_subchunks,
            worker_threads: worker_count,
            queue_wait_ms: 0,
            load_ms: started.elapsed().as_millis(),
            keys_requested: keys.len(),
            keys_found,
            exact_get_batches: usize::from(!keys.is_empty()),
            prefix_scans: 0,
            decode_ms: direct_decode_ms,
            db_read_ms,
            biome_parse_ms: direct_timing.biome_us / 1_000,
            biome_parse_us: direct_timing.biome_us,
            subchunk_parse_ms: direct_timing.subchunk_us / 1_000,
            subchunk_parse_us: direct_timing.subchunk_us,
            surface_scan_ms: direct_timing.surface_us / 1_000,
            surface_scan_us: direct_timing.surface_us,
            block_entity_parse_ms: 0,
            block_entity_parse_us: 0,
            full_reload_ms: 0,
            legacy_terrain_records: 0,
            legacy_biome_samples: 0,
            legacy_biome_colors: 0,
            terrain_source_legacy: source_legacy,
            terrain_source_subchunk: source_subchunk,
            legacy_pocket_chunks: 0,
            detected_format: self.format(),
            computed_surface_columns: columns,
            raw_height_mismatch_columns: direct_mismatches,
            missing_subchunk_columns: direct_missing,
            legacy_fallback_columns: source_legacy,
            legacy_biome_preferred_columns: 0,
            modern_biome_fallback_columns: 0,
        };
        merge_fallback_load_stats(&mut load, fallback_stats);
        load.load_ms = started.elapsed().as_millis();

        Ok((
            output,
            SurfaceMapBatchStats {
                load,
                chunks: positions.len(),
                columns,
                materials,
            },
        ))
    }
}

fn push_surface_key(
    keys: &mut StorageKeyBatchBuilder,
    requests: &mut Vec<SurfaceRecordRequest>,
    chunk_index: usize,
    key: ChunkKey,
    kind: SurfaceRecordKind,
) {
    let encoded = key.encode_inline();
    keys.push(encoded.as_bytes());
    requests.push(SurfaceRecordRequest { chunk_index, kind });
}

fn apply_surface_values(
    chunks: &mut [RawSurfaceChunk],
    requests: &[SurfaceRecordRequest],
    values: Vec<Option<Bytes>>,
) -> Result<usize> {
    let mut found = 0usize;
    for (request, value) in requests.iter().copied().zip(values) {
        let Some(value) = value else {
            continue;
        };
        found = found.saturating_add(1);
        let Some(chunk) = chunks.get_mut(request.chunk_index) else {
            continue;
        };
        chunk.found_any = true;
        match request.kind {
            SurfaceRecordKind::LegacyTerrain => chunk.legacy_terrain = Some(value),
            SurfaceRecordKind::Subchunk(y) => {
                chunk.subchunks.insert(y, value);
            }
            SurfaceRecordKind::Data3D
            | SurfaceRecordKind::Data2D
            | SurfaceRecordKind::Data2DLegacy => {
                if chunk.biome_record.is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "chunk {:?} contains multiple biome record families",
                        chunk.pos
                    )));
                }
                let tag = match request.kind {
                    SurfaceRecordKind::Data3D => ChunkRecordTag::Data3D,
                    SurfaceRecordKind::Data2D => ChunkRecordTag::Data2D,
                    SurfaceRecordKind::Data2DLegacy => ChunkRecordTag::Data2DLegacy,
                    SurfaceRecordKind::LegacyTerrain | SurfaceRecordKind::Subchunk(_) => {
                        unreachable!("non-biome record matched biome branch")
                    }
                };
                chunk.biome_record = Some((tag, value));
            }
        }
    }
    Ok(found)
}

fn decode_direct_surface_chunk(raw: RawSurfaceChunk) -> Result<SurfaceDecodeOutput> {
    // Historical fixed-array/numeric terrain uses a different material-name migration path. Keep it
    // on the canonical compatibility implementation instead of duplicating that policy here.
    if raw.legacy_terrain.is_some()
        || raw
            .subchunks
            .values()
            .any(|value| !matches!(value.first().copied(), Some(1 | 8 | 9)))
    {
        return Ok(SurfaceDecodeOutput::CompatibilityFallback(raw.pos));
    }

    let biome_started = Instant::now();
    let biome_data = parse_surface_biome_record(raw.biome_record.as_ref())?;
    let version = raw
        .biome_record
        .as_ref()
        .map_or(ChunkVersion::New, |(tag, _)| match tag {
            ChunkRecordTag::Data3D => ChunkVersion::New,
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => ChunkVersion::Old,
            _ => ChunkVersion::New,
        });
    let height_map = biome_data
        .as_ref()
        .map(|biome_data| surface_height_map(raw.pos, biome_data));
    let mut render_biomes = BTreeMap::new();
    if let Some(biome_data) = biome_data {
        for storage in biome_data.storages {
            render_biomes.insert(storage.y.unwrap_or(i32::MIN), storage);
        }
    }
    let biome_us = biome_started.elapsed().as_micros();

    let subchunk_started = Instant::now();
    let mut subchunks = BTreeMap::new();
    for (y, value) in raw.subchunks {
        let subchunk = SubChunk::read(y, value, SubChunkDecodeMode::SurfaceColumns)?;
        if !matches!(subchunk.format, SubChunkFormat::Paletted { .. }) {
            return Ok(SurfaceDecodeOutput::CompatibilityFallback(raw.pos));
        }
        subchunks.insert(y, subchunk);
    }
    let subchunk_us = subchunk_started.elapsed().as_micros();
    let subchunks_decoded = subchunks.len();

    let surface_started = Instant::now();
    let chunk = build_direct_surface_map(
        raw.pos,
        version,
        &subchunks,
        height_map.as_ref(),
        &render_biomes,
    )?;
    let surface_us = surface_started.elapsed().as_micros();
    let populated = chunk
        .columns
        .iter()
        .filter(|column| column.is_some())
        .count();
    let raw_height_mismatches = height_map.as_ref().map_or(0, |height_map| {
        chunk
            .columns
            .iter()
            .enumerate()
            .filter(|(index, column)| {
                let Some(column) = column else {
                    return false;
                };
                let z = index / 16;
                let x = index % 16;
                height_map[z][x].is_some_and(|raw_height| raw_height != column.surface_y)
            })
            .count()
    });

    Ok(SurfaceDecodeOutput::Direct(DirectSurfaceChunk {
        chunk,
        timing: DirectSurfaceTiming {
            biome_us,
            subchunk_us,
            surface_us,
        },
        subchunks_decoded,
        raw_height_mismatches,
        missing_columns: SURFACE_COLUMN_COUNT.saturating_sub(populated),
    }))
}

fn parse_surface_biome_record(
    record: Option<&(ChunkRecordTag, Bytes)>,
) -> Result<Option<BiomeData>> {
    let Some((tag, value)) = record else {
        return Ok(None);
    };
    let parsed = match tag {
        ChunkRecordTag::Data3D => parse_data3d(value),
        ChunkRecordTag::Data2D => parse_legacy_data2d(value),
        ChunkRecordTag::Data2DLegacy => parse_data2d_legacy(value),
        _ => unreachable!("surface biome record contains only biome tags"),
    }
    .map_err(|error| BedrockWorldError::CorruptWorld(format!("biome data: {error}")))?;
    Ok(Some(parsed))
}

fn surface_height_map(pos: ChunkPos, biome_data: &BiomeData) -> [[Option<i16>; 16]; 16] {
    let mut heights = [[None; 16]; 16];
    let (min_y, _) = pos.y_range(biome_data.version);
    for local_z in 0..16usize {
        for local_x in 0..16usize {
            let index = local_z * 16 + local_x;
            heights[local_z][local_x] = biome_data
                .height_map
                .get(index)
                .and_then(|height| i16::try_from(i32::from(*height).saturating_add(min_y)).ok());
        }
    }
    heights
}

fn build_direct_surface_map(
    pos: ChunkPos,
    version: ChunkVersion,
    subchunks: &BTreeMap<i8, SubChunk>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Result<SurfaceMapChunk> {
    let mut materials = Vec::<SurfaceMapMaterial>::with_capacity(32);
    let mut columns = Box::new(std::array::from_fn(|_| None));
    let (min_y, max_y) = pos.y_range(version);
    for local_z in 0..16u8 {
        for local_x in 0..16u8 {
            let column = sample_direct_surface_column(
                local_x,
                local_z,
                min_y,
                max_y,
                subchunks,
                height_map,
                render_biomes,
                &mut materials,
            )?;
            columns[usize::from(local_z) * 16 + usize::from(local_x)] = column;
        }
    }
    Ok(SurfaceMapChunk {
        pos,
        materials,
        columns,
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_direct_surface_column(
    local_x: u8,
    local_z: u8,
    min_y: i32,
    max_y: i32,
    subchunks: &BTreeMap<i8, SubChunk>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
    materials: &mut Vec<SurfaceMapMaterial>,
) -> Result<Option<SurfaceMapColumn>> {
    let mut overlay = None::<PendingOverlay>;
    let mut top_water = None::<PendingWater>;
    let mut water_depth = 0u8;

    for y in (min_y..=max_y).rev() {
        let subchunk_y = i8::try_from(y.div_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "surface y={y} cannot be represented as a SubChunk index"
            ))
        })?;
        let Some(subchunk) = subchunks.get(&subchunk_y) else {
            continue;
        };
        let local_y = u8::try_from(y.rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!("invalid local surface y for {y}"))
        })?;
        let height = i16::try_from(y).unwrap_or(if y < 0 { i16::MIN } else { i16::MAX });
        let mut saw_layer = false;
        for entry in subchunk.visible_block_surface_states_at(local_x, local_y, local_z) {
            saw_layer = true;
            let role = terrain_surface_role(&entry.state.name);
            if let Some(column) = scan_direct_surface_state(
                local_x,
                local_z,
                y,
                height,
                entry.state,
                role,
                TerrainSampleSource::Subchunk,
                render_biomes,
                materials,
                &mut overlay,
                &mut top_water,
                &mut water_depth,
            )? {
                return Ok(Some(column));
            }
        }
        if saw_layer {
            continue;
        }
    }

    if let Some(water) = top_water {
        let relief_y = height_map
            .and_then(|height_map| height_map[usize::from(local_z)][usize::from(local_x)])
            .unwrap_or(water.y);
        return Ok(Some(SurfaceMapColumn {
            surface_y: water.y,
            surface_material: water.material,
            relief_y,
            relief_material: water.material,
            overlay_y: overlay.map_or(NO_HEIGHT, |overlay| overlay.y),
            overlay_material: overlay.map_or(NO_MATERIAL, |overlay| overlay.material),
            water_depth,
            water_material: water.material,
            underwater_y: NO_HEIGHT,
            underwater_material: NO_MATERIAL,
            biome: direct_biome_at(local_x, local_z, i32::from(water.y), render_biomes),
            source: water.source,
        }));
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn scan_direct_surface_state(
    local_x: u8,
    local_z: u8,
    y: i32,
    height: i16,
    state: &BlockState,
    role: TerrainSurfaceRole,
    source: TerrainSampleSource,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
    materials: &mut Vec<SurfaceMapMaterial>,
    overlay: &mut Option<PendingOverlay>,
    top_water: &mut Option<PendingWater>,
    water_depth: &mut u8,
) -> Result<Option<SurfaceMapColumn>> {
    match role {
        TerrainSurfaceRole::Air => {
            if top_water.is_some() {
                *water_depth = water_depth.saturating_add(1);
            }
            Ok(None)
        }
        TerrainSurfaceRole::Overlay => {
            if let Some(water) = top_water.take() {
                let relief_material = intern_material(materials, state)?;
                return Ok(Some(SurfaceMapColumn {
                    surface_y: water.y,
                    surface_material: water.material,
                    relief_y: height,
                    relief_material,
                    overlay_y: overlay.map_or(NO_HEIGHT, |overlay| overlay.y),
                    overlay_material: overlay.map_or(NO_MATERIAL, |overlay| overlay.material),
                    water_depth: water_depth.saturating_add(1),
                    water_material: water.material,
                    underwater_y: height,
                    underwater_material: relief_material,
                    biome: direct_biome_at(local_x, local_z, y, render_biomes),
                    source: water.source,
                }));
            }
            if overlay.is_none() {
                *overlay = Some(PendingOverlay {
                    y: height,
                    material: intern_material(materials, state)?,
                });
            }
            Ok(None)
        }
        TerrainSurfaceRole::Water => {
            if top_water.is_none() {
                *top_water = Some(PendingWater {
                    y: height,
                    material: intern_material(materials, state)?,
                    source,
                });
            } else {
                *water_depth = water_depth.saturating_add(1);
            }
            Ok(None)
        }
        TerrainSurfaceRole::Primary => {
            let primary_material = intern_material(materials, state)?;
            let biome = direct_biome_at(local_x, local_z, y, render_biomes);
            if let Some(water) = top_water.take() {
                return Ok(Some(SurfaceMapColumn {
                    surface_y: water.y,
                    surface_material: water.material,
                    relief_y: height,
                    relief_material: primary_material,
                    overlay_y: overlay.map_or(NO_HEIGHT, |overlay| overlay.y),
                    overlay_material: overlay.map_or(NO_MATERIAL, |overlay| overlay.material),
                    water_depth: water_depth.saturating_add(1),
                    water_material: water.material,
                    underwater_y: height,
                    underwater_material: primary_material,
                    biome,
                    source: water.source,
                }));
            }
            Ok(Some(SurfaceMapColumn {
                surface_y: height,
                surface_material: primary_material,
                relief_y: height,
                relief_material: primary_material,
                overlay_y: overlay.map_or(NO_HEIGHT, |overlay| overlay.y),
                overlay_material: overlay.map_or(NO_MATERIAL, |overlay| overlay.material),
                water_depth: 0,
                water_material: NO_MATERIAL,
                underwater_y: NO_HEIGHT,
                underwater_material: NO_MATERIAL,
                biome,
                source,
            }))
        }
    }
}

fn direct_biome_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Option<TerrainColumnBiome> {
    direct_biome_id_at(local_x, local_z, y, render_biomes).map(TerrainColumnBiome::Id)
}

fn direct_biome_id_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Option<u32> {
    let bucket = y.div_euclid(16) * 16;
    let direct = render_biomes
        .get(&bucket)
        .or_else(|| render_biomes.values().next())
        .and_then(|storage| biome_id_from_storage(storage, local_x, local_z, y))
        .filter(|id| *id != 0);
    if direct.is_some() {
        return direct;
    }
    for storage in render_biomes.values().rev() {
        if storage.y.is_none() {
            if let Some(id) = storage
                .biome_id_at(local_x, 0, local_z)
                .filter(|id| *id != 0)
            {
                return Some(id);
            }
            continue;
        }
        for local_y in (0..16u8).rev() {
            if let Some(id) = storage
                .biome_id_at(local_x, local_y, local_z)
                .filter(|id| *id != 0)
            {
                return Some(id);
            }
        }
    }
    None
}

fn biome_id_from_storage(
    storage: &BiomeStorage,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    let local_y = if let Some(start_y) = storage.y {
        u8::try_from(y - start_y).ok()?
    } else {
        0
    };
    storage.biome_id_at(local_x, local_y, local_z)
}

fn compact_surface_chunk(chunk: &super::ChunkData) -> Result<SurfaceMapChunk> {
    let mut materials = Vec::<SurfaceMapMaterial>::with_capacity(32);
    let mut columns = Box::new(std::array::from_fn(|_| None));
    let Some(samples) = chunk.column_samples.as_ref() else {
        return Ok(SurfaceMapChunk {
            pos: chunk.pos,
            materials,
            columns,
        });
    };

    for local_z in 0..16u8 {
        for local_x in 0..16u8 {
            let Some(sample) = samples.get(local_x, local_z) else {
                continue;
            };
            let surface_material = intern_material(&mut materials, &sample.surface_block_state)?;
            let relief_material = intern_material(&mut materials, &sample.relief_block_state)?;
            let (overlay_y, overlay_material) = sample.overlay.as_ref().map_or(
                Ok::<(i16, u16), BedrockWorldError>((NO_HEIGHT, NO_MATERIAL)),
                |overlay| {
                    Ok((
                        overlay.y,
                        intern_material(&mut materials, &overlay.block_state)?,
                    ))
                },
            )?;
            let (water_depth, water_material, underwater_y, underwater_material) =
                sample.water.as_ref().map_or(
                    Ok::<(u8, u16, i16, u16), BedrockWorldError>((
                        0,
                        NO_MATERIAL,
                        NO_HEIGHT,
                        NO_MATERIAL,
                    )),
                    |water| {
                        Ok((
                            water.depth,
                            intern_material(&mut materials, &water.block_state)?,
                            water.underwater_y.unwrap_or(NO_HEIGHT),
                            water
                                .underwater_block_state
                                .as_ref()
                                .map(|state| intern_material(&mut materials, state))
                                .transpose()?
                                .unwrap_or(NO_MATERIAL),
                        ))
                    },
                )?;
            columns[usize::from(local_z) * 16 + usize::from(local_x)] = Some(SurfaceMapColumn {
                surface_y: sample.surface_y,
                surface_material,
                relief_y: sample.relief_y,
                relief_material,
                overlay_y,
                overlay_material,
                water_depth,
                water_material,
                underwater_y,
                underwater_material,
                biome: sample.biome,
                source: sample.source,
            });
        }
    }

    Ok(SurfaceMapChunk {
        pos: chunk.pos,
        materials,
        columns,
    })
}

fn intern_material(materials: &mut Vec<SurfaceMapMaterial>, state: &BlockState) -> Result<u16> {
    if let Some(index) = materials
        .iter()
        .position(|material| material.matches_state(state))
    {
        return u16::try_from(index).map_err(|_| {
            BedrockWorldError::Validation("surface material table exceeds u16".to_string())
        });
    }
    let index = u16::try_from(materials.len()).map_err(|_| {
        BedrockWorldError::Validation("surface material table exceeds u16".to_string())
    })?;
    materials.push(SurfaceMapMaterial::from_state(state));
    Ok(index)
}

fn sort_surface_positions(positions: &mut [ChunkPos], priority: ChunkLoadPriority) {
    match priority {
        ChunkLoadPriority::RowMajor => positions.sort(),
        ChunkLoadPriority::DistanceFrom { chunk_x, chunk_z } => positions.sort_by_key(|pos| {
            let dx = i64::from(pos.x) - i64::from(chunk_x);
            let dz = i64::from(pos.z) - i64::from(chunk_z);
            (
                dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
                pos.z,
                pos.x,
                pos.dimension,
            )
        }),
    }
}

fn surface_storage_read_options(options: &SurfaceMapQueryOptions) -> StorageReadOptions {
    StorageReadOptions {
        threading: match options.threading {
            WorldThreadingOptions::Auto => StorageThreadingOptions::Auto,
            WorldThreadingOptions::Fixed(threads) => StorageThreadingOptions::Fixed(threads),
            WorldThreadingOptions::Single => StorageThreadingOptions::Single,
        },
        scan_mode: StorageScanMode::Sequential,
        cache_policy: options.storage_cache_policy,
        pipeline: StoragePipelineOptions {
            queue_depth: options.pipeline.queue_depth,
            table_batch_size: options.pipeline.chunk_batch_size,
            progress_interval: options.pipeline.progress_interval,
        },
        cancel: None,
        progress: None,
    }
}

fn surface_decode_pool(worker_count: usize) -> Result<Arc<ThreadPool>> {
    static POOLS: OnceLock<Mutex<HashMap<usize, Arc<ThreadPool>>>> = OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(pools) = pools.lock()
        && let Some(pool) = pools.get(&worker_count)
    {
        return Ok(Arc::clone(pool));
    }
    let pool = Arc::new(
        ThreadPoolBuilder::new()
            .num_threads(worker_count.max(1))
            .thread_name(|index| format!("bedrock-surface-worker-{index}"))
            .build()
            .map_err(|error| {
                BedrockWorldError::ConcurrentWrite(format!(
                    "failed to build surface decode pool: {error}"
                ))
            })?,
    );
    let mut pools = pools.lock().map_err(|_| {
        BedrockWorldError::ConcurrentWrite("surface decode pool registry poisoned".to_string())
    })?;
    Ok(Arc::clone(
        pools
            .entry(worker_count)
            .or_insert_with(|| Arc::clone(&pool)),
    ))
}

fn merge_fallback_load_stats(target: &mut ChunkLoadStats, fallback: ChunkLoadStats) {
    target.subchunks_decoded = target
        .subchunks_decoded
        .saturating_add(fallback.subchunks_decoded);
    target.worker_threads = target.worker_threads.max(fallback.worker_threads);
    target.queue_wait_ms = target.queue_wait_ms.saturating_add(fallback.queue_wait_ms);
    target.keys_requested = target
        .keys_requested
        .saturating_add(fallback.keys_requested);
    target.keys_found = target.keys_found.saturating_add(fallback.keys_found);
    target.exact_get_batches = target
        .exact_get_batches
        .saturating_add(fallback.exact_get_batches);
    target.prefix_scans = target.prefix_scans.saturating_add(fallback.prefix_scans);
    target.decode_ms = target.decode_ms.saturating_add(fallback.decode_ms);
    target.db_read_ms = target.db_read_ms.saturating_add(fallback.db_read_ms);
    target.biome_parse_us = target
        .biome_parse_us
        .saturating_add(fallback.biome_parse_us);
    target.subchunk_parse_us = target
        .subchunk_parse_us
        .saturating_add(fallback.subchunk_parse_us);
    target.surface_scan_us = target
        .surface_scan_us
        .saturating_add(fallback.surface_scan_us);
    target.biome_parse_ms = target.biome_parse_us / 1_000;
    target.subchunk_parse_ms = target.subchunk_parse_us / 1_000;
    target.surface_scan_ms = target.surface_scan_us / 1_000;
    target.full_reload_ms = target
        .full_reload_ms
        .saturating_add(fallback.full_reload_ms);
    target.legacy_terrain_records = target
        .legacy_terrain_records
        .saturating_add(fallback.legacy_terrain_records);
    target.legacy_biome_samples = target
        .legacy_biome_samples
        .saturating_add(fallback.legacy_biome_samples);
    target.legacy_biome_colors = target
        .legacy_biome_colors
        .saturating_add(fallback.legacy_biome_colors);
    target.legacy_pocket_chunks = target
        .legacy_pocket_chunks
        .saturating_add(fallback.legacy_pocket_chunks);
    target.raw_height_mismatch_columns = target
        .raw_height_mismatch_columns
        .saturating_add(fallback.raw_height_mismatch_columns);
    target.missing_subchunk_columns = target
        .missing_subchunk_columns
        .saturating_add(fallback.missing_subchunk_columns);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_optional_fields_use_internal_sentinels() {
        let column = SurfaceMapColumn {
            surface_y: 64,
            surface_material: 0,
            relief_y: 63,
            relief_material: 1,
            overlay_y: NO_HEIGHT,
            overlay_material: NO_MATERIAL,
            water_depth: 0,
            water_material: NO_MATERIAL,
            underwater_y: NO_HEIGHT,
            underwater_material: NO_MATERIAL,
            biome: None,
            source: TerrainSampleSource::Subchunk,
        };
        assert_eq!(column.overlay_y(), None);
        assert_eq!(column.overlay_material(), None);
        assert_eq!(column.water_material(), None);
        assert_eq!(column.underwater_y(), None);
        assert_eq!(column.underwater_material(), None);
    }

    #[test]
    fn material_interner_reuses_state_without_column_clone() {
        let state = BlockState {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: Some(1),
        };
        let mut materials = Vec::new();
        let first = intern_material(&mut materials, &state).unwrap();
        let second = intern_material(&mut materials, &state).unwrap();
        assert_eq!(first, second);
        assert_eq!(materials.len(), 1);
    }
}
