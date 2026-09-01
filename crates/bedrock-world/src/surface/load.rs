//! Chunk and surface loading requests, results, progress, and execution limits.

use super::*;
use crate::chunk::LegacyTerrain;

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
    pub biome_data: BTreeMap<i32, BiomeStorage>,
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
    /// Returns whether this request produced all data required by its [`ChunkDataRequest`].
    ///
    /// This does not indicate whether [`crate::world::World::is_chunk_saved`] is true; an exact
    /// request for an all-air SubChunk can be unsatisfied while the chunk is persisted.
    #[must_use]
    pub const fn request_satisfied(&self) -> bool {
        self.is_loaded
    }
}

impl ChunkData {
    #[must_use]
    /// Returns the sampled terrain column at local chunk coordinates.
    pub fn column_sample_at(&self, local_x: u8, local_z: u8) -> Option<&TerrainColumnSample> {
        self.column_samples.as_ref()?.get(local_x, local_z)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RawChunkData {
    pub(crate) pos: ChunkPos,
    pub(crate) biome_record: Option<(ChunkRecordTag, Bytes)>,
    pub(crate) subchunks: BTreeMap<i8, Bytes>,
    pub(crate) block_entities: Option<Bytes>,
    pub(crate) legacy_terrain: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_field_names)]
pub(crate) struct ChunkDecodeTiming {
    pub(crate) biome_parse_us: u128,
    pub(crate) subchunk_parse_us: u128,
    pub(crate) surface_scan_us: u128,
    pub(crate) block_entity_parse_us: u128,
}

impl ChunkDecodeTiming {
    pub(crate) fn add(&mut self, other: Self) {
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
pub(crate) enum RenderRecordKind {
    LegacyTerrain,
    Data3D,
    Data2D,
    Data2DLegacy,
    Subchunk(i8),
    BlockEntity,
}

impl RenderRecordKind {
    pub(crate) fn biome_tag(self) -> ChunkRecordTag {
        match self {
            Self::Data3D => ChunkRecordTag::Data3D,
            Self::Data2D => ChunkRecordTag::Data2D,
            Self::Data2DLegacy => ChunkRecordTag::Data2DLegacy,
            Self::LegacyTerrain | Self::Subchunk(_) | Self::BlockEntity => {
                unreachable!("non-biome render record has no biome tag")
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderRecordRequest {
    pub(crate) chunk_index: usize,
    pub(crate) kind: RenderRecordKind,
}

#[derive(Debug, Clone)]
/// Options controlling render region loading.
pub struct RegionLoadOptions {
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

impl Default for RegionLoadOptions {
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

impl From<RegionLoadOptions> for ChunkLoadOptions {
    fn from(options: RegionLoadOptions) -> Self {
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
    pub(crate) fn block_entities_if(mut self, enabled: bool) -> Self {
        self.block_entities = enabled;
        self
    }

    pub(crate) fn preferred_decode_mode(&self) -> SubChunkDecodeMode {
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
pub struct Region {
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
pub struct RegionLoad {
    /// Inclusive chunk region requested by the load.
    pub region: Region,
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
    pub(crate) fn from_first(pos: ChunkPos) -> Self {
        Self {
            dimension: pos.dimension,
            min_chunk_x: pos.x,
            min_chunk_z: pos.z,
            max_chunk_x: pos.x,
            max_chunk_z: pos.z,
            chunk_count: 1,
        }
    }

    pub(crate) fn include(&mut self, pos: ChunkPos) {
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
    pub(crate) pool: rayon::ThreadPool,
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

pub(crate) fn default_world_worker_budget() -> usize {
    let logical = std::thread::available_parallelism().map_or(1, usize::from);
    logical.div_ceil(2).clamp(2, 6)
}

pub(crate) fn world_executor(worker_count: usize) -> Result<Arc<WorldExecutor>> {
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
pub struct CancelFlag(pub(crate) Arc<AtomicBool>);

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

    pub(crate) fn emit(&self, progress: WorldScanProgress) {
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
