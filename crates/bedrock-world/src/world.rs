//! High-level lazy world access built on top of the storage layer.
//!
//! The methods in this module are intentionally split into blocking and async
//! forms. Blocking methods are the canonical implementation and are appropriate
//! for CLI tools, background worker threads, and tests. Async methods are thin
//! wrappers that offload the same work with `tokio::task::spawn_blocking`.

use crate::chunk::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, BlockPos, BlockState,
    BlockStatePaletteEntry, Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    GlobalRecordKind, LegacyBiomeSample, LegacyTerrain, MapRecordId, SubChunk, SubChunkDecodeMode,
    block_storage_index, parse_subchunk_with_mode,
};
use crate::error::{BedrockWorldError, Result};
use crate::level_dat::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt, serialize_root_nbt};
use crate::parsed::{
    ActorRecord, ActorSource, Biome2d, Biome3d, BlockEntityRecord, HeightMap2d, ItemStack,
    ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData, ParsedDbEntry,
    ParsedDbValue, ParsedEntity, ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData,
    ParsedVillageData, ParsedWorld, WorldParseOptions, WorldParseReport, collect_item_stacks,
    encode_actor_digest_ids, encode_consecutive_roots, encode_global_record,
    encode_hardcoded_spawn_area_records, encode_map_record, parse_actor_digest_ids,
    parse_block_entities_from_value, parse_chunk_records, parse_chunk_records_with_options,
    parse_data3d, parse_entities_from_value, parse_global_record, parse_global_storage_entries,
    parse_hardcoded_spawn_area_records, parse_legacy_data2d, parse_map_record, parse_world_storage,
};
use crate::player::{PlayerData, PlayerId};
use crate::storage::backend::BedrockLevelDbStorage;
use crate::storage::{
    PocketChunksDatStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag, StorageOp,
    StorageProgressSink, StorageReadOptions, StorageScanMode, StorageThreadingOptions,
    StorageVisitorControl, WorldStorage,
};
pub use crate::surface::{TerrainSurfaceRole, terrain_surface_overlay_alpha, terrain_surface_role};
use crate::surface::{is_air_block_name, is_water_block_name};
use bytes::Bytes;
use rayon::{ThreadPoolBuilder, prelude::*};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Options used when opening or constructing a [`BedrockWorld`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Reject mutating operations when set.
    pub read_only: bool,
    /// Preferred world storage format. [`WorldFormatHint::Auto`] detects the
    /// backend from `db/CURRENT` and old `chunks.dat` files.
    pub format: WorldFormatHint,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            read_only: true,
            format: WorldFormatHint::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Preferred storage format selection used when opening a world.
pub enum WorldFormatHint {
    #[default]
    /// Automatically choose the appropriate mode.
    Auto,
    /// Modern Bedrock `LevelDB` world.
    LevelDb,
    /// Pre-`LevelDB` Pocket Edition `chunks.dat` world.
    PocketChunksDat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Detected world storage format.
pub enum WorldFormat {
    #[default]
    /// Modern Bedrock `LevelDB` world.
    LevelDb,
    /// Old `LevelDB` world using `LegacyTerrain` records.
    LevelDbLegacyTerrain,
    /// Pre-`LevelDB` Pocket Edition `chunks.dat` world.
    PocketChunksDat,
}

/// Lazy handle to a Minecraft Bedrock world folder.
///
/// A handle stores the world path and a storage backend. It does not scan or
/// parse the database until a query method is called.
pub struct BedrockWorld<S = Arc<dyn WorldStorage>> {
    path: PathBuf,
    options: OpenOptions,
    storage: S,
    format: WorldFormat,
}

/// Storage handle accepted by generic [`BedrockWorld`] methods.
pub trait WorldStorageHandle: Clone + Send + Sync + 'static {
    /// Returns the raw storage backend behind this handle.
    fn storage(&self) -> &dyn WorldStorage;
}

impl<T> WorldStorageHandle for T
where
    T: WorldStorage + Clone + Send + Sync + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self
    }
}

impl<T> WorldStorageHandle for Arc<T>
where
    T: WorldStorage + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

impl WorldStorageHandle for Arc<dyn WorldStorage> {
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Options for surface-column lookup.
pub struct SurfaceColumnOptions {
    /// Whether air blocks are skipped when finding a surface column.
    pub skip_air: bool,
    /// Whether water is treated as transparent context over terrain.
    pub transparent_water: bool,
}

impl Default for SurfaceColumnOptions {
    fn default() -> Self {
        Self {
            skip_air: true,
            transparent_water: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Legacy surface-column query result.
pub struct SurfaceColumn {
    /// World Y coordinate selected as the visible surface.
    pub y: i32,
    /// Block name selected for this result.
    pub block_name: String,
    /// Biome id associated with the sampled column.
    pub biome_id: Option<u32>,
    /// Number of water blocks above the underwater support block.
    pub water_depth: u8,
    /// Block name below water, when found.
    pub under_water_block_name: Option<String>,
    /// Whether the value came from a fallback path.
    pub is_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Controls how much subchunk data exact surface loading reads.
pub enum ExactSurfaceSubchunkPolicy {
    /// Load the full subchunk range required by the request.
    Full,
    /// Use height hints first and reload when verification requires it.
    HintThenVerify,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Bounded pipeline settings for world scans and render loads.
pub struct WorldPipelineOptions {
    /// Maximum queued work items; zero selects an automatic default.
    pub queue_depth: usize,
    /// Chunk batch size; zero selects an automatic default.
    pub chunk_batch_size: usize,
    /// Subchunk decode worker count; zero selects an automatic default.
    pub subchunk_decode_workers: usize,
    /// Progress callback interval; zero selects an automatic default.
    pub progress_interval: usize,
}

impl WorldPipelineOptions {
    #[must_use]
    /// Resolves the effective bounded queue depth.
    pub fn resolve_queue_depth(self, workers: usize, work_items: usize) -> usize {
        self.queue_depth
            .max(if self.queue_depth == 0 {
                workers
                    .max(1)
                    .saturating_mul(2)
                    .max(work_items.clamp(1, 256))
            } else {
                1
            })
            .max(1)
    }

    #[must_use]
    /// Resolves the effective progress callback interval.
    pub fn resolve_progress_interval(self) -> usize {
        self.progress_interval
            .max(if self.progress_interval == 0 { 256 } else { 1 })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Ordering policy for render chunk loading.
pub enum ChunkLoadPriority {
    #[default]
    /// Process chunks in row-major order.
    RowMajor,
    /// Prioritize chunks by distance from a center chunk.
    DistanceFrom {
        /// Center chunk X coordinate used for distance sorting.
        chunk_x: i32,
        /// Center chunk Z coordinate used for distance sorting.
        chunk_z: i32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Biome loading policy for exact surface requests.
pub enum ExactSurfaceBiomeLoad {
    /// No optional data is requested or available.
    None,
    #[default]
    /// Load biome data needed for top-column sampling.
    TopColumns,
    /// Load all matching biome data.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// One independently requested subchunk representation.
pub enum SubchunkDataRequirement {
    /// Compute exact visible terrain columns without materializing 3D indices.
    SurfaceColumns(ExactSurfaceSubchunkPolicy),
    /// Decode one fixed world Y layer with 3D random access.
    Layer(i32),
    /// Decode one cave slice with 3D random access.
    CaveSlice(i32),
    /// Decode every subchunk in the chunk with full 3D random access.
    Full3dIndices,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Biome payload requested with map data.
pub enum BiomeDataRequirement {
    /// Do not load optional biome records.
    #[default]
    None,
    /// Load biome data needed by surface-column sampling.
    SurfaceColumns,
    /// Load biome data for one world Y layer.
    Layer(i32),
    /// Retain every matching biome storage.
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Composable map-data contract used to plan storage reads and subchunk decoding.
///
/// Add only the representations a consumer needs. The loader unions all requested
/// subchunk records and chooses the least expensive decoder that satisfies them.
pub struct ChunkDataRequest {
    /// Independent subchunk representations to load.
    pub subchunks: Vec<SubchunkDataRequirement>,
    /// Whether raw height-map data is required.
    pub height_map: bool,
    /// Optional biome payload requirement.
    pub biome: BiomeDataRequirement,
    /// Whether block-entity NBT is required.
    pub block_entities: bool,
}

impl ChunkDataRequest {
    #[must_use]
    /// Starts an empty request.
    pub const fn new() -> Self {
        Self {
            subchunks: Vec::new(),
            height_map: false,
            biome: BiomeDataRequirement::None,
            block_entities: false,
        }
    }

    #[must_use]
    /// Requests exact terrain columns with the selected subchunk probing policy.
    pub fn surface_columns(mut self, policy: ExactSurfaceSubchunkPolicy) -> Self {
        self.push_subchunk_requirement(SubchunkDataRequirement::SurfaceColumns(policy));
        self
    }

    #[must_use]
    /// Requests a fixed Y layer.
    pub fn layer(mut self, y: i32) -> Self {
        self.push_subchunk_requirement(SubchunkDataRequirement::Layer(y));
        self
    }

    #[must_use]
    /// Requests a cave slice at one world Y coordinate.
    pub fn cave_slice(mut self, y: i32) -> Self {
        self.push_subchunk_requirement(SubchunkDataRequirement::CaveSlice(y));
        self
    }

    #[must_use]
    /// Requests full 3D random-access indices for every subchunk in a chunk.
    pub fn full_3d_indices(mut self) -> Self {
        self.push_subchunk_requirement(SubchunkDataRequirement::Full3dIndices);
        self
    }

    #[must_use]
    /// Requests raw height-map data.
    pub const fn height_map(mut self) -> Self {
        self.height_map = true;
        self
    }

    #[must_use]
    /// Sets the optional biome payload requirement.
    pub const fn biome(mut self, biome: BiomeDataRequirement) -> Self {
        self.biome = biome;
        self
    }

    #[must_use]
    /// Requests block-entity NBT records.
    pub const fn block_entities(mut self) -> Self {
        self.block_entities = true;
        self
    }

    fn push_subchunk_requirement(&mut self, requirement: SubchunkDataRequirement) {
        if !self.subchunks.contains(&requirement) {
            self.subchunks.push(requirement);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Source payload used for a terrain column sample.
pub enum TerrainSampleSource {
    /// Data sourced from decoded subchunks.
    Subchunk,
    /// Old `LevelDB`-era terrain record.
    LegacyTerrain,
    /// Data sourced from legacy terrain as a fallback.
    LegacyFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Biome value associated with a terrain column.
pub enum TerrainColumnBiome {
    /// Numeric biome id value.
    Id(u32),
    /// Legacy biome sample value.
    Legacy(LegacyBiomeSample),
}

#[derive(Debug, Clone, PartialEq)]
/// Thin overlay block above a sampled surface.
pub struct TerrainColumnOverlay {
    /// World Y coordinate of the overlay block.
    pub y: i16,
    /// Block state selected as the overlay.
    pub block_state: BlockState,
    /// Storage or terrain source that produced this value.
    pub source: TerrainSampleSource,
}

#[derive(Debug, Clone, PartialEq)]
/// Water context for a sampled surface column.
pub struct TerrainColumnWater {
    /// Y coordinate of the visible surface block.
    pub surface_y: i16,
    /// Water block state at the visible surface.
    pub block_state: BlockState,
    /// Depth in blocks.
    pub depth: u8,
    /// Y coordinate of the first underwater support block, when found.
    pub underwater_y: Option<i16>,
    /// Block state below water, when found.
    pub underwater_block_state: Option<BlockState>,
    /// Storage or terrain source that produced this value.
    pub source: TerrainSampleSource,
}

#[derive(Debug, Clone, PartialEq)]
/// Canonical terrain surface sample for one local X/Z column.
pub struct TerrainColumnSample {
    /// Y coordinate of the visible surface block.
    pub surface_y: i16,
    /// Block state selected as the visible surface.
    pub surface_block_state: BlockState,
    /// Y coordinate of the supporting relief block.
    pub relief_y: i16,
    /// Block state selected as relief/support.
    pub relief_block_state: BlockState,
    /// Optional thin overlay block above the primary surface.
    pub overlay: Option<TerrainColumnOverlay>,
    /// Optional water context for this sampled column.
    pub water: Option<TerrainColumnWater>,
    /// Biome loading policy for the render request.
    pub biome: Option<TerrainColumnBiome>,
    /// Storage or terrain source that produced this value.
    pub source: TerrainSampleSource,
}

#[derive(Debug, Clone, PartialEq)]
/// Fixed 16x16 terrain column sample grid.
pub struct TerrainColumnSamples {
    columns: Box<[Option<TerrainColumnSample>; 16 * 16]>,
}

impl TerrainColumnSamples {
    #[must_use]
    /// Creates a new value.
    pub fn new() -> Self {
        Self {
            columns: Box::new(std::array::from_fn(|_| None)),
        }
    }

    #[must_use]
    /// Returns the value at the requested coordinates.
    pub fn get(&self, local_x: u8, local_z: u8) -> Option<&TerrainColumnSample> {
        self.columns
            .get(column_index(local_x, local_z)?)
            .and_then(Option::as_ref)
    }

    /// Stores a value at the requested coordinates.
    pub fn set(&mut self, local_x: u8, local_z: u8, sample: TerrainColumnSample) {
        if let Some(index) = column_index(local_x, local_z) {
            if let Some(slot) = self.columns.get_mut(index) {
                *slot = Some(sample);
            }
        }
    }

    #[must_use]
    /// Returns the number of populated sampled columns.
    pub fn sampled_columns(&self) -> usize {
        self.columns
            .iter()
            .filter(|sample| sample.is_some())
            .count()
    }

    /// Iterates over populated values.
    pub fn iter(&self) -> impl Iterator<Item = &TerrainColumnSample> {
        self.columns.iter().filter_map(Option::as_ref)
    }
}

impl Default for TerrainColumnSamples {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Diagnostics collected while loading render chunks.
pub struct ChunkLoadStats {
    /// Number of chunks requested by the caller.
    pub requested_chunks: usize,
    /// Number of chunks with renderable data loaded.
    pub loaded_chunks: usize,
    /// Number of subchunks decoded while loading.
    pub subchunks_decoded: usize,
    /// Number of worker threads used by the operation.
    pub worker_threads: usize,
    /// Milliseconds spent waiting for bounded pipeline capacity.
    pub queue_wait_ms: u128,
    /// Total chunk load time in milliseconds.
    pub load_ms: u128,
    /// Number of exact storage keys requested.
    pub keys_requested: usize,
    /// Number of requested storage keys found.
    pub keys_found: usize,
    /// Number of exact batch-get operations issued.
    pub exact_get_batches: usize,
    /// Number of prefix scans issued as fallback or discovery work.
    pub prefix_scans: usize,
    /// Milliseconds spent decoding loaded records.
    pub decode_ms: u128,
    /// Milliseconds spent reading from the storage backend.
    pub db_read_ms: u128,
    /// Milliseconds spent parsing biome records.
    pub biome_parse_ms: u128,
    /// Microseconds spent parsing biome records.
    pub biome_parse_us: u128,
    /// Milliseconds spent parsing subchunk records.
    pub subchunk_parse_ms: u128,
    /// Microseconds spent parsing subchunk records.
    pub subchunk_parse_us: u128,
    /// Milliseconds spent computing surface columns.
    pub surface_scan_ms: u128,
    /// Microseconds spent computing surface columns.
    pub surface_scan_us: u128,
    /// Milliseconds spent parsing block-entity records.
    pub block_entity_parse_ms: u128,
    /// Microseconds spent parsing block-entity records.
    pub block_entity_parse_us: u128,
    /// Milliseconds spent on full reloads for exact surface requests.
    pub full_reload_ms: u128,
    /// Number of legacy terrain records loaded.
    pub legacy_terrain_records: usize,
    /// Number of legacy biome samples decoded.
    pub legacy_biome_samples: usize,
    /// Compatibility RGB values decoded from legacy biome samples.
    pub legacy_biome_colors: usize,
    /// Number of sampled columns sourced from legacy terrain.
    pub terrain_source_legacy: usize,
    /// Number of sampled columns sourced from subchunks.
    pub terrain_source_subchunk: usize,
    /// Number of virtual legacy chunks loaded from `chunks.dat`.
    pub legacy_pocket_chunks: usize,
    /// World format detected during the load.
    pub detected_format: WorldFormat,
    /// Number of surface columns computed from block data.
    pub computed_surface_columns: usize,
    /// Columns whose raw heightmap disagreed with computed surface data.
    pub raw_height_mismatch_columns: usize,
    /// Columns missing required subchunk data.
    pub missing_subchunk_columns: usize,
    /// Columns that fell back to legacy terrain data.
    pub legacy_fallback_columns: usize,
    /// Columns where legacy RGB biome samples took precedence.
    pub legacy_biome_preferred_columns: usize,
    /// Columns where modern biome ids were used as fallback.
    pub modern_biome_fallback_columns: usize,
}

#[derive(Debug, Clone)]
/// Options controlling render chunk loading.
pub struct ChunkLoadOptions {
    /// Composable map-data contract requested by the caller.
    pub data_request: ChunkDataRequest,
    /// Subchunk decode mode used while loading render data.
    pub subchunk_decode: SubChunkDecodeMode,
    /// Threading policy for this operation.
    pub threading: WorldThreadingOptions,
    /// Bounded pipeline settings for this operation.
    pub pipeline: WorldPipelineOptions,
    /// Optional cancellation flag checked during long-running work.
    pub cancel: Option<CancelFlag>,
    /// Optional progress sink invoked during long-running work.
    pub progress: Option<ProgressSink>,
    /// Ordering policy for chunk loading.
    pub priority: ChunkLoadPriority,
    /// Backend cache strategy for exact storage reads used by render loading.
    pub storage_cache_policy: StorageCachePolicy,
}

impl Default for ChunkLoadOptions {
    fn default() -> Self {
        Self {
            data_request: ChunkDataRequest::new()
                .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                .biome(BiomeDataRequirement::SurfaceColumns),
            subchunk_decode: SubChunkDecodeMode::FullIndices,
            threading: WorldThreadingOptions::Auto,
            pipeline: WorldPipelineOptions::default(),
            cancel: None,
            progress: None,
            priority: ChunkLoadPriority::RowMajor,
            storage_cache_policy: StorageCachePolicy::Use,
        }
    }
}

impl ChunkLoadOptions {
    #[must_use]
    /// Creates options from an explicit composable map-data contract.
    pub fn for_data_request(data_request: ChunkDataRequest) -> Self {
        Self {
            data_request,
            ..Self::default()
        }
    }

    #[must_use]
    /// Creates a surface-column load that avoids materializing full 3D palette indices.
    pub fn exact_surface_columns(
        subchunks: ExactSurfaceSubchunkPolicy,
        biome: ExactSurfaceBiomeLoad,
        block_entities: bool,
    ) -> Self {
        Self {
            data_request: ChunkDataRequest::new()
                .surface_columns(subchunks)
                .biome(match biome {
                    ExactSurfaceBiomeLoad::None => BiomeDataRequirement::None,
                    ExactSurfaceBiomeLoad::TopColumns => BiomeDataRequirement::SurfaceColumns,
                    ExactSurfaceBiomeLoad::All => BiomeDataRequirement::All,
                })
                .block_entities_if(block_entities),
            ..Self::default()
        }
    }

    #[must_use]
    /// Creates a raw-height-map load with no subchunk index materialization.
    pub fn raw_height_map() -> Self {
        Self {
            data_request: ChunkDataRequest::new().height_map(),
            ..Self::default()
        }
    }

    #[must_use]
    /// Creates a fixed-layer load for one world Y coordinate.
    pub fn layer(y: i32) -> Self {
        Self {
            data_request: ChunkDataRequest::new().layer(y),
            ..Self::default()
        }
    }

    #[must_use]
    /// Creates a biome-only load for one world Y coordinate.
    pub fn biome(y: i32, load_all: bool) -> Self {
        Self {
            data_request: ChunkDataRequest::new().biome(if load_all {
                BiomeDataRequirement::All
            } else {
                BiomeDataRequirement::Layer(y)
            }),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Block entity included with render chunk data.
pub struct ChunkBlockEntity {
    /// Identifier value decoded from storage or NBT.
    pub id: Option<String>,
    /// World block position `[x, y, z]` decoded from NBT, when present.
    pub position: Option<[i32; 3]>,
    /// Original or parsed Bedrock NBT payload.
    pub nbt: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Loaded render-oriented chunk data.
pub struct ChunkData {
    /// Chunk position represented by this render data.
    pub pos: ChunkPos,
    /// Whether enough records were found to treat the chunk as loaded.
    pub is_loaded: bool,
    /// Height-map values in Bedrock `z * 16 + x` column order.
    pub height_map: Option<[[Option<i16>; 16]; 16]>,
    /// Legacy biome samples decoded from old terrain records.
    pub legacy_biomes: Option<[[Option<LegacyBiomeSample>; 16]; 16]>,
    /// Compatibility RGB values decoded from legacy biome samples.
    pub legacy_biome_colors: Option<[[Option<u32>; 16]; 16]>,
    /// Parsed biome storage records keyed by vertical section.
    pub biome_data: BTreeMap<i32, ParsedBiomeStorage>,
    /// Exact-surface subchunk loading policy.
    pub subchunks: BTreeMap<i8, SubChunk>,
    /// Whether block-entity records are loaded with render data.
    pub block_entities: Vec<ChunkBlockEntity>,
    /// `LegacyTerrain` record when present for old `LevelDB` worlds.
    pub legacy_terrain: Option<LegacyTerrain>,
    /// Canonical surface-column samples computed from actual block data.
    pub column_samples: Option<TerrainColumnSamples>,
    /// Bedrock format or payload version.
    pub version: crate::ChunkVersion,
}

impl ChunkData {
    #[must_use]
    /// Returns the sampled terrain column at local chunk coordinates.
    pub fn column_sample_at(&self, local_x: u8, local_z: u8) -> Option<&TerrainColumnSample> {
        self.column_samples.as_ref()?.get(local_x, local_z)
    }
}

#[derive(Debug, Clone)]
struct RawChunkData {
    pos: ChunkPos,
    biome_record: Option<(crate::ChunkVersion, Bytes)>,
    subchunks: BTreeMap<i8, Bytes>,
    block_entities: Option<Bytes>,
    legacy_terrain: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_field_names)]
struct ChunkDecodeTiming {
    biome_parse_us: u128,
    subchunk_parse_us: u128,
    surface_scan_us: u128,
    block_entity_parse_us: u128,
}

impl ChunkDecodeTiming {
    fn add(&mut self, other: Self) {
        self.biome_parse_us = self.biome_parse_us.saturating_add(other.biome_parse_us);
        self.subchunk_parse_us = self
            .subchunk_parse_us
            .saturating_add(other.subchunk_parse_us);
        self.surface_scan_us = self.surface_scan_us.saturating_add(other.surface_scan_us);
        self.block_entity_parse_us = self
            .block_entity_parse_us
            .saturating_add(other.block_entity_parse_us);
    }
}

#[derive(Debug, Clone, Copy)]
enum RenderRecordKind {
    LegacyTerrain,
    Data3D,
    Data2D,
    Data2DLegacy,
    Subchunk(i8),
    BlockEntity,
}

#[derive(Debug, Clone, Copy)]
struct RenderRecordRequest {
    chunk_index: usize,
    kind: RenderRecordKind,
}

#[derive(Debug, Clone)]
/// Options controlling render region loading.
pub struct WorldChunkQueryRegionLoadOptions {
    /// Composable map-data contract requested by the caller.
    pub data_request: ChunkDataRequest,
    /// Subchunk decode mode used while loading render data.
    pub subchunk_decode: SubChunkDecodeMode,
    /// Threading policy for this operation.
    pub threading: WorldThreadingOptions,
    /// Bounded pipeline settings for this operation.
    pub pipeline: WorldPipelineOptions,
    /// Optional cancellation flag checked during long-running work.
    pub cancel: Option<CancelFlag>,
    /// Optional progress sink invoked during long-running work.
    pub progress: Option<ProgressSink>,
    /// Ordering policy for chunk loading.
    pub priority: ChunkLoadPriority,
    /// Backend cache strategy for exact storage reads used by render loading.
    pub storage_cache_policy: StorageCachePolicy,
}

impl Default for WorldChunkQueryRegionLoadOptions {
    fn default() -> Self {
        Self {
            data_request: ChunkLoadOptions::default().data_request,
            subchunk_decode: SubChunkDecodeMode::FullIndices,
            threading: WorldThreadingOptions::Auto,
            pipeline: WorldPipelineOptions::default(),
            cancel: None,
            progress: None,
            priority: ChunkLoadPriority::RowMajor,
            storage_cache_policy: StorageCachePolicy::Use,
        }
    }
}

impl From<WorldChunkQueryRegionLoadOptions> for ChunkLoadOptions {
    fn from(options: WorldChunkQueryRegionLoadOptions) -> Self {
        Self {
            data_request: options.data_request,
            subchunk_decode: options.subchunk_decode,
            threading: options.threading,
            pipeline: options.pipeline,
            cancel: options.cancel,
            progress: options.progress,
            priority: options.priority,
            storage_cache_policy: options.storage_cache_policy,
        }
    }
}

impl ChunkDataRequest {
    fn block_entities_if(mut self, enabled: bool) -> Self {
        self.block_entities = enabled;
        self
    }

    fn preferred_decode_mode(&self) -> SubChunkDecodeMode {
        if self.subchunks.iter().any(|requirement| {
            matches!(
                requirement,
                SubchunkDataRequirement::Layer(_)
                    | SubchunkDataRequirement::CaveSlice(_)
                    | SubchunkDataRequirement::Full3dIndices
            )
        }) {
            SubChunkDecodeMode::FullIndices
        } else if self
            .subchunks
            .iter()
            .any(|requirement| matches!(requirement, SubchunkDataRequirement::SurfaceColumns(_)))
        {
            SubChunkDecodeMode::SurfaceColumns
        } else {
            SubChunkDecodeMode::CountsOnly
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Inclusive chunk rectangle to load or scan for rendering.
pub struct WorldChunkQueryRegion {
    /// Bedrock dimension covered by this region.
    pub dimension: crate::Dimension,
    /// Inclusive minimum chunk X coordinate.
    pub min_chunk_x: i32,
    /// Inclusive minimum chunk Z coordinate.
    pub min_chunk_z: i32,
    /// Inclusive maximum chunk X coordinate.
    pub max_chunk_x: i32,
    /// Inclusive maximum chunk Z coordinate.
    pub max_chunk_z: i32,
}

#[derive(Debug, Clone, PartialEq)]
/// Loaded render region and load diagnostics.
pub struct WorldChunkQueryRegionData {
    /// Inclusive chunk region requested by the load.
    pub region: WorldChunkQueryRegion,
    /// Parsed or loaded chunks in this result.
    pub chunks: Vec<ChunkData>,
    /// Load diagnostics and timing counters.
    pub stats: ChunkLoadStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Inclusive chunk bounds discovered in a world.
pub struct ChunkBounds {
    /// Bedrock dimension covered by these bounds.
    pub dimension: crate::Dimension,
    /// Inclusive minimum chunk X coordinate.
    pub min_chunk_x: i32,
    /// Inclusive minimum chunk Z coordinate.
    pub min_chunk_z: i32,
    /// Inclusive maximum chunk X coordinate.
    pub max_chunk_x: i32,
    /// Inclusive maximum chunk Z coordinate.
    pub max_chunk_z: i32,
    /// Number of chunks represented by these bounds.
    pub chunk_count: usize,
}

impl ChunkBounds {
    fn from_first(pos: ChunkPos) -> Self {
        Self {
            dimension: pos.dimension,
            min_chunk_x: pos.x,
            min_chunk_z: pos.z,
            max_chunk_x: pos.x,
            max_chunk_z: pos.z,
            chunk_count: 1,
        }
    }

    fn include(&mut self, pos: ChunkPos) {
        self.min_chunk_x = self.min_chunk_x.min(pos.x);
        self.min_chunk_z = self.min_chunk_z.min(pos.z);
        self.max_chunk_x = self.max_chunk_x.max(pos.x);
        self.max_chunk_z = self.max_chunk_z.max(pos.z);
        self.chunk_count = self.chunk_count.saturating_add(1);
    }
}

/// Persistent decode executor shared by world operations with the same worker budget.
pub struct WorldExecutor {
    worker_count: usize,
    pool: rayon::ThreadPool,
}

impl std::fmt::Debug for WorldExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorldExecutor")
            .field("worker_count", &self.worker_count)
            .finish_non_exhaustive()
    }
}

impl WorldExecutor {
    /// Creates a fixed persistent world executor.
    pub fn new(worker_count: usize) -> Result<Self> {
        let worker_count = worker_count.clamp(1, MAX_WORLD_THREADS);
        let pool = ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("bedrock-world-worker-{index}"))
            .build()
            .map_err(|error| {
                BedrockWorldError::ConcurrentWrite(format!(
                    "failed to build persistent world executor: {error}"
                ))
            })?;
        Ok(Self { worker_count, pool })
    }

    /// Number of worker threads owned by this executor.
    #[must_use]
    pub const fn worker_count(&self) -> usize {
        self.worker_count
    }
}

fn default_world_worker_budget() -> usize {
    let logical = std::thread::available_parallelism().map_or(1, usize::from);
    logical.div_ceil(2).clamp(2, 6)
}

fn world_executor(worker_count: usize) -> Result<Arc<WorldExecutor>> {
    static EXECUTORS: OnceLock<Mutex<HashMap<usize, Arc<WorldExecutor>>>> = OnceLock::new();
    let worker_count = worker_count.clamp(1, MAX_WORLD_THREADS);
    let executors = EXECUTORS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(executors) = executors.lock()
        && let Some(executor) = executors.get(&worker_count)
    {
        return Ok(Arc::clone(executor));
    }
    let executor = Arc::new(WorldExecutor::new(worker_count)?);
    let mut executors = executors.lock().map_err(|_| {
        BedrockWorldError::ConcurrentWrite("world executor registry poisoned".to_string())
    })?;
    Ok(Arc::clone(
        executors
            .entry(worker_count)
            .or_insert_with(|| Arc::clone(&executor)),
    ))
}

#[derive(Debug, Clone)]
/// Options controlling world scan operations.
pub struct WorldScanOptions {
    /// Threading policy for this operation.
    pub threading: WorldThreadingOptions,
    /// Bounded pipeline settings for this operation.
    pub pipeline: WorldPipelineOptions,
    /// Optional cancellation flag checked during long-running work.
    pub cancel: Option<CancelFlag>,
    /// Optional progress sink invoked during long-running work.
    pub progress: Option<ProgressSink>,
}

impl Default for WorldScanOptions {
    fn default() -> Self {
        Self {
            threading: WorldThreadingOptions::Auto,
            pipeline: WorldPipelineOptions::default(),
            cancel: None,
            progress: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Threading policy for world-level operations.
pub enum WorldThreadingOptions {
    #[default]
    /// Automatically choose the appropriate mode.
    Auto,
    /// Use a fixed worker count.
    Fixed(usize),
    /// Use a single worker.
    Single,
}

/// max world threads constant.
pub const MAX_WORLD_THREADS: usize = 512;

impl WorldThreadingOptions {
    #[must_use]
    /// Resolves this policy to an effective worker count.
    pub fn resolve(self, work_items: usize) -> usize {
        self.resolve_unchecked(work_items)
    }

    #[must_use]
    /// Resolves this policy without reporting validation errors.
    pub fn resolve_unchecked(self, work_items: usize) -> usize {
        match self {
            Self::Single => 1,
            Self::Fixed(threads) => threads.clamp(1, MAX_WORLD_THREADS),
            Self::Auto => default_world_worker_budget().min(work_items.max(1)),
        }
    }

    /// Resolves this policy and validates explicit worker counts.
    pub fn resolve_checked(self, work_items: usize) -> Result<usize> {
        match self {
            Self::Fixed(0) => Err(BedrockWorldError::Validation(
                "thread count must be in 1..=512".to_string(),
            )),
            Self::Fixed(threads) if threads > MAX_WORLD_THREADS => Err(
                BedrockWorldError::Validation("thread count must be in 1..=512".to_string()),
            ),
            _ => Ok(self.resolve_unchecked(work_items)),
        }
    }
}

#[derive(Debug, Clone, Default)]
/// Shareable cancellation flag for world operations.
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    #[must_use]
    /// Creates a new uncancelled flag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation for operations sharing this flag.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    /// Creates a flag from a shared atomic cancellation marker.
    pub fn from_shared(cancelled: Arc<AtomicBool>) -> Self {
        Self(cancelled)
    }

    #[must_use]
    /// Converts this flag into a storage-layer cancellation flag.
    pub fn to_storage_cancel(&self) -> StorageCancelFlag {
        StorageCancelFlag::from_shared(Arc::clone(&self.0))
    }

    #[must_use]
    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
/// Callback sink for world scan progress.
pub struct ProgressSink {
    inner: Arc<Mutex<Box<dyn FnMut(WorldScanProgress) + Send>>>,
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProgressSink")
            .finish_non_exhaustive()
    }
}

impl ProgressSink {
    #[must_use]
    /// Creates a progress sink from a callback invoked during scans.
    pub fn new(callback: impl FnMut(WorldScanProgress) + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(callback))),
        }
    }

    fn emit(&self, progress: WorldScanProgress) {
        if let Ok(mut callback) = self.inner.lock() {
            callback(progress);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Progress update emitted during world scans.
pub struct WorldScanProgress {
    /// Number of entries observed when progress was emitted.
    pub entries_seen: usize,
}

impl BedrockWorld<Arc<dyn WorldStorage>> {
    /// Opens a world on the calling thread with automatic format detection.
    pub fn open_blocking(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let format = detect_world_format(&path, options.format)?;
        let storage: Arc<dyn WorldStorage> = match format {
            WorldFormat::LevelDb | WorldFormat::LevelDbLegacyTerrain => {
                let db_path = path.join("db");
                if options.read_only {
                    Arc::new(BedrockLevelDbStorage::open_read_only(db_path)?)
                } else {
                    Arc::new(BedrockLevelDbStorage::open(db_path)?)
                }
            }
            WorldFormat::PocketChunksDat => {
                if !options.read_only {
                    log::warn!(
                        "opening legacy chunks.dat world as read-only despite read_only=false"
                    );
                }
                Arc::new(PocketChunksDatStorage::open(&path)?)
            }
        };
        log::debug!(
            "opened Bedrock world (path={}, format={:?}, read_only={})",
            path.display(),
            format,
            options.read_only
        );
        Ok(Self {
            path,
            options,
            storage,
            format,
        })
    }

    #[cfg(feature = "async")]
    /// Opens a world on a blocking worker thread and returns an async handle.
    pub async fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(path, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[must_use]
    /// Creates a world handle from an already-open storage backend.
    pub fn from_storage(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: OpenOptions,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world handle from an already-open storage backend and explicit format.
    pub fn from_storage_with_format(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: OpenOptions,
        format: WorldFormat,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format,
        }
    }
}

impl BedrockWorld<BedrockLevelDbStorage> {
    /// Opens a world with a concrete `BedrockLevelDbStorage` backend on the calling thread.
    pub fn open_typed_blocking(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let format = detect_world_format(&path, options.format)?;
        match format {
            WorldFormat::LevelDb | WorldFormat::LevelDbLegacyTerrain => {
                let db_path = path.join("db");
                let storage = if options.read_only {
                    BedrockLevelDbStorage::open_read_only(db_path)?
                } else {
                    BedrockLevelDbStorage::open(db_path)?
                };
                Ok(Self {
                    path,
                    options,
                    storage,
                    format,
                })
            }
            WorldFormat::PocketChunksDat => Err(BedrockWorldError::UnsupportedChunkFormat(
                "typed LevelDB open does not support legacy chunks.dat worlds".to_string(),
            )),
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    #[must_use]
    /// Creates a world handle from a concrete storage backend.
    pub fn from_typed_storage(path: impl Into<PathBuf>, storage: S, options: OpenOptions) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world handle from a concrete storage backend and explicit format.
    pub fn from_typed_storage_with_format(
        path: impl Into<PathBuf>,
        storage: S,
        options: OpenOptions,
        format: WorldFormat,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format,
        }
    }

    #[must_use]
    /// Returns the underlying raw storage backend.
    pub fn storage(&self) -> &dyn WorldStorage {
        self.storage.storage()
    }

    /// Returns the concrete storage handle used by this world.
    pub const fn storage_backend(&self) -> &S {
        &self.storage
    }

    #[must_use]
    /// Returns the world folder path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    /// Returns the detected world storage format.
    pub const fn format(&self) -> WorldFormat {
        self.format
    }

    /// Read level dat blocking.
    pub fn read_level_dat_blocking(&self) -> Result<LevelDatDocument> {
        read_level_dat_document(&self.path.join("level.dat"))
    }

    /// Write level dat blocking.
    pub fn write_level_dat_blocking(&self, document: &LevelDatDocument) -> Result<()> {
        self.ensure_writable()?;
        write_level_dat_document(&self.path.join("level.dat"), document)
    }

    /// Compact the underlying world storage after writes.
    pub fn compact_storage_blocking(&self) -> Result<()> {
        self.ensure_writable()?;
        self.storage().compact()
    }

    /// List players blocking.
    pub fn list_players_blocking(&self) -> Result<Vec<PlayerId>> {
        let mut players = Vec::new();
        if self.storage().get(b"~local_player")?.is_some() {
            players.push(PlayerId::Local);
        }
        self.storage().for_each_prefix_key(
            b"player_",
            StorageReadOptions::default(),
            &mut |key| {
                if let Some(player) = PlayerId::from_storage_key(key) {
                    players.push(player);
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(players)
    }

    /// Classify keys blocking.
    pub fn classify_keys_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<BTreeMap<String, usize>> {
        let mut counts = BTreeMap::new();
        let mut allocation_free_counts = HashMap::<BedrockDbKeyKind, usize>::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                let kind = BedrockDbKeyKind::classify(key);
                if matches!(
                    kind,
                    BedrockDbKeyKind::Other | BedrockDbKeyKind::Village | BedrockDbKeyKind::Global
                ) {
                    let key = BedrockDbKey::decode(key);
                    *counts.entry(key.summary_kind()).or_default() += 1;
                } else {
                    *allocation_free_counts.entry(kind).or_default() += 1;
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        for (kind, count) in allocation_free_counts {
            *counts.entry(kind.summary_kind()).or_default() += count;
        }
        emit_progress(&options, entries_seen);
        Ok(counts)
    }

    /// List chunk positions blocking.
    pub fn list_chunk_positions_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let mut positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    positions.insert(chunk_key.pos);
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(positions.into_iter().collect())
    }

    /// List render chunk positions blocking.
    pub fn list_render_chunk_positions_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let started = Instant::now();
        log::debug!(
            "listing render chunk positions (threading={:?}, queue_depth={}, progress_interval={})",
            options.threading,
            options.pipeline.queue_depth,
            options.pipeline.progress_interval
        );
        let mut positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        let outcome =
            self.storage()
                .for_each_key(to_storage_read_options(&options), &mut |key| {
                    check_cancelled(&options)?;
                    entries_seen = entries_seen.saturating_add(1);
                    if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                        if chunk_key.tag.is_render_chunk_record() {
                            positions.insert(chunk_key.pos);
                        }
                    }
                    if entries_seen.is_multiple_of(8192) {
                        emit_progress(&options, entries_seen);
                    }
                    Ok(StorageVisitorControl::Continue)
                })?;
        let positions = positions.into_iter().collect::<Vec<_>>();
        log::debug!(
            "render chunk position listing complete (entries_seen={}, positions={}, visited={}, tables_scanned={}, worker_threads={}, queue_wait_ms={}, cancel_checks={}, elapsed_ms={})",
            entries_seen,
            positions.len(),
            outcome.visited,
            outcome.tables_scanned,
            outcome.worker_threads,
            outcome.queue_wait_ms,
            outcome.cancel_checks,
            started.elapsed().as_millis()
        );
        Ok(positions)
    }

    #[allow(clippy::too_many_lines)]
    /// List render chunk positions in region blocking.
    pub fn list_chunk_positions_in_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let started = Instant::now();
        validate_render_region(region)?;
        let x_count = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
        let z_count = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
        let capacity = usize::try_from(x_count.saturating_mul(z_count))
            .map_err(|_| BedrockWorldError::Validation("render region is too large".to_string()))?;
        let mut positions = Vec::with_capacity(capacity);
        for z in region.min_chunk_z..=region.max_chunk_z {
            for x in region.min_chunk_x..=region.max_chunk_x {
                positions.push(ChunkPos {
                    x,
                    z,
                    dimension: region.dimension,
                });
            }
        }
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let worker_count = options.threading.resolve_checked(positions.len())?;
        log::debug!(
            "indexing render chunk region (dimension={:?}, min=({}, {}), max=({}, {}), workers={})",
            region.dimension,
            region.min_chunk_x,
            region.min_chunk_z,
            region.max_chunk_x,
            region.max_chunk_z,
            worker_count
        );
        if worker_count == 1 {
            let render_positions = positions
                .into_iter()
                .filter_map(
                    |pos| match self.has_render_chunk_records_blocking(pos, &options) {
                        Ok(true) => Some(Ok(pos)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    },
                )
                .collect::<Result<Vec<_>>>()?;
            log::debug!(
                "render chunk region index complete (dimension={:?}, candidates={}, positions={}, workers={}, queue_depth=0, elapsed_ms={})",
                region.dimension,
                capacity,
                render_positions.len(),
                worker_count,
                started.elapsed().as_millis()
            );
            return Ok(render_positions);
        }

        let scan_options = WorldScanOptions {
            threading: WorldThreadingOptions::Single,
            pipeline: options.pipeline,
            cancel: options.cancel.clone(),
            progress: options.progress.clone(),
        };
        let next_position = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue_depth = options
            .pipeline
            .resolve_queue_depth(worker_count, positions.len());
        let (sender, receiver) = mpsc::sync_channel::<Result<Option<ChunkPos>>>(queue_depth);
        let executor = world_executor(worker_count)?;
        executor.pool.scope(|scope| {
            for worker_index in 0..worker_count {
                let next_position = Arc::clone(&next_position);
                let sender = sender.clone();
                let positions = &positions;
                let scan_options = scan_options.clone();
                scope.spawn(move |_| {
                    log::trace!("render region index worker {worker_index} started");
                    loop {
                        if scan_options
                            .cancel
                            .as_ref()
                            .is_some_and(CancelFlag::is_cancelled)
                        {
                            return;
                        }
                        let index = next_position.fetch_add(1, Ordering::Relaxed);
                        let Some(pos) = positions.get(index).copied() else {
                            log::trace!("render region index worker {worker_index} finished");
                            return;
                        };
                        let result = self
                            .has_render_chunk_records_blocking(pos, &scan_options)
                            .map(|is_renderable| is_renderable.then_some(pos));
                        if sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(sender);

            let mut render_positions = Vec::new();
            for result in receiver {
                if let Some(pos) = result? {
                    render_positions.push(pos);
                }
            }
            render_positions.sort();
            log::debug!(
                "render chunk region index complete (dimension={:?}, candidates={}, positions={}, workers={}, queue_depth={}, elapsed_ms={})",
                region.dimension,
                positions.len(),
                render_positions.len(),
                worker_count,
                queue_depth,
                started.elapsed().as_millis()
            );
            Ok(render_positions)
        })
    }

    /// Discover chunk bounds blocking.
    pub fn discover_chunk_bounds_blocking(
        &self,
        dimension: crate::Dimension,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkBounds>> {
        let mut bounds: Option<ChunkBounds> = None;
        let mut seen_positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos.dimension == dimension && seen_positions.insert(chunk_key.pos)
                    {
                        match &mut bounds {
                            Some(bounds) => bounds.include(chunk_key.pos),
                            None => bounds = Some(ChunkBounds::from_first(chunk_key.pos)),
                        }
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(bounds)
    }

    /// Nearest loaded chunk to spawn blocking.
    pub fn nearest_loaded_chunk_to_spawn_blocking(
        &self,
        dimension: crate::Dimension,
        spawn_block_x: i32,
        spawn_block_z: i32,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkPos>> {
        let spawn_chunk = BlockPos {
            x: spawn_block_x,
            y: 0,
            z: spawn_block_z,
        }
        .to_chunk_pos(dimension);
        let mut best = None::<(i64, ChunkPos)>;
        let mut seen_positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos.dimension == dimension && seen_positions.insert(chunk_key.pos)
                    {
                        let dx = i64::from(chunk_key.pos.x) - i64::from(spawn_chunk.x);
                        let dz = i64::from(chunk_key.pos.z) - i64::from(spawn_chunk.z);
                        let distance = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
                        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                            best = Some((distance, chunk_key.pos));
                        }
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(best.map(|(_, pos)| pos))
    }

    /// Get player blocking.
    pub fn get_player_blocking(&self, id: &PlayerId) -> Result<Option<PlayerData>> {
        let Some(key) = id.storage_key() else {
            if *id == PlayerId::LegacyLevelDat {
                let document = self.read_level_dat_blocking()?;
                return Ok(Some(PlayerData::from_nbt(id.clone(), document.root)?));
            }
            return Ok(None);
        };
        self.storage()
            .get(key.as_ref())?
            .map(|bytes| PlayerData::from_raw(id.clone(), bytes))
            .transpose()
    }

    /// Put player blocking.
    pub fn put_player_blocking(&self, player: &PlayerData) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.storage().put(key.as_ref(), &player.raw)
    }

    /// Get chunk blocking.
    pub fn get_chunk_blocking(&self, pos: ChunkPos) -> Result<Chunk> {
        let mut records = Vec::new();
        let prefix = chunk_record_prefix(pos);
        self.storage().for_each_prefix(
            &prefix,
            StorageReadOptions::default(),
            &mut |raw_key, value| {
                if let Ok(key) = ChunkKey::decode(raw_key) {
                    if key.pos == pos {
                        records.push(ChunkRecord {
                            key,
                            value: value.clone(),
                        });
                    }
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        let version = records
            .iter()
            .find(|record| record.key.tag == ChunkRecordTag::Version)
            .and_then(|record| record.value.first().copied());
        Ok(Chunk {
            pos,
            version,
            records,
        })
    }

    /// Reads and decodes a subchunk on the calling thread.
    pub fn get_subchunk_blocking(&self, pos: ChunkPos, y: i8) -> Result<Option<crate::SubChunk>> {
        self.get_chunk_blocking(pos)?.get_subchunk(y)
    }

    /// Parses the world on the calling thread using the selected retention options.
    pub fn parse_world_blocking(&self, options: WorldParseOptions) -> Result<ParsedWorld> {
        let level_dat = self.read_level_dat_blocking()?;
        parse_world_storage(level_dat, self.storage(), options)
    }

    /// Parses all known records for one chunk on the calling thread.
    pub fn parse_chunk_blocking(&self, pos: ChunkPos) -> Result<ParsedChunkData> {
        let chunk = self.get_chunk_blocking(pos)?;
        Ok(parse_chunk_records(pos, chunk.records))
    }

    /// Parses one chunk on the calling thread using custom parse options.
    pub fn parse_chunk_with_options_blocking(
        &self,
        pos: ChunkPos,
        options: WorldParseOptions,
    ) -> Result<ParsedChunkData> {
        let chunk = self.get_chunk_blocking(pos)?;
        Ok(parse_chunk_records_with_options(
            pos,
            chunk.records,
            options,
        ))
    }

    /// Parse subchunk blocking.
    pub fn parse_subchunk_blocking(
        &self,
        pos: ChunkPos,
        y: i8,
        options: WorldParseOptions,
    ) -> Result<Option<crate::SubChunk>> {
        let key = ChunkKey::subchunk(pos, y);
        self.storage()
            .get(&key.encode())?
            .map(|value| parse_subchunk_with_mode(y, value, options.subchunk_decode_mode))
            .transpose()
    }

    /// Get biome storage blocking.
    pub fn get_biome_storage_blocking(
        &self,
        pos: ChunkPos,
        y: i32,
    ) -> Result<Option<ParsedBiomeStorage>> {
        let Some(biome_data) = self.get_biome_data_blocking(pos)? else {
            return Ok(None);
        };
        for storage in biome_data.storages {
            if biome_storage_contains_y(&storage, y) {
                return Ok(Some(storage));
            }
        }
        Ok(None)
    }

    /// Get biome storages blocking.
    pub fn get_biome_storages_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<Vec<ParsedBiomeStorage>>> {
        Ok(self
            .get_biome_data_blocking(pos)?
            .map(|biome_data| biome_data.storages))
    }

    fn get_biome_data_blocking(&self, pos: ChunkPos) -> Result<Option<ParsedBiomeData>> {
        for (tag, version) in [
            (ChunkRecordTag::Data3D, crate::ChunkVersion::New),
            (ChunkRecordTag::Data2D, crate::ChunkVersion::Old),
            (ChunkRecordTag::Data2DLegacy, crate::ChunkVersion::Old),
        ] {
            let key = ChunkKey::new(pos, tag).encode();
            let Some(value) = self.storage().get(&key)? else {
                continue;
            };
            let biome_data = match version {
                crate::ChunkVersion::New => parse_data3d(&value),
                crate::ChunkVersion::Old => parse_legacy_data2d(&value),
            }
            .map_err(|error| BedrockWorldError::CorruptWorld(format!("biome data: {error}")))?;
            return Ok(Some(biome_data));
        }
        Ok(None)
    }

    fn has_render_chunk_records_blocking(
        &self,
        pos: ChunkPos,
        options: &WorldScanOptions,
    ) -> Result<bool> {
        let prefix = chunk_record_prefix(pos);
        let mut found = false;
        self.storage().for_each_prefix_key(
            &prefix,
            to_storage_read_options(options),
            &mut |key| {
                check_cancelled(options)?;
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos == pos && chunk_key.tag.is_render_chunk_record() {
                        found = true;
                        return Ok(StorageVisitorControl::Stop);
                    }
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(found)
    }

    /// Get height at blocking.
    pub fn get_height_at_blocking(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<i16>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .get_height_map_blocking(pos)?
            .and_then(|heights| heights[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get height map blocking.
    pub fn get_height_map_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<i16>; 16]; 16]>> {
        if let Some(biome_data) = self
            .get_biome_data_blocking(pos)
            .map_err(|error| BedrockWorldError::CorruptWorld(format!("height data: {error}")))?
        {
            return Ok(Some(render_height_map_from_biome_data(pos, &biome_data)));
        }
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        if let Some(value) = self.storage().get(&key)? {
            let terrain = LegacyTerrain::parse(value)?;
            return Ok(Some(render_height_map_from_legacy_terrain(&terrain)));
        }
        Ok(None)
    }

    /// Get legacy biome colors blocking.
    pub fn get_legacy_biome_colors_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<u32>; 16]; 16]>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(None);
        };
        let terrain = LegacyTerrain::parse(value)?;
        Ok(Some(render_biome_colors_from_legacy_terrain(&terrain)))
    }

    /// Get legacy biome samples blocking.
    pub fn get_legacy_biome_samples_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<LegacyBiomeSample>; 16]; 16]>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(None);
        };
        let terrain = LegacyTerrain::parse(value)?;
        Ok(Some(render_biomes_from_legacy_terrain(&terrain)))
    }

    /// Get legacy biome color blocking.
    pub fn get_legacy_biome_color_blocking(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<u32>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .get_legacy_biome_colors_blocking(pos)?
            .and_then(|colors| colors[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get legacy biome sample blocking.
    pub fn get_legacy_biome_sample_blocking(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<LegacyBiomeSample>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .get_legacy_biome_samples_blocking(pos)?
            .and_then(|samples| samples[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get biome id blocking.
    pub fn get_biome_id_blocking(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
        y: i32,
    ) -> Result<Option<u32>> {
        validate_local_column(local_x, local_z)?;
        let Some(storage) = self.get_biome_storage_blocking(pos, y)? else {
            return Ok(None);
        };
        Ok(biome_id_from_storage(&storage, local_x, local_z, y))
    }

    /// Get surface column blocking.
    pub fn get_surface_column_blocking(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
        options: SurfaceColumnOptions,
    ) -> Result<Option<SurfaceColumn>> {
        validate_local_column(local_x, local_z)?;
        let (min_y, max_y) = pos.y_range(crate::ChunkVersion::New);
        let start_y = match self.get_height_at_blocking(pos, local_x, local_z)? {
            Some(height) => i32::from(height).clamp(min_y, max_y),
            None => return Ok(None),
        };
        for y in (min_y..=start_y).rev() {
            let Some(block) = self.block_state_in_chunk_column(pos, local_x, y, local_z)? else {
                continue;
            };
            if options.skip_air && is_air_block_name(&block.name) {
                continue;
            }
            let biome_id = self.get_biome_id_blocking(pos, local_x, local_z, y)?;
            let (water_depth, under_water_block_name) =
                if options.transparent_water && is_water_block_name(&block.name) {
                    self.find_solid_under_water(pos, local_x, local_z, y, min_y)?
                } else {
                    (0, None)
                };
            return Ok(Some(SurfaceColumn {
                y,
                block_name: block.name,
                biome_id,
                water_depth,
                under_water_block_name,
                is_fallback: false,
            }));
        }
        Ok(None)
    }

    /// Load render chunk blocking.
    pub fn query_chunk_data_blocking(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        let (mut chunks, _) = self.query_chunk_data_with_stats_blocking([pos], options)?;
        chunks.pop().ok_or_else(|| {
            BedrockWorldError::CorruptWorld("exact render load returned no chunk".to_string())
        })
    }

    /// Loads only canonical terrain column samples for one chunk.
    ///
    /// The request remains configurable for subchunk, biome, block-entity, storage,
    /// cancellation, and threading policy, but this entry point always retains packed
    /// palette indices rather than materializing full 3D index arrays.
    pub fn load_surface_columns_blocking(
        &self,
        pos: ChunkPos,
        mut options: ChunkLoadOptions,
    ) -> Result<Option<TerrainColumnSamples>> {
        let mut request = options.data_request.clone();
        if !request
            .subchunks
            .iter()
            .any(|requirement| matches!(requirement, SubchunkDataRequirement::SurfaceColumns(_)))
        {
            return Err(BedrockWorldError::Validation(
                "surface-column loads require a SurfaceColumns data requirement".to_string(),
            ));
        }
        request.subchunks.retain(|requirement| {
            !matches!(
                requirement,
                SubchunkDataRequirement::Layer(_)
                    | SubchunkDataRequirement::CaveSlice(_)
                    | SubchunkDataRequirement::Full3dIndices
            )
        });
        options.data_request = request;
        Ok(self.query_chunk_data_blocking(pos, options)?.column_samples)
    }

    /// Load render chunks blocking.
    pub fn query_chunk_data_many_blocking(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<Vec<ChunkData>> {
        Ok(self
            .query_chunk_data_with_stats_blocking(positions, options)?
            .0)
    }

    /// Load render chunks with stats blocking.
    pub fn query_chunk_data_with_stats_blocking(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        let started = Instant::now();
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            log::debug!("loading render chunks skipped (chunks=0)");
            return Ok((Vec::new(), ChunkLoadStats::default()));
        }
        let mut positions = positions;
        sort_render_chunk_positions(&mut positions, options.priority);
        let worker_count = options.threading.resolve_checked(positions.len())?;
        log::debug!(
            "loading render chunks (chunks={}, workers={}, data_request={:?}, queue_depth={}, priority={:?})",
            positions.len(),
            worker_count,
            options.data_request,
            options
                .pipeline
                .resolve_queue_depth(worker_count, positions.len()),
            options.priority
        );
        self.load_render_chunks_exact_batch_blocking_sorted(
            positions,
            options,
            worker_count,
            started,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn load_render_chunks_exact_batch_blocking_sorted(
        &self,
        positions: Vec<ChunkPos>,
        options: ChunkLoadOptions,
        worker_count: usize,
        started: Instant,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        check_render_load_cancelled(&options)?;
        let mut raw_chunks = positions
            .iter()
            .copied()
            .map(|pos| RawChunkData {
                pos,
                biome_record: None,
                subchunks: BTreeMap::new(),
                block_entities: None,
                legacy_terrain: None,
            })
            .collect::<Vec<_>>();

        let mut keys = Vec::new();
        let mut requests = Vec::new();
        for (chunk_index, pos) in positions.iter().copied().enumerate() {
            if request_needs_legacy_terrain(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::LegacyTerrain,
                );
            }
            if request_needs_biome_record(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data3D,
                );
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data2D,
                );
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data2DLegacy,
                );
            }
            if !request_uses_hint_surface_subchunks(&options) {
                for y in planned_render_subchunk_ys(pos, &options, None)? {
                    push_render_record_request(
                        &mut keys,
                        &mut requests,
                        chunk_index,
                        pos,
                        RenderRecordKind::Subchunk(y),
                    );
                }
            }
            if request_loads_block_entities(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::BlockEntity,
                );
            }
        }

        let mut keys_requested = keys.len();
        let mut exact_get_batches = 0usize;
        let mut db_read_ms = 0u128;
        let storage_read_options = to_render_storage_read_options(&options);
        let db_started = Instant::now();
        let values = self
            .storage()
            .get_many_ordered_with_control(&keys, storage_read_options.clone())?;
        db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
        exact_get_batches = exact_get_batches.saturating_add(usize::from(!keys.is_empty()));
        let mut keys_found = apply_render_record_values(&mut raw_chunks, &requests, values);

        if request_needs_legacy_terrain_fallback(&options) {
            let mut fallback_keys = Vec::new();
            let mut fallback_requests = Vec::new();
            for (chunk_index, raw) in raw_chunks.iter().enumerate() {
                if raw.subchunks.is_empty() && raw.legacy_terrain.is_none() {
                    push_render_record_request(
                        &mut fallback_keys,
                        &mut fallback_requests,
                        chunk_index,
                        raw.pos,
                        RenderRecordKind::LegacyTerrain,
                    );
                }
            }
            if !fallback_keys.is_empty() {
                let db_started = Instant::now();
                let values = self
                    .storage()
                    .get_many_ordered_with_control(&fallback_keys, storage_read_options.clone())?;
                db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
                exact_get_batches = exact_get_batches.saturating_add(1);
                keys_requested = keys_requested.saturating_add(fallback_keys.len());
                keys_found = keys_found.saturating_add(apply_render_record_values(
                    &mut raw_chunks,
                    &fallback_requests,
                    values,
                ));
            }
        }

        if request_uses_hint_surface_subchunks(&options) {
            let mut needed_keys = Vec::new();
            let mut needed_requests = Vec::new();
            for (chunk_index, raw) in raw_chunks.iter().enumerate() {
                let biome_data = parse_render_biome_record(raw.biome_record.as_ref())?;
                let height_map = if let Some(biome_data) = biome_data.as_ref() {
                    Some(render_height_map_from_biome_data(raw.pos, biome_data))
                } else {
                    legacy_height_map_from_raw(raw.legacy_terrain.as_ref())?
                };
                for y in planned_render_subchunk_ys(raw.pos, &options, height_map.as_ref())? {
                    if raw.subchunks.contains_key(&y) {
                        continue;
                    }
                    push_render_record_request(
                        &mut needed_keys,
                        &mut needed_requests,
                        chunk_index,
                        raw.pos,
                        RenderRecordKind::Subchunk(y),
                    );
                }
            }
            if !needed_keys.is_empty() {
                let db_started = Instant::now();
                let values = self
                    .storage()
                    .get_many_ordered_with_control(&needed_keys, storage_read_options.clone())?;
                db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
                exact_get_batches = exact_get_batches.saturating_add(1);
                keys_requested = keys_requested.saturating_add(needed_keys.len());
                keys_found = keys_found.saturating_add(apply_render_record_values(
                    &mut raw_chunks,
                    &needed_requests,
                    values,
                ));
            }
        }

        check_render_load_cancelled(&options)?;
        let decode_started = Instant::now();
        let (mut chunks, decode_timing) = if worker_count == 1 {
            let mut chunks = Vec::with_capacity(raw_chunks.len());
            let mut timing = ChunkDecodeTiming::default();
            for raw in raw_chunks {
                check_render_load_cancelled(&options)?;
                let (chunk, chunk_timing) = render_chunk_from_raw(raw, &options)?;
                timing.add(chunk_timing);
                chunks.push(chunk);
                emit_render_load_progress(&options, chunks.len());
            }
            (chunks, timing)
        } else {
            let executor = world_executor(worker_count)?;
            let decoded = executor.pool.install(|| {
                raw_chunks
                    .into_par_iter()
                    .map(|raw| {
                        check_render_load_cancelled(&options)?;
                        render_chunk_from_raw(raw, &options)
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
            let mut chunks = Vec::with_capacity(decoded.len());
            let mut timing = ChunkDecodeTiming::default();
            for (chunk, chunk_timing) in decoded {
                timing.add(chunk_timing);
                chunks.push(chunk);
            }
            (chunks, timing)
        };
        let full_reload_ms =
            self.reload_incomplete_needed_exact_surface_chunks_blocking(&mut chunks, &options)?;
        let decode_ms = decode_started.elapsed().as_millis();
        let mut stats = render_load_stats(&chunks, worker_count, 0, started.elapsed().as_millis());
        stats.keys_requested = keys_requested;
        stats.keys_found = keys_found;
        stats.exact_get_batches = exact_get_batches;
        stats.prefix_scans = 0;
        stats.decode_ms = decode_ms;
        stats.db_read_ms = db_read_ms;
        stats.biome_parse_us = decode_timing.biome_parse_us;
        stats.subchunk_parse_us = decode_timing.subchunk_parse_us;
        stats.surface_scan_us = decode_timing.surface_scan_us;
        stats.block_entity_parse_us = decode_timing.block_entity_parse_us;
        stats.biome_parse_ms = stats.biome_parse_us / 1_000;
        stats.subchunk_parse_ms = stats.subchunk_parse_us / 1_000;
        stats.surface_scan_ms = stats.surface_scan_us / 1_000;
        stats.block_entity_parse_ms = stats.block_entity_parse_us / 1_000;
        stats.full_reload_ms = full_reload_ms;
        stats.detected_format = self.format;
        stats.legacy_pocket_chunks = if self.format == WorldFormat::PocketChunksDat {
            stats.legacy_terrain_records
        } else {
            0
        };
        log_render_load_complete(&stats);
        Ok((chunks, stats))
    }

    fn reload_incomplete_needed_exact_surface_chunks_blocking(
        &self,
        chunks: &mut [ChunkData],
        options: &ChunkLoadOptions,
    ) -> Result<u128> {
        if !request_uses_hint_surface_subchunks(options) {
            return Ok(0);
        }

        let mut full_options = options.clone();
        exact_surface_full_request(&mut full_options);
        let mut reload_indexes = Vec::new();
        let mut reload_positions = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            if needed_exact_surface_chunk_requires_full_reload(chunk)? {
                reload_indexes.push(index);
                reload_positions.push(chunk.pos);
            }
        }
        if reload_positions.is_empty() {
            return Ok(0);
        }
        check_render_load_cancelled(options)?;
        let started = Instant::now();
        let worker_count = options.threading.resolve_checked(reload_positions.len())?;
        full_options.threading = if worker_count <= 1 {
            WorldThreadingOptions::Single
        } else {
            WorldThreadingOptions::Fixed(worker_count)
        };
        let (reloaded, stats) =
            self.query_chunk_data_with_stats_blocking(reload_positions, full_options)?;
        for (chunk_index, reloaded_chunk) in reload_indexes.into_iter().zip(reloaded) {
            if let Some(chunk) = chunks.get_mut(chunk_index) {
                *chunk = reloaded_chunk;
            }
        }
        let elapsed = started.elapsed().as_millis().max(stats.load_ms);
        log::debug!(
            "hint surface full reload complete (chunks={}, workers={}, load_ms={}, db_read_ms={}, decode_ms={})",
            stats.requested_chunks,
            stats.worker_threads,
            stats.load_ms,
            stats.db_read_ms,
            stats.decode_ms
        );
        Ok(elapsed)
    }

    /// Load render region blocking.
    pub fn query_chunk_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData> {
        if region.min_chunk_x > region.max_chunk_x || region.min_chunk_z > region.max_chunk_z {
            return Err(BedrockWorldError::Validation(format!(
                "invalid render region: min=({}, {}) max=({}, {})",
                region.min_chunk_x, region.min_chunk_z, region.max_chunk_x, region.max_chunk_z
            )));
        }
        let chunk_count_x = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
        let chunk_count_z = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
        let capacity = usize::try_from(chunk_count_x.saturating_mul(chunk_count_z))
            .map_err(|_| BedrockWorldError::Validation("render region is too large".to_string()))?;
        let mut positions = Vec::with_capacity(capacity);
        for z in region.min_chunk_z..=region.max_chunk_z {
            for x in region.min_chunk_x..=region.max_chunk_x {
                positions.push(ChunkPos {
                    x,
                    z,
                    dimension: region.dimension,
                });
            }
        }
        let (chunks, stats) =
            self.query_chunk_data_with_stats_blocking(positions, options.into())?;
        Ok(WorldChunkQueryRegionData {
            region,
            chunks,
            stats,
        })
    }

    /// Get block state at blocking.
    pub fn get_block_state_at_blocking(
        &self,
        dimension: crate::Dimension,
        block_pos: BlockPos,
    ) -> Result<Option<BlockState>> {
        let chunk_pos = block_pos.to_chunk_pos(dimension);
        let (_, block_y, _) = block_pos.in_chunk_offset();
        let subchunk_y = block_y_to_subchunk_y(block_y)?;
        let Some(subchunk) = self.parse_subchunk_blocking(
            chunk_pos,
            subchunk_y,
            WorldParseOptions {
                subchunk_decode_mode: SubChunkDecodeMode::FullIndices,
                ..WorldParseOptions::summary()
            },
        )?
        else {
            return Ok(None);
        };
        let (local_x, _, local_z) = block_pos.in_chunk_offset();
        let local_y = u8::try_from(block_y - i32::from(subchunk_y) * 16).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={block_y} is outside subchunk bounds"))
        })?;
        Ok(subchunk.block_state_at(local_x, local_y, local_z).cloned())
    }

    /// Decodes the subchunk layer containing the requested world Y coordinate.
    pub fn get_subchunk_layer_blocking(
        &self,
        pos: ChunkPos,
        y: i32,
        mode: SubChunkDecodeMode,
    ) -> Result<Option<SubChunk>> {
        let subchunk_y = block_y_to_subchunk_y(y)?;
        self.parse_subchunk_blocking(
            pos,
            subchunk_y,
            WorldParseOptions {
                subchunk_decode_mode: mode,
                ..WorldParseOptions::summary()
            },
        )
    }

    fn block_state_in_chunk_column(
        &self,
        pos: ChunkPos,
        local_x: u8,
        y: i32,
        local_z: u8,
    ) -> Result<Option<BlockState>> {
        let subchunk_y = block_y_to_subchunk_y(y)?;
        let Some(subchunk) = self.parse_subchunk_blocking(
            pos,
            subchunk_y,
            WorldParseOptions {
                subchunk_decode_mode: SubChunkDecodeMode::FullIndices,
                ..WorldParseOptions::summary()
            },
        )?
        else {
            return Ok(None);
        };
        let local_y = u8::try_from(y - i32::from(subchunk_y) * 16).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={y} is outside subchunk bounds"))
        })?;
        Ok(subchunk.block_state_at(local_x, local_y, local_z).cloned())
    }

    fn find_solid_under_water(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
        water_y: i32,
        min_y: i32,
    ) -> Result<(u8, Option<String>)> {
        let mut depth = 0_u8;
        for y in (min_y..water_y).rev() {
            let Some(block) = self.block_state_in_chunk_column(pos, local_x, y, local_z)? else {
                continue;
            };
            if is_air_block_name(&block.name) || is_water_block_name(&block.name) {
                depth = depth.saturating_add(1);
                continue;
            }
            depth = depth.saturating_add(1);
            return Ok((depth, Some(block.name)));
        }
        Ok((depth, None))
    }

    /// Parse global data blocking.
    pub fn parse_global_data_blocking(&self) -> Result<Vec<ParsedDbEntry>> {
        parse_global_storage_entries(self.storage(), WorldParseOptions::summary())
    }

    /// Scan entities blocking.
    pub fn scan_entities_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedEntity>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
        let mut entities = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::ActorPrefix { .. } => {
                        entities.extend(parse_entities_from_value(value, &mut report));
                    }
                    BedrockDbKey::Chunk(chunk_key) if chunk_key.tag == ChunkRecordTag::Entity => {
                        entities.extend(parse_entities_from_value(value, &mut report));
                    }
                    _ => {}
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((entities, report))
    }

    /// Scan block entities blocking.
    pub fn scan_block_entities_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedBlockEntity>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
        let mut block_entities = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.tag == ChunkRecordTag::BlockEntity {
                        block_entities.extend(parse_block_entities_from_value(value, &mut report));
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((block_entities, report))
    }

    /// Scan items blocking.
    pub fn scan_items_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, WorldParseReport)> {
        let mut report = WorldParseReport::default();
        let mut items = Vec::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::LocalPlayer | BedrockDbKey::RemotePlayer(_) => {
                        match parse_root_nbt(value) {
                            Ok(nbt) => {
                                let mut player_items = collect_item_stacks(&nbt);
                                report.item_count =
                                    report.item_count.saturating_add(player_items.len());
                                items.append(&mut player_items);
                            }
                            Err(error) => report
                                .parse_errors
                                .push(format!("player item scan failed: {error}")),
                        }
                    }
                    BedrockDbKey::ActorPrefix { .. } => {
                        for entity in parse_entities_from_value(value, &mut report) {
                            items.extend(entity.items);
                        }
                    }
                    BedrockDbKey::Chunk(chunk_key) if chunk_key.tag == ChunkRecordTag::Entity => {
                        for entity in parse_entities_from_value(value, &mut report) {
                            items.extend(entity.items);
                        }
                    }
                    BedrockDbKey::Chunk(chunk_key)
                        if chunk_key.tag == ChunkRecordTag::BlockEntity =>
                    {
                        for block_entity in parse_block_entities_from_value(value, &mut report) {
                            items.extend(block_entity.items);
                        }
                    }
                    _ => {}
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok((items, report))
    }

    /// Scans map records through the full global-data parser.
    ///
    /// Prefer [`Self::scan_map_records_blocking`] when only `map_` records are
    /// needed because it uses an exact prefix scan.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_maps_blocking(&self) -> Result<Vec<ParsedMapData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::MapData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Reads a single typed map record by exact `map_<id>` key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or map NBT parse errors.
    pub fn read_map_record_blocking(&self, id: &MapRecordId) -> Result<Option<ParsedMapData>> {
        self.storage()
            .get(&id.storage_key())?
            .map(|value| parse_map_record(id.clone(), value))
            .transpose()
    }

    /// Prefix-scans typed map records without scanning unrelated globals.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or map NBT parse errors.
    pub fn scan_map_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedMapData>> {
        let mut records = Vec::new();
        self.storage().for_each_prefix_ref(
            b"map_",
            to_storage_read_options(&options),
            &mut |entry| {
                check_cancelled(&options)?;
                let Some(id) = MapRecordId::from_storage_key(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                records.push(parse_map_record(id, Bytes::copy_from_slice(entry.value))?);
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(records)
    }

    /// Writes a map record after serialize -> parse roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed records, or storage errors from the commit.
    pub fn write_map_record_blocking(&self, record: &ParsedMapData) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_map_record(record)?;
        parse_map_record(record.record_id.clone(), value.clone())?;
        let mut transaction = self.transaction();
        transaction.put_raw_key(record.record_id.storage_key(), value);
        transaction.commit()
    }

    /// Deletes a map record by exact id.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_map_record_blocking(&self, id: &MapRecordId) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(id.storage_key());
        transaction.commit()
    }

    /// Scans village records through the full global-data parser.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_villages_blocking(&self) -> Result<Vec<ParsedVillageData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::VillageData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Scan villages lightweight blocking.
    pub fn scan_villages_lightweight_blocking(
        &self,
        cancel: &CancelFlag,
    ) -> Result<Vec<ParsedVillageData>> {
        let mut villages = Vec::new();
        let options = StorageReadOptions {
            cancel: Some(cancel.to_storage_cancel()),
            ..StorageReadOptions::default()
        };
        self.storage()
            .for_each_prefix_ref(b"VILLAGE_", options, &mut |entry| {
                if cancel.is_cancelled() {
                    return Err(BedrockWorldError::Cancelled {
                        operation: "village scan",
                    });
                }
                let BedrockDbKey::Village(key) = BedrockDbKey::decode(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                let roots = parse_consecutive_root_nbt(entry.value).unwrap_or_default();
                villages.push(ParsedVillageData {
                    key,
                    roots,
                    raw: Bytes::new(),
                });
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(villages)
    }

    /// Scans global records through the full global-data parser.
    ///
    /// Prefer [`Self::scan_global_records_blocking`] when only typed global
    /// records are needed.
    ///
    /// # Errors
    ///
    /// Returns storage or parse errors from the underlying world scan.
    pub fn scan_globals_blocking(&self) -> Result<Vec<ParsedGlobalData>> {
        Ok(self
            .parse_global_data_blocking()?
            .into_iter()
            .filter_map(|entry| match entry.value {
                ParsedDbValue::GlobalData(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    /// Reads a single typed global record by exact key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or global NBT parse errors.
    pub fn read_global_record_blocking(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<ParsedGlobalData>> {
        let key = kind.storage_key();
        self.storage()
            .get(&key)?
            .map(|value| parse_global_record(kind.clone(), kind.name(), value))
            .transpose()
    }

    /// Scans known global records while preserving each typed key kind.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or global NBT parse errors.
    pub fn scan_global_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedGlobalData>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Global(kind) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                records.push(parse_global_record(
                    kind.clone(),
                    kind.name(),
                    value.clone(),
                )?);
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(records)
    }

    /// Writes a global record after serialize -> parse roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed records, or storage errors from the commit.
    pub fn write_global_record_blocking(&self, record: &ParsedGlobalData) -> Result<()> {
        self.ensure_writable()?;
        let value = encode_global_record(record)?;
        parse_global_record(record.kind.clone(), record.name.clone(), value.clone())?;
        let mut transaction = self.transaction();
        transaction.put_raw_key(record.kind.storage_key(), value);
        transaction.commit()
    }

    /// Deletes a typed global record.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_global_record_blocking(&self, kind: GlobalRecordKind) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(kind.storage_key());
        transaction.commit()
    }

    /// Reads the Data2D/Data3D height map for a chunk.
    ///
    /// # Errors
    ///
    /// Returns storage errors or biome/heightmap parse errors.
    pub fn get_heightmap_blocking(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        self.get_biome_data_blocking(pos)?
            .map(|data| HeightMap2d::new(data.height_map))
            .transpose()
    }

    /// Writes a chunk height map while preserving existing `Data3D` biome storages.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for invalid height map length, or storage errors.
    pub fn put_heightmap_blocking(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<()> {
        self.ensure_writable()?;
        let existing = self.get_biome_data_blocking(pos)?;
        let storages = existing.map_or_else(Vec::new, |data| data.storages);
        let value = match version {
            ChunkVersion::Old => Biome2d::new(height_map.values, vec![0; 256])?.encode()?,
            ChunkVersion::New => Biome3d::new(height_map.values, storages)?.encode()?,
        };
        let tag = match version {
            ChunkVersion::Old => ChunkRecordTag::Data2D,
            ChunkVersion::New => ChunkRecordTag::Data3D,
        };
        self.put_raw_record_blocking(&ChunkKey::new(pos, tag), &value)
    }

    /// Writes a full `Data3D` biome payload after roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed biome storage, or storage errors.
    pub fn put_biome_storage_blocking(&self, pos: ChunkPos, biome: Biome3d) -> Result<()> {
        self.ensure_writable()?;
        let value = biome.encode()?;
        Biome3d::parse(&value)?;
        self.put_raw_record_blocking(&ChunkKey::new(pos, ChunkRecordTag::Data3D), &value)
    }

    /// Scans hardcoded spawn area records across the world.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or HSA payload validation errors.
    pub fn scan_hsa_records_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<ParsedHardcodedSpawnArea>)>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                if chunk_key.tag == ChunkRecordTag::HardcodedSpawners {
                    records.push((chunk_key.pos, parse_hardcoded_spawn_area_records(value)?));
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(records)
    }

    /// Writes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for invalid bounds/lengths, or storage errors.
    pub fn put_hsa_for_chunk_blocking(
        &self,
        pos: ChunkPos,
        areas: &[ParsedHardcodedSpawnArea],
    ) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.put_hsa_for_chunk(pos, areas)?;
        transaction.commit()
    }

    /// Deletes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors.
    pub fn delete_hsa_for_chunk_blocking(&self, pos: ChunkPos) -> Result<()> {
        self.delete_raw_record_blocking(&ChunkKey::new(pos, ChunkRecordTag::HardcodedSpawners))
    }

    /// Reads all block entities from a chunk's consecutive NBT payload.
    ///
    /// # Errors
    ///
    /// Returns storage errors or block-entity NBT parse errors.
    pub fn block_entities_in_chunk_blocking(
        &self,
        pos: ChunkPos,
    ) -> Result<Vec<BlockEntityRecord>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(Vec::new());
        };
        let mut report = WorldParseReport::default();
        Ok(parse_block_entities_from_value(&value, &mut report)
            .into_iter()
            .enumerate()
            .map(|(index, entity)| BlockEntityRecord {
                chunk: pos,
                index,
                entity,
            })
            .collect())
    }

    /// Replaces a chunk's block entity payload after coordinate validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when entity coordinates do not belong to `pos`, or storage errors.
    pub fn put_block_entities_blocking(
        &self,
        pos: ChunkPos,
        entities: &[ParsedBlockEntity],
    ) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.put_block_entities(pos, entities)?;
        transaction.commit()
    }

    /// Edits one block entity in place and rewrites the chunk payload.
    ///
    /// # Errors
    ///
    /// Returns validation errors when no block entity exists at `block`, when
    /// the edited NBT no longer parses as a block entity, or storage/read-only
    /// errors from the write.
    pub fn edit_block_entity_at_blocking<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut NbtTag) -> Result<()>,
    {
        self.ensure_writable()?;
        let mut entities = self
            .block_entities_in_chunk_blocking(pos)?
            .into_iter()
            .map(|record| record.entity)
            .collect::<Vec<_>>();
        let Some(index) = entities
            .iter()
            .position(|entity| entity.position == Some([block.x, block.y, block.z]))
        else {
            return Err(BedrockWorldError::Validation(format!(
                "no block entity exists at {},{},{}",
                block.x, block.y, block.z
            )));
        };
        edit(&mut entities[index].nbt)?;
        let mut report = WorldParseReport::default();
        entities[index] = parse_block_entities_from_value(
            &Bytes::from(serialize_root_nbt(&entities[index].nbt)?),
            &mut report,
        )
        .into_iter()
        .next()
        .ok_or_else(|| BedrockWorldError::Validation("edited block entity vanished".to_string()))?;
        self.put_block_entities_blocking(pos, &entities)
    }

    /// Deletes one block entity by absolute block position.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from rewriting/deleting the payload.
    pub fn delete_block_entity_at_blocking(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        self.ensure_writable()?;
        let entities = self
            .block_entities_in_chunk_blocking(pos)?
            .into_iter()
            .map(|record| record.entity)
            .filter(|entity| entity.position != Some([block.x, block.y, block.z]))
            .collect::<Vec<_>>();
        if entities.is_empty() {
            return self
                .delete_raw_record_blocking(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity));
        }
        self.put_block_entities_blocking(pos, &entities)
    }

    /// Reads actors from both legacy inline `Entity` and modern digest/prefix storage.
    ///
    /// # Errors
    ///
    /// Returns storage errors or digest validation errors.
    pub fn actors_in_chunk_blocking(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let mut records = Vec::new();
        let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity);
        if let Some(value) = self.storage().get(&inline_key.encode())? {
            let mut report = WorldParseReport::default();
            records.extend(
                parse_entities_from_value(&value, &mut report)
                    .into_iter()
                    .map(|entity| ActorRecord {
                        uid: entity.unique_id.map(ActorUid),
                        source: ActorSource::InlineChunk(inline_key.clone()),
                        entity,
                        raw: value.clone(),
                    }),
            );
        }
        let digest_key = ActorDigestKey::new(pos).storage_key();
        let Some(digest) = self.storage().get(&digest_key)? else {
            return Ok(records);
        };
        let ids = parse_actor_digest_ids(&digest)?;
        let actor_keys = ids.iter().map(|id| id.storage_key()).collect::<Vec<_>>();
        let values = self.storage().get_many(&actor_keys)?;
        for (id, value) in ids.into_iter().zip(values) {
            let Some(value) = value else {
                continue;
            };
            let mut report = WorldParseReport::default();
            records.extend(
                parse_entities_from_value(&value, &mut report)
                    .into_iter()
                    .map(|entity| ActorRecord {
                        uid: Some(id),
                        source: ActorSource::ActorPrefix(id),
                        entity,
                        raw: value.clone(),
                    }),
            );
        }
        Ok(records)
    }

    /// Writes a modern actor record and updates the chunk actor digest.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when `actor` has no `UniqueID`, or storage errors from the commit.
    pub fn put_actor_blocking(&self, pos: ChunkPos, actor: &ParsedEntity) -> Result<()> {
        self.ensure_writable()?;
        let uid = actor.unique_id.map(ActorUid).ok_or_else(|| {
            BedrockWorldError::Validation("actor UniqueID is required".to_string())
        })?;
        let value = Bytes::from(serialize_root_nbt(&actor.nbt)?);
        parse_entities_from_value(&value, &mut WorldParseReport::default());
        let mut transaction = self.transaction();
        transaction.put_actor(pos, uid, value)?;
        transaction.commit()
    }

    /// Deletes a modern actor record and removes it from the chunk digest.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_actor_blocking(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_actor(pos, uid)?;
        transaction.commit()
    }

    /// Moves a modern actor between chunk digests and rewrites its actorprefix payload.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors when `actor` has no `UniqueID`, or storage errors from the commit.
    pub fn move_actor_blocking(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: &ParsedEntity,
    ) -> Result<()> {
        self.ensure_writable()?;
        let uid = actor.unique_id.map(ActorUid).ok_or_else(|| {
            BedrockWorldError::Validation("actor UniqueID is required".to_string())
        })?;
        let value = Bytes::from(serialize_root_nbt(&actor.nbt)?);
        let mut transaction = self.transaction();
        transaction.delete_actor(from, uid)?;
        transaction.put_actor(to, uid, value)?;
        transaction.commit()
    }

    #[cfg(feature = "async")]
    /// List players.
    pub async fn list_players(&self) -> Result<Vec<PlayerId>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_players_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Classify keys.
    pub async fn classify_keys(
        &self,
        options: WorldScanOptions,
    ) -> Result<BTreeMap<String, usize>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.classify_keys_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List chunk positions.
    pub async fn list_chunk_positions(&self, options: WorldScanOptions) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_chunk_positions_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions.
    pub async fn list_render_chunk_positions(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_render_chunk_positions_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions in region.
    pub async fn list_render_chunk_positions_in_region(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.list_chunk_positions_in_region_blocking(region, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Discover chunk bounds.
    pub async fn discover_chunk_bounds(
        &self,
        dimension: crate::Dimension,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkBounds>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.discover_chunk_bounds_blocking(dimension, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Nearest loaded chunk to spawn.
    pub async fn nearest_loaded_chunk_to_spawn(
        &self,
        dimension: crate::Dimension,
        spawn_block_x: i32,
        spawn_block_z: i32,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.nearest_loaded_chunk_to_spawn_blocking(
                dimension,
                spawn_block_x,
                spawn_block_z,
                options,
            )
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Parse chunk.
    pub async fn parse_chunk(
        &self,
        pos: ChunkPos,
        options: WorldParseOptions,
    ) -> Result<ParsedChunkData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.parse_chunk_with_options_blocking(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunk.
    pub async fn load_render_chunk(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_data_blocking(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunks.
    pub async fn load_render_chunks(
        &self,
        positions: Vec<ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<Vec<ChunkData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.query_chunk_data_many_blocking(positions, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render region.
    pub async fn load_render_region(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_region_blocking(region, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan entities.
    pub async fn scan_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedEntity>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_entities_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan block entities.
    pub async fn scan_block_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedBlockEntity>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_block_entities_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan items.
    pub async fn scan_items(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_items_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan maps.
    pub async fn scan_maps(&self) -> Result<Vec<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_maps_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan villages.
    pub async fn scan_villages(&self) -> Result<Vec<ParsedVillageData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_villages_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan globals.
    pub async fn scan_globals(&self) -> Result<Vec<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_globals_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::read_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn read_map_record(&self, id: MapRecordId) -> Result<Option<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.read_map_record_blocking(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_map_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_map_records(&self, options: WorldScanOptions) -> Result<Vec<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_map_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::write_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn write_map_record(&self, record: ParsedMapData) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.write_map_record_blocking(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_map_record(&self, id: MapRecordId) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_map_record_blocking(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::read_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn read_global_record(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.read_global_record_blocking(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_global_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_global_records(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_global_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::write_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn write_global_record(&self, record: ParsedGlobalData) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.write_global_record_blocking(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_global_record(&self, kind: GlobalRecordKind) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_global_record_blocking(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::get_heightmap_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or heightmap parse errors.
    #[cfg(feature = "async")]
    pub async fn get_heightmap(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.get_heightmap_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_heightmap_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_heightmap(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_heightmap_blocking(pos, version, height_map))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_biome_storage_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_biome_storage(&self, pos: ChunkPos, biome: Biome3d) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_biome_storage_blocking(pos, biome))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_hsa_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or HSA parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_hsa_records(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<ParsedHardcodedSpawnArea>)>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_hsa_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_hsa_for_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_hsa_for_chunk(
        &self,
        pos: ChunkPos,
        areas: Vec<ParsedHardcodedSpawnArea>,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_hsa_for_chunk_blocking(pos, &areas))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_hsa_for_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_hsa_for_chunk(&self, pos: ChunkPos) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_hsa_for_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::block_entities_in_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or block-entity parse errors.
    #[cfg(feature = "async")]
    pub async fn block_entities_in_chunk(&self, pos: ChunkPos) -> Result<Vec<BlockEntityRecord>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.block_entities_in_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_block_entities_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_block_entities(
        &self,
        pos: ChunkPos,
        entities: Vec<ParsedBlockEntity>,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_block_entities_blocking(pos, &entities))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::edit_block_entity_at_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn edit_block_entity_at<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut NbtTag) -> Result<()> + Send + 'static,
    {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.edit_block_entity_at_blocking(pos, block, edit))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_block_entity_at_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_block_entity_at(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_block_entity_at_blocking(pos, block))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::actors_in_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or actor digest validation errors.
    #[cfg(feature = "async")]
    pub async fn actors_in_chunk(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.actors_in_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_actor(&self, pos: ChunkPos, actor: ParsedEntity) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_actor_blocking(pos, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_actor(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_actor_blocking(pos, uid))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::move_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn move_actor(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: ParsedEntity,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.move_actor_blocking(from, to, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    #[must_use]
    fn blocking_clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            options: self.options.clone(),
            storage: self.storage.clone(),
            format: self.format,
        }
    }

    /// Put raw record blocking.
    pub fn put_raw_record_blocking(&self, key: &ChunkKey, value: &[u8]) -> Result<()> {
        self.ensure_writable()?;
        self.storage().put(&key.encode(), value)
    }

    /// Delete raw record blocking.
    pub fn delete_raw_record_blocking(&self, key: &ChunkKey) -> Result<()> {
        self.ensure_writable()?;
        self.storage().delete(&key.encode())
    }

    #[must_use]
    /// Starts a buffered world transaction.
    pub fn transaction(&self) -> WorldTransaction<'_, S> {
        WorldTransaction {
            storage: &self.storage,
            batch: StorageBatch::new(),
            read_only: self.options.read_only,
        }
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.options.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        Ok(())
    }
}

/// Batched raw record and player writes for a [`BedrockWorld`].
pub struct WorldTransaction<'a, S = Arc<dyn WorldStorage>>
where
    S: WorldStorageHandle,
{
    storage: &'a S,
    batch: StorageBatch,
    read_only: bool,
}

impl<S> WorldTransaction<'_, S>
where
    S: WorldStorageHandle,
{
    /// Stages a raw chunk record write.
    pub fn put_raw_record(&mut self, key: &ChunkKey, value: impl Into<Bytes>) {
        self.batch.put(key.encode(), value.into());
    }

    /// Stages a raw chunk record delete.
    pub fn delete_raw_record(&mut self, key: &ChunkKey) {
        self.batch.delete(key.encode());
    }

    /// Stages a raw key/value write.
    pub fn put_raw_key(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.batch.put(key.into(), value.into());
    }

    /// Stages a raw key delete.
    pub fn delete_raw_key(&mut self, key: impl Into<Bytes>) {
        self.batch.delete(key.into());
    }

    /// Stages deletion of every raw record and modern actor owned by one chunk.
    ///
    /// # Errors
    ///
    /// Returns storage or actor-digest parse errors.
    pub fn delete_chunk(&mut self, pos: ChunkPos) -> Result<usize> {
        let mut raw_keys = Vec::new();
        self.storage.storage().for_each_prefix(
            &chunk_record_prefix(pos),
            StorageReadOptions::default(),
            &mut |raw_key, _| {
                if ChunkKey::decode(raw_key).is_ok_and(|key| key.pos == pos) {
                    raw_keys.push(Bytes::copy_from_slice(raw_key));
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        let mut deleted = raw_keys.len();
        for raw_key in raw_keys {
            self.batch.delete(raw_key);
        }

        let actor_digest_key = ActorDigestKey::new(pos).storage_key();
        if let Some(digest) = self.storage.storage().get(&actor_digest_key)? {
            for actor_uid in parse_actor_digest_ids(&digest)? {
                self.batch.delete(actor_uid.storage_key());
                deleted = deleted.saturating_add(1);
            }
        }
        self.batch.delete(actor_digest_key);
        Ok(deleted)
    }

    /// Stages a validated block-entity payload for one chunk.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors.
    pub fn put_block_entities(
        &mut self,
        pos: ChunkPos,
        entities: &[ParsedBlockEntity],
    ) -> Result<()> {
        validate_block_entities_in_chunk(pos, entities)?;
        let roots = entities
            .iter()
            .map(|entity| entity.nbt.clone())
            .collect::<Vec<_>>();
        let value = encode_consecutive_roots(&roots)?;
        let mut report = WorldParseReport::default();
        let parsed = parse_block_entities_from_value(&value, &mut report);
        validate_block_entities_in_chunk(pos, &parsed)?;
        self.put_raw_record(&ChunkKey::new(pos, ChunkRecordTag::BlockEntity), value);
        Ok(())
    }

    /// Stages a validated hardcoded-spawn-area payload for one chunk.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors.
    pub fn put_hsa_for_chunk(
        &mut self,
        pos: ChunkPos,
        areas: &[ParsedHardcodedSpawnArea],
    ) -> Result<()> {
        let value = encode_hardcoded_spawn_area_records(areas)?;
        parse_hardcoded_spawn_area_records(&value)?;
        self.put_raw_record(
            &ChunkKey::new(pos, ChunkRecordTag::HardcodedSpawners),
            value,
        );
        Ok(())
    }

    /// Stages a player record write using the player's storage key.
    ///
    /// # Errors
    ///
    /// Returns validation errors when the player id does not map to a `LevelDB`
    /// key.
    pub fn put_player(&mut self, player: &PlayerData) -> Result<()> {
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.batch
            .put(Bytes::copy_from_slice(key.as_ref()), player.raw.clone());
        Ok(())
    }

    /// Stages a typed map record write after roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed map data.
    pub fn put_map_record(&mut self, record: &ParsedMapData) -> Result<()> {
        let value = encode_map_record(record)?;
        parse_map_record(record.record_id.clone(), value.clone())?;
        self.batch.put(record.record_id.storage_key(), value);
        Ok(())
    }

    /// Stages a typed map record delete.
    pub fn delete_map_record(&mut self, id: &MapRecordId) {
        self.batch.delete(id.storage_key());
    }

    /// Stages a typed global record write after roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for malformed global data.
    pub fn put_global_record(&mut self, record: &ParsedGlobalData) -> Result<()> {
        let value = encode_global_record(record)?;
        parse_global_record(record.kind.clone(), record.name.clone(), value.clone())?;
        self.batch.put(record.kind.storage_key(), value);
        Ok(())
    }

    /// Stages a typed global record delete.
    pub fn delete_global_record(&mut self, kind: &GlobalRecordKind) {
        self.batch.delete(kind.storage_key());
    }

    /// Stages a modern actor write and updates the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed actor NBT or digest data.
    pub fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, value: Bytes) -> Result<()> {
        parse_entities_from_value(&value, &mut WorldParseReport::default());
        self.batch.put(uid.storage_key(), value);
        self.replace_actor_digest(pos, |ids| {
            if !ids.contains(&uid) {
                ids.push(uid);
            }
        })?;
        Ok(())
    }

    /// Stages a modern actor delete and removes it from the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed existing digest data.
    pub fn delete_actor(&mut self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        self.batch.delete(uid.storage_key());
        self.replace_actor_digest(pos, |ids| ids.retain(|id| *id != uid))
    }

    /// Validates and commits all staged writes atomically through the storage backend.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for unsafe key/value combinations, or storage errors.
    pub fn commit(self) -> Result<()> {
        if self.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        validate_batch(&self.batch)?;
        self.storage.storage().write_batch(&self.batch)
    }

    fn replace_actor_digest<F>(&mut self, pos: ChunkPos, update: F) -> Result<()>
    where
        F: FnOnce(&mut Vec<ActorUid>),
    {
        let key = ActorDigestKey::new(pos).storage_key();
        let mut ids = self
            .storage
            .storage()
            .get(&key)?
            .map_or_else(|| Ok(Vec::new()), |value| parse_actor_digest_ids(&value))?;
        update(&mut ids);
        if ids.is_empty() {
            self.batch.delete(key);
        } else {
            self.batch.put(key, encode_actor_digest_ids(&ids));
        }
        Ok(())
    }
}

fn validate_batch(batch: &StorageBatch) -> Result<()> {
    for op in batch.ops() {
        match op {
            StorageOp::Put { key, value } => {
                if key.is_empty() {
                    return Err(BedrockWorldError::Validation(
                        "batch contains empty key".to_string(),
                    ));
                }
                if value.is_empty() {
                    return Err(BedrockWorldError::Validation(format!(
                        "batch put for key {key:?} contains empty value"
                    )));
                }
            }
            StorageOp::Delete { key } => {
                if key.is_empty() {
                    return Err(BedrockWorldError::Validation(
                        "batch contains empty delete key".to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_block_entities_in_chunk(pos: ChunkPos, entities: &[ParsedBlockEntity]) -> Result<()> {
    for entity in entities {
        let Some([x, y, z]) = entity.position else {
            return Err(BedrockWorldError::Validation(
                "block entity is missing x/y/z position".to_string(),
            ));
        };
        let block_pos = BlockPos { x, y, z };
        if block_pos.to_chunk_pos(pos.dimension) != pos {
            return Err(BedrockWorldError::Validation(format!(
                "block entity at {x},{y},{z} is outside chunk {pos:?}"
            )));
        }
    }
    Ok(())
}

fn check_cancelled(options: &WorldScanOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelFlag::is_cancelled)
    {
        return Err(BedrockWorldError::Cancelled {
            operation: "world scan",
        });
    }
    Ok(())
}

fn emit_progress(options: &WorldScanOptions, entries_seen: usize) {
    if let Some(progress) = &options.progress {
        progress.emit(WorldScanProgress { entries_seen });
    }
}

fn check_render_load_cancelled(options: &ChunkLoadOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelFlag::is_cancelled)
    {
        return Err(BedrockWorldError::Cancelled {
            operation: "render chunk load",
        });
    }
    Ok(())
}

fn emit_render_load_progress(options: &ChunkLoadOptions, completed_chunks: usize) {
    if completed_chunks.is_multiple_of(options.pipeline.resolve_progress_interval()) {
        if let Some(progress) = &options.progress {
            progress.emit(WorldScanProgress {
                entries_seen: completed_chunks,
            });
        }
    }
}

fn sort_render_chunk_positions(positions: &mut [ChunkPos], priority: ChunkLoadPriority) {
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

fn push_render_record_request(
    keys: &mut Vec<Bytes>,
    requests: &mut Vec<RenderRecordRequest>,
    chunk_index: usize,
    pos: ChunkPos,
    kind: RenderRecordKind,
) {
    let key = match kind {
        RenderRecordKind::LegacyTerrain => {
            ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode()
        }
        RenderRecordKind::Data3D => ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
        RenderRecordKind::Data2D => ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
        RenderRecordKind::Data2DLegacy => ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
        RenderRecordKind::Subchunk(y) => ChunkKey::subchunk(pos, y).encode(),
        RenderRecordKind::BlockEntity => ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
    };
    keys.push(key);
    requests.push(RenderRecordRequest { chunk_index, kind });
}

fn apply_render_record_values(
    chunks: &mut [RawChunkData],
    requests: &[RenderRecordRequest],
    values: Vec<Option<Bytes>>,
) -> usize {
    let mut found = 0usize;
    for (request, value) in requests.iter().copied().zip(values) {
        let Some(value) = value else {
            continue;
        };
        found = found.saturating_add(1);
        let Some(chunk) = chunks.get_mut(request.chunk_index) else {
            continue;
        };
        match request.kind {
            RenderRecordKind::LegacyTerrain => {
                chunk.legacy_terrain = Some(value);
            }
            RenderRecordKind::Data3D => {
                if chunk.biome_record.is_none() {
                    chunk.biome_record = Some((crate::ChunkVersion::New, value));
                }
            }
            RenderRecordKind::Data2D | RenderRecordKind::Data2DLegacy => {
                if chunk.biome_record.is_none() {
                    chunk.biome_record = Some((crate::ChunkVersion::Old, value));
                }
            }
            RenderRecordKind::Subchunk(y) => {
                chunk.subchunks.insert(y, value);
            }
            RenderRecordKind::BlockEntity => {
                chunk.block_entities = Some(value);
            }
        }
    }
    found
}

fn planned_render_subchunk_ys(
    pos: ChunkPos,
    options: &ChunkLoadOptions,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
) -> Result<BTreeSet<i8>> {
    let mut subchunk_ys = BTreeSet::new();
    let request = options.data_request.clone();
    let (min_y, max_y) = pos.subchunk_index_range(crate::ChunkVersion::New);
    for requirement in request.subchunks {
        match requirement {
            SubchunkDataRequirement::SurfaceColumns(subchunks) => match subchunks {
                ExactSurfaceSubchunkPolicy::Full => {
                    for y in min_y..=max_y {
                        subchunk_ys.insert(y);
                    }
                }
                ExactSurfaceSubchunkPolicy::HintThenVerify => {
                    if let Some(height_map) = height_map {
                        insert_needed_surface_subchunks(
                            &mut subchunk_ys,
                            Some(height_map),
                            min_y,
                            max_y,
                        );
                    } else {
                        for y in min_y..=max_y {
                            subchunk_ys.insert(y);
                        }
                    }
                }
            },
            SubchunkDataRequirement::Layer(y) | SubchunkDataRequirement::CaveSlice(y) => {
                subchunk_ys.insert(block_y_to_subchunk_y(y)?);
            }
            SubchunkDataRequirement::Full3dIndices => {
                for y in min_y..=max_y {
                    subchunk_ys.insert(y);
                }
            }
        }
    }
    Ok(subchunk_ys)
}

fn request_needs_biome_record(options: &ChunkLoadOptions) -> bool {
    let request = &options.data_request;
    request.height_map || !matches!(request.biome, BiomeDataRequirement::None)
}

fn request_needs_legacy_terrain(options: &ChunkLoadOptions) -> bool {
    options.data_request.height_map
        || request_builds_column_samples(options)
        || !matches!(options.data_request.biome, BiomeDataRequirement::None)
}

fn request_needs_legacy_terrain_fallback(options: &ChunkLoadOptions) -> bool {
    !request_needs_legacy_terrain(options) && !options.data_request.subchunks.is_empty()
}

fn request_loads_block_entities(options: &ChunkLoadOptions) -> bool {
    options.data_request.block_entities
}

fn request_builds_column_samples(options: &ChunkLoadOptions) -> bool {
    options
        .data_request
        .subchunks
        .iter()
        .any(|requirement| matches!(requirement, SubchunkDataRequirement::SurfaceColumns(_)))
}

fn request_uses_hint_surface_subchunks(options: &ChunkLoadOptions) -> bool {
    options.data_request.subchunks.iter().any(|requirement| {
        matches!(
            requirement,
            SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::HintThenVerify)
        )
    })
}

fn exact_surface_full_request(options: &mut ChunkLoadOptions) {
    let mut request = options.data_request.clone();
    for requirement in &mut request.subchunks {
        if matches!(
            requirement,
            SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::HintThenVerify)
        ) {
            *requirement =
                SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::Full);
        }
    }
    options.data_request = request;
}

fn insert_render_biome_storages(
    render_biomes: &mut BTreeMap<i32, ParsedBiomeStorage>,
    biome_data: Option<ParsedBiomeData>,
    request: &ChunkDataRequest,
) {
    let Some(biome_data) = biome_data else {
        return;
    };
    match request.biome {
        BiomeDataRequirement::SurfaceColumns | BiomeDataRequirement::All => {
            for storage in biome_data.storages {
                let key = storage.y.unwrap_or(i32::MIN);
                render_biomes.insert(key, storage);
            }
        }
        BiomeDataRequirement::Layer(y) => {
            let mut fallback = None;
            for storage in biome_data.storages {
                if biome_storage_contains_y(&storage, y) {
                    render_biomes.insert(biome_storage_bucket_y(y), storage);
                    return;
                }
                fallback.get_or_insert(storage);
            }
            if let Some(storage) = fallback {
                render_biomes.insert(biome_storage_bucket_y(y), storage);
            }
        }
        BiomeDataRequirement::None => {}
    }
}

fn parse_render_biome_record(
    record: Option<&(crate::ChunkVersion, Bytes)>,
) -> Result<Option<ParsedBiomeData>> {
    let Some((version, value)) = record else {
        return Ok(None);
    };
    let data = match version {
        crate::ChunkVersion::New => parse_data3d(value),
        crate::ChunkVersion::Old => parse_legacy_data2d(value),
    }
    .map_err(|error| BedrockWorldError::CorruptWorld(format!("biome data: {error}")))?;
    Ok(Some(data))
}

fn render_height_map_from_biome_data(
    pos: ChunkPos,
    biome_data: &ParsedBiomeData,
) -> [[Option<i16>; 16]; 16] {
    let mut heights = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let index = height_map_index(local_x, local_z);
            heights[usize::from(local_z)][usize::from(local_x)] = biome_data
                .height_map
                .get(index)
                .and_then(|height| normalize_biome_height(pos, biome_data.version, *height));
        }
    }
    heights
}

fn normalize_biome_height(
    pos: ChunkPos,
    version: crate::ChunkVersion,
    stored_height: i16,
) -> Option<i16> {
    let (min_y, _) = pos.y_range(version);
    i16::try_from(i32::from(stored_height) + min_y).ok()
}

fn legacy_height_map_from_raw(
    raw_legacy_terrain: Option<&Bytes>,
) -> Result<Option<[[Option<i16>; 16]; 16]>> {
    let Some(raw_legacy_terrain) = raw_legacy_terrain else {
        return Ok(None);
    };
    let terrain = LegacyTerrain::parse(raw_legacy_terrain.clone())?;
    Ok(Some(render_height_map_from_legacy_terrain(&terrain)))
}

fn render_height_map_from_legacy_terrain(terrain: &LegacyTerrain) -> [[Option<i16>; 16]; 16] {
    let mut heights = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            heights[usize::from(local_z)][usize::from(local_x)] =
                terrain.height_at(local_x, local_z).map(i16::from);
        }
    }
    heights
}

fn render_biomes_from_legacy_terrain(
    terrain: &LegacyTerrain,
) -> [[Option<LegacyBiomeSample>; 16]; 16] {
    let mut samples = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            samples[usize::from(local_z)][usize::from(local_x)] =
                terrain.biome_sample_at(local_x, local_z);
        }
    }
    samples
}

fn render_biome_colors_from_legacy_terrain(terrain: &LegacyTerrain) -> [[Option<u32>; 16]; 16] {
    let mut colors = [[None; 16]; 16];
    let samples = render_biomes_from_legacy_terrain(terrain);
    for local_z in 0..16 {
        for local_x in 0..16 {
            colors[local_z][local_x] = samples[local_z][local_x].map(LegacyBiomeSample::rgb_u32);
        }
    }
    colors
}

struct SurfaceProjection<'a> {
    min_subchunk_y: i8,
    subchunks: Vec<Option<&'a SubChunk>>,
    storages: Vec<Option<Vec<SurfaceStorageProjection<'a>>>>,
}

struct SurfaceStorageProjection<'a> {
    states: &'a [BlockState],
    roles: Vec<TerrainSurfaceRole>,
    indices: Cow<'a, [u16]>,
}

struct ProjectedSurfaceEntry<'a> {
    entry: BlockStatePaletteEntry<'a>,
    role: TerrainSurfaceRole,
}

struct ProjectedSurfaceStatesAt<'projection, 'chunk> {
    storages: std::iter::Rev<
        std::iter::Enumerate<std::slice::Iter<'projection, SurfaceStorageProjection<'chunk>>>,
    >,
    block_index: usize,
}

impl<'chunk> Iterator for ProjectedSurfaceStatesAt<'_, 'chunk> {
    type Item = ProjectedSurfaceEntry<'chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        for (storage_index, storage) in &mut self.storages {
            let Some(palette_index) = storage.indices.get(self.block_index).copied() else {
                continue;
            };
            let palette_index = usize::from(palette_index);
            let Some(state) = storage.states.get(palette_index) else {
                continue;
            };
            let Some(role) = storage.roles.get(palette_index).copied() else {
                continue;
            };
            if role != TerrainSurfaceRole::Air {
                return Some(ProjectedSurfaceEntry {
                    entry: BlockStatePaletteEntry {
                        state,
                        storage_index,
                    },
                    role,
                });
            }
        }
        None
    }
}

impl<'a> SurfaceProjection<'a> {
    fn new(subchunks: &'a BTreeMap<i8, SubChunk>) -> Option<Self> {
        let (&min_subchunk_y, _) = subchunks.first_key_value()?;
        let (&max_subchunk_y, _) = subchunks.last_key_value()?;
        let span = i16::from(max_subchunk_y)
            .saturating_sub(i16::from(min_subchunk_y))
            .saturating_add(1);
        let len = usize::try_from(span).ok()?;
        let mut projected = vec![None; len];
        let mut storage_projection = (0..len).map(|_| None).collect::<Vec<_>>();
        for (&y, subchunk) in subchunks {
            let index = i16::from(y).saturating_sub(i16::from(min_subchunk_y));
            if let Ok(index) = usize::try_from(index) {
                if let Some(slot) = projected.get_mut(index) {
                    *slot = Some(subchunk);
                    storage_projection[index] = match &subchunk.format {
                        crate::chunk::SubChunkFormat::Paletted { storages, .. } => storages
                            .iter()
                            .map(|storage| {
                                Some(SurfaceStorageProjection {
                                    states: &storage.states,
                                    roles: storage
                                        .states
                                        .iter()
                                        .map(|state| terrain_surface_role(&state.name))
                                        .collect(),
                                    indices: storage.surface_indices()?,
                                })
                            })
                            .collect::<Option<Vec<_>>>(),
                        _ => None,
                    };
                }
            }
        }
        Some(Self {
            min_subchunk_y,
            subchunks: projected,
            storages: storage_projection,
        })
    }

    fn get(&self, y: i8) -> Option<&'a SubChunk> {
        let index = i16::from(y).checked_sub(i16::from(self.min_subchunk_y))?;
        self.subchunks
            .get(usize::try_from(index).ok()?)?
            .as_ref()
            .copied()
    }

    fn surface_states_at(
        &self,
        y: i8,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<ProjectedSurfaceStatesAt<'_, 'a>> {
        let index =
            usize::try_from(i16::from(y).checked_sub(i16::from(self.min_subchunk_y))?).ok()?;
        let storages = self.storages.get(index)?.as_ref()?;
        Some(ProjectedSurfaceStatesAt {
            storages: storages.iter().enumerate().rev(),
            block_index: block_storage_index(local_x, local_y, local_z),
        })
    }
}

fn build_terrain_column_samples(
    pos: ChunkPos,
    version: crate::ChunkVersion,
    subchunks: &BTreeMap<i8, SubChunk>,
    legacy_terrain: Option<&LegacyTerrain>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, ParsedBiomeStorage>,
) -> Result<TerrainColumnSamples> {
    let mut columns = TerrainColumnSamples::new();
    let projection = SurfaceProjection::new(subchunks);
    let (min_y, max_y) = if legacy_terrain.is_some() && subchunks.is_empty() {
        (0, 127)
    } else {
        pos.y_range(version)
    };

    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            if let Some(sample) = sample_column_top_down(
                local_x,
                local_z,
                min_y,
                max_y,
                subchunks,
                projection.as_ref(),
                legacy_terrain,
                height_map,
                legacy_biomes,
                render_biomes,
            )? {
                columns.set(local_x, local_z, sample);
            }
        }
    }
    Ok(columns)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn sample_column_top_down(
    local_x: u8,
    local_z: u8,
    min_y: i32,
    max_y: i32,
    subchunks: &BTreeMap<i8, SubChunk>,
    projection: Option<&SurfaceProjection<'_>>,
    legacy_terrain: Option<&LegacyTerrain>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, ParsedBiomeStorage>,
) -> Result<Option<TerrainColumnSample>> {
    let mut overlay: Option<TerrainColumnOverlay> = None;
    let mut top_water: Option<(i16, BlockState, TerrainSampleSource)> = None;
    let mut water_depth = 0_u8;
    for y in (min_y..=max_y).rev() {
        let height = i16::try_from(y).unwrap_or(if y < 0 { i16::MIN } else { i16::MAX });

        let subchunk_y = block_y_to_subchunk_y(y)?;
        let local_y = u8::try_from(y - i32::from(subchunk_y) * 16).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={y} has invalid local subchunk offset"))
        })?;
        let mut saw_subchunk_layer = false;
        if let Some(subchunk) = projection.and_then(|projection| projection.get(subchunk_y)) {
            if let Some(projected_states) = projection.and_then(|projection| {
                projection.surface_states_at(subchunk_y, local_x, local_y, local_z)
            }) {
                for projected in projected_states {
                    saw_subchunk_layer = true;
                    if let Some(sample) = scan_terrain_surface_state(
                        local_x,
                        local_z,
                        y,
                        height,
                        projected.entry.state,
                        projected.role,
                        TerrainSampleSource::Subchunk,
                        &mut overlay,
                        &mut top_water,
                        &mut water_depth,
                        legacy_biomes,
                        render_biomes,
                    ) {
                        return Ok(Some(sample));
                    }
                }
            } else {
                for entry in subchunk.visible_block_surface_states_at(local_x, local_y, local_z) {
                    saw_subchunk_layer = true;
                    let role = terrain_surface_role(&entry.state.name);
                    if let Some(sample) = scan_terrain_surface_state(
                        local_x,
                        local_z,
                        y,
                        height,
                        entry.state,
                        role,
                        TerrainSampleSource::Subchunk,
                        &mut overlay,
                        &mut top_water,
                        &mut water_depth,
                        legacy_biomes,
                        render_biomes,
                    ) {
                        return Ok(Some(sample));
                    }
                }
            }
            if saw_subchunk_layer {
                continue;
            }
            if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z) {
                let data = subchunk
                    .legacy_block_data_at(local_x, local_y, local_z)
                    .unwrap_or(0);
                let state = legacy_world_block_state(id, data);
                let role = terrain_surface_role(&state.name);
                if let Some(sample) = scan_terrain_surface_state(
                    local_x,
                    local_z,
                    y,
                    height,
                    &state,
                    role,
                    TerrainSampleSource::Subchunk,
                    &mut overlay,
                    &mut top_water,
                    &mut water_depth,
                    legacy_biomes,
                    render_biomes,
                ) {
                    return Ok(Some(sample));
                }
                continue;
            }
        }

        if let Some((state, source)) =
            legacy_terrain_block_state_at(local_x, y, local_z, subchunks, legacy_terrain)
        {
            let role = terrain_surface_role(&state.name);
            if let Some(sample) = scan_terrain_surface_state(
                local_x,
                local_z,
                y,
                height,
                &state,
                role,
                source,
                &mut overlay,
                &mut top_water,
                &mut water_depth,
                legacy_biomes,
                render_biomes,
            ) {
                return Ok(Some(sample));
            }
        }
    }

    if let Some((water_height, water_state, water_source)) = top_water {
        let biome = terrain_biome_at(
            local_x,
            local_z,
            i32::from(water_height),
            legacy_biomes,
            render_biomes,
        );
        let relief_y = raw_height_at(height_map, local_x, local_z).unwrap_or(water_height);
        return Ok(Some(TerrainColumnSample {
            surface_y: water_height,
            surface_block_state: water_state.clone(),
            relief_y,
            relief_block_state: water_state.clone(),
            overlay,
            water: Some(TerrainColumnWater {
                surface_y: water_height,
                block_state: water_state,
                depth: water_depth,
                underwater_y: None,
                underwater_block_state: None,
                source: water_source,
            }),
            biome,
            source: water_source,
        }));
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn scan_terrain_surface_state(
    local_x: u8,
    local_z: u8,
    y: i32,
    height: i16,
    state: &BlockState,
    role: TerrainSurfaceRole,
    source: TerrainSampleSource,
    overlay: &mut Option<TerrainColumnOverlay>,
    top_water: &mut Option<(i16, BlockState, TerrainSampleSource)>,
    water_depth: &mut u8,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, ParsedBiomeStorage>,
) -> Option<TerrainColumnSample> {
    match role {
        TerrainSurfaceRole::Air => {
            if top_water.is_some() {
                *water_depth = (*water_depth).saturating_add(1);
            }
            None
        }
        TerrainSurfaceRole::Overlay => {
            if let Some((water_height, water_state, water_source)) = top_water.take() {
                let biome = terrain_biome_at(local_x, local_z, y, legacy_biomes, render_biomes);
                return Some(TerrainColumnSample {
                    surface_y: water_height,
                    surface_block_state: water_state.clone(),
                    relief_y: height,
                    relief_block_state: state.clone(),
                    overlay: overlay.take(),
                    water: Some(TerrainColumnWater {
                        surface_y: water_height,
                        block_state: water_state,
                        depth: (*water_depth).saturating_add(1),
                        underwater_y: Some(height),
                        underwater_block_state: Some(state.clone()),
                        source: water_source,
                    }),
                    biome,
                    source: water_source,
                });
            }
            if overlay.is_none() {
                *overlay = Some(TerrainColumnOverlay {
                    y: height,
                    block_state: state.clone(),
                    source,
                });
            }
            None
        }
        TerrainSurfaceRole::Water => {
            if top_water.is_none() {
                *top_water = Some((height, state.clone(), source));
            } else {
                *water_depth = (*water_depth).saturating_add(1);
            }
            None
        }
        TerrainSurfaceRole::Primary => {
            let biome = terrain_biome_at(local_x, local_z, y, legacy_biomes, render_biomes);
            if let Some((water_height, water_state, water_source)) = top_water.take() {
                return Some(TerrainColumnSample {
                    surface_y: water_height,
                    surface_block_state: water_state.clone(),
                    relief_y: height,
                    relief_block_state: state.clone(),
                    overlay: overlay.take(),
                    water: Some(TerrainColumnWater {
                        surface_y: water_height,
                        block_state: water_state,
                        depth: (*water_depth).saturating_add(1),
                        underwater_y: Some(height),
                        underwater_block_state: Some(state.clone()),
                        source: water_source,
                    }),
                    biome,
                    source: water_source,
                });
            }
            Some(TerrainColumnSample {
                surface_y: height,
                surface_block_state: state.clone(),
                relief_y: height,
                relief_block_state: state.clone(),
                overlay: overlay.take(),
                water: None,
                biome,
                source,
            })
        }
    }
}

fn legacy_terrain_block_state_at(
    local_x: u8,
    y: i32,
    local_z: u8,
    subchunks: &BTreeMap<i8, SubChunk>,
    legacy_terrain: Option<&LegacyTerrain>,
) -> Option<(BlockState, TerrainSampleSource)> {
    let terrain = legacy_terrain?;
    if !(0..=127).contains(&y) {
        return None;
    }
    let legacy_y = u8::try_from(y).ok()?;
    let id = terrain.block_id_at(local_x, legacy_y, local_z)?;
    let data = terrain
        .block_data_at(local_x, legacy_y, local_z)
        .unwrap_or(0);
    let source = if subchunks.is_empty() {
        TerrainSampleSource::LegacyTerrain
    } else {
        TerrainSampleSource::LegacyFallback
    };
    Some((legacy_world_block_state(id, data), source))
}

fn terrain_biome_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, ParsedBiomeStorage>,
) -> Option<TerrainColumnBiome> {
    legacy_biomes
        .and_then(|samples| samples[usize::from(local_z)][usize::from(local_x)])
        .map(TerrainColumnBiome::Legacy)
        .or_else(|| {
            render_biome_id_at(local_x, local_z, y, render_biomes).map(TerrainColumnBiome::Id)
        })
}

fn render_biome_id_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    render_biomes: &BTreeMap<i32, ParsedBiomeStorage>,
) -> Option<u32> {
    let direct = render_biomes
        .get(&biome_storage_bucket_y(y))
        .or_else(|| render_biomes.values().next())
        .and_then(|storage| {
            biome_id_from_storage(storage, local_x, local_z, y).filter(|id| *id != 0)
        });
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
        for local_y in (0..16_u8).rev() {
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

fn render_chunk_from_raw(
    raw: RawChunkData,
    options: &ChunkLoadOptions,
) -> Result<(ChunkData, ChunkDecodeTiming)> {
    let mut timing = ChunkDecodeTiming::default();
    let biome_started = Instant::now();
    let legacy_terrain = raw.legacy_terrain.map(LegacyTerrain::parse).transpose()?;
    let version = raw.biome_record.as_ref().map_or_else(
        || {
            if legacy_terrain.is_some() {
                crate::ChunkVersion::Old
            } else {
                crate::ChunkVersion::New
            }
        },
        |(version, _)| *version,
    );
    let biome_data = parse_render_biome_record(raw.biome_record.as_ref())?;
    let height_map = biome_data
        .as_ref()
        .map(|biome_data| render_height_map_from_biome_data(raw.pos, biome_data))
        .or_else(|| {
            legacy_terrain
                .as_ref()
                .map(render_height_map_from_legacy_terrain)
        });
    let legacy_biomes = legacy_terrain
        .as_ref()
        .map(render_biomes_from_legacy_terrain);
    let legacy_biome_colors = legacy_terrain
        .as_ref()
        .map(render_biome_colors_from_legacy_terrain);
    let mut render_biomes = BTreeMap::new();
    let data_request = &options.data_request;
    insert_render_biome_storages(&mut render_biomes, biome_data, data_request);
    timing.biome_parse_us = biome_started.elapsed().as_micros();

    let mut subchunks = BTreeMap::new();
    let subchunk_started = Instant::now();
    let preferred_decode = options.data_request.preferred_decode_mode();
    let subchunk_decode = match (preferred_decode, options.subchunk_decode) {
        (SubChunkDecodeMode::FullIndices, SubChunkDecodeMode::PackedIndices) => {
            SubChunkDecodeMode::PackedIndices
        }
        (SubChunkDecodeMode::CountsOnly, SubChunkDecodeMode::CountsOnly) => {
            SubChunkDecodeMode::CountsOnly
        }
        (SubChunkDecodeMode::SurfaceColumns, SubChunkDecodeMode::SurfaceColumns) => {
            SubChunkDecodeMode::SurfaceColumns
        }
        _ => preferred_decode,
    };
    for (y, value) in raw.subchunks {
        check_render_load_cancelled(options)?;
        subchunks.insert(y, parse_subchunk_with_mode(y, value, subchunk_decode)?);
    }
    timing.subchunk_parse_us = subchunk_started.elapsed().as_micros();

    let block_entity_started = Instant::now();
    let block_entities = if request_loads_block_entities(options) {
        if let Some(value) = raw.block_entities {
            let mut report = WorldParseReport::default();
            parse_block_entities_from_value(&value, &mut report)
                .into_iter()
                .map(|entity| render_block_entity_from_nbt(entity.nbt))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    timing.block_entity_parse_us = block_entity_started.elapsed().as_micros();

    let surface_scan_started = Instant::now();
    let column_samples = if request_builds_column_samples(options) {
        Some(build_terrain_column_samples(
            raw.pos,
            version,
            &subchunks,
            legacy_terrain.as_ref(),
            height_map.as_ref(),
            legacy_biomes.as_ref(),
            &render_biomes,
        )?)
    } else {
        None
    };
    timing.surface_scan_us = surface_scan_started.elapsed().as_micros();

    Ok((
        ChunkData {
            pos: raw.pos,
            is_loaded: height_map.is_some()
                || legacy_biome_colors.is_some()
                || legacy_biomes.is_some()
                || !render_biomes.is_empty()
                || !subchunks.is_empty()
                || !block_entities.is_empty()
                || legacy_terrain.is_some(),
            height_map,
            legacy_biomes,
            legacy_biome_colors,
            biome_data: render_biomes,
            subchunks,
            block_entities,
            legacy_terrain,
            column_samples,
            version,
        },
        timing,
    ))
}

fn render_load_stats(
    chunks: &[ChunkData],
    worker_threads: usize,
    queue_wait_ms: u128,
    load_ms: u128,
) -> ChunkLoadStats {
    ChunkLoadStats {
        requested_chunks: chunks.len(),
        loaded_chunks: chunks.iter().filter(|chunk| chunk.is_loaded).count(),
        subchunks_decoded: chunks
            .iter()
            .map(|chunk| chunk.subchunks.len())
            .sum::<usize>(),
        worker_threads,
        queue_wait_ms,
        load_ms,
        keys_requested: 0,
        keys_found: 0,
        exact_get_batches: 0,
        prefix_scans: 0,
        decode_ms: 0,
        db_read_ms: 0,
        biome_parse_ms: 0,
        biome_parse_us: 0,
        subchunk_parse_ms: 0,
        subchunk_parse_us: 0,
        surface_scan_ms: 0,
        surface_scan_us: 0,
        block_entity_parse_ms: 0,
        block_entity_parse_us: 0,
        full_reload_ms: 0,
        legacy_terrain_records: chunks
            .iter()
            .filter(|chunk| chunk.legacy_terrain.is_some())
            .count(),
        legacy_biome_samples: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biomes.is_some())
            .count(),
        legacy_biome_colors: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biome_colors.is_some())
            .count(),
        terrain_source_legacy: chunks
            .iter()
            .filter(|chunk| chunk.legacy_terrain.is_some() && chunk.subchunks.is_empty())
            .count(),
        terrain_source_subchunk: chunks
            .iter()
            .filter(|chunk| !chunk.subchunks.is_empty())
            .count(),
        legacy_pocket_chunks: 0,
        detected_format: WorldFormat::LevelDb,
        computed_surface_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .map(TerrainColumnSamples::sampled_columns)
            .sum(),
        raw_height_mismatch_columns: chunks.iter().map(raw_height_mismatch_columns).sum(),
        missing_subchunk_columns: chunks.iter().map(missing_surface_columns).sum(),
        legacy_fallback_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| sample.source == TerrainSampleSource::LegacyFallback)
            .count(),
        legacy_biome_preferred_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| matches!(sample.biome, Some(TerrainColumnBiome::Legacy(_))))
            .count(),
        modern_biome_fallback_columns: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biomes.is_some())
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| matches!(sample.biome, Some(TerrainColumnBiome::Id(_))))
            .count(),
    }
}

fn log_render_load_complete(stats: &ChunkLoadStats) {
    log::debug!(
        "render chunk load complete (requested_chunks={}, loaded_chunks={}, missing_chunks={}, subchunks_decoded={}, legacy_terrain_records={}, legacy_biome_samples={}, legacy_biome_colors={}, terrain_source_legacy={}, terrain_source_subchunk={}, legacy_pocket_chunks={}, detected_format={:?}, computed_surface_columns={}, raw_height_mismatch_columns={}, missing_subchunk_columns={}, legacy_fallback_columns={}, legacy_biome_preferred_columns={}, modern_biome_fallback_columns={}, worker_threads={}, queue_wait_ms={}, load_ms={}, exact_get_batches={}, keys_requested={}, keys_found={}, prefix_scans={}, db_read_ms={}, decode_ms={}, biome_parse_ms={}, subchunk_parse_ms={}, surface_scan_ms={}, block_entity_parse_ms={}, full_reload_ms={})",
        stats.requested_chunks,
        stats.loaded_chunks,
        stats.requested_chunks.saturating_sub(stats.loaded_chunks),
        stats.subchunks_decoded,
        stats.legacy_terrain_records,
        stats.legacy_biome_samples,
        stats.legacy_biome_colors,
        stats.terrain_source_legacy,
        stats.terrain_source_subchunk,
        stats.legacy_pocket_chunks,
        stats.detected_format,
        stats.computed_surface_columns,
        stats.raw_height_mismatch_columns,
        stats.missing_subchunk_columns,
        stats.legacy_fallback_columns,
        stats.legacy_biome_preferred_columns,
        stats.modern_biome_fallback_columns,
        stats.worker_threads,
        stats.queue_wait_ms,
        stats.load_ms,
        stats.exact_get_batches,
        stats.keys_requested,
        stats.keys_found,
        stats.prefix_scans,
        stats.db_read_ms,
        stats.decode_ms,
        stats.biome_parse_ms,
        stats.subchunk_parse_ms,
        stats.surface_scan_ms,
        stats.block_entity_parse_ms,
        stats.full_reload_ms
    );
}

fn to_storage_read_options(options: &WorldScanOptions) -> StorageReadOptions {
    StorageReadOptions {
        threading: match options.threading {
            WorldThreadingOptions::Auto => StorageThreadingOptions::Auto,
            WorldThreadingOptions::Fixed(threads) => StorageThreadingOptions::Fixed(threads),
            WorldThreadingOptions::Single => StorageThreadingOptions::Single,
        },
        scan_mode: match options.threading {
            WorldThreadingOptions::Single => StorageScanMode::Sequential,
            WorldThreadingOptions::Auto | WorldThreadingOptions::Fixed(_) => {
                StorageScanMode::ParallelTables
            }
        },
        cache_policy: StorageCachePolicy::Bypass,
        pipeline: crate::storage::StoragePipelineOptions {
            queue_depth: options.pipeline.queue_depth,
            table_batch_size: options.pipeline.chunk_batch_size,
            progress_interval: options.pipeline.progress_interval,
        },
        cancel: options
            .cancel
            .as_ref()
            .map(|cancel| StorageCancelFlag::from_shared(cancel.0.clone())),
        progress: options.progress.as_ref().map(|progress| {
            let progress = progress.clone();
            StorageProgressSink::new(move |storage_progress| {
                progress.emit(WorldScanProgress {
                    entries_seen: storage_progress.entries_seen,
                });
            })
        }),
    }
}

fn to_render_storage_read_options(options: &ChunkLoadOptions) -> StorageReadOptions {
    StorageReadOptions {
        threading: match options.threading {
            WorldThreadingOptions::Auto => StorageThreadingOptions::Auto,
            WorldThreadingOptions::Fixed(threads) => StorageThreadingOptions::Fixed(threads),
            WorldThreadingOptions::Single => StorageThreadingOptions::Single,
        },
        scan_mode: StorageScanMode::Sequential,
        cache_policy: options.storage_cache_policy,
        pipeline: crate::storage::StoragePipelineOptions {
            queue_depth: options.pipeline.queue_depth,
            table_batch_size: options.pipeline.chunk_batch_size,
            progress_interval: options.pipeline.progress_interval,
        },
        cancel: options.cancel.as_ref().map(CancelFlag::to_storage_cancel),
        progress: None,
    }
}

fn chunk_record_prefix(pos: ChunkPos) -> Bytes {
    let mut bytes = Vec::with_capacity(if pos.dimension == crate::Dimension::Overworld {
        8
    } else {
        12
    });
    bytes.extend_from_slice(&pos.x.to_le_bytes());
    bytes.extend_from_slice(&pos.z.to_le_bytes());
    if pos.dimension != crate::Dimension::Overworld {
        bytes.extend_from_slice(&pos.dimension.id().to_le_bytes());
    }
    Bytes::from(bytes)
}

fn validate_render_region(region: WorldChunkQueryRegion) -> Result<()> {
    if region.min_chunk_x > region.max_chunk_x || region.min_chunk_z > region.max_chunk_z {
        return Err(BedrockWorldError::Validation(format!(
            "invalid render region: min=({}, {}) max=({}, {})",
            region.min_chunk_x, region.min_chunk_z, region.max_chunk_x, region.max_chunk_z
        )));
    }
    Ok(())
}

fn render_block_entity_from_nbt(nbt: NbtTag) -> ChunkBlockEntity {
    let root = match &nbt {
        NbtTag::Compound(root) => Some(root),
        _ => None,
    };
    ChunkBlockEntity {
        id: root
            .and_then(|root| nbt_string_field(root, "id"))
            .map(ToString::to_string),
        position: root.and_then(|root| {
            Some([
                nbt_int_field(root, "x")?,
                nbt_int_field(root, "y")?,
                nbt_int_field(root, "z")?,
            ])
        }),
        nbt,
    }
}

fn nbt_string_field<'a>(
    root: &'a indexmap::IndexMap<String, NbtTag>,
    key: &str,
) -> Option<&'a str> {
    match root.get(key) {
        Some(NbtTag::String(value)) => Some(value),
        _ => None,
    }
}

fn nbt_int_field(root: &indexmap::IndexMap<String, NbtTag>, key: &str) -> Option<i32> {
    match root.get(key) {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn detect_world_format(path: &Path, hint: WorldFormatHint) -> Result<WorldFormat> {
    match hint {
        WorldFormatHint::Auto => {
            if path.join("db").join("CURRENT").is_file() {
                return Ok(detect_leveldb_world_format(path));
            }
            if path.join("chunks.dat").is_file() {
                return Ok(WorldFormat::PocketChunksDat);
            }
            Err(BedrockWorldError::Validation(format!(
                "could not detect Bedrock world storage at {}; expected db/CURRENT or chunks.dat",
                path.display()
            )))
        }
        WorldFormatHint::LevelDb => {
            let current = path.join("db").join("CURRENT");
            if !current.is_file() {
                return Err(BedrockWorldError::Validation(format!(
                    "LevelDB world missing {}",
                    current.display()
                )));
            }
            Ok(detect_leveldb_world_format(path))
        }
        WorldFormatHint::PocketChunksDat => {
            let chunks = path.join("chunks.dat");
            if !chunks.is_file() {
                return Err(BedrockWorldError::Validation(format!(
                    "Pocket chunks.dat world missing {}",
                    chunks.display()
                )));
            }
            Ok(WorldFormat::PocketChunksDat)
        }
    }
}

fn detect_leveldb_world_format(path: &Path) -> WorldFormat {
    let Ok(document) = read_level_dat_document(&path.join("level.dat")) else {
        return WorldFormat::LevelDb;
    };
    let NbtTag::Compound(root) = &document.root else {
        return WorldFormat::LevelDb;
    };
    let storage_version = nbt_int_field(root, "StorageVersion");
    let network_version = nbt_int_field(root, "NetworkVersion");
    if storage_version.is_some_and(|version| version <= 4)
        || network_version.is_some_and(|version| version <= 91)
    {
        WorldFormat::LevelDbLegacyTerrain
    } else {
        WorldFormat::LevelDb
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Dimension, HardcodedSpawnAreaKind, MemoryStorage, NbtTag, StorageBatch, StorageReadOptions,
        StorageScanOutcome, block_storage_index,
    };
    use indexmap::IndexMap;
    use std::sync::Arc;

    #[derive(Clone)]
    struct KeyOnlyPlayerStorage;

    impl WorldStorage for KeyOnlyPlayerStorage {
        fn get(&self, _key: &[u8]) -> Result<Option<Bytes>> {
            Ok(None)
        }

        fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn delete(&self, _key: &[u8]) -> Result<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn for_each_key(
            &self,
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Ok(StorageScanOutcome::empty())
        }

        fn for_each_prefix(
            &self,
            _prefix: &[u8],
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Err(BedrockWorldError::Validation(
                "player listing requested values".to_string(),
            ))
        }

        fn for_each_prefix_key(
            &self,
            prefix: &[u8],
            _options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            assert_eq!(prefix, b"player_");
            let _ = visitor(b"player_12345")?;
            Ok(StorageScanOutcome::empty())
        }

        fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn flush(&self) -> Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    fn temp_world_dir(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        std::env::temp_dir().join(format!(
            "bedrock-world-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn exact_surface_request(
        subchunks: ExactSurfaceSubchunkPolicy,
        biome: ExactSurfaceBiomeLoad,
        block_entities: bool,
    ) -> ChunkDataRequest {
        ChunkDataRequest::new()
            .surface_columns(subchunks)
            .biome(match biome {
                ExactSurfaceBiomeLoad::None => BiomeDataRequirement::None,
                ExactSurfaceBiomeLoad::TopColumns => BiomeDataRequirement::SurfaceColumns,
                ExactSurfaceBiomeLoad::All => BiomeDataRequirement::All,
            })
            .block_entities_if(block_entities)
    }

    #[test]
    fn player_listing_uses_key_only_prefix_scan() {
        let world = BedrockWorld::from_typed_storage(
            "memory",
            KeyOnlyPlayerStorage,
            OpenOptions::default(),
        );

        assert_eq!(
            world.list_players_blocking().expect("list players"),
            vec![PlayerId::Xuid("12345".to_string())]
        );
    }

    #[test]
    fn world_threading_uses_bounded_desktop_background_budget() {
        let expected_auto = default_world_worker_budget().min(10_000);
        assert_eq!(
            WorldThreadingOptions::Auto
                .resolve_checked(10_000)
                .expect("auto threads"),
            expected_auto
        );
        assert_eq!(
            WorldThreadingOptions::Fixed(MAX_WORLD_THREADS)
                .resolve_checked(10_000)
                .expect("max fixed threads"),
            MAX_WORLD_THREADS
        );
        assert!(WorldThreadingOptions::Fixed(0).resolve_checked(10).is_err());
        assert!(
            WorldThreadingOptions::Fixed(MAX_WORLD_THREADS + 1)
                .resolve_checked(10)
                .is_err()
        );
    }

    #[test]
    fn map_and_global_records_roundtrip_through_world_transactions() {
        let storage = Arc::new(MemoryStorage::new());
        let world = BedrockWorld::from_storage(
            "memory",
            storage.clone(),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let map_id = MapRecordId::new("9").expect("map id");
        let map = ParsedMapData {
            id: map_id.to_string(),
            record_id: map_id.clone(),
            roots: vec![NbtTag::Compound(IndexMap::from([(
                "scale".to_string(),
                NbtTag::Byte(1),
            )]))],
            known_fields: crate::MapKnownFields::default(),
            pixels: None,
            raw: Bytes::new(),
        };

        world.write_map_record_blocking(&map).expect("write map");
        let read_map = world
            .read_map_record_blocking(&map_id)
            .expect("read map")
            .expect("map exists");
        assert_eq!(read_map.known_fields.scale, Some(1));

        let global = ParsedGlobalData {
            name: "scoreboard".to_string(),
            kind: GlobalRecordKind::Scoreboard,
            roots: vec![NbtTag::Compound(IndexMap::new())],
            raw: Bytes::new(),
        };
        world
            .write_global_record_blocking(&global)
            .expect("write global");
        assert!(
            world
                .read_global_record_blocking(GlobalRecordKind::Scoreboard)
                .expect("read global")
                .is_some()
        );

        world
            .delete_map_record_blocking(&map_id)
            .expect("delete map");
        assert!(
            world
                .read_map_record_blocking(&map_id)
                .expect("read deleted")
                .is_none()
        );
    }

    #[test]
    fn hsa_and_block_entities_roundtrip_with_chunk_validation() {
        let storage = Arc::new(MemoryStorage::new());
        let world = BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let area = ParsedHardcodedSpawnArea {
            kind: HardcodedSpawnAreaKind::NetherFortress,
            min: [0, 32, 0],
            max: [15, 80, 15],
        };
        world
            .put_hsa_for_chunk_blocking(pos, std::slice::from_ref(&area))
            .expect("write hsa");
        assert_eq!(
            world
                .scan_hsa_records_blocking(WorldScanOptions::default())
                .expect("scan hsa")[0]
                .1,
            vec![area]
        );

        let block_entity = ParsedBlockEntity {
            id: Some("Chest".to_string()),
            position: Some([1, 64, 1]),
            is_movable: Some(true),
            custom_name: None,
            items: Vec::new(),
            nbt: NbtTag::Compound(IndexMap::from([
                ("id".to_string(), NbtTag::String("Chest".to_string())),
                ("x".to_string(), NbtTag::Int(1)),
                ("y".to_string(), NbtTag::Int(64)),
                ("z".to_string(), NbtTag::Int(1)),
            ])),
        };
        world
            .put_block_entities_blocking(pos, std::slice::from_ref(&block_entity))
            .expect("write block entity");
        assert_eq!(
            world
                .block_entities_in_chunk_blocking(pos)
                .expect("read block entities")[0]
                .entity
                .position,
            Some([1, 64, 1])
        );
    }

    #[test]
    fn actor_write_updates_digest_and_prefix_together() {
        let storage = Arc::new(MemoryStorage::new());
        let world = BedrockWorld::from_storage(
            "memory",
            storage.clone(),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let pos = ChunkPos {
            x: 2,
            z: 3,
            dimension: Dimension::Overworld,
        };
        let actor_nbt = NbtTag::Compound(IndexMap::from([
            (
                "identifier".to_string(),
                NbtTag::String("minecraft:pig".to_string()),
            ),
            ("UniqueID".to_string(), NbtTag::Long(77)),
            (
                "Pos".to_string(),
                NbtTag::List(vec![
                    NbtTag::Float(32.0),
                    NbtTag::Float(64.0),
                    NbtTag::Float(48.0),
                ]),
            ),
        ]));
        let actor = ParsedEntity {
            identifier: Some("minecraft:pig".to_string()),
            definitions: Vec::new(),
            unique_id: Some(77),
            position: Some([32.0, 64.0, 48.0]),
            rotation: None,
            motion: None,
            items: Vec::new(),
            nbt: actor_nbt,
        };

        world.put_actor_blocking(pos, &actor).expect("put actor");
        let digest = storage
            .get(&ActorDigestKey::new(pos).storage_key())
            .expect("get digest")
            .expect("digest exists");
        assert_eq!(
            parse_actor_digest_ids(&digest).expect("parse digest"),
            vec![ActorUid(77)]
        );
        assert!(
            storage
                .get(&ActorUid(77).storage_key())
                .expect("get actor")
                .is_some()
        );

        world
            .delete_actor_blocking(pos, ActorUid(77))
            .expect("delete actor");
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .expect("get deleted digest")
                .is_none()
        );
        assert!(
            storage
                .get(&ActorUid(77).storage_key())
                .expect("get deleted actor")
                .is_none()
        );
    }

    #[test]
    fn render_chunk_priority_distance_orders_from_center() {
        let mut positions = vec![
            ChunkPos {
                x: 12,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 1,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: -3,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
        ];

        sort_render_chunk_positions(
            &mut positions,
            ChunkLoadPriority::DistanceFrom {
                chunk_x: 0,
                chunk_z: 0,
            },
        );

        let ordered = positions
            .iter()
            .map(|pos| (pos.x, pos.z))
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec![(0, 0), (1, 0), (-3, 0), (12, 0)]);
    }

    #[test]
    fn world_pipeline_options_resolve_automatic_bounds() {
        let options = WorldPipelineOptions::default();

        assert!(options.resolve_queue_depth(4, 64) >= 1);
        assert_eq!(options.resolve_progress_interval(), 256);

        let explicit = WorldPipelineOptions {
            queue_depth: 7,
            progress_interval: 9,
            ..WorldPipelineOptions::default()
        };
        assert_eq!(explicit.resolve_queue_depth(4, 64), 7);
        assert_eq!(explicit.resolve_progress_interval(), 9);
    }

    #[test]
    fn generic_memory_storage_matches_dynamic_storage_queries() {
        let storage = MemoryStorage::new();
        storage
            .put(b"~local_player", b"local")
            .expect("put local player");
        storage
            .put(b"player_remote", b"remote")
            .expect("put remote player");

        let generic_world =
            BedrockWorld::from_typed_storage("memory", storage.clone(), OpenOptions::default());
        let dynamic_world = BedrockWorld::from_storage(
            "memory",
            Arc::new(storage) as Arc<dyn WorldStorage>,
            OpenOptions::default(),
        );

        assert_eq!(
            generic_world.list_players_blocking().expect("generic"),
            dynamic_world.list_players_blocking().expect("dynamic")
        );
        assert_eq!(
            generic_world
                .classify_keys_blocking(WorldScanOptions::default())
                .expect("generic classify"),
            dynamic_world
                .classify_keys_blocking(WorldScanOptions::default())
                .expect("dynamic classify")
        );
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    #[test]
    fn generic_leveldb_storage_matches_dynamic_storage_queries() {
        let temp = temp_world_dir("generic-leveldb");
        std::fs::create_dir_all(&temp).expect("temp dir");
        let db_path = temp.join("db");
        let db = bedrock_leveldb::Db::open(&db_path, bedrock_leveldb::OpenOptions::default())
            .expect("initialize db");
        drop(db);
        let storage = BedrockLevelDbStorage::open(&db_path).expect("open storage");
        storage
            .put(b"~local_player", b"local")
            .expect("put local player");
        storage
            .put(b"player_remote", b"remote")
            .expect("put remote player");
        storage.flush().expect("flush");

        let generic_world =
            BedrockWorld::from_typed_storage(&temp, storage.clone(), OpenOptions::default());
        let dynamic_world = BedrockWorld::from_storage(
            &temp,
            Arc::new(storage) as Arc<dyn WorldStorage>,
            OpenOptions::default(),
        );

        assert_eq!(
            generic_world.list_players_blocking().expect("generic"),
            dynamic_world.list_players_blocking().expect("dynamic")
        );
        assert_eq!(
            generic_world
                .classify_keys_blocking(WorldScanOptions::default())
                .expect("generic classify"),
            dynamic_world
                .classify_keys_blocking(WorldScanOptions::default())
                .expect("dynamic classify")
        );
        std::fs::remove_dir_all(temp).expect("cleanup");
    }

    #[test]
    fn transaction_respects_read_only_option() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::new(pos, ChunkRecordTag::Version);
        let encoded = key.encode();
        let storage = Arc::new(MemoryStorage::new());
        let read_only_world =
            BedrockWorld::from_storage("memory", storage.clone(), OpenOptions::default());
        let mut transaction = read_only_world.transaction();
        transaction.put_raw_record(&key, Bytes::from_static(b"\x01"));

        let error = transaction.commit().expect_err("read-only commit");

        assert_eq!(error.kind(), crate::BedrockWorldErrorKind::ReadOnly);
        assert_eq!(storage.get(&encoded).expect("get"), None);

        let writable_world = BedrockWorld::from_storage(
            "memory",
            storage.clone(),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let mut transaction = writable_world.transaction();
        transaction.put_raw_record(&key, Bytes::from_static(b"\x02"));
        transaction.commit().expect("writable commit");

        assert_eq!(
            storage.get(&encoded).expect("get"),
            Some(Bytes::from_static(b"\x02"))
        );
    }

    #[test]
    fn transaction_replaces_chunk_records_and_typed_payloads_in_one_commit() {
        let pos = ChunkPos {
            x: 3,
            z: -2,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        let old_key = ChunkKey::new(pos, ChunkRecordTag::Version);
        storage
            .put(&old_key.encode(), b"\x01")
            .expect("put old chunk record");
        let world = BedrockWorld::from_storage(
            "memory",
            storage.clone(),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let block_entity = ParsedBlockEntity {
            id: Some("Chest".to_string()),
            position: Some([49, 64, -31]),
            is_movable: None,
            custom_name: None,
            items: Vec::new(),
            nbt: NbtTag::Compound(IndexMap::from([
                ("id".to_string(), NbtTag::String("Chest".to_string())),
                ("x".to_string(), NbtTag::Int(49)),
                ("y".to_string(), NbtTag::Int(64)),
                ("z".to_string(), NbtTag::Int(-31)),
            ])),
        };
        let area = ParsedHardcodedSpawnArea {
            kind: HardcodedSpawnAreaKind::NetherFortress,
            min: [48, 32, -32],
            max: [63, 80, -17],
        };
        let new_key = ChunkKey::new(pos, ChunkRecordTag::FinalizedState);

        let mut transaction = world.transaction();
        assert_eq!(transaction.delete_chunk(pos).expect("stage delete"), 1);
        transaction.put_raw_record(&new_key, Bytes::from_static(b"\x02\0\0\0"));
        transaction
            .put_block_entities(pos, std::slice::from_ref(&block_entity))
            .expect("stage block entities");
        transaction
            .put_hsa_for_chunk(pos, std::slice::from_ref(&area))
            .expect("stage hardcoded spawn area");
        transaction.commit().expect("commit replacement");

        assert_eq!(storage.get(&old_key.encode()).expect("get old"), None);
        assert_eq!(
            storage.get(&new_key.encode()).expect("get new"),
            Some(Bytes::from_static(b"\x02\0\0\0"))
        );
        assert_eq!(
            world
                .block_entities_in_chunk_blocking(pos)
                .expect("read block entities")[0]
                .entity
                .position,
            block_entity.position
        );
        assert_eq!(
            world
                .scan_hsa_records_blocking(WorldScanOptions::default())
                .expect("read hardcoded spawn areas")[0]
                .1,
            vec![area]
        );
    }

    #[test]
    fn biome_and_height_queries_read_legacy_data2d_in_zx_column_order() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_asymmetric_data2d_bytes(),
            )
            .expect("put Data2D");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        assert_eq!(
            world
                .get_biome_id_blocking(pos, 3, 2, 64)
                .expect("biome id"),
            Some(32)
        );
        assert_eq!(
            world
                .get_biome_id_blocking(pos, 2, 3, 64)
                .expect("biome id"),
            Some(23)
        );
        assert_eq!(
            world.get_height_at_blocking(pos, 3, 2).expect("height"),
            Some(132)
        );
        assert_eq!(
            world.get_height_at_blocking(pos, 2, 3).expect("height"),
            Some(123)
        );
    }

    #[test]
    fn data3d_height_map_is_normalized_to_dimension_min_y() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
                &test_data3d_height_bytes(130),
            )
            .expect("put Data3D");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        assert_eq!(
            world.get_height_at_blocking(pos, 4, 2).expect("height"),
            Some(66)
        );
        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: ChunkDataRequest::new().height_map(),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load render chunk");

        assert_eq!(
            chunk.height_map.expect("height map")[usize::from(2_u8)][usize::from(4_u8)],
            Some(66)
        );
        assert!(chunk.column_samples.is_none());
    }

    #[test]
    fn render_chunk_exact_load_preserves_data2d_xz_height_and_biome_coordinates() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_asymmetric_data2d_bytes(),
            )
            .expect("put Data2D");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load render chunk");
        let height_map = chunk.height_map.as_ref().expect("height map");
        let biome_storage = chunk
            .biome_data
            .values()
            .next()
            .expect("render biome storage");

        assert_eq!(height_map[3][1], Some(113));
        assert_eq!(height_map[1][3], Some(131));
        assert_eq!(biome_storage.biome_id_at(1, 0, 3), Some(13));
        assert_eq!(biome_storage.biome_id_at(3, 0, 1), Some(31));
    }

    #[test]
    fn subchunk_layer_query_uses_block_y() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(&ChunkKey::subchunk(pos, -1).encode(), &[8, 0])
            .expect("put subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let subchunk = world
            .get_subchunk_layer_blocking(pos, -1, SubChunkDecodeMode::CountsOnly)
            .expect("query")
            .expect("subchunk");
        assert_eq!(subchunk.y, -1);
    }

    #[test]
    fn render_chunk_needed_surface_subchunks_avoids_full_y_range() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 7),
            )
            .expect("put Data2D");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes(),
            )
            .expect("put subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let needed = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::HintThenVerify,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("needed render chunk");
        let full = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("full render chunk");
        assert!(needed.subchunks.contains_key(&4));
        assert_eq!(needed.subchunks.get(&4), full.subchunks.get(&4));
        assert!(needed.subchunks.len() <= full.subchunks.len());
    }

    #[test]
    fn render_chunk_needed_surface_subchunks_include_lookup_above_heightmap() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 7),
            )
            .expect("put Data2D");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:stone"),
            )
            .expect("put heightmap subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 5).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
            )
            .expect("put upper subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::HintThenVerify,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("needed render chunk");

        assert!(chunk.subchunks.contains_key(&4));
        assert!(chunk.subchunks.contains_key(&5));
        assert!(!chunk.subchunks.contains_key(&9));
        let sample = chunk
            .column_sample_at(0, 0)
            .expect("computed surface sample");
        assert_eq!(sample.surface_y, 95);
        assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
    }

    #[test]
    fn render_chunk_needed_exact_surface_reloads_full_when_window_top_is_touched() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 7),
            )
            .expect("put Data2D");
        storage
            .put(
                &ChunkKey::subchunk(pos, 8).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:stone"),
            )
            .expect("put window-top subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 9).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
            )
            .expect("put hidden upper subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::HintThenVerify,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("needed render chunk");

        assert!(chunk.subchunks.contains_key(&8));
        assert!(chunk.subchunks.contains_key(&9));
        let sample = chunk
            .column_sample_at(0, 0)
            .expect("computed surface sample");
        assert_eq!(sample.surface_y, 159);
        assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
    }

    #[test]
    fn render_chunk_needed_exact_surface_reloads_full_when_raw_height_is_stale() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(0, 7),
            )
            .expect("put stale Data2D");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:stone"),
            )
            .expect("put stale-height subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:air"),
            )
            .expect("put high empty hint-window subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 10).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
            )
            .expect("put true roof subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::HintThenVerify,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("needed render chunk");

        assert!(chunk.subchunks.contains_key(&10));
        let sample = chunk
            .column_sample_at(0, 0)
            .expect("computed surface sample");
        assert_eq!(sample.surface_y, 175);
        assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
    }

    #[test]
    fn render_chunk_raw_heightmap_request_does_not_build_surface_samples() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(0, 7),
            )
            .expect("put raw height");
        storage
            .put(
                &ChunkKey::subchunk(pos, 10).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
            )
            .expect("put high surface subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: ChunkDataRequest::new().height_map(),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load raw heightmap chunk");

        assert_eq!(chunk.height_map.as_ref().unwrap()[0][0], Some(0));
        assert!(chunk.column_samples.is_none());
        assert!(chunk.subchunks.is_empty());
    }

    #[test]
    fn render_chunk_needed_surface_subchunks_fall_back_to_full_without_heightmap() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::subchunk(pos, 5).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
            )
            .expect("put upper subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::HintThenVerify,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("needed render chunk");

        assert!(chunk.subchunks.contains_key(&5));
        let sample = chunk
            .column_sample_at(0, 0)
            .expect("computed surface sample");
        assert_eq!(sample.surface_y, 95);
        assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
    }

    #[test]
    fn render_chunk_loads_block_entities_when_requested() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        let block_entity = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("Banner".to_string())),
            ("x".to_string(), NbtTag::Int(3)),
            ("y".to_string(), NbtTag::Int(65)),
            ("z".to_string(), NbtTag::Int(4)),
        ]));
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
                &crate::nbt::serialize_root_nbt(&block_entity).expect("serialize block entity"),
            )
            .expect("put block entity");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let without_entities = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load render chunk without block entities");
        let with_entities = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::TopColumns,
                        true,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load render chunk with block entities");

        assert!(without_entities.block_entities.is_empty());
        assert_eq!(with_entities.block_entities.len(), 1);
        assert_eq!(
            with_entities.block_entities[0].id.as_deref(),
            Some("Banner")
        );
        assert_eq!(with_entities.block_entities[0].position, Some([3, 65, 4]));
    }

    #[test]
    fn surface_column_query_returns_top_block_and_water_context() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 7),
            )
            .expect("put Data2D");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes(),
            )
            .expect("put subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let column = world
            .get_surface_column_blocking(pos, 0, 0, SurfaceColumnOptions::default())
            .expect("surface query")
            .expect("surface column");

        assert_eq!(column.y, 65);
        assert_eq!(column.block_name, "minecraft:water");
        assert_eq!(column.biome_id, Some(7));
        assert_eq!(column.water_depth, 1);
        assert_eq!(
            column.under_water_block_name.as_deref(),
            Some("minecraft:sand")
        );
    }

    #[test]
    fn chunk_bounds_and_nearest_loaded_chunk_use_key_only_scan() {
        let storage = Arc::new(MemoryStorage::new());
        let positions = [
            ChunkPos {
                x: -4,
                z: 3,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 2,
                z: -1,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 9,
                z: 9,
                dimension: Dimension::Nether,
            },
        ];
        for pos in positions {
            storage
                .put(&ChunkKey::new(pos, ChunkRecordTag::Version).encode(), &[1])
                .expect("put chunk version");
        }
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let bounds = world
            .discover_chunk_bounds_blocking(Dimension::Overworld, WorldScanOptions::default())
            .expect("bounds")
            .expect("overworld bounds");
        assert_eq!(bounds.min_chunk_x, -4);
        assert_eq!(bounds.max_chunk_z, 3);
        assert_eq!(bounds.chunk_count, 2);

        let nearest = world
            .nearest_loaded_chunk_to_spawn_blocking(
                Dimension::Overworld,
                0,
                0,
                WorldScanOptions::default(),
            )
            .expect("nearest")
            .expect("nearest chunk");
        assert_eq!(nearest.x, 2);
        assert_eq!(nearest.z, -1);
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn render_region_index_uses_key_only_scan_and_parallel_load_keeps_order() {
        let storage = Arc::new(MemoryStorage::new());
        let render_positions = [
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
        for pos in render_positions {
            storage
                .put(
                    &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                    &test_data2d_bytes(64, 3),
                )
                .expect("put render chunk");
        }
        storage
            .put(
                &ChunkKey::new(
                    ChunkPos {
                        x: 2,
                        z: 0,
                        dimension: Dimension::Overworld,
                    },
                    ChunkRecordTag::Version,
                )
                .encode(),
                &[1],
            )
            .expect("put non-render chunk");
        storage
            .put(
                &ChunkKey::new(
                    ChunkPos {
                        x: 0,
                        z: 0,
                        dimension: Dimension::Nether,
                    },
                    ChunkRecordTag::Data2D,
                )
                .encode(),
                &test_data2d_bytes(64, 3),
            )
            .expect("put nether chunk");

        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());
        let visible = world
            .list_chunk_positions_in_region_blocking(
                WorldChunkQueryRegion {
                    dimension: Dimension::Overworld,
                    min_chunk_x: 0,
                    min_chunk_z: 0,
                    max_chunk_x: 2,
                    max_chunk_z: 0,
                },
                WorldScanOptions {
                    threading: WorldThreadingOptions::Fixed(2),
                    ..WorldScanOptions::default()
                },
            )
            .expect("render region index");

        assert_eq!(visible, render_positions.to_vec());

        let chunks = world
            .query_chunk_data_many_blocking(
                visible,
                ChunkLoadOptions {
                    threading: WorldThreadingOptions::Fixed(2),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("parallel render chunk load");
        assert_eq!(
            chunks.iter().map(|chunk| chunk.pos).collect::<Vec<_>>(),
            render_positions.to_vec()
        );
    }

    #[test]
    fn legacy_terrain_is_renderable_and_exact_batch_loaded() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_terrain_bytes(2, 65),
            )
            .expect("put legacy terrain");
        let world = BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            WorldFormat::LevelDbLegacyTerrain,
        );

        let positions = world
            .list_chunk_positions_in_region_blocking(
                WorldChunkQueryRegion {
                    dimension: Dimension::Overworld,
                    min_chunk_x: 0,
                    min_chunk_z: 0,
                    max_chunk_x: 0,
                    max_chunk_z: 0,
                },
                WorldScanOptions::default(),
            )
            .expect("legacy render index");
        assert_eq!(positions, vec![pos]);

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                positions,
                ChunkLoadOptions {
                    threading: WorldThreadingOptions::Single,
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("legacy exact render load");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_loaded);
        assert!(chunks[0].legacy_terrain.is_some());
        assert_eq!(chunks[0].height_map.as_ref().unwrap()[0][0], Some(65));
        assert!(chunks[0].legacy_biomes.is_some());
        assert!(chunks[0].legacy_biome_colors.is_some());
        assert_eq!(stats.prefix_scans, 0);
        assert_eq!(stats.legacy_terrain_records, 1);
        assert_eq!(stats.legacy_biome_samples, 1);
        assert_eq!(stats.legacy_biome_colors, 1);
        assert_eq!(stats.terrain_source_legacy, 1);
        assert_eq!(stats.detected_format, WorldFormat::LevelDbLegacyTerrain);
    }

    #[test]
    fn legacy_terrain_biome_rgb_takes_priority_over_data2d_biome_id() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut terrain = test_legacy_terrain_bytes(2, 65);
        write_legacy_biome_sample(&mut terrain, 0, 0, 12, 0x0034_a853);
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &terrain,
            )
            .expect("put legacy terrain");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(2, 24),
            )
            .expect("put conflicting old data2d");
        let world = BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            WorldFormat::LevelDbLegacyTerrain,
        );

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                [pos],
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::All,
                        false,
                    ),
                    threading: WorldThreadingOptions::Single,
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load conflicting legacy render chunk");

        let sample = chunks[0]
            .column_sample_at(0, 0)
            .expect("computed column sample");
        assert_eq!(
            sample.biome,
            Some(TerrainColumnBiome::Legacy(LegacyBiomeSample {
                biome_id: 12,
                red: 0x34,
                green: 0xa8,
                blue: 0x53,
            }))
        );
        assert_eq!(stats.legacy_biome_preferred_columns, 256);
        assert_eq!(stats.modern_biome_fallback_columns, 0);
    }

    #[test]
    fn modern_data2d_biome_remains_available_without_legacy_terrain() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(2, 24),
            )
            .expect("put modern data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
            )
            .expect("put surface subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                [pos],
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::All,
                        false,
                    ),
                    threading: WorldThreadingOptions::Single,
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load modern render chunk");

        let sample = chunks[0]
            .column_sample_at(0, 0)
            .expect("computed column sample");
        assert_eq!(sample.biome, Some(TerrainColumnBiome::Id(24)));
        assert_eq!(stats.legacy_biome_preferred_columns, 0);
        assert_eq!(stats.modern_biome_fallback_columns, 0);
    }

    #[test]
    fn legacy_terrain_exposes_biome_colors_without_transposing_columns() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut terrain = test_legacy_terrain_bytes(2, 65);
        write_legacy_biome_sample(&mut terrain, 0, 0, 1, 0x0011_2233);
        write_legacy_biome_sample(&mut terrain, 15, 0, 2, 0x0044_5566);
        write_legacy_biome_sample(&mut terrain, 0, 15, 3, 0x0077_8899);
        write_legacy_biome_sample(&mut terrain, 15, 15, 4, 0x00aa_bbcc);
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &terrain,
            )
            .expect("put legacy terrain");
        let world = BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            WorldFormat::LevelDbLegacyTerrain,
        );

        let chunk = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load legacy render chunk");
        let colors = chunk.legacy_biome_colors.expect("legacy biome colors");
        let samples = chunk.legacy_biomes.expect("legacy biome samples");
        assert_eq!(colors[0][0], Some(0x0011_2233));
        assert_eq!(colors[0][15], Some(0x0044_5566));
        assert_eq!(colors[15][0], Some(0x0077_8899));
        assert_eq!(colors[15][15], Some(0x00aa_bbcc));
        assert_eq!(samples[0][0].map(|sample| sample.biome_id), Some(1));
        assert_eq!(samples[0][15].map(|sample| sample.biome_id), Some(2));
        assert_eq!(samples[15][0].map(|sample| sample.biome_id), Some(3));
        assert_eq!(samples[15][15].map(|sample| sample.biome_id), Some(4));
        assert_eq!(
            world
                .get_legacy_biome_color_blocking(pos, 15, 0)
                .expect("legacy biome color"),
            Some(0x0044_5566)
        );
        assert_eq!(
            world
                .get_legacy_biome_sample_blocking(pos, 15, 0)
                .expect("legacy biome sample")
                .map(|sample| (sample.biome_id, sample.rgb_u32())),
            Some((2, 0x0044_5566))
        );
    }

    #[test]
    fn render_load_keeps_subchunks_when_legacy_terrain_is_also_present() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_terrain_bytes(1, 1),
            )
            .expect("put legacy terrain");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes(),
            )
            .expect("put subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                [pos],
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load mixed render chunk");

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].legacy_terrain.is_some());
        assert!(chunks[0].subchunks.contains_key(&0));
        assert_eq!(stats.legacy_terrain_records, 1);
        assert_eq!(stats.terrain_source_subchunk, 1);
        assert_eq!(stats.terrain_source_legacy, 0);
    }

    #[test]
    fn exact_surface_column_samples_use_top_block_not_raw_heightmap() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(1, 3),
            )
            .expect("put misleading raw height");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
            )
            .expect("put surface subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                [pos],
                ChunkLoadOptions {
                    data_request: exact_surface_request(
                        ExactSurfaceSubchunkPolicy::Full,
                        ExactSurfaceBiomeLoad::TopColumns,
                        false,
                    ),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load exact surface chunk");

        let sample = chunks[0]
            .column_sample_at(0, 0)
            .expect("computed column sample");
        assert_eq!(sample.surface_y, 15);
        assert_eq!(sample.surface_block_state.name, "minecraft:grass_block");
        assert_eq!(sample.source, TerrainSampleSource::Subchunk);
        assert_eq!(stats.computed_surface_columns, 256);
        assert_eq!(stats.raw_height_mismatch_columns, 256);
    }

    #[test]
    fn exact_surface_columns_match_full_indices() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(1, 3),
            )
            .expect("put height map");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
            )
            .expect("put surface subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());
        let surface_request = exact_surface_request(
            ExactSurfaceSubchunkPolicy::Full,
            ExactSurfaceBiomeLoad::TopColumns,
            false,
        );
        let full = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: surface_request.clone().full_3d_indices(),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load full indices");
        let surface = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: surface_request,
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load surface columns");

        assert_eq!(full.column_samples, surface.column_samples);
        let samples = world
            .load_surface_columns_blocking(
                pos,
                ChunkLoadOptions::exact_surface_columns(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
            )
            .expect("load surface samples")
            .expect("surface samples");
        assert_eq!(surface.column_samples.as_ref(), Some(&samples));
    }

    #[test]
    fn specialized_render_load_options_select_the_minimal_decode_contract() {
        let surface = ChunkLoadOptions::exact_surface_columns(
            ExactSurfaceSubchunkPolicy::HintThenVerify,
            ExactSurfaceBiomeLoad::TopColumns,
            false,
        );
        assert!(matches!(
            surface.data_request.subchunks.as_slice(),
            [SubchunkDataRequirement::SurfaceColumns(
                ExactSurfaceSubchunkPolicy::HintThenVerify
            )]
        ));
        assert_eq!(
            surface.data_request.preferred_decode_mode(),
            SubChunkDecodeMode::SurfaceColumns
        );

        let layer = ChunkLoadOptions::layer(64);
        assert!(matches!(
            layer.data_request.subchunks.as_slice(),
            [SubchunkDataRequirement::Layer(64)]
        ));
        assert_eq!(
            layer.data_request.preferred_decode_mode(),
            SubChunkDecodeMode::FullIndices
        );

        let height_map = ChunkLoadOptions::raw_height_map();
        assert!(height_map.data_request.height_map);
        assert_eq!(
            height_map.data_request.preferred_decode_mode(),
            SubChunkDecodeMode::CountsOnly
        );
    }

    #[test]
    fn composable_map_data_request_unions_subchunk_reads_and_decoder_needs() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let request = ChunkDataRequest::new().layer(0).cave_slice(31).height_map();
        let options = ChunkLoadOptions::for_data_request(request.clone());
        let planned = planned_render_subchunk_ys(pos, &options, None).expect("plan subchunks");
        assert_eq!(planned.into_iter().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(
            request.preferred_decode_mode(),
            SubChunkDecodeMode::FullIndices
        );

        let surface = ChunkDataRequest::new()
            .surface_columns(ExactSurfaceSubchunkPolicy::HintThenVerify)
            .biome(BiomeDataRequirement::SurfaceColumns);
        assert_eq!(
            surface.preferred_decode_mode(),
            SubChunkDecodeMode::SurfaceColumns
        );
        assert!(!surface.height_map);
    }

    #[test]
    fn exact_surface_samples_keep_visual_overlay_and_primary_thin_blocks() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_named_subchunk_bytes_with_values(
                    &[
                        "minecraft:air",
                        "minecraft:grass_block",
                        "minecraft:stone_button",
                        "minecraft:red_carpet",
                        "minecraft:snow_layer",
                        "minecraft:vine",
                    ],
                    |local_x, _, local_y| match (local_x, local_y) {
                        (_, 0) => 1,
                        (0, 1) => 2,
                        (1, 1) => 3,
                        (2, 1) => 4,
                        (3, 1) => 5,
                        _ => 0,
                    },
                ),
            )
            .expect("put overlay subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let full = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load exact surface chunk");
        let surface = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    subchunk_decode: SubChunkDecodeMode::SurfaceColumns,
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load surface-column chunk");
        assert_eq!(full.column_samples, surface.column_samples);

        let button = surface.column_sample_at(0, 0).expect("button column");
        assert_eq!(button.surface_y, 0);
        assert_eq!(button.surface_block_state.name, "minecraft:grass_block");
        assert_eq!(
            button
                .overlay
                .as_ref()
                .map(|overlay| overlay.block_state.name.as_str()),
            Some("minecraft:stone_button")
        );
        let carpet = surface.column_sample_at(1, 0).expect("carpet column");
        assert_eq!(carpet.surface_y, 1);
        assert_eq!(carpet.surface_block_state.name, "minecraft:red_carpet");
        assert!(carpet.overlay.is_none());
        let snow = surface.column_sample_at(2, 0).expect("snow column");
        assert_eq!(snow.surface_y, 1);
        assert_eq!(snow.surface_block_state.name, "minecraft:snow_layer");
        assert!(snow.overlay.is_none());
        let vine = surface.column_sample_at(3, 0).expect("vine column");
        assert_eq!(vine.surface_y, 0);
        assert_eq!(
            vine.overlay
                .as_ref()
                .map(|overlay| overlay.block_state.name.as_str()),
            Some("minecraft:vine")
        );
    }

    #[test]
    fn exact_surface_samples_high_roof_from_secondary_storage() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(&ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(), &{
                let mut bytes = Vec::with_capacity(768);
                for _ in 0..256 {
                    bytes.extend_from_slice(&0_i16.to_le_bytes());
                }
                bytes.extend(std::iter::repeat_n(1_u8, 256));
                bytes
            })
            .expect("put low raw height map");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_named_subchunk_bytes_with_values(
                    &["minecraft:air", "minecraft:stone"],
                    |_, _, local_y| u16::from(local_y == 0),
                ),
            )
            .expect("put low ground subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 10).encode(),
                &test_named_layered_subchunk_bytes(
                    &["minecraft:air"],
                    &["minecraft:air", "minecraft:copper_block"],
                    |_, _, _| 0,
                    |_, _, local_y| u16::from(local_y == 15),
                ),
            )
            .expect("put high secondary-storage roof");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load exact surface chunk");
        let sample = chunk.column_sample_at(0, 0).expect("roof column");

        assert_eq!(sample.surface_y, 175);
        assert_eq!(sample.surface_block_state.name, "minecraft:copper_block");
        assert_eq!(sample.source, TerrainSampleSource::Subchunk);
        assert_eq!(
            chunk.height_map.as_ref().expect("raw height map")[0][0],
            Some(0)
        );
    }

    #[test]
    fn exact_surface_samples_process_secondary_storage_water_and_overlay() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_named_layered_subchunk_bytes(
                    &["minecraft:air", "minecraft:sand", "minecraft:grass_block"],
                    &["minecraft:air", "minecraft:water", "minecraft:stone_button"],
                    |local_x, _, local_y| match (local_x, local_y) {
                        (0, 0) => 1,
                        (1, 1) => 2,
                        _ => 0,
                    },
                    |local_x, _, local_y| match (local_x, local_y) {
                        (0, 0) => 1,
                        (1, 1) => 2,
                        _ => 0,
                    },
                ),
            )
            .expect("put layered water and overlay");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load exact surface chunk");
        let water = chunk.column_sample_at(0, 0).expect("water column");
        assert_eq!(water.surface_y, 0);
        assert_eq!(water.surface_block_state.name, "minecraft:water");
        assert_eq!(water.relief_y, 0);
        assert_eq!(water.relief_block_state.name, "minecraft:sand");
        assert_eq!(
            water.water.as_ref().and_then(|water| water.underwater_y),
            Some(0)
        );
        let overlay = chunk.column_sample_at(1, 0).expect("overlay column");
        assert_eq!(overlay.surface_y, 1);
        assert_eq!(overlay.surface_block_state.name, "minecraft:grass_block");
        assert_eq!(
            overlay
                .overlay
                .as_ref()
                .map(|overlay| overlay.block_state.name.as_str()),
            Some("minecraft:stone_button")
        );
    }

    #[test]
    fn exact_surface_samples_keep_transparent_water_relief_context() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_named_subchunk_bytes_with_values(
                    &["minecraft:air", "minecraft:sand", "minecraft:water"],
                    |_, _, local_y| match local_y {
                        0 => 1,
                        1 | 2 => 2,
                        _ => 0,
                    },
                ),
            )
            .expect("put water subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
            .expect("load exact surface chunk");
        let sample = chunk.column_sample_at(0, 0).expect("water column");
        let water = sample.water.as_ref().expect("water context");
        assert_eq!(sample.surface_y, 2);
        assert_eq!(sample.surface_block_state.name, "minecraft:water");
        assert_eq!(sample.relief_y, 0);
        assert_eq!(sample.relief_block_state.name, "minecraft:sand");
        assert_eq!(water.depth, 2);
        assert_eq!(water.underwater_y, Some(0));
        assert_eq!(
            water
                .underwater_block_state
                .as_ref()
                .map(|state| state.name.as_str()),
            Some("minecraft:sand")
        );
    }

    #[test]
    fn render_chunk_exact_load_preserves_legacy_subchunk_xzy_coordinates() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_asymmetric_legacy_subchunk_bytes(),
            )
            .expect("put legacy subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let chunk = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: ChunkDataRequest::new().layer(10),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load legacy subchunk render chunk");
        let subchunk = chunk.subchunks.get(&0).expect("loaded legacy subchunk");

        assert_eq!(subchunk.legacy_block_id_at(0, 10, 0), Some(1));
        assert_eq!(subchunk.legacy_block_id_at(15, 10, 0), Some(12));
        assert_eq!(subchunk.legacy_block_id_at(0, 10, 15), Some(24));
        assert_eq!(subchunk.legacy_block_id_at(15, 10, 15), Some(45));
    }

    #[test]
    fn layer_query_does_not_read_surface_fallback_records() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_uniform_named_subchunk_bytes("minecraft:stone"),
            )
            .expect("put layer subchunk");
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                [pos],
                ChunkLoadOptions::for_data_request(ChunkDataRequest::new().layer(0)),
            )
            .expect("query fixed layer");

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].subchunks.contains_key(&0));
        assert_eq!(stats.keys_requested, 1);
        assert_eq!(stats.keys_found, 1);
        assert_eq!(stats.legacy_terrain_records, 0);
    }

    #[test]
    fn chunk_query_defaults_to_reusing_storage_blocks() {
        assert_eq!(
            ChunkLoadOptions::default().storage_cache_policy,
            StorageCachePolicy::Use
        );
    }

    #[test]
    fn decode_timing_preserves_sub_millisecond_samples() {
        let mut total = ChunkDecodeTiming::default();
        total.add(ChunkDecodeTiming {
            biome_parse_us: 125,
            subchunk_parse_us: 250,
            surface_scan_us: 375,
            block_entity_parse_us: 500,
        });
        total.add(ChunkDecodeTiming {
            biome_parse_us: 875,
            subchunk_parse_us: 750,
            surface_scan_us: 625,
            block_entity_parse_us: 500,
        });

        assert_eq!(total.biome_parse_us, 1_000);
        assert_eq!(total.subchunk_parse_us, 1_000);
        assert_eq!(total.surface_scan_us, 1_000);
        assert_eq!(total.block_entity_parse_us, 1_000);
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn render_chunk_exact_batch_keeps_shuffled_positions_bound_to_records() {
        let storage = Arc::new(MemoryStorage::new());
        let fixtures = [
            (
                ChunkPos {
                    x: -3,
                    z: 1,
                    dimension: Dimension::Overworld,
                },
                "minecraft:signature_a",
            ),
            (
                ChunkPos {
                    x: 2,
                    z: -4,
                    dimension: Dimension::Overworld,
                },
                "minecraft:signature_b",
            ),
            (
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                "minecraft:signature_c",
            ),
        ];
        for (pos, block_name) in fixtures.iter().copied() {
            storage
                .put(
                    &ChunkKey::subchunk(pos, 4).encode(),
                    &test_uniform_named_subchunk_bytes(block_name),
                )
                .expect("put named subchunk");
        }
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());

        let (chunks, stats) = world
            .query_chunk_data_with_stats_blocking(
                vec![fixtures[1].0, fixtures[0].0, fixtures[2].0, fixtures[1].0],
                ChunkLoadOptions {
                    data_request: ChunkDataRequest::new().layer(64),
                    threading: WorldThreadingOptions::Fixed(4),
                    priority: ChunkLoadPriority::DistanceFrom {
                        chunk_x: 0,
                        chunk_z: 0,
                    },
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load shuffled render chunks");

        assert_eq!(chunks.len(), 4);
        assert_eq!(stats.prefix_scans, 0);
        assert!(stats.exact_get_batches > 0);
        for chunk in chunks {
            let expected = fixtures
                .iter()
                .find_map(|(pos, block_name)| (*pos == chunk.pos).then_some(*block_name))
                .expect("known chunk position");
            let subchunk = chunk.subchunks.get(&4).expect("loaded subchunk");
            let state = subchunk
                .block_state_at(0, 0, 0)
                .expect("decoded signature block");
            assert_eq!(state.name, expected, "chunk {:?}", chunk.pos);
        }
    }

    fn test_surface_subchunk_bytes() -> Vec<u8> {
        let palette = ["minecraft:air", "minecraft:sand", "minecraft:water"];
        let mut bytes = vec![8, 1, 2 << 1];
        let values_per_word = 16_usize;
        let mut words = vec![0_u32; 256];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for (local_y, value) in [(0_u8, 1_u32), (1, 2)] {
                    let block_index = block_storage_index(local_x, local_y, local_z);
                    let word_index = block_index / values_per_word;
                    let bit_offset = (block_index % values_per_word) * 2;
                    words[word_index] |= value << bit_offset;
                }
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_uniform_named_subchunk_bytes(block_name: &str) -> Vec<u8> {
        let palette = ["minecraft:air", block_name];
        let mut bytes = vec![8, 1, 1 << 1];
        let mut words = vec![0_u32; 128];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..16_u8 {
                    let block_index = block_storage_index(local_x, local_y, local_z);
                    let word_index = block_index / 32;
                    let bit_offset = block_index % 32;
                    words[word_index] |= 1_u32 << bit_offset;
                }
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_named_subchunk_bytes_with_values(
        palette: &[&str],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) -> Vec<u8> {
        let bits_per_value = match palette.len() {
            0..=2 => 1_u8,
            3..=4 => 2_u8,
            5..=16 => 4_u8,
            _ => 8_u8,
        };
        let values_per_word = usize::from(32 / bits_per_value);
        let word_count = 4096_usize.div_ceil(values_per_word);
        let mut bytes = vec![8, 1, bits_per_value << 1];
        let mut words = vec![0_u32; word_count];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..16_u8 {
                    let value = value_at(local_x, local_z, local_y);
                    if value == 0 {
                        continue;
                    }
                    let block_index = block_storage_index(local_x, local_y, local_z);
                    let word_index = block_index / values_per_word;
                    let bit_offset = (block_index % values_per_word) * usize::from(bits_per_value);
                    words[word_index] |= u32::from(value) << bit_offset;
                }
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String((*name).to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_named_layered_subchunk_bytes(
        lower_palette: &[&str],
        upper_palette: &[&str],
        lower_value_at: impl Fn(u8, u8, u8) -> u16,
        upper_value_at: impl Fn(u8, u8, u8) -> u16,
    ) -> Vec<u8> {
        let mut bytes = vec![8, 2];
        append_named_palette_storage(&mut bytes, lower_palette, lower_value_at);
        append_named_palette_storage(&mut bytes, upper_palette, upper_value_at);
        bytes
    }

    fn append_named_palette_storage(
        bytes: &mut Vec<u8>,
        palette: &[&str],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) {
        let bits_per_value = match palette.len() {
            0..=2 => 1_u8,
            3..=4 => 2_u8,
            5..=16 => 4_u8,
            _ => 8_u8,
        };
        let values_per_word = usize::from(32 / bits_per_value);
        let word_count = 4096_usize.div_ceil(values_per_word);
        let mut words = vec![0_u32; word_count];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..16_u8 {
                    let value = value_at(local_x, local_z, local_y);
                    if value == 0 {
                        continue;
                    }
                    let block_index = block_storage_index(local_x, local_y, local_z);
                    let word_index = block_index / values_per_word;
                    let bit_offset = (block_index % values_per_word) * usize::from(bits_per_value);
                    words[word_index] |= u32::from(value) << bit_offset;
                }
            }
        }
        bytes.push(bits_per_value << 1);
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String((*name).to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
    }

    fn test_asymmetric_legacy_subchunk_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; crate::LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
        bytes[0] = 2;
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let block_id = match (local_x >= 8, local_z >= 8) {
                    (false, false) => 1,
                    (true, false) => 12,
                    (false, true) => 24,
                    (true, true) => 45,
                };
                let index = crate::LegacySubChunk::block_index(local_x, 10, local_z)
                    .expect("legacy subchunk index");
                bytes[1 + index] = block_id;
            }
        }
        bytes
    }

    fn test_data2d_bytes(height: i16, biome: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(768);
        for _ in 0..256 {
            bytes.extend_from_slice(&height.to_le_bytes());
        }
        bytes.extend(std::iter::repeat_n(biome, 256));
        bytes
    }

    fn test_data3d_height_bytes(height: i16) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(512);
        for _ in 0..256 {
            bytes.extend_from_slice(&height.to_le_bytes());
        }
        bytes
    }

    fn test_asymmetric_data2d_bytes() -> Vec<u8> {
        let mut bytes = Vec::with_capacity(768);
        for local_z in 0..16_i16 {
            for local_x in 0..16_i16 {
                let height = 100 + local_x * 10 + local_z;
                bytes.extend_from_slice(&height.to_le_bytes());
            }
        }
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                bytes.push(local_x * 10 + local_z);
            }
        }
        bytes
    }

    fn test_legacy_terrain_bytes(block_id: u8, height: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; crate::LEGACY_TERRAIN_VALUE_LEN];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..=height.min(127) {
                    let index = crate::LegacyTerrain::block_index(local_x, local_y, local_z)
                        .expect("legacy block index");
                    bytes[index] = block_id;
                }
                bytes[crate::LEGACY_TERRAIN_BLOCK_COUNT
                    + crate::LEGACY_TERRAIN_BLOCK_COUNT / 2 * 3
                    + raw_2d_column_index(local_x, local_z)] = height;
            }
        }
        bytes
    }

    fn write_legacy_biome_sample(
        bytes: &mut [u8],
        local_x: u8,
        local_z: u8,
        biome_id: u8,
        color: u32,
    ) {
        let offset = crate::LEGACY_TERRAIN_BLOCK_COUNT
            + crate::LEGACY_TERRAIN_BLOCK_COUNT / 2 * 3
            + 16 * 16
            + raw_2d_column_index(local_x, local_z) * 4;
        bytes[offset] = biome_id;
        bytes[offset + 1] = ((color >> 16) & 0xff) as u8;
        bytes[offset + 2] = ((color >> 8) & 0xff) as u8;
        bytes[offset + 3] = (color & 0xff) as u8;
    }

    fn raw_2d_column_index(local_x: u8, local_z: u8) -> usize {
        usize::from(local_z) * 16 + usize::from(local_x)
    }
}

fn validate_local_column(local_x: u8, local_z: u8) -> Result<()> {
    if local_x >= 16 || local_z >= 16 {
        return Err(BedrockWorldError::Validation(format!(
            "local biome coordinates must be 0..15, got x={local_x}, z={local_z}"
        )));
    }
    Ok(())
}

fn insert_needed_surface_subchunks(
    subchunk_ys: &mut BTreeSet<i8>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    min_subchunk_y: i8,
    max_subchunk_y: i8,
) {
    const SURFACE_LOOKDOWN_SUBCHUNKS: i8 = 6;
    const SURFACE_LOOKUP_SUBCHUNKS: i8 = 4;
    let Some(height_map) = height_map else {
        return;
    };
    for row in height_map {
        for height in row.iter().flatten() {
            if let Ok(surface_y) = block_y_to_subchunk_y(i32::from(*height)) {
                let lower_y = surface_y
                    .saturating_sub(SURFACE_LOOKDOWN_SUBCHUNKS)
                    .max(min_subchunk_y);
                let upper_y = surface_y
                    .saturating_add(SURFACE_LOOKUP_SUBCHUNKS)
                    .clamp(min_subchunk_y, max_subchunk_y);
                for subchunk_y in lower_y..=upper_y {
                    subchunk_ys.insert(subchunk_y);
                }
            }
        }
    }
}

fn block_y_to_subchunk_y(y: i32) -> Result<i8> {
    let subchunk_y = y.div_euclid(16);
    i8::try_from(subchunk_y).map_err(|_| {
        BedrockWorldError::Validation(format!(
            "block y={y} cannot be represented as a Bedrock subchunk index"
        ))
    })
}

fn biome_storage_contains_y(storage: &ParsedBiomeStorage, y: i32) -> bool {
    storage
        .y
        .is_none_or(|start_y| (start_y..start_y + 16).contains(&y))
}

fn biome_storage_bucket_y(y: i32) -> i32 {
    y.div_euclid(16) * 16
}

fn biome_id_from_storage(
    storage: &ParsedBiomeStorage,
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

fn height_map_index(local_x: u8, local_z: u8) -> usize {
    usize::from(local_z) * 16 + usize::from(local_x)
}

fn column_index(local_x: u8, local_z: u8) -> Option<usize> {
    (local_x < 16 && local_z < 16).then_some(height_map_index(local_x, local_z))
}

fn raw_height_at(
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    local_x: u8,
    local_z: u8,
) -> Option<i16> {
    height_map?[usize::from(local_z)][usize::from(local_x)]
}

fn raw_height_mismatch_columns(chunk: &ChunkData) -> usize {
    let Some(samples) = chunk.column_samples.as_ref() else {
        return 0;
    };
    let Some(height_map) = chunk.height_map.as_ref() else {
        return 0;
    };
    let mut mismatches = 0usize;
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            if let Some(sample) = samples.get(local_x, local_z) {
                if height_map[usize::from(local_z)][usize::from(local_x)]
                    .is_some_and(|raw_height| raw_height != sample.surface_y)
                {
                    mismatches = mismatches.saturating_add(1);
                }
            }
        }
    }
    mismatches
}

fn missing_surface_columns(chunk: &ChunkData) -> usize {
    chunk.column_samples.as_ref().map_or(0, |samples| {
        256usize.saturating_sub(samples.sampled_columns())
    })
}

fn needed_exact_surface_chunk_requires_full_reload(chunk: &ChunkData) -> Result<bool> {
    let Some(samples) = chunk.column_samples.as_ref() else {
        return Ok(false);
    };
    if samples.sampled_columns() < 16 * 16 {
        return Ok(true);
    }
    if raw_height_mismatch_columns(chunk) > 0 {
        return Ok(true);
    }
    let Some(loaded_max_subchunk_y) = chunk.subchunks.keys().next_back().copied() else {
        return Ok(true);
    };
    let (_, world_max_subchunk_y) = chunk.pos.subchunk_index_range(chunk.version);
    if loaded_max_subchunk_y >= world_max_subchunk_y {
        return Ok(false);
    }
    for sample in samples.iter() {
        if block_y_to_subchunk_y(i32::from(sample.surface_y))? == loaded_max_subchunk_y {
            return Ok(true);
        }
        if let Some(overlay) = sample.overlay.as_ref() {
            if block_y_to_subchunk_y(i32::from(overlay.y))? == loaded_max_subchunk_y {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn legacy_world_block_state(id: u8, data: u8) -> BlockState {
    let mut states = BTreeMap::new();
    states.insert("data".to_string(), NbtTag::Byte(data as i8));
    BlockState {
        name: legacy_world_block_name(id, data),
        states,
        version: None,
    }
}

#[allow(clippy::too_many_lines)]
fn legacy_world_block_name(id: u8, data: u8) -> String {
    let name = match id {
        0 => "minecraft:air",
        1 => match data & 0x7 {
            1 => "minecraft:granite",
            2 => "minecraft:polished_granite",
            3 => "minecraft:diorite",
            4 => "minecraft:polished_diorite",
            5 => "minecraft:andesite",
            6 => "minecraft:polished_andesite",
            _ => "minecraft:stone",
        },
        2 => "minecraft:grass_block",
        3 => match data & 0x3 {
            1 => "minecraft:coarse_dirt",
            2 => "minecraft:podzol",
            _ => "minecraft:dirt",
        },
        4 => "minecraft:cobblestone",
        5 => legacy_world_wood_name(data, "planks"),
        6 => "minecraft:oak_sapling",
        7 => "minecraft:bedrock",
        8 | 9 => "minecraft:water",
        10 | 11 => "minecraft:lava",
        12 => match data & 0x1 {
            1 => "minecraft:red_sand",
            _ => "minecraft:sand",
        },
        13 => "minecraft:gravel",
        14 => "minecraft:gold_ore",
        15 => "minecraft:iron_ore",
        16 => "minecraft:coal_ore",
        17 => legacy_world_wood_name(data, "log"),
        18 => legacy_world_wood_name(data, "leaves"),
        19 => "minecraft:sponge",
        20 => "minecraft:glass",
        21 => "minecraft:lapis_ore",
        22 => "minecraft:lapis_block",
        24 => "minecraft:sandstone",
        26 => "minecraft:bed",
        30 => "minecraft:cobweb",
        31 => match data {
            1 => "minecraft:short_grass",
            2 => "minecraft:fern",
            _ => "minecraft:dead_bush",
        },
        32 => "minecraft:dead_bush",
        35 => legacy_world_wool_name(data),
        37 => "minecraft:dandelion",
        38 => "minecraft:poppy",
        39 => "minecraft:brown_mushroom",
        40 => "minecraft:red_mushroom",
        41 => "minecraft:gold_block",
        42 => "minecraft:iron_block",
        43 | 44 => "minecraft:stone_slab",
        45 => "minecraft:bricks",
        46 => "minecraft:tnt",
        47 => "minecraft:bookshelf",
        48 => "minecraft:mossy_cobblestone",
        49 => "minecraft:obsidian",
        50 => "minecraft:torch",
        51 => "minecraft:fire",
        52 => "minecraft:spawner",
        53 => "minecraft:oak_stairs",
        54 => "minecraft:chest",
        56 => "minecraft:diamond_ore",
        57 => "minecraft:diamond_block",
        58 => "minecraft:crafting_table",
        59 => "minecraft:wheat",
        60 => "minecraft:farmland",
        61 | 62 => "minecraft:furnace",
        63 | 68 => "minecraft:oak_sign",
        64 => "minecraft:oak_door",
        65 => "minecraft:ladder",
        66 => "minecraft:rail",
        67 => "minecraft:cobblestone_stairs",
        71 => "minecraft:iron_door",
        73 | 74 => "minecraft:redstone_ore",
        78 => "minecraft:snow",
        79 => "minecraft:ice",
        80 => "minecraft:snow_block",
        81 => "minecraft:cactus",
        82 => "minecraft:clay",
        83 => "minecraft:sugar_cane",
        85 => "minecraft:oak_fence",
        86 => "minecraft:pumpkin",
        87 => "minecraft:netherrack",
        88 => "minecraft:soul_sand",
        89 => "minecraft:glowstone",
        91 => "minecraft:jack_o_lantern",
        95 => "minecraft:invisible_bedrock",
        98 => "minecraft:stone_bricks",
        99 | 100 => "minecraft:mushroom_stem",
        103 => "minecraft:melon",
        106 => "minecraft:vine",
        107 => "minecraft:oak_fence_gate",
        108 => "minecraft:brick_stairs",
        109 => "minecraft:stone_brick_stairs",
        110 => "minecraft:mycelium",
        111 => "minecraft:lily_pad",
        112 => "minecraft:nether_bricks",
        121 => "minecraft:end_stone",
        129 => "minecraft:emerald_ore",
        133 => "minecraft:emerald_block",
        155 => "minecraft:quartz_block",
        159 | 172 => "minecraft:terracotta",
        161 => legacy_world_wood_name(data.saturating_add(4), "leaves"),
        162 => legacy_world_wood_name(data.saturating_add(4), "log"),
        169 => "minecraft:sea_lantern",
        170 => "minecraft:hay_block",
        171 => "minecraft:white_carpet",
        173 => "minecraft:coal_block",
        174 => "minecraft:packed_ice",
        175 => "minecraft:sunflower",
        _ => return format!("legacy:{id}"),
    };
    name.to_string()
}

fn legacy_world_wood_name(data: u8, suffix: &'static str) -> &'static str {
    match (data & 0x7, suffix) {
        (1, "planks") => "minecraft:spruce_planks",
        (2, "planks") => "minecraft:birch_planks",
        (3, "planks") => "minecraft:jungle_planks",
        (4, "planks") => "minecraft:acacia_planks",
        (5, "planks") => "minecraft:dark_oak_planks",
        (_, "planks") => "minecraft:oak_planks",
        (1, "log") => "minecraft:spruce_log",
        (2, "log") => "minecraft:birch_log",
        (3, "log") => "minecraft:jungle_log",
        (4, "log") => "minecraft:acacia_log",
        (5, "log") => "minecraft:dark_oak_log",
        (_, "log") => "minecraft:oak_log",
        (1, "leaves") => "minecraft:spruce_leaves",
        (2, "leaves") => "minecraft:birch_leaves",
        (3, "leaves") => "minecraft:jungle_leaves",
        (4, "leaves") => "minecraft:acacia_leaves",
        (5, "leaves") => "minecraft:dark_oak_leaves",
        _ => "minecraft:oak_leaves",
    }
}

fn legacy_world_wool_name(data: u8) -> &'static str {
    match data & 0x0f {
        1 => "minecraft:orange_wool",
        2 => "minecraft:magenta_wool",
        3 => "minecraft:light_blue_wool",
        4 => "minecraft:yellow_wool",
        5 => "minecraft:lime_wool",
        6 => "minecraft:pink_wool",
        7 => "minecraft:gray_wool",
        8 => "minecraft:light_gray_wool",
        9 => "minecraft:cyan_wool",
        10 => "minecraft:purple_wool",
        11 => "minecraft:blue_wool",
        12 => "minecraft:brown_wool",
        13 => "minecraft:green_wool",
        14 => "minecraft:red_wool",
        15 => "minecraft:black_wool",
        _ => "minecraft:white_wool",
    }
}
