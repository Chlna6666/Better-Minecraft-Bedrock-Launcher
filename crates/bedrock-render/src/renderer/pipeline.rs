#![allow(
    clippy::bool_to_int_with_if,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::elidable_lifetime_names,
    clippy::field_reassign_with_default,
    clippy::large_types_passed_by_value,
    clippy::manual_clamp,
    clippy::manual_contains,
    clippy::manual_is_multiple_of,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::match_wildcard_for_single_variants,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::obfuscated_if_else,
    clippy::redundant_closure,
    clippy::ref_option,
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_wraps,
    clippy::unused_async,
    clippy::useless_conversion,
    clippy::used_underscore_binding
)]

use super::cache::{
    TILE_AUTHORITY_FLAG_EMPTY, TILE_AUTHORITY_FLAG_NON_EMPTY, TileAuthorityBlobReader,
    TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityChunkState, TileAuthorityChunkTileRef,
    TileAuthorityCommit, TileAuthorityDependency, TileAuthorityEntry, TileAuthorityIndexSnapshot,
};
use super::gpu::{GpuProcessResult, GpuRenderContext};
use crate::error::{BedrockRenderError, Result};
use crate::palette::{RenderPalette, RgbaColor};
use bedrock_world::{
    BedrockLevelDbStorage, BedrockWorld, BiomeDataRequirement, BlockPos, BlockState,
    CancelFlag as WorldCancelFlag, ChunkBlockEntity, ChunkData, ChunkDataRequest, ChunkLoadOptions,
    ChunkLoadPriority, ChunkLoadStats, ChunkPos, Dimension, ExactSurfaceSubchunkPolicy,
    LegacyBiomeSample, NbtTag, OpenOptions as WorldOpenOptions, PartitionedWorldStorage,
    StorageCachePolicy, StoragePipelineOptions, StorageReadOptions, StorageScanMode,
    StorageThreadingOptions, StorageVisitorControl, SubChunk, SubChunkDecodeMode,
    TerrainColumnBiome, TerrainColumnOverlay, TerrainColumnSample, TerrainColumnSamples,
    WorldChunkQueryRegion, WorldChunkQueryRegionData, WorldChunkQueryRegionLoadOptions,
    WorldPipelineOptions, WorldScanOptions, WorldStorage, WorldStorageHandle,
    WorldThreadingOptions, terrain_surface_overlay_alpha,
};
#[cfg(feature = "png")]
use image::codecs::png::PngEncoder;
#[cfg(feature = "webp")]
use image::codecs::webp::WebPEncoder;
#[cfg(any(feature = "png", feature = "webp"))]
use image::{ExtendedColorType, ImageEncoder};
use rayon::{ThreadPoolBuilder, slice::ParallelSliceMut};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};
use xxhash_rust::xxh3::xxh3_128;

/// Renderer cache schema version used in tile cache keys.
pub const RENDERER_CACHE_VERSION: u32 = 51;
/// Default embedded palette version used in tile cache keys.
pub const DEFAULT_PALETTE_VERSION: u32 = 16;
/// Maximum fixed worker thread count accepted by render options.
pub const MAX_RENDER_THREADS: usize = 512;
/// Maximum width or height of a rendered tile in pixels.
pub const MAX_TILE_SIZE_PIXELS: u32 = 4096;
const REGION_BAKE_ESTIMATED_BYTES_PER_CHUNK: usize = 4096;
const DEFAULT_EXPORT_MEMORY_BUDGET_BYTES: usize = 1024 * 1024 * 1024;
const DEFAULT_INTERACTIVE_MEMORY_BUDGET_BYTES: usize = 512 * 1024 * 1024;
const MIN_AUTO_MEMORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const MAX_AUTO_MEMORY_BUDGET_BYTES: usize = 4 * 1024 * 1024 * 1024;
const PARALLEL_TILE_PRIORITY_SORT_THRESHOLD: usize = 128;
const SESSION_BATCH_CULL_FULL_INDEX_THRESHOLD_CHUNKS: usize = 128;
const MISSING_HEIGHT: i16 = i16::MIN;
const FAST_RGBA_ZSTD_MAGIC: &[u8; 4] = b"BRT2";
const FAST_RGBA_ZSTD_V1_VERSION: u32 = 1;
const FAST_RGBA_ZSTD_VERSION: u32 = 2;
const FAST_RGBA_ZSTD_V1_HEADER_LEN: usize = 24;
const FAST_RGBA_ZSTD_HEADER_LEN: usize = 40;
const FAST_RGBA_ZSTD_VALIDATION_KIND_NONE: u32 = 0;
const FAST_RGBA_ZSTD_VALIDATION_KIND_SIMPLE_TILE: u32 = 1;
const FAST_RGBA_ZSTD_FLAG_NON_EMPTY: u32 = 1;
const FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE: u32 = 1 << 1;
const FAST_RGBA_ZSTD_KNOWN_FLAGS: u32 =
    FAST_RGBA_ZSTD_FLAG_NON_EMPTY | FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE;
const FAST_RGBA_ZSTD_LEVEL: i32 = 1;
const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;
#[cfg(test)]
static TILE_CACHE_WRITE_ID: AtomicUsize = AtomicUsize::new(0);

/// Source of render-ready chunk data used by [`MapRenderer`].
pub trait RenderChunkSource: Send + Sync {
    /// Lists all chunks with records relevant to map rendering.
    fn list_render_chunk_positions_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>>;

    /// Lists renderable chunks inside an inclusive chunk region.
    fn list_chunk_positions_in_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>>;

    /// Loads render data for a region.
    fn query_chunk_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData>;

    /// Loads render data for explicit chunks with stats.
    fn query_chunk_data_with_stats_blocking(
        &self,
        positions: &[ChunkPos],
        options: ChunkLoadOptions,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)>;

    /// Loads render data for one chunk.
    fn query_chunk_data_blocking(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData>;
}

impl<S> RenderChunkSource for BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    fn list_render_chunk_positions_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        Ok(BedrockWorld::list_render_chunk_positions_blocking(
            self, options,
        )?)
    }

    fn list_chunk_positions_in_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        Ok(BedrockWorld::list_chunk_positions_in_region_blocking(
            self, region, options,
        )?)
    }

    fn query_chunk_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData> {
        Ok(BedrockWorld::query_chunk_region_blocking(
            self, region, options,
        )?)
    }

    fn query_chunk_data_with_stats_blocking(
        &self,
        positions: &[ChunkPos],
        options: ChunkLoadOptions,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        Ok(BedrockWorld::query_chunk_data_with_stats_blocking(
            self,
            positions.iter().copied(),
            options,
        )?)
    }

    fn query_chunk_data_blocking(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        Ok(BedrockWorld::query_chunk_data_blocking(self, pos, options)?)
    }
}

/// Tile coordinate in chunk-tile space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileCoord {
    /// Tile X coordinate.
    pub x: i32,
    /// Tile Z coordinate.
    pub z: i32,
    /// Bedrock dimension rendered by this tile.
    pub dimension: Dimension,
}

/// Render mode used to sample and color world data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RenderMode {
    /// Resolved biome color layer sampled at world Y.
    Biome {
        /// World Y coordinate to sample.
        y: i32,
    },
    /// Deterministic diagnostic biome-id layer sampled at world Y.
    RawBiomeLayer {
        /// World Y coordinate to sample.
        y: i32,
    },
    /// Top-down terrain surface render using height maps and subchunks.
    SurfaceBlocks,
    /// Fixed block layer sampled at world Y.
    LayerBlocks {
        /// World Y coordinate to sample.
        y: i32,
    },
    /// Height gradient render from the computed top block surface.
    HeightMap,
    /// Diagnostic height gradient render from raw Bedrock height-map data.
    RawHeightMap,
    /// Cave diagnostic slice sampled at world Y.
    CaveSlice {
        /// World Y coordinate to sample.
        y: i32,
    },
}

/// Requested tile output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Encode `TileImage::encoded` as WebP.
    WebP,
    /// Encode `TileImage::encoded` as PNG.
    Png,
    /// Encode `TileImage::encoded` as zstd-compressed raw RGBA tile bytes.
    FastRgbaZstd,
    /// Return raw RGBA pixels without encoded bytes.
    Rgba,
}

/// Decoded pixel layout used by interactive tile streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TilePixelFormat {
    /// Red, green, blue, alpha byte order.
    Rgba8,
    /// Blue, green, red, alpha byte order.
    ///
    /// This output requires a per-tile RGBA-to-BGRA conversion and allocation.
    /// Prefer [`TilePixelFormat::Rgba8`] for latency-sensitive interactive streams.
    Bgra8,
}

/// Output contract for the v2 decoded tile stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTileOutputOptions {
    /// Pixel order emitted in [`DecodedTileImage::pixels`].
    pub pixel_format: TilePixelFormat,
}

impl Default for RenderTileOutputOptions {
    fn default() -> Self {
        Self {
            pixel_format: TilePixelFormat::Rgba8,
        }
    }
}

/// A single tile render request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderJob {
    /// Tile coordinate to render.
    pub coord: TileCoord,
    /// Render mode to use.
    pub mode: RenderMode,
    /// Output tile width and height in pixels.
    pub tile_size: u32,
    /// Blocks represented by each output pixel.
    pub scale: u32,
    /// Number of output pixels generated per source block.
    pub pixels_per_block: u32,
}

impl RenderJob {
    /// Creates a default `256x256`, one-block-per-pixel render job.
    #[must_use]
    pub const fn new(coord: TileCoord, mode: RenderMode) -> Self {
        Self {
            coord,
            mode,
            tile_size: 256,
            scale: 1,
            pixels_per_block: 1,
        }
    }

    /// Creates a render job from a chunk-tile layout.
    ///
    /// # Errors
    ///
    /// Returns an error if the layout is invalid or cannot produce an integer tile size.
    pub fn chunk_tile(coord: TileCoord, mode: RenderMode, layout: ChunkTileLayout) -> Result<Self> {
        validate_layout(layout)?;
        let tile_size = layout.tile_size().ok_or_else(|| {
            BedrockRenderError::Validation(
                "chunks_per_tile * 16 * pixels_per_block must be divisible by blocks_per_pixel"
                    .to_string(),
            )
        })?;
        Ok(Self {
            coord,
            mode,
            tile_size,
            scale: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
        })
    }
}

/// Tile layout in chunks and pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderLayout {
    /// Number of chunks covered by one tile edge.
    pub chunks_per_tile: u32,
    /// Number of world blocks represented by one output pixel.
    pub blocks_per_pixel: u32,
    /// Number of output pixels generated for each world block.
    pub pixels_per_block: u32,
}

impl Default for RenderLayout {
    fn default() -> Self {
        Self {
            chunks_per_tile: 16,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
        }
    }
}

impl RenderLayout {
    /// Computes the output tile size in pixels, if the layout is valid.
    #[must_use]
    pub fn tile_size(self) -> Option<u32> {
        let tile_blocks = self.chunks_per_tile.checked_mul(16)?;
        let output_pixels = tile_blocks.checked_mul(self.pixels_per_block)?;
        if self.blocks_per_pixel == 0
            || self.pixels_per_block == 0
            || output_pixels % self.blocks_per_pixel != 0
        {
            return None;
        }
        let tile_size = output_pixels / self.blocks_per_pixel;
        (tile_size <= MAX_TILE_SIZE_PIXELS).then_some(tile_size)
    }
}

/// Alias for render layouts used specifically by chunk tile planning.
pub type ChunkTileLayout = RenderLayout;

/// Region bake layout in chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionLayout {
    /// Number of chunks covered by one baked region edge.
    pub chunks_per_region: u32,
}

impl Default for RegionLayout {
    fn default() -> Self {
        Self {
            chunks_per_region: 32,
        }
    }
}

impl RegionLayout {
    /// Validates the region layout.
    ///
    /// # Errors
    ///
    /// Returns an error if `chunks_per_region` is zero or greater than `256`.
    pub fn validate(self) -> Result<()> {
        if self.chunks_per_region == 0 {
            return Err(BedrockRenderError::Validation(
                "chunks_per_region must be greater than zero".to_string(),
            ));
        }
        if self.chunks_per_region > 256 {
            return Err(BedrockRenderError::Validation(
                "chunks_per_region must be <= 256".to_string(),
            ));
        }
        Ok(())
    }
}

/// Region coordinate in baked-region space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionCoord {
    /// Region X coordinate.
    pub x: i32,
    /// Region Z coordinate.
    pub z: i32,
    /// Bedrock dimension for this region.
    pub dimension: Dimension,
}

impl RegionCoord {
    /// Converts a chunk position into the region that contains it.
    #[must_use]
    pub fn from_chunk(pos: ChunkPos, layout: RegionLayout) -> Self {
        let span = i32::try_from(layout.chunks_per_region).unwrap_or(32).max(1);
        Self {
            x: pos.x.div_euclid(span),
            z: pos.z.div_euclid(span),
            dimension: pos.dimension,
        }
    }

    /// Returns the inclusive chunk bounds covered by this region.
    #[must_use]
    pub fn chunk_region(self, layout: RegionLayout) -> ChunkRegion {
        let span = i32::try_from(layout.chunks_per_region).unwrap_or(32).max(1);
        let min_chunk_x = self.x.saturating_mul(span);
        let min_chunk_z = self.z.saturating_mul(span);
        ChunkRegion::new(
            self.dimension,
            min_chunk_x,
            min_chunk_z,
            min_chunk_x.saturating_add(span.saturating_sub(1)),
            min_chunk_z.saturating_add(span.saturating_sub(1)),
        )
    }
}

/// Inclusive rectangular chunk region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRegion {
    /// Bedrock dimension for this region.
    pub dimension: Dimension,
    /// Minimum chunk X coordinate.
    pub min_chunk_x: i32,
    /// Minimum chunk Z coordinate.
    pub min_chunk_z: i32,
    /// Maximum chunk X coordinate.
    pub max_chunk_x: i32,
    /// Maximum chunk Z coordinate.
    pub max_chunk_z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RegionBakeKey {
    coord: RegionCoord,
    mode: RenderMode,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegionBakeCacheKey {
    key: RegionBakeKey,
    layout: RegionLayout,
    surface_hash: u64,
    renderer_version: u32,
    palette_version: u32,
}

type RegionBakeMap = BTreeMap<RegionBakeKey, Arc<RegionBake>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ChunkBakeKey {
    pos: ChunkPos,
    mode: RenderMode,
}

struct TileComposeTask {
    tile_index: usize,
    job: RenderJob,
    bakes: BTreeMap<ChunkPos, ChunkBake>,
    diagnostics: RenderDiagnostics,
}

struct TileComposeResult {
    tile_index: usize,
    tile: TileImage,
}

#[derive(Debug, Clone)]
struct RegionPlan {
    key: RegionBakeKey,
    region: ChunkRegion,
    chunk_positions: Vec<ChunkPos>,
}

struct RegionWaveRenderResult {
    rendered_tiles: BTreeSet<usize>,
    diagnostics: RenderDiagnostics,
    stats: RenderPipelineStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegionWaveWorkerSplit {
    region_workers: usize,
    world_workers_per_region: usize,
}

impl ChunkRegion {
    /// Creates an inclusive chunk region.
    #[must_use]
    pub const fn new(
        dimension: Dimension,
        min_chunk_x: i32,
        min_chunk_z: i32,
        max_chunk_x: i32,
        max_chunk_z: i32,
    ) -> Self {
        Self {
            dimension,
            min_chunk_x,
            min_chunk_z,
            max_chunk_x,
            max_chunk_z,
        }
    }
}

impl From<ChunkRegion> for WorldChunkQueryRegion {
    fn from(region: ChunkRegion) -> Self {
        Self {
            dimension: region.dimension,
            min_chunk_x: region.min_chunk_x,
            min_chunk_z: region.min_chunk_z,
            max_chunk_x: region.max_chunk_x,
            max_chunk_z: region.max_chunk_z,
        }
    }
}

/// Path scheme for writing planned tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilePathScheme {
    /// Hierarchical web-map layout.
    WebMap,
    /// Single flat filename layout.
    Flat,
}

/// Planned tile plus the chunk region it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTile {
    /// Render job for this tile.
    pub job: RenderJob,
    /// Inclusive chunk bounds needed by the tile.
    pub region: ChunkRegion,
    /// Layout used to produce the job.
    pub layout: ChunkTileLayout,
    /// Optional exact chunk positions for sparse exports.
    pub chunk_positions: Option<Arc<[ChunkPos]>>,
}

impl PlannedTile {
    /// Builds a relative path for this tile.
    #[must_use]
    pub fn relative_path(&self, scheme: TilePathScheme, extension: &str) -> PathBuf {
        let dimension = dimension_slug(self.job.coord.dimension);
        let mode = mode_slug(self.job.mode);
        let extension = extension.trim_start_matches('.');
        match scheme {
            TilePathScheme::WebMap => PathBuf::from(dimension)
                .join(mode)
                .join(format!(
                    "{}c-{}bpp-{}ppb",
                    self.layout.chunks_per_tile,
                    self.layout.blocks_per_pixel,
                    self.layout.pixels_per_block
                ))
                .join(self.job.coord.x.to_string())
                .join(format!("{}.{}", self.job.coord.z, extension)),
            TilePathScheme::Flat => PathBuf::from(format!(
                "{dimension}_{mode}_cpt{}_bpp{}_ppb{}_x{}_z{}.{}",
                self.layout.chunks_per_tile,
                self.layout.blocks_per_pixel,
                self.layout.pixels_per_block,
                self.job.coord.x,
                self.job.coord.z,
                extension
            )),
        }
    }
}

/// Runtime options used for tile, batch, region, and web-map rendering.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Requested output format.
    pub format: ImageFormat,
    /// Lossy encoder quality when the selected encoder supports quality.
    pub quality: u8,
    /// Requested render backend.
    pub backend: RenderBackend,
    /// CPU pipeline scheduling policy.
    pub cpu: RenderCpuPipelineOptions,
    /// Tile scheduling priority.
    pub priority: RenderTilePriority,
    /// Worker-thread policy.
    pub threading: RenderThreadingOptions,
    /// Execution profile used for automatic thread and memory budgets.
    pub execution_profile: RenderExecutionProfile,
    /// Region-bake memory budget policy.
    pub memory_budget: RenderMemoryBudget,
    /// CPU worker policy used by streaming render APIs for DB/decode/encode stages.
    pub cpu_workers: RenderThreadingOptions,
    /// Capacity for the internal compose/write pipeline; zero means automatic.
    pub pipeline_depth: usize,
    /// Optional cancellation flag observed by long-running operations.
    pub cancel: Option<RenderCancelFlag>,
    /// Optional progress callback.
    pub progress: Option<RenderProgressSink>,
    /// Optional diagnostics callback.
    pub diagnostics: Option<RenderDiagnosticsSink>,
    /// Surface render behavior.
    pub surface: SurfaceRenderOptions,
    /// Tile cache behavior.
    pub cache_policy: RenderCachePolicy,
    /// Seed used to validate fast tile cache entries; zero disables validation.
    pub tile_cache_validation_seed: u64,
    /// Region-bake layout used by region-backed rendering.
    pub region_layout: RegionLayout,
    /// Performance policy for interactive fast paths, sidecar cache, and surface loading.
    pub performance: RenderPerformanceOptions,
    /// GPU device, scheduling, fallback, and diagnostics policy.
    pub gpu: RenderGpuOptions,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::WebP,
            quality: 90,
            backend: RenderBackend::Cpu,
            cpu: RenderCpuPipelineOptions::default(),
            priority: RenderTilePriority::RowMajor,
            threading: RenderThreadingOptions::Auto,
            execution_profile: RenderExecutionProfile::Export,
            memory_budget: RenderMemoryBudget::Auto,
            cpu_workers: RenderThreadingOptions::Auto,
            pipeline_depth: 0,
            cancel: None,
            progress: None,
            diagnostics: None,
            surface: SurfaceRenderOptions::default(),
            cache_policy: RenderCachePolicy::Bypass,
            tile_cache_validation_seed: 0,
            region_layout: RegionLayout::default(),
            performance: RenderPerformanceOptions::default(),
            gpu: RenderGpuOptions::default(),
        }
    }
}

impl RenderOptions {
    /// Returns an aggressive interactive profile intended for viewport rendering.
    ///
    /// This keeps final tiles exact, but enables faster surface loading and
    /// final tile cache usage when used with [`MapRenderSessionConfig::max_speed`].
    #[must_use]
    pub fn max_speed_interactive() -> Self {
        Self {
            backend: RenderBackend::Cpu,
            priority: RenderTilePriority::RowMajor,
            threading: RenderThreadingOptions::Auto,
            execution_profile: RenderExecutionProfile::Interactive,
            memory_budget: RenderMemoryBudget::Auto,
            pipeline_depth: 0,
            cache_policy: RenderCachePolicy::Use,
            performance: RenderPerformanceOptions {
                profile: RenderPerformanceProfile::MaxSpeed,
                progressive_preview: true,
                surface_load: RenderSurfaceLoadPolicy::HintThenVerify,
            },
            ..Self::default()
        }
    }

    /// Returns an aggressive CPU export profile for large batch/web-map renders.
    #[must_use]
    pub fn max_speed_export() -> Self {
        Self {
            backend: RenderBackend::Cpu,
            priority: RenderTilePriority::RowMajor,
            threading: RenderThreadingOptions::Auto,
            execution_profile: RenderExecutionProfile::Export,
            memory_budget: RenderMemoryBudget::Auto,
            cpu_workers: RenderThreadingOptions::Auto,
            pipeline_depth: 0,
            cache_policy: RenderCachePolicy::Use,
            performance: RenderPerformanceOptions {
                profile: RenderPerformanceProfile::MaxSpeed,
                progressive_preview: false,
                surface_load: RenderSurfaceLoadPolicy::HintThenVerify,
            },
            ..Self::default()
        }
    }
}

/// CPU load/bake/compose pipeline policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderCpuPipelineOptions {
    /// Queue depth for bounded CPU pipeline channels; zero chooses automatically.
    pub queue_depth: usize,
    /// Number of chunks assigned to one load/bake batch; zero chooses automatically.
    pub chunk_batch_size: usize,
    /// Number of worker threads reserved for encode/write work; zero chooses automatically.
    pub encode_workers: usize,
    /// Hard cap for all CPU workers used by one interactive render request; zero chooses automatically.
    pub max_total_threads: usize,
    /// Hard cap for database/world-load workers; zero chooses automatically.
    pub max_db_workers: usize,
    /// Hard cap for region/chunk bake workers; zero chooses automatically.
    pub max_bake_workers: usize,
    /// Hard cap for tile compose workers; zero chooses automatically.
    pub max_compose_workers: usize,
    /// Maximum regions allowed in flight for interactive region waves; zero chooses automatically.
    pub max_in_flight_regions: usize,
}

impl RenderCpuPipelineOptions {
    fn resolve_queue_depth(self, workers: usize, work_items: usize) -> usize {
        let capped_workers = if self.max_total_threads > 0 {
            workers.min(self.max_total_threads).max(1)
        } else {
            workers.max(1)
        };
        self.queue_depth
            .max(if self.queue_depth == 0 {
                capped_workers
                    .saturating_mul(2)
                    .max(work_items.clamp(1, 128))
            } else {
                1
            })
            .max(1)
    }

    fn to_world_pipeline(self) -> WorldPipelineOptions {
        WorldPipelineOptions {
            queue_depth: self.queue_depth,
            chunk_batch_size: self.chunk_batch_size,
            subchunk_decode_workers: 0,
            progress_interval: 0,
        }
    }
}

/// Tile scheduling priority for interactive and export pipelines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderTilePriority {
    /// Stable tile x/z order.
    #[default]
    RowMajor,
    /// Render tiles closest to the supplied tile coordinate first.
    DistanceFrom { tile_x: i32, tile_z: i32 },
}

/// Requested render backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderBackend {
    /// Force CPU composition.
    #[default]
    Cpu,
    /// Prefer GPU composition when a compatible backend is available.
    Auto,
    /// Require or attempt the configured wgpu backend.
    Wgpu,
}

/// Requested GPU backend for wgpu rendering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderGpuBackend {
    /// Select the best supported GPU backend for the platform.
    #[default]
    Auto,
    /// Use the native Direct3D 11 compute backend on Windows.
    Dx11,
    /// Use wgpu's DirectX 12 backend.
    Dx12,
    /// Use wgpu's Vulkan backend.
    Vulkan,
}

/// CPU fallback policy when GPU work is unavailable or fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderGpuFallbackPolicy {
    /// Fall back to CPU and record the reason in stats.
    #[default]
    AllowCpu,
    /// Return an error instead of falling back to CPU.
    Required,
}

/// Which parts of the render/query pipeline may use GPU acceleration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderGpuPipelineLevel {
    /// GPU may process tile/region RGBA compose outputs.
    #[default]
    ComposeOnly,
    /// GPU may also process professional overlay primitives.
    Overlays,
    /// GPU may run experimental query/stat aggregation kernels.
    ExperimentalQueries,
}

/// GPU device and scheduling options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderGpuOptions {
    /// Preferred wgpu backend.
    pub backend: RenderGpuBackend,
    /// CPU fallback behavior.
    pub fallback_policy: RenderGpuFallbackPolicy,
    /// Maximum GPU pipeline depth.
    pub pipeline_level: RenderGpuPipelineLevel,
    /// Maximum in-flight GPU jobs; zero selects an automatic profile.
    pub max_in_flight: usize,
    /// Target pixels per GPU batch; zero selects an automatic profile.
    pub batch_pixels: usize,
    /// Staging buffer pool size in bytes; zero disables explicit pooling.
    pub staging_pool_bytes: usize,
    /// Whether to collect verbose GPU diagnostics.
    pub diagnostics: bool,
}

impl Default for RenderGpuOptions {
    fn default() -> Self {
        Self {
            backend: RenderGpuBackend::Auto,
            fallback_policy: RenderGpuFallbackPolicy::AllowCpu,
            pipeline_level: RenderGpuPipelineLevel::ComposeOnly,
            max_in_flight: 0,
            batch_pixels: 0,
            staging_pool_bytes: 0,
            diagnostics: false,
        }
    }
}

/// High-level performance profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderPerformanceProfile {
    /// Conservative default behavior.
    #[default]
    Balanced,
    /// Favor latency and throughput with cache and reduced cold-path reads.
    MaxSpeed,
}

/// Surface record-loading strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderSurfaceLoadPolicy {
    /// Read all surface subchunks up front.
    #[default]
    Full,
    /// Use height-map hints first, then batch-refill incomplete chunks.
    HintThenVerify,
}

/// Performance policy for render operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RenderPerformanceOptions {
    /// Performance profile.
    pub profile: RenderPerformanceProfile,
    /// Emit preview stream events before final exact rendered events when possible.
    pub progressive_preview: bool,
    /// Surface subchunk loading policy.
    pub surface_load: RenderSurfaceLoadPolicy,
}

impl RenderBackend {
    /// Returns the cache slug for this backend request.
    #[must_use]
    pub const fn cache_slug(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Auto => "auto",
            Self::Wgpu => "wgpu",
        }
    }
}

impl RenderGpuBackend {
    /// Returns a stable cache slug for this GPU backend request.
    #[must_use]
    pub const fn cache_slug(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dx11 => "dx11",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
        }
    }
}

/// Backend actually used by a render operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ResolvedRenderBackend {
    /// CPU-only rendering.
    #[default]
    Cpu,
    /// GPU rendering through native Direct3D 11.
    Dx11,
    /// GPU rendering through wgpu DX12.
    WgpuDx12,
    /// GPU rendering through wgpu Vulkan.
    WgpuVulkan,
    /// A mix of CPU and GPU tiles.
    Mixed,
}

impl ResolvedRenderBackend {
    /// Human-readable backend label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Dx11 => "dx11",
            Self::WgpuDx12 => "wgpu-dx12",
            Self::WgpuVulkan => "wgpu-vulkan",
            Self::Mixed => "mixed",
        }
    }
}

/// Execution profile used to resolve automatic resources.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderExecutionProfile {
    /// Offline/export workload that may use all logical CPUs.
    #[default]
    Export,
    /// Foreground workload that reserves more CPU capacity for the caller.
    Interactive,
}

impl RenderExecutionProfile {
    /// Resolves an automatic thread count for a work-item count.
    #[must_use]
    pub fn default_auto_threads(self, work_items: usize) -> usize {
        let logical_threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let resolved = match self {
            Self::Export => logical_threads,
            Self::Interactive => (logical_threads / 2).clamp(1, 6),
        };
        resolved.min(work_items.max(1))
    }
}

/// Region-bake memory budget policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderMemoryBudget {
    /// Choose a bounded budget from currently available system memory.
    #[default]
    Auto,
    /// Use an explicit byte budget.
    FixedBytes(u64),
    /// Disable memory-budget chunking.
    Disabled,
}

impl RenderMemoryBudget {
    /// Resolves this policy into a concrete byte budget, if enabled.
    #[must_use]
    pub fn resolve_bytes(self, profile: RenderExecutionProfile) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::FixedBytes(bytes) => usize::try_from(bytes).ok(),
            Self::Auto => Some(auto_memory_budget_bytes(profile)),
        }
    }
}

fn auto_memory_budget_bytes(profile: RenderExecutionProfile) -> usize {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let available_bytes = usize::try_from(system.available_memory()).unwrap_or(0);
    let budget = available_bytes
        .checked_div(5)
        .unwrap_or(DEFAULT_EXPORT_MEMORY_BUDGET_BYTES);
    let capped = budget.clamp(MIN_AUTO_MEMORY_BUDGET_BYTES, MAX_AUTO_MEMORY_BUDGET_BYTES);
    match profile {
        RenderExecutionProfile::Export => capped,
        RenderExecutionProfile::Interactive => capped.min(DEFAULT_INTERACTIVE_MEMORY_BUDGET_BYTES),
    }
}

/// Aggregated timing and throughput stats from a web-map render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderPipelineStats {
    /// Number of planned output tiles.
    pub planned_tiles: usize,
    /// Number of planned baked regions.
    pub planned_regions: usize,
    /// Number of unique chunks referenced by the render.
    pub unique_chunks: usize,
    /// Number of chunks baked.
    pub baked_chunks: usize,
    /// Number of regions baked.
    pub baked_regions: usize,
    /// Number of baked chunks copied into region payloads.
    pub region_chunks_copied: usize,
    /// Number of baked chunks skipped because they did not fit the target region.
    pub region_chunks_out_of_bounds: usize,
    /// Number of tile pixel samples that could not find a baked region.
    pub tile_missing_region_samples: usize,
    /// Tile cache hits.
    pub cache_hits: usize,
    /// Tile cache misses.
    pub cache_misses: usize,
    /// Tile cache hits satisfied from the in-memory LRU.
    pub cache_memory_hits: usize,
    /// Tile cache hits decoded from validated disk entries.
    pub cache_disk_fresh_hits: usize,
    /// Tile cache hits decoded from legacy disk entries without validation.
    pub cache_disk_stale_hits: usize,
    /// Negative tile cache hits for tiles with no renderable chunks.
    pub cache_empty_negative_hits: usize,
    /// Tile cache entries probed.
    pub cache_probes: usize,
    /// Tile cache validation mismatches.
    pub cache_validation_mismatches: usize,
    /// Time spent reading tile cache bytes, in milliseconds.
    pub cache_read_ms: u128,
    /// Time spent decoding tile cache bytes, in milliseconds.
    pub cache_decode_ms: u128,
    /// Time spent decoding persisted tile blob payloads, in milliseconds.
    pub tile_blob_decode_ms: u128,
    /// Elapsed time until the first cache-ready tile was emitted.
    pub cache_first_ready_ms: u128,
    /// Region cache hits.
    pub region_cache_hits: usize,
    /// Region cache misses.
    pub region_cache_misses: usize,
    /// Tile-index trusted fast-path hits.
    pub tile_index_trusted_hits: usize,
    /// Tile-index hits that required dependency validation.
    pub tile_index_validated_hits: usize,
    /// Tile-index misses.
    pub tile_index_misses: usize,
    /// Tile-index empty negative hits.
    pub tile_index_empty_hits: usize,
    /// Time spent reading tile-index data, in milliseconds.
    pub tile_index_read_ms: u128,
    /// Time spent validating tile dependencies, in milliseconds.
    pub tile_dep_validation_ms: u128,
    /// Persistent tile-cache writer drops caused by queue pressure or unavailable writer.
    pub tile_cache_writer_dropped: usize,
    /// Whether the world quick signature allowed trusted tile-index reads.
    pub world_signature_trusted: bool,
    /// Whether the world quick signature changed relative to the loaded index.
    pub world_signature_changed: bool,
    /// Tile-index entries rejected due to corrupt index/blob/decode state.
    pub index_corrupt_misses: usize,
    /// Time spent baking chunks, in milliseconds.
    pub bake_ms: u128,
    /// Time spent loading world render records, in milliseconds.
    pub world_load_ms: u128,
    /// Time spent baking regions, in milliseconds.
    pub region_bake_ms: u128,
    /// Time spent copying baked chunks into region payloads, in milliseconds.
    pub region_copy_ms: u128,
    /// Time spent composing tiles, in milliseconds.
    pub tile_compose_ms: u128,
    /// Time spent encoding images, in milliseconds.
    pub encode_ms: u128,
    /// Time spent writing encoded tiles, in milliseconds.
    pub write_ms: u128,
    /// Aggregate worker idle time, in milliseconds.
    pub worker_idle_ms: u128,
    /// Aggregate queue wait time, in milliseconds.
    pub queue_wait_ms: u128,
    /// Queue wait time in CPU load/bake/compose stages, in milliseconds.
    pub cpu_queue_wait_ms: u128,
    /// Prefix scans performed by render chunk loading.
    pub render_prefix_scans: usize,
    /// Exact DB read batches performed by render chunk loading.
    pub exact_get_batches: usize,
    /// Exact DB keys requested by render chunk loading.
    pub exact_keys_requested: usize,
    /// Exact DB keys found by render chunk loading.
    pub exact_keys_found: usize,
    /// Time spent in exact DB reads, in milliseconds.
    pub db_read_ms: u128,
    /// Time spent decoding render chunk records, in milliseconds.
    pub decode_ms: u128,
    /// Time spent parsing biome records, in milliseconds.
    pub biome_parse_ms: u128,
    /// Time spent parsing subchunks, in milliseconds.
    pub subchunk_parse_ms: u128,
    /// Time spent scanning surface columns, in milliseconds.
    pub surface_scan_ms: u128,
    /// Time spent parsing block entities, in milliseconds.
    pub block_entity_parse_ms: u128,
    /// Time spent in `HintThenVerify` full reloads, in milliseconds.
    pub full_reload_ms: u128,
    /// Peak world decode worker thread count reported by bedrock-world.
    pub world_worker_threads: usize,
    /// Peak region cache memory, in bytes.
    pub peak_cache_bytes: usize,
    /// Peak active task count.
    pub active_tasks_peak: usize,
    /// Peak worker thread count.
    pub peak_worker_threads: usize,
    /// Backend used by tile composition.
    pub resolved_backend: ResolvedRenderBackend,
    /// Number of tiles composed on CPU.
    pub cpu_tiles: usize,
    /// Number of tiles processed on GPU.
    pub gpu_tiles: usize,
    /// Requested GPU backend.
    pub gpu_requested_backend: RenderGpuBackend,
    /// Actual GPU backend selected by the adapter.
    pub gpu_actual_backend: RenderGpuBackend,
    /// GPU adapter name, when available.
    pub gpu_adapter_name: Option<String>,
    /// GPU device name, when available.
    pub gpu_device_name: Option<String>,
    /// Last GPU fallback reason, when GPU work fell back to CPU.
    pub gpu_fallback_reason: Option<String>,
    /// Time spent waiting for GPU queue access, in milliseconds.
    pub gpu_queue_wait_ms: u128,
    /// Time spent preparing GPU buffers, in milliseconds.
    pub gpu_prepare_ms: u128,
    /// Time spent uploading to GPU, in milliseconds.
    pub gpu_upload_ms: u128,
    /// Time spent dispatching GPU work, in milliseconds.
    pub gpu_dispatch_ms: u128,
    /// Time spent reading GPU output back to CPU memory, in milliseconds.
    pub gpu_readback_ms: u128,
    /// Bytes uploaded to GPU.
    pub gpu_uploaded_bytes: usize,
    /// Bytes read back from GPU.
    pub gpu_readback_bytes: usize,
    /// Peak in-flight GPU jobs.
    pub gpu_peak_in_flight: usize,
    /// Number of GPU buffer reuses.
    pub gpu_buffer_reuses: usize,
    /// Time spent CPU-decoding LevelDB/NBT render inputs, in milliseconds.
    pub cpu_decode_ms: u128,
    /// Time spent packing CPU-decoded render data into compact frame inputs.
    pub cpu_frame_pack_ms: u128,
    /// Time spent decoding compact chunk frames for render input.
    pub chunk_frame_decode_ms: u128,
    /// Whole-render tile throughput.
    pub tiles_per_second: u64,
    /// Whole-render chunk throughput.
    pub chunks_per_second: u64,
    /// Approximate CPU worker utilization, scaled by 1000.
    pub cpu_worker_utilization_per_mille: u16,
}

/// Result returned by streaming web-map render APIs.
#[derive(Debug, Clone)]
pub struct RenderWebTilesResult {
    /// Aggregated render diagnostics.
    pub diagnostics: RenderDiagnostics,
    /// Aggregated pipeline statistics.
    pub stats: RenderPipelineStats,
}

/// Built-in terrain lighting presets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TerrainLightingPreset {
    /// Disable terrain lighting.
    Off,
    /// Balanced terrain relief.
    Soft,
    /// Higher-contrast terrain relief.
    Strong,
}

/// Terrain lighting and relief parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainLightingOptions {
    /// Whether lighting is enabled.
    pub enabled: bool,
    /// Direction of the light source in degrees.
    pub light_azimuth_degrees: f32,
    /// Elevation of the light source in degrees.
    pub light_elevation_degrees: f32,
    /// Strength of the sampled terrain normal.
    pub normal_strength: f32,
    /// Strength of shadow darkening.
    pub shadow_strength: f32,
    /// Strength of highlight brightening.
    pub highlight_strength: f32,
    /// Ambient occlusion applied on relief.
    pub ambient_occlusion: f32,
    /// Maximum land shadow percentage.
    pub max_shadow: f32,
    /// Softness used to compress steep land slopes.
    pub land_slope_softness: f32,
    /// Strength of local edge relief.
    pub edge_relief_strength: f32,
    /// Minimum neighbor delta before edge relief is applied.
    pub edge_relief_threshold: f32,
    /// Maximum edge-relief shadow percentage.
    pub edge_relief_max_shadow: f32,
    /// Edge-relief highlight multiplier.
    pub edge_relief_highlight: f32,
    /// Whether underwater terrain relief is enabled.
    pub underwater_relief_enabled: bool,
    /// Underwater relief strength multiplier.
    pub underwater_relief_strength: f32,
    /// Depth at which underwater relief fades out.
    pub underwater_depth_fade: f32,
    /// Minimum underwater light contribution.
    pub underwater_min_light: f32,
}

impl TerrainLightingOptions {
    /// Returns a lighting configuration with lighting disabled.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            light_azimuth_degrees: 315.0,
            light_elevation_degrees: 45.0,
            normal_strength: 0.0,
            shadow_strength: 0.0,
            highlight_strength: 0.0,
            ambient_occlusion: 0.0,
            max_shadow: 0.0,
            land_slope_softness: 0.0,
            edge_relief_strength: 0.0,
            edge_relief_threshold: 3.0,
            edge_relief_max_shadow: 0.0,
            edge_relief_highlight: 0.0,
            underwater_relief_enabled: false,
            underwater_relief_strength: 0.0,
            underwater_depth_fade: 1.0,
            underwater_min_light: 0.0,
        }
    }

    /// Returns the default balanced terrain lighting preset.
    #[must_use]
    pub const fn soft() -> Self {
        Self {
            enabled: true,
            light_azimuth_degrees: 315.0,
            light_elevation_degrees: 45.0,
            normal_strength: 1.55,
            shadow_strength: 0.48,
            highlight_strength: 0.34,
            ambient_occlusion: 0.055,
            max_shadow: 42.0,
            land_slope_softness: 7.0,
            edge_relief_strength: 0.24,
            edge_relief_threshold: 2.0,
            edge_relief_max_shadow: 20.0,
            edge_relief_highlight: 0.12,
            underwater_relief_enabled: true,
            underwater_relief_strength: 0.75,
            underwater_depth_fade: 8.0,
            underwater_min_light: 0.35,
        }
    }

    /// Returns a high-contrast terrain lighting preset.
    #[must_use]
    pub const fn strong() -> Self {
        Self {
            enabled: true,
            light_azimuth_degrees: 315.0,
            light_elevation_degrees: 42.0,
            normal_strength: 2.2,
            shadow_strength: 0.62,
            highlight_strength: 0.48,
            ambient_occlusion: 0.08,
            max_shadow: 48.0,
            land_slope_softness: 8.0,
            edge_relief_strength: 0.28,
            edge_relief_threshold: 3.0,
            edge_relief_max_shadow: 18.0,
            edge_relief_highlight: 0.10,
            underwater_relief_enabled: true,
            underwater_relief_strength: 1.25,
            underwater_depth_fade: 12.0,
            underwater_min_light: 0.25,
        }
    }

    /// Returns options for a built-in preset.
    #[must_use]
    pub const fn preset(preset: TerrainLightingPreset) -> Self {
        match preset {
            TerrainLightingPreset::Off => Self::off(),
            TerrainLightingPreset::Soft => Self::soft(),
            TerrainLightingPreset::Strong => Self::strong(),
        }
    }
}

impl Default for TerrainLightingOptions {
    fn default() -> Self {
        Self::soft()
    }
}

/// Per-block 2D boundary and contact-shadow options for surface rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockBoundaryRenderOptions {
    /// Whether per-block boundary shading is enabled.
    pub enabled: bool,
    /// Overall boundary shadow strength.
    pub strength: f32,
    /// Subtle line strength used on flat terrain.
    pub flat_strength: f32,
    /// Minimum height difference before height-contact emphasis is applied.
    pub height_threshold: f32,
    /// Maximum boundary shadow percentage.
    pub max_shadow: f32,
    /// Boundary highlight multiplier.
    pub highlight_strength: f32,
    /// Softness used to compress large height differences.
    pub softness: f32,
    /// Width of the boundary line in output pixels.
    pub line_width_pixels: f32,
}

impl BlockBoundaryRenderOptions {
    /// Returns disabled block-boundary rendering.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            strength: 0.0,
            flat_strength: 0.0,
            height_threshold: 1.0,
            max_shadow: 0.0,
            highlight_strength: 0.0,
            softness: 1.0,
            line_width_pixels: 1.0,
        }
    }
}

impl Default for BlockBoundaryRenderOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 0.34,
            flat_strength: 0.08,
            height_threshold: 1.0,
            max_shadow: 14.0,
            highlight_strength: 0.08,
            softness: 6.0,
            line_width_pixels: 1.0,
        }
    }
}

/// Per-block top-down volume shading options for surface rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockVolumeRenderOptions {
    /// Whether top-down block volume shading is enabled.
    pub enabled: bool,
    /// Width of the exposed face band in output pixels.
    pub face_width_pixels: f32,
    /// Strength of exposed vertical face shadows.
    pub face_shadow_strength: f32,
    /// Strength of contact shadows cast by higher neighboring blocks.
    pub contact_shadow_strength: f32,
    /// Strength of short cast shadows from nearby higher blocks.
    pub cast_shadow_strength: f32,
    /// Maximum number of blocks sampled along the light direction.
    pub cast_shadow_max_blocks: u32,
    /// Height difference scale used by cast shadows.
    pub cast_shadow_height_scale: f32,
    /// Strength of exposed edge highlights.
    pub highlight_strength: f32,
    /// Maximum block-volume shadow percentage.
    pub max_shadow: f32,
    /// Maximum block-volume highlight percentage.
    pub max_highlight: f32,
    /// Minimum height difference before block-volume shading is applied.
    pub height_threshold: f32,
    /// Softness used to compress large height differences.
    pub softness: f32,
}

impl BlockVolumeRenderOptions {
    /// Returns disabled block-volume rendering.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            face_width_pixels: 0.0,
            face_shadow_strength: 0.0,
            contact_shadow_strength: 0.0,
            cast_shadow_strength: 0.0,
            cast_shadow_max_blocks: 0,
            cast_shadow_height_scale: 1.0,
            highlight_strength: 0.0,
            max_shadow: 0.0,
            max_highlight: 0.0,
            height_threshold: 1.0,
            softness: 1.0,
        }
    }
}

impl Default for BlockVolumeRenderOptions {
    fn default() -> Self {
        Self::off()
    }
}

/// Material bucket used by the CPU atlas surface renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u8)]
enum SurfaceMaterialId {
    #[default]
    Unknown = 0,
    Grass = 1,
    Foliage = 2,
    Snow = 3,
    Stone = 4,
    Dirt = 5,
    Sand = 6,
    Wood = 7,
    Water = 8,
    Lava = 9,
    Metal = 10,
    Plant = 11,
    Built = 12,
}

impl SurfaceMaterialId {
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Grass,
            2 => Self::Foliage,
            3 => Self::Snow,
            4 => Self::Stone,
            5 => Self::Dirt,
            6 => Self::Sand,
            7 => Self::Wood,
            8 => Self::Water,
            9 => Self::Lava,
            10 => Self::Metal,
            11 => Self::Plant,
            12 => Self::Built,
            _ => Self::Unknown,
        }
    }

    const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Built-in deterministic texture family used by the atlas renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u8)]
enum MapAtlasTextureId {
    #[default]
    Plains = 0,
    DenseCanopy = 1,
    SparseCanopy = 2,
    SnowRidge = 3,
    StoneContour = 4,
    DirtPatch = 5,
    SandRipples = 6,
    WaterGrid = 7,
    LavaAccent = 8,
    Structure = 9,
}

/// Deterministic texture-atlas and map-contour controls for the CPU atlas renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasRenderOptions {
    /// Whether atlas shading is enabled for surface block rendering.
    pub enabled: bool,
    /// Strength of fixed material texture atlas samples.
    pub texture_detail_strength: f32,
    /// Height interval in blocks between atlas contour lines.
    pub height_contour_interval: u32,
    /// Strength of height contour darkening.
    pub height_contour_strength: f32,
    /// Strength of deterministic slope hatching on mountains.
    pub slope_hatching_strength: f32,
    /// Strength of dense forest canopy shaping.
    pub forest_canopy_strength: f32,
    /// Strength of snow ridge/crack detail.
    pub snow_ridge_strength: f32,
    /// Strength of water grid detail.
    pub water_grid_strength: f32,
    /// Strength of shoreline darkening.
    pub shoreline_shadow_strength: f32,
    /// Strength of the chunk/grid overlay.
    pub chunk_grid_strength: f32,
    /// Strength of material edge lines between neighboring material buckets.
    pub material_edge_strength: f32,
    /// Strength of short terrain cast shadows.
    pub cast_shadow_strength: f32,
    /// Strength of local ambient occlusion from height relief.
    pub ambient_occlusion_strength: f32,
}

impl AtlasRenderOptions {
    /// Returns a disabled atlas configuration.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            texture_detail_strength: 0.0,
            height_contour_interval: 0,
            height_contour_strength: 0.0,
            slope_hatching_strength: 0.0,
            forest_canopy_strength: 0.0,
            snow_ridge_strength: 0.0,
            water_grid_strength: 0.0,
            shoreline_shadow_strength: 0.0,
            chunk_grid_strength: 0.0,
            material_edge_strength: 0.0,
            cast_shadow_strength: 0.0,
            ambient_occlusion_strength: 0.0,
        }
    }
}

impl Default for AtlasRenderOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            texture_detail_strength: 0.74,
            height_contour_interval: 4,
            height_contour_strength: 0.55,
            slope_hatching_strength: 0.42,
            forest_canopy_strength: 0.82,
            snow_ridge_strength: 0.72,
            water_grid_strength: 0.34,
            shoreline_shadow_strength: 0.52,
            chunk_grid_strength: 0.20,
            material_edge_strength: 0.34,
            cast_shadow_strength: 0.36,
            ambient_occlusion_strength: 0.46,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerrainHeightNeighborhood {
    center: i16,
    north_west: i16,
    north: i16,
    north_east: i16,
    west: i16,
    east: i16,
    south_west: i16,
    south: i16,
    south_east: i16,
}

impl TerrainHeightNeighborhood {
    fn sobel_gradient(self) -> (f32, f32) {
        let dx =
            (f32::from(self.north_east) + 2.0 * f32::from(self.east) + f32::from(self.south_east)
                - f32::from(self.north_west)
                - 2.0 * f32::from(self.west)
                - f32::from(self.south_west))
                / 8.0;
        let dz =
            (f32::from(self.south_west) + 2.0 * f32::from(self.south) + f32::from(self.south_east)
                - f32::from(self.north_west)
                - 2.0 * f32::from(self.north)
                - f32::from(self.north_east))
                / 8.0;
        (dx, dz)
    }

    fn edge_relief(self, threshold: f32) -> TerrainEdgeRelief {
        let threshold = threshold.max(0.0);
        let center = f32::from(self.center);
        let mut higher_neighbor_delta = 0.0_f32;
        let mut lower_neighbor_delta = 0.0_f32;
        for neighbor in [
            self.north_west,
            self.north,
            self.north_east,
            self.west,
            self.east,
            self.south_west,
            self.south,
            self.south_east,
        ] {
            let delta = f32::from(neighbor) - center;
            higher_neighbor_delta = higher_neighbor_delta.max(delta);
            lower_neighbor_delta = lower_neighbor_delta.max(-delta);
        }

        let max_delta = higher_neighbor_delta.max(lower_neighbor_delta);
        if max_delta <= threshold {
            return TerrainEdgeRelief::default();
        }
        let amount = ((max_delta - threshold) / 16.0).clamp(0.0, 1.0);
        let pit_edge = higher_neighbor_delta >= lower_neighbor_delta;
        TerrainEdgeRelief {
            shadow: if pit_edge { amount } else { amount * 0.45 },
            highlight: if pit_edge { amount * 0.25 } else { amount },
        }
    }

    fn block_boundary_relief(self, threshold: f32, softness: f32) -> TerrainEdgeRelief {
        let threshold = threshold.max(0.0);
        let softness = softness.max(0.001);
        let center = f32::from(self.center);
        let mut higher_neighbor_delta = 0.0_f32;
        let mut lower_neighbor_delta = 0.0_f32;
        for neighbor in [
            self.north_west,
            self.north,
            self.north_east,
            self.west,
            self.east,
            self.south_west,
            self.south,
            self.south_east,
        ] {
            let delta = f32::from(neighbor) - center;
            higher_neighbor_delta = higher_neighbor_delta.max(delta);
            lower_neighbor_delta = lower_neighbor_delta.max(-delta);
        }

        let max_delta = higher_neighbor_delta.max(lower_neighbor_delta);
        if max_delta <= threshold {
            return TerrainEdgeRelief::default();
        }
        let delta = max_delta - threshold;
        let amount = (delta / (delta + softness)).clamp(0.0, 1.0);
        let pit_edge = higher_neighbor_delta >= lower_neighbor_delta;
        TerrainEdgeRelief {
            shadow: if pit_edge { amount } else { amount * 0.45 },
            highlight: if pit_edge { amount * 0.25 } else { amount },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct TerrainEdgeRelief {
    shadow: f32,
    highlight: f32,
}

#[derive(Debug, Clone, Copy)]
struct BlockBoundaryContext {
    pixel_x: u32,
    pixel_z: u32,
    pixels_per_block: u32,
    blocks_per_pixel: u32,
}

impl BlockBoundaryContext {
    const fn new(job: &RenderJob, pixel_x: u32, pixel_z: u32) -> Self {
        Self {
            pixel_x,
            pixel_z,
            pixels_per_block: job.pixels_per_block,
            blocks_per_pixel: job.scale,
        }
    }
}

struct BlockVolumeContext<'a> {
    pixel_x: u32,
    pixel_z: u32,
    pixels_per_block: u32,
    blocks_per_pixel: u32,
    block_heights: &'a [i32],
    block_aux: &'a [u32],
    grid_width: u32,
    grid_height: u32,
    grid_padding: u32,
}

impl<'a> BlockVolumeContext<'a> {
    const fn new(
        job: &RenderJob,
        pixel_x: u32,
        pixel_z: u32,
        block_heights: &'a [i32],
        block_aux: &'a [u32],
        grid_width: u32,
        grid_height: u32,
        grid_padding: u32,
    ) -> Self {
        Self {
            pixel_x,
            pixel_z,
            pixels_per_block: job.pixels_per_block,
            blocks_per_pixel: job.scale,
            block_heights,
            block_aux,
            grid_width,
            grid_height,
            grid_padding,
        }
    }

    fn height_at(&self, block_x: i32, block_z: i32) -> Option<i32> {
        let x = block_x + i32::try_from(self.grid_padding).ok()?;
        let z = block_z + i32::try_from(self.grid_padding).ok()?;
        if x < 0
            || z < 0
            || x >= i32::try_from(self.grid_width).ok()?
            || z >= i32::try_from(self.grid_height).ok()?
        {
            return None;
        }
        let index = usize::try_from(z)
            .ok()?
            .checked_mul(usize::try_from(self.grid_width).ok()?)?
            .checked_add(usize::try_from(x).ok()?)?;
        let height = *self.block_heights.get(index)?;
        (height != i32::from(MISSING_HEIGHT)).then_some(height)
    }

    fn aux_at(&self, block_x: i32, block_z: i32) -> Option<u32> {
        if self.block_aux.is_empty() {
            return None;
        }
        let x = block_x + i32::try_from(self.grid_padding).ok()?;
        let z = block_z + i32::try_from(self.grid_padding).ok()?;
        if x < 0
            || z < 0
            || x >= i32::try_from(self.grid_width).ok()?
            || z >= i32::try_from(self.grid_height).ok()?
        {
            return None;
        }
        let index = usize::try_from(z)
            .ok()?
            .checked_mul(usize::try_from(self.grid_width).ok()?)?
            .checked_add(usize::try_from(x).ok()?)?;
        self.block_aux.get(index).copied()
    }
}

/// Options that affect surface block rendering.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceRenderOptions {
    /// Blend water over the block below instead of drawing opaque water.
    pub transparent_water: bool,
    /// Apply biome tinting to grass, foliage, and water.
    pub biome_tint: bool,
    /// Apply simple height shading.
    pub height_shading: bool,
    /// Terrain lighting options.
    pub lighting: TerrainLightingOptions,
    /// Per-block 2D boundary and height-contact shadow options.
    pub block_boundaries: BlockBoundaryRenderOptions,
    /// Per-block top-down volume shading options.
    pub block_volume: BlockVolumeRenderOptions,
    /// Deterministic material-atlas and contour options.
    pub atlas: AtlasRenderOptions,
    /// Skip air blocks while searching for a surface block.
    pub skip_air: bool,
    /// Draw unknown blocks with the diagnostic unknown-block color.
    pub render_unknown_blocks: bool,
}

impl Default for SurfaceRenderOptions {
    fn default() -> Self {
        Self {
            transparent_water: true,
            biome_tint: true,
            height_shading: true,
            lighting: TerrainLightingOptions::default(),
            block_boundaries: BlockBoundaryRenderOptions::default(),
            block_volume: BlockVolumeRenderOptions::default(),
            atlas: AtlasRenderOptions::default(),
            skip_air: true,
            render_unknown_blocks: true,
        }
    }
}

/// Worker-thread selection policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderThreadingOptions {
    /// Resolve threads automatically from the execution profile.
    #[default]
    Auto,
    /// Use a fixed number of worker threads.
    Fixed(usize),
    /// Force single-threaded rendering.
    Single,
}

impl RenderThreadingOptions {
    /// Resolves a thread count using the export profile.
    #[must_use]
    pub fn resolve(self, work_items: usize) -> usize {
        self.resolve_unchecked(work_items)
    }

    /// Resolves a thread count without validating a fixed count.
    #[must_use]
    pub fn resolve_unchecked(self, work_items: usize) -> usize {
        self.resolve_for_profile_unchecked(RenderExecutionProfile::Export, work_items)
    }

    /// Resolves a thread count for a profile without validating a fixed count.
    #[must_use]
    pub fn resolve_for_profile_unchecked(
        self,
        profile: RenderExecutionProfile,
        work_items: usize,
    ) -> usize {
        match self {
            Self::Single => 1,
            Self::Fixed(threads) => threads.clamp(1, MAX_RENDER_THREADS),
            Self::Auto => profile.default_auto_threads(work_items),
        }
    }

    /// Resolves and validates a thread count using the export profile.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit fixed thread count is outside `1..=512`.
    pub fn resolve_checked(self, work_items: usize) -> Result<usize> {
        self.resolve_for_profile_checked(RenderExecutionProfile::Export, work_items)
    }

    /// Resolves and validates a thread count for a profile.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit fixed thread count is outside `1..=512`.
    pub fn resolve_for_profile_checked(
        self,
        profile: RenderExecutionProfile,
        work_items: usize,
    ) -> Result<usize> {
        match self {
            Self::Fixed(0) => Err(BedrockRenderError::Validation(
                "thread count must be in 1..=512".to_string(),
            )),
            Self::Fixed(threads) if threads > MAX_RENDER_THREADS => Err(
                BedrockRenderError::Validation("thread count must be in 1..=512".to_string()),
            ),
            _ => Ok(self.resolve_for_profile_unchecked(profile, work_items)),
        }
    }

    /// Resolves a thread count and applies an optional hard cap and reserved foreground capacity.
    ///
    /// # Errors
    ///
    /// Returns an error when a fixed thread count or maximum thread cap is outside `1..=512`,
    /// or when an explicit fixed thread count exceeds the effective cap.
    pub fn resolve_for_profile_with_limits(
        self,
        profile: RenderExecutionProfile,
        work_items: usize,
        max_threads: Option<usize>,
        reserve_threads: usize,
    ) -> Result<usize> {
        let requested = self.resolve_for_profile_checked(profile, work_items)?;
        let mut allowed = MAX_RENDER_THREADS;
        if let Some(max_threads) = max_threads {
            if max_threads == 0 || max_threads > MAX_RENDER_THREADS {
                return Err(BedrockRenderError::Validation(
                    "max thread count must be in 1..=512".to_string(),
                ));
            }
            allowed = allowed.min(max_threads);
        }
        if reserve_threads > 0 {
            let available = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);
            allowed = allowed.min(available.saturating_sub(reserve_threads).max(1));
        }
        if matches!(self, Self::Fixed(_)) && requested > allowed {
            return Err(BedrockRenderError::Validation(format!(
                "thread count {requested} exceeds effective limit {allowed}"
            )));
        }
        Ok(requested.min(allowed).max(1))
    }
}

/// Tile cache behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderCachePolicy {
    /// Read and write cache entries.
    Use,
    /// Skip cache reads, render every requested tile, and write rendered entries.
    Refresh,
    /// Skip cache lookups and writes.
    #[default]
    Bypass,
}

/// Shared cancellation flag for long render operations.
#[derive(Debug, Clone, Default)]
pub struct RenderCancelFlag(Arc<AtomicBool>);

impl RenderCancelFlag {
    /// Creates a new unset cancellation flag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the flag as cancelled.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Shared control handle for render-owned tasks.
#[derive(Debug, Clone, Default)]
pub struct RenderTaskControl {
    cancel: RenderCancelFlag,
    paused: Arc<AtomicBool>,
}

impl RenderTaskControl {
    /// Creates a new active task control.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cancellation flag used by render options.
    #[must_use]
    pub fn cancel_flag(&self) -> RenderCancelFlag {
        self.cancel.clone()
    }

    /// Requests task cancellation.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Pauses cooperative render work.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    /// Resumes cooperative render work.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Returns whether this control is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Direct LevelDB-backed render source for map tile metadata and sessions.
#[derive(Clone)]
pub struct LevelDbRenderSource {
    world_path: PathBuf,
    world: Arc<BedrockWorld<BedrockLevelDbStorage>>,
    full_render_chunk_index: Arc<OnceLock<Arc<[ChunkPos]>>>,
}

impl std::fmt::Debug for LevelDbRenderSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LevelDbRenderSource")
            .field("world_path", &self.world_path)
            .finish_non_exhaustive()
    }
}

impl LevelDbRenderSource {
    /// Opens a Bedrock world database for read-only render access.
    ///
    /// # Errors
    ///
    /// Returns an error if the `db` directory cannot be opened.
    pub fn open_read_only(world_path: impl AsRef<Path>) -> Result<Self> {
        let world_path = world_path.as_ref().to_path_buf();
        let storage = BedrockLevelDbStorage::open_read_only_best_effort(world_path.join("db"))?;
        let world = Arc::new(BedrockWorld::from_typed_storage(
            world_path.clone(),
            storage,
            WorldOpenOptions::default(),
        ));
        Ok(Self {
            world_path,
            world,
            full_render_chunk_index: Arc::new(OnceLock::new()),
        })
    }
}

impl RenderChunkSource for LevelDbRenderSource {
    fn list_render_chunk_positions_blocking(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        Ok(self.full_render_chunk_index(&options)?.as_ref().to_vec())
    }

    fn list_chunk_positions_in_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let width = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
        let height = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
        let area = usize::try_from(width.saturating_mul(height))
            .map_err(|_| BedrockRenderError::Validation("query region is too large".to_string()))?;
        if !should_use_full_render_chunk_index(area) {
            return Ok(self
                .world
                .list_chunk_positions_in_region_blocking(region, options)?);
        }
        Ok(self
            .full_render_chunk_index(&options)?
            .iter()
            .copied()
            .filter(|position| {
                position.dimension == region.dimension
                    && (region.min_chunk_x..=region.max_chunk_x).contains(&position.x)
                    && (region.min_chunk_z..=region.max_chunk_z).contains(&position.z)
            })
            .collect())
    }

    fn query_chunk_region_blocking(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData> {
        Ok(self.world.query_chunk_region_blocking(region, options)?)
    }

    fn query_chunk_data_with_stats_blocking(
        &self,
        positions: &[ChunkPos],
        options: ChunkLoadOptions,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        Ok(self
            .world
            .query_chunk_data_with_stats_blocking(positions.iter().copied(), options)?)
    }

    fn query_chunk_data_blocking(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        Ok(self.world.query_chunk_data_blocking(pos, options)?)
    }
}

impl LevelDbRenderSource {
    fn full_render_chunk_index(&self, options: &WorldScanOptions) -> Result<Arc<[ChunkPos]>> {
        if let Some(index) = self.full_render_chunk_index.get() {
            return Ok(Arc::clone(index));
        }
        let index = Arc::<[ChunkPos]>::from(self.scan_render_chunk_positions(None, options)?);
        if self.full_render_chunk_index.set(Arc::clone(&index)).is_ok() {
            Ok(index)
        } else {
            Ok(self.full_render_chunk_index.get().map_or(index, Arc::clone))
        }
    }

    fn scan_render_chunk_positions(
        &self,
        region: Option<ChunkRegion>,
        options: &WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        if options
            .cancel
            .as_ref()
            .is_some_and(WorldCancelFlag::is_cancelled)
        {
            return Err(BedrockRenderError::Cancelled);
        }
        let scan_options = StorageReadOptions {
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
            pipeline: StoragePipelineOptions {
                queue_depth: options.pipeline.queue_depth,
                table_batch_size: options.pipeline.chunk_batch_size,
                progress_interval: options.pipeline.progress_interval,
            },
            cancel: options
                .cancel
                .as_ref()
                .map(WorldCancelFlag::to_storage_cancel),
            progress: None,
        };
        let (_, partitions) = self.world.storage_backend().scan_keys_partitioned(
            scan_options,
            || Vec::<ChunkPos>::with_capacity(4096),
            |positions, key| {
                if options
                    .cancel
                    .as_ref()
                    .is_some_and(WorldCancelFlag::is_cancelled)
                {
                    return Ok(StorageVisitorControl::Stop);
                }
                if let bedrock_world::BedrockDbKey::Chunk(chunk_key) =
                    bedrock_world::BedrockDbKey::decode(key)
                {
                    let position = chunk_key.pos;
                    if chunk_key.tag.is_render_chunk_record()
                        && region
                            .is_none_or(|region| render_chunk_region_contains(region, position))
                    {
                        positions.push(position);
                    }
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        if options
            .cancel
            .as_ref()
            .is_some_and(WorldCancelFlag::is_cancelled)
        {
            return Err(BedrockRenderError::Cancelled);
        }
        let total_positions = partitions.iter().map(Vec::len).sum();
        let mut positions = Vec::with_capacity(total_positions);
        positions.extend(partitions.into_iter().flatten());
        positions.sort_unstable();
        positions.dedup();
        Ok(positions)
    }
}

/// Callback sink for tile render progress.
#[derive(Clone)]
pub struct RenderProgressSink {
    inner: Arc<dyn Fn(RenderProgress) + Send + Sync>,
}

impl std::fmt::Debug for RenderProgressSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderProgressSink")
            .finish_non_exhaustive()
    }
}

impl RenderProgressSink {
    /// Creates a new progress sink from a callback.
    #[must_use]
    pub fn new(callback: impl Fn(RenderProgress) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(callback),
        }
    }

    fn emit(&self, progress: RenderProgress) {
        (self.inner)(progress);
    }
}

/// Progress update emitted during batch rendering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderProgress {
    /// Number of tiles completed.
    pub completed_tiles: usize,
    /// Total number of tiles in the operation.
    pub total_tiles: usize,
}

/// Diagnostic counters emitted during rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderDiagnostics {
    /// Number of sampled positions whose chunk was missing.
    pub missing_chunks: usize,
    /// Number of sampled positions whose height map was missing.
    pub missing_heightmaps: usize,
    /// Number of pixels drawn from unknown block names.
    pub unknown_blocks: usize,
    /// Unknown block counts grouped by block name.
    pub unknown_blocks_by_name: BTreeMap<String, usize>,
    /// Number of pixels that used fallback rendering.
    pub fallback_pixels: usize,
    /// Number of transparent pixels emitted.
    pub transparent_pixels: usize,
    /// Number of purple diagnostic pixels emitted.
    pub purple_error_pixels: usize,
    /// Number of chunks baked while rendering.
    pub baked_chunks: usize,
    /// Number of cache hits.
    pub cache_hits: usize,
    /// Number of cache misses.
    pub cache_misses: usize,
}

impl RenderDiagnostics {
    /// Adds another diagnostics value into this one with saturating counters.
    pub fn add(&mut self, other: Self) {
        self.missing_chunks = self.missing_chunks.saturating_add(other.missing_chunks);
        self.missing_heightmaps = self
            .missing_heightmaps
            .saturating_add(other.missing_heightmaps);
        self.unknown_blocks = self.unknown_blocks.saturating_add(other.unknown_blocks);
        for (name, count) in other.unknown_blocks_by_name {
            *self.unknown_blocks_by_name.entry(name).or_default() += count;
        }
        self.fallback_pixels = self.fallback_pixels.saturating_add(other.fallback_pixels);
        self.transparent_pixels = self
            .transparent_pixels
            .saturating_add(other.transparent_pixels);
        self.purple_error_pixels = self
            .purple_error_pixels
            .saturating_add(other.purple_error_pixels);
        self.baked_chunks = self.baked_chunks.saturating_add(other.baked_chunks);
        self.cache_hits = self.cache_hits.saturating_add(other.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(other.cache_misses);
    }

    fn record_unknown_block(&mut self, name: &str) {
        self.unknown_blocks = self.unknown_blocks.saturating_add(1);
        self.purple_error_pixels = self.purple_error_pixels.saturating_add(1);
        *self
            .unknown_blocks_by_name
            .entry(name.to_string())
            .or_default() += 1;
    }

    fn record_transparent_pixel(&mut self) {
        self.fallback_pixels = self.fallback_pixels.saturating_add(1);
        self.transparent_pixels = self.transparent_pixels.saturating_add(1);
    }
}

/// Callback sink for aggregated render diagnostics.
#[derive(Clone)]
pub struct RenderDiagnosticsSink {
    inner: Arc<dyn Fn(RenderDiagnostics) + Send + Sync>,
}

impl std::fmt::Debug for RenderDiagnosticsSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderDiagnosticsSink")
            .finish_non_exhaustive()
    }
}

impl RenderDiagnosticsSink {
    /// Creates a new diagnostics sink from a callback.
    #[must_use]
    pub fn new(callback: impl Fn(RenderDiagnostics) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(callback),
        }
    }

    fn emit(&self, diagnostics: RenderDiagnostics) {
        (self.inner)(diagnostics);
    }
}

/// Rendered tile pixels and optional encoded image bytes.
#[derive(Debug, Clone)]
pub struct TileImage {
    /// Tile coordinate rendered by this image.
    pub coord: TileCoord,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Raw RGBA bytes shared between stream consumers and caches.
    pub rgba: Arc<[u8]>,
    /// Encoded image bytes when an encoded format was requested.
    pub encoded: Option<Vec<u8>>,
}

/// Decoded tile pixels emitted by the interactive v2 tile stream.
#[derive(Debug, Clone)]
pub struct DecodedTileImage {
    /// Tile coordinate rendered by this image.
    pub coord: TileCoord,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Raw decoded pixel bytes in `pixel_format` order.
    pub pixels: Arc<[u8]>,
    /// Byte order used by `pixels`.
    pub pixel_format: TilePixelFormat,
}

/// Decoded bytes from a `FastRgbaZstd` tile cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastRgbaZstdTile {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Optional fast validation value stored in the cache header.
    pub validation_value: Option<u64>,
    /// Raw RGBA bytes.
    pub rgba: Vec<u8>,
}

struct FastRgbaZstdSharedTile {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}

/// Header metadata from a `FastRgbaZstd` tile cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastRgbaZstdHeader {
    /// Encoded cache format version.
    pub version: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Expected raw RGBA byte length.
    pub rgba_len: u64,
    /// Header byte length before the zstd payload.
    pub header_len: usize,
    /// Validation kind stored in the cache entry.
    pub validation_kind: u32,
    /// Format flags reserved for future use.
    pub flags: u32,
    /// Optional fast validation value stored in the cache header.
    pub validation_value: Option<u64>,
}

impl FastRgbaZstdHeader {
    /// Returns true when the cache entry explicitly contains visible pixels.
    #[must_use]
    pub const fn is_non_empty(self) -> bool {
        self.flags & FAST_RGBA_ZSTD_FLAG_NON_EMPTY != 0
    }

    /// Returns true when the cache entry explicitly represents an empty tile.
    #[must_use]
    pub const fn is_empty_negative(self) -> bool {
        self.flags & FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE != 0
    }

    /// Returns true for older entries written before content flags existed.
    #[must_use]
    pub const fn has_legacy_content_flags(self) -> bool {
        self.flags & FAST_RGBA_ZSTD_KNOWN_FLAGS == 0
    }
}

/// Collection of rendered tiles.
#[derive(Debug, Clone)]
pub struct TileSet {
    /// Rendered tile images.
    pub tiles: Vec<TileImage>,
}

/// Options used when baking a chunk before tile composition.
#[derive(Debug, Clone)]
pub struct BakeOptions {
    /// Render mode to bake.
    pub mode: RenderMode,
    /// Surface render behavior used for surface bakes.
    pub surface: SurfaceRenderOptions,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            mode: RenderMode::SurfaceBlocks,
            surface: SurfaceRenderOptions::default(),
        }
    }
}

/// Diagnostics returned by bake operations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BakeDiagnostics {
    /// Render diagnostics collected while baking.
    pub render: RenderDiagnostics,
}

/// Baked data for one chunk.
#[derive(Debug, Clone)]
pub struct ChunkBake {
    /// Chunk position.
    pub pos: ChunkPos,
    /// Render mode used for this bake.
    pub mode: RenderMode,
    /// Baked payload.
    pub payload: ChunkBakePayload,
    /// Diagnostics collected while baking.
    pub diagnostics: RenderDiagnostics,
}

/// Two-dimensional RGBA color plane.
#[derive(Debug, Clone)]
pub struct RgbaPlane {
    /// Plane width.
    pub width: u32,
    /// Plane height.
    pub height: u32,
    /// RGBA colors in row-major order.
    pub pixels: Vec<RgbaColor>,
}

/// Two-dimensional height plane.
#[derive(Debug, Clone)]
pub struct HeightPlane {
    /// Plane width.
    pub width: u32,
    /// Plane height.
    pub height: u32,
    /// Heights in row-major order.
    pub heights: Vec<i16>,
}

/// Two-dimensional water-depth plane.
#[derive(Debug, Clone)]
pub struct DepthPlane {
    /// Plane width.
    pub width: u32,
    /// Plane height.
    pub height: u32,
    /// Depth values in row-major order.
    pub depths: Vec<u8>,
}

/// Baked surface data for color, height, relief, and water depth.
#[derive(Debug, Clone)]
pub struct SurfacePlane {
    /// Surface colors.
    pub colors: RgbaPlane,
    /// Surface heights.
    pub heights: HeightPlane,
    /// Heights used for terrain relief, such as seabed heights under water.
    pub relief_heights: HeightPlane,
    /// Water depths for transparent-water rendering.
    pub water_depths: DepthPlane,
}

/// Baked CPU atlas surface G-buffer.
#[derive(Debug, Clone)]
pub struct SurfacePlaneAtlas {
    /// Base atlas albedo after biome tint and overlays.
    pub colors: RgbaPlane,
    /// Surface heights.
    pub heights: HeightPlane,
    /// Heights used for terrain relief, such as seabed heights under water.
    pub relief_heights: HeightPlane,
    /// Water depths for transparent-water rendering.
    pub water_depths: DepthPlane,
    /// Material bucket for each source block.
    pub materials: DepthPlane,
    /// Per-block shape flags.
    pub shape_flags: DepthPlane,
    /// Overlay alpha retained from the surface column.
    pub overlay_alpha: DepthPlane,
}

/// Payload produced by baking a chunk.
#[derive(Debug, Clone)]
pub enum ChunkBakePayload {
    /// Simple color plane.
    Colors(RgbaPlane),
    /// Surface render planes.
    Surface(SurfacePlane),
    /// CPU atlas surface G-buffer planes.
    SurfaceAtlas(SurfacePlaneAtlas),
    /// Height-map colors and source heights.
    HeightMap {
        /// Height-map colors.
        colors: RgbaPlane,
        /// Source heights.
        heights: HeightPlane,
    },
}

/// Payload produced by baking a region.
#[derive(Debug, Clone)]
pub enum RegionBakePayload {
    /// Simple color plane.
    Colors(RgbaPlane),
    /// Surface render planes.
    Surface(SurfacePlane),
    /// CPU atlas surface G-buffer planes.
    SurfaceAtlas(SurfacePlaneAtlas),
    /// Height-map colors and source heights.
    HeightMap {
        /// Height-map colors.
        colors: RgbaPlane,
        /// Source heights.
        heights: HeightPlane,
    },
}

/// Baked data for one region.
#[derive(Debug, Clone)]
pub struct RegionBake {
    /// Region coordinate.
    pub coord: RegionCoord,
    /// Region layout.
    pub layout: RegionLayout,
    /// Render mode used for this bake.
    pub mode: RenderMode,
    /// Inclusive chunk bounds that have real baked payload data.
    pub covered_chunk_region: ChunkRegion,
    /// Inclusive chunk bounds used as the payload coordinate origin.
    pub chunk_region: ChunkRegion,
    /// Baked region payload.
    pub payload: RegionBakePayload,
    /// Diagnostics collected while baking.
    pub diagnostics: RenderDiagnostics,
    /// World record-load stats collected while baking.
    pub load_stats: Option<ChunkLoadStats>,
    /// Time spent copying baked chunk payloads into the region payload, in milliseconds.
    pub copy_ms: u128,
    /// Baked chunks copied into this region payload.
    pub chunks_copied: usize,
    /// Baked chunks skipped because their position was outside this region payload.
    pub chunks_out_of_bounds: usize,
}

#[derive(Debug, Clone, Default)]
struct TileComposeStats {
    backend: ResolvedRenderBackend,
    tile_missing_region_samples: usize,
    cpu_tiles: usize,
    gpu: RenderGpuDiagnostics,
}

/// Per-operation GPU diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderGpuDiagnostics {
    /// Requested GPU backend.
    pub requested_backend: RenderGpuBackend,
    /// Actual backend selected by wgpu.
    pub actual_backend: RenderGpuBackend,
    /// Adapter name reported by wgpu.
    pub adapter_name: Option<String>,
    /// Device name reported by wgpu.
    pub device_name: Option<String>,
    /// Last fallback reason, if CPU fallback was used.
    pub fallback_reason: Option<String>,
    /// Tiles successfully processed by GPU.
    pub tiles: usize,
    /// Queue wait time in milliseconds.
    pub queue_wait_ms: u128,
    /// GPU preparation time in milliseconds.
    pub prepare_ms: u128,
    /// Upload time in milliseconds.
    pub upload_ms: u128,
    /// Dispatch time in milliseconds.
    pub dispatch_ms: u128,
    /// Readback time in milliseconds.
    pub readback_ms: u128,
    /// Bytes uploaded to GPU.
    pub uploaded_bytes: usize,
    /// Bytes read back from GPU.
    pub readback_bytes: usize,
    /// Peak in-flight GPU operations.
    pub peak_in_flight: usize,
    /// Buffer pool reuse count.
    pub buffer_reuses: usize,
}

struct PreparedTileCompose {
    colors: Vec<u32>,
    heights: Vec<i32>,
    water_depths: Vec<u32>,
    atlas_enabled: bool,
    block_heights: Vec<i32>,
    block_aux: Vec<u32>,
    block_height_grid_width: u32,
    block_height_grid_height: u32,
    block_height_grid_padding: u32,
    block_volume_enabled: bool,
    diagnostics: RenderDiagnostics,
    lighting_enabled: bool,
}

impl TileComposeStats {
    fn cpu() -> Self {
        Self {
            backend: ResolvedRenderBackend::Cpu,
            cpu_tiles: 1,
            ..Self::default()
        }
    }

    fn gpu(diagnostics: RenderGpuDiagnostics) -> Self {
        Self {
            backend: resolved_backend_from_gpu(diagnostics.actual_backend),
            gpu: diagnostics,
            ..Self::default()
        }
    }

    fn add(&mut self, other: Self) {
        if self.cpu_tiles == 0 && self.gpu.tiles == 0 {
            self.backend = other.backend;
        } else {
            self.backend = merge_backends(self.backend, other.backend);
        }
        self.tile_missing_region_samples = self
            .tile_missing_region_samples
            .saturating_add(other.tile_missing_region_samples);
        self.cpu_tiles = self.cpu_tiles.saturating_add(other.cpu_tiles);
        self.gpu.add(other.gpu);
    }
}

fn merge_backends(
    left: ResolvedRenderBackend,
    right: ResolvedRenderBackend,
) -> ResolvedRenderBackend {
    if left == right {
        left
    } else {
        ResolvedRenderBackend::Mixed
    }
}

fn resolved_backend_from_gpu(backend: RenderGpuBackend) -> ResolvedRenderBackend {
    match backend {
        RenderGpuBackend::Dx11 => ResolvedRenderBackend::Dx11,
        RenderGpuBackend::Dx12 => ResolvedRenderBackend::WgpuDx12,
        RenderGpuBackend::Vulkan => ResolvedRenderBackend::WgpuVulkan,
        RenderGpuBackend::Auto => ResolvedRenderBackend::Mixed,
    }
}

impl RenderGpuDiagnostics {
    fn add(&mut self, other: Self) {
        if other.tiles != 0
            && (self.tiles == 0
                || matches!(self.actual_backend, RenderGpuBackend::Auto)
                || matches!(self.requested_backend, RenderGpuBackend::Auto))
        {
            self.requested_backend = other.requested_backend;
            self.actual_backend = other.actual_backend;
            self.adapter_name.clone_from(&other.adapter_name);
            self.device_name.clone_from(&other.device_name);
        }
        if other.fallback_reason.is_some() {
            self.fallback_reason = other.fallback_reason;
        }
        self.tiles = self.tiles.saturating_add(other.tiles);
        self.queue_wait_ms = self.queue_wait_ms.saturating_add(other.queue_wait_ms);
        self.prepare_ms = self.prepare_ms.saturating_add(other.prepare_ms);
        self.upload_ms = self.upload_ms.saturating_add(other.upload_ms);
        self.dispatch_ms = self.dispatch_ms.saturating_add(other.dispatch_ms);
        self.readback_ms = self.readback_ms.saturating_add(other.readback_ms);
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(other.uploaded_bytes);
        self.readback_bytes = self.readback_bytes.saturating_add(other.readback_bytes);
        self.peak_in_flight = self.peak_in_flight.max(other.peak_in_flight);
        self.buffer_reuses = self.buffer_reuses.saturating_add(other.buffer_reuses);
    }
}

impl RenderPipelineStats {
    fn add_tile_compose_stats(&mut self, stats: TileComposeStats) {
        if self.cpu_tiles == 0 && self.gpu_tiles == 0 {
            self.resolved_backend = stats.backend;
        } else {
            self.resolved_backend = merge_backends(self.resolved_backend, stats.backend);
        }
        self.tile_missing_region_samples = self
            .tile_missing_region_samples
            .saturating_add(stats.tile_missing_region_samples);
        self.cpu_tiles = self.cpu_tiles.saturating_add(stats.cpu_tiles);
        self.add_gpu_diagnostics(stats.gpu);
    }

    fn add_pipeline_stats(&mut self, stats: Self) {
        self.planned_tiles = self.planned_tiles.saturating_add(stats.planned_tiles);
        self.planned_regions = self.planned_regions.saturating_add(stats.planned_regions);
        self.unique_chunks = self.unique_chunks.saturating_add(stats.unique_chunks);
        self.baked_chunks = self.baked_chunks.saturating_add(stats.baked_chunks);
        self.baked_regions = self.baked_regions.saturating_add(stats.baked_regions);
        self.region_chunks_copied = self
            .region_chunks_copied
            .saturating_add(stats.region_chunks_copied);
        self.region_chunks_out_of_bounds = self
            .region_chunks_out_of_bounds
            .saturating_add(stats.region_chunks_out_of_bounds);
        self.tile_missing_region_samples = self
            .tile_missing_region_samples
            .saturating_add(stats.tile_missing_region_samples);
        self.cache_hits = self.cache_hits.saturating_add(stats.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(stats.cache_misses);
        self.cache_probes = self.cache_probes.saturating_add(stats.cache_probes);
        self.cache_validation_mismatches = self
            .cache_validation_mismatches
            .saturating_add(stats.cache_validation_mismatches);
        self.cache_read_ms = self.cache_read_ms.saturating_add(stats.cache_read_ms);
        self.cache_decode_ms = self.cache_decode_ms.saturating_add(stats.cache_decode_ms);
        self.tile_blob_decode_ms = self
            .tile_blob_decode_ms
            .saturating_add(stats.tile_blob_decode_ms);
        self.region_cache_hits = self
            .region_cache_hits
            .saturating_add(stats.region_cache_hits);
        self.region_cache_misses = self
            .region_cache_misses
            .saturating_add(stats.region_cache_misses);
        self.tile_index_trusted_hits = self
            .tile_index_trusted_hits
            .saturating_add(stats.tile_index_trusted_hits);
        self.tile_index_validated_hits = self
            .tile_index_validated_hits
            .saturating_add(stats.tile_index_validated_hits);
        self.tile_index_misses = self
            .tile_index_misses
            .saturating_add(stats.tile_index_misses);
        self.tile_index_empty_hits = self
            .tile_index_empty_hits
            .saturating_add(stats.tile_index_empty_hits);
        self.tile_index_read_ms = self
            .tile_index_read_ms
            .saturating_add(stats.tile_index_read_ms);
        self.tile_dep_validation_ms = self
            .tile_dep_validation_ms
            .saturating_add(stats.tile_dep_validation_ms);
        self.tile_cache_writer_dropped = self
            .tile_cache_writer_dropped
            .saturating_add(stats.tile_cache_writer_dropped);
        self.world_signature_trusted |= stats.world_signature_trusted;
        self.world_signature_changed |= stats.world_signature_changed;
        self.index_corrupt_misses = self
            .index_corrupt_misses
            .saturating_add(stats.index_corrupt_misses);
        self.bake_ms = self.bake_ms.saturating_add(stats.bake_ms);
        self.world_load_ms = self.world_load_ms.saturating_add(stats.world_load_ms);
        self.region_bake_ms = self.region_bake_ms.saturating_add(stats.region_bake_ms);
        self.region_copy_ms = self.region_copy_ms.saturating_add(stats.region_copy_ms);
        self.tile_compose_ms = self.tile_compose_ms.saturating_add(stats.tile_compose_ms);
        self.encode_ms = self.encode_ms.saturating_add(stats.encode_ms);
        self.write_ms = self.write_ms.saturating_add(stats.write_ms);
        self.worker_idle_ms = self.worker_idle_ms.saturating_add(stats.worker_idle_ms);
        self.queue_wait_ms = self.queue_wait_ms.saturating_add(stats.queue_wait_ms);
        self.cpu_queue_wait_ms = self
            .cpu_queue_wait_ms
            .saturating_add(stats.cpu_queue_wait_ms);
        self.render_prefix_scans = self
            .render_prefix_scans
            .saturating_add(stats.render_prefix_scans);
        self.exact_get_batches = self
            .exact_get_batches
            .saturating_add(stats.exact_get_batches);
        self.exact_keys_requested = self
            .exact_keys_requested
            .saturating_add(stats.exact_keys_requested);
        self.exact_keys_found = self.exact_keys_found.saturating_add(stats.exact_keys_found);
        self.db_read_ms = self.db_read_ms.saturating_add(stats.db_read_ms);
        self.decode_ms = self.decode_ms.saturating_add(stats.decode_ms);
        self.cpu_decode_ms = self.cpu_decode_ms.saturating_add(stats.cpu_decode_ms);
        self.cpu_frame_pack_ms = self
            .cpu_frame_pack_ms
            .saturating_add(stats.cpu_frame_pack_ms);
        self.chunk_frame_decode_ms = self
            .chunk_frame_decode_ms
            .saturating_add(stats.chunk_frame_decode_ms);
        self.biome_parse_ms = self.biome_parse_ms.saturating_add(stats.biome_parse_ms);
        self.subchunk_parse_ms = self
            .subchunk_parse_ms
            .saturating_add(stats.subchunk_parse_ms);
        self.surface_scan_ms = self.surface_scan_ms.saturating_add(stats.surface_scan_ms);
        self.block_entity_parse_ms = self
            .block_entity_parse_ms
            .saturating_add(stats.block_entity_parse_ms);
        self.full_reload_ms = self.full_reload_ms.saturating_add(stats.full_reload_ms);
        self.world_worker_threads = self.world_worker_threads.max(stats.world_worker_threads);
        self.peak_cache_bytes = self.peak_cache_bytes.max(stats.peak_cache_bytes);
        self.active_tasks_peak = self.active_tasks_peak.max(stats.active_tasks_peak);
        self.peak_worker_threads = self.peak_worker_threads.max(stats.peak_worker_threads);
        if stats.cpu_tiles != 0 || stats.gpu_tiles != 0 {
            if self.cpu_tiles == 0 && self.gpu_tiles == 0 {
                self.resolved_backend = stats.resolved_backend;
            } else {
                self.resolved_backend =
                    merge_backends(self.resolved_backend, stats.resolved_backend);
            }
        }
        self.cpu_tiles = self.cpu_tiles.saturating_add(stats.cpu_tiles);
        self.gpu_tiles = self.gpu_tiles.saturating_add(stats.gpu_tiles);
        self.gpu_requested_backend = stats.gpu_requested_backend;
        self.gpu_actual_backend = stats.gpu_actual_backend;
        if stats.gpu_adapter_name.is_some() {
            self.gpu_adapter_name = stats.gpu_adapter_name;
        }
        if stats.gpu_device_name.is_some() {
            self.gpu_device_name = stats.gpu_device_name;
        }
        if stats.gpu_fallback_reason.is_some() {
            self.gpu_fallback_reason = stats.gpu_fallback_reason;
        }
        self.gpu_queue_wait_ms = self
            .gpu_queue_wait_ms
            .saturating_add(stats.gpu_queue_wait_ms);
        self.gpu_prepare_ms = self.gpu_prepare_ms.saturating_add(stats.gpu_prepare_ms);
        self.gpu_upload_ms = self.gpu_upload_ms.saturating_add(stats.gpu_upload_ms);
        self.gpu_dispatch_ms = self.gpu_dispatch_ms.saturating_add(stats.gpu_dispatch_ms);
        self.gpu_readback_ms = self.gpu_readback_ms.saturating_add(stats.gpu_readback_ms);
        self.gpu_uploaded_bytes = self
            .gpu_uploaded_bytes
            .saturating_add(stats.gpu_uploaded_bytes);
        self.gpu_readback_bytes = self
            .gpu_readback_bytes
            .saturating_add(stats.gpu_readback_bytes);
        self.gpu_peak_in_flight = self.gpu_peak_in_flight.max(stats.gpu_peak_in_flight);
        self.gpu_buffer_reuses = self
            .gpu_buffer_reuses
            .saturating_add(stats.gpu_buffer_reuses);
        self.tiles_per_second = self.tiles_per_second.max(stats.tiles_per_second);
        self.chunks_per_second = self.chunks_per_second.max(stats.chunks_per_second);
        self.cpu_worker_utilization_per_mille = self
            .cpu_worker_utilization_per_mille
            .max(stats.cpu_worker_utilization_per_mille);
    }

    fn add_gpu_diagnostics(&mut self, diagnostics: RenderGpuDiagnostics) {
        if diagnostics.tiles != 0 {
            self.gpu_tiles = self.gpu_tiles.saturating_add(diagnostics.tiles);
            self.gpu_requested_backend = diagnostics.requested_backend;
            self.gpu_actual_backend = diagnostics.actual_backend;
            if diagnostics.adapter_name.is_some() {
                self.gpu_adapter_name = diagnostics.adapter_name;
            }
            if diagnostics.device_name.is_some() {
                self.gpu_device_name = diagnostics.device_name;
            }
        }
        if diagnostics.fallback_reason.is_some() {
            self.gpu_fallback_reason = diagnostics.fallback_reason;
        }
        self.gpu_queue_wait_ms = self
            .gpu_queue_wait_ms
            .saturating_add(diagnostics.queue_wait_ms);
        self.gpu_prepare_ms = self.gpu_prepare_ms.saturating_add(diagnostics.prepare_ms);
        self.gpu_upload_ms = self.gpu_upload_ms.saturating_add(diagnostics.upload_ms);
        self.gpu_dispatch_ms = self.gpu_dispatch_ms.saturating_add(diagnostics.dispatch_ms);
        self.gpu_readback_ms = self.gpu_readback_ms.saturating_add(diagnostics.readback_ms);
        self.gpu_uploaded_bytes = self
            .gpu_uploaded_bytes
            .saturating_add(diagnostics.uploaded_bytes);
        self.gpu_readback_bytes = self
            .gpu_readback_bytes
            .saturating_add(diagnostics.readback_bytes);
        self.gpu_peak_in_flight = self.gpu_peak_in_flight.max(diagnostics.peak_in_flight);
        self.gpu_buffer_reuses = self
            .gpu_buffer_reuses
            .saturating_add(diagnostics.buffer_reuses);
    }

    fn add_render_load_stats(&mut self, stats: &ChunkLoadStats) {
        self.exact_get_batches = self
            .exact_get_batches
            .saturating_add(stats.exact_get_batches);
        self.exact_keys_requested = self
            .exact_keys_requested
            .saturating_add(stats.keys_requested);
        self.exact_keys_found = self.exact_keys_found.saturating_add(stats.keys_found);
        self.render_prefix_scans = self.render_prefix_scans.saturating_add(stats.prefix_scans);
        self.world_load_ms = self.world_load_ms.saturating_add(stats.load_ms);
        self.db_read_ms = self.db_read_ms.saturating_add(stats.db_read_ms);
        self.decode_ms = self.decode_ms.saturating_add(stats.decode_ms);
        self.cpu_decode_ms = self.cpu_decode_ms.saturating_add(stats.decode_ms);
        self.chunk_frame_decode_ms = self.chunk_frame_decode_ms.saturating_add(stats.decode_ms);
        self.biome_parse_ms = self.biome_parse_ms.saturating_add(stats.biome_parse_ms);
        self.subchunk_parse_ms = self
            .subchunk_parse_ms
            .saturating_add(stats.subchunk_parse_ms);
        self.surface_scan_ms = self.surface_scan_ms.saturating_add(stats.surface_scan_ms);
        self.block_entity_parse_ms = self
            .block_entity_parse_ms
            .saturating_add(stats.block_entity_parse_ms);
        self.full_reload_ms = self.full_reload_ms.saturating_add(stats.full_reload_ms);
        self.world_worker_threads = self.world_worker_threads.max(stats.worker_threads);
        self.cpu_queue_wait_ms = self.cpu_queue_wait_ms.saturating_add(stats.queue_wait_ms);
    }
}

fn finalize_pipeline_throughput(stats: &mut RenderPipelineStats, elapsed: Duration) {
    let elapsed_secs = elapsed.as_secs_f64();
    if elapsed_secs > 0.0 {
        stats.tiles_per_second = (stats.planned_tiles as f64 / elapsed_secs) as u64;
        stats.chunks_per_second = (stats.unique_chunks as f64 / elapsed_secs) as u64;
    }

    let elapsed_ms = elapsed.as_millis();
    if elapsed_ms == 0 || stats.peak_worker_threads == 0 {
        return;
    }

    let active_ms = stats
        .world_load_ms
        .saturating_add(stats.decode_ms)
        .saturating_add(stats.region_bake_ms)
        .saturating_add(stats.tile_compose_ms)
        .saturating_add(stats.encode_ms)
        .saturating_add(stats.write_ms);
    let capacity_ms = elapsed_ms.saturating_mul(stats.peak_worker_threads as u128);
    if capacity_ms > 0 {
        stats.cpu_worker_utilization_per_mille = active_ms
            .saturating_mul(1000)
            .saturating_div(capacity_ms)
            .min(1000) as u16;
    }
}

impl RgbaPlane {
    fn new(width: u32, height: u32, fill: RgbaColor) -> Result<Self> {
        Ok(Self {
            width,
            height,
            pixels: vec![fill; plane_len(width, height)?],
        })
    }

    fn color_at(&self, pixel_x: u32, pixel_z: u32) -> Option<RgbaColor> {
        plane_index(self.width, self.height, pixel_x, pixel_z)
            .and_then(|index| self.pixels.get(index).copied())
    }

    fn set_color(&mut self, pixel_x: u32, pixel_z: u32, color: RgbaColor) {
        if let Some(index) = plane_index(self.width, self.height, pixel_x, pixel_z)
            && let Some(pixel) = self.pixels.get_mut(index)
        {
            *pixel = color;
        }
    }

    fn copy_16x16_from(&mut self, dst_x: u32, dst_z: u32, source: &Self) -> bool {
        if source.width != 16 || source.height != 16 {
            return false;
        }
        let Ok(dst_x) = usize::try_from(dst_x) else {
            return false;
        };
        let Ok(dst_z) = usize::try_from(dst_z) else {
            return false;
        };
        let Ok(width) = usize::try_from(self.width) else {
            return false;
        };
        let Ok(height) = usize::try_from(self.height) else {
            return false;
        };
        if dst_x.saturating_add(16) > width || dst_z.saturating_add(16) > height {
            return false;
        }
        for row in 0..16 {
            let dst_start = (dst_z + row) * width + dst_x;
            let src_start = row * 16;
            self.pixels[dst_start..dst_start + 16]
                .copy_from_slice(&source.pixels[src_start..src_start + 16]);
        }
        true
    }
}

impl HeightPlane {
    fn new(width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            width,
            height,
            heights: vec![MISSING_HEIGHT; plane_len(width, height)?],
        })
    }

    fn height_at(&self, pixel_x: u32, pixel_z: u32) -> Option<i16> {
        let height = plane_index(self.width, self.height, pixel_x, pixel_z)
            .and_then(|index| self.heights.get(index).copied())?;
        (height != MISSING_HEIGHT).then_some(height)
    }

    fn set_height(&mut self, pixel_x: u32, pixel_z: u32, height: i16) {
        if let Some(index) = plane_index(self.width, self.height, pixel_x, pixel_z)
            && let Some(pixel) = self.heights.get_mut(index)
        {
            *pixel = height;
        }
    }
}

impl DepthPlane {
    fn new(width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            width,
            height,
            depths: vec![0; plane_len(width, height)?],
        })
    }

    fn depth_at(&self, pixel_x: u32, pixel_z: u32) -> u8 {
        plane_index(self.width, self.height, pixel_x, pixel_z)
            .and_then(|index| self.depths.get(index).copied())
            .unwrap_or(0)
    }

    fn set_depth(&mut self, pixel_x: u32, pixel_z: u32, depth: u8) {
        if let Some(index) = plane_index(self.width, self.height, pixel_x, pixel_z)
            && let Some(pixel) = self.depths.get_mut(index)
        {
            *pixel = depth;
        }
    }
}

impl RegionBake {
    fn color_at_chunk_local(&self, chunk: ChunkPos, local_x: u8, local_z: u8) -> Option<RgbaColor> {
        let (pixel_x, pixel_z) = self.region_pixel(chunk, local_x, local_z)?;
        self.color_at_region_pixel(pixel_x, pixel_z)
    }

    fn height_at_chunk_local(&self, chunk: ChunkPos, local_x: u8, local_z: u8) -> Option<i16> {
        let (pixel_x, pixel_z) = self.region_pixel(chunk, local_x, local_z)?;
        self.height_at_region_pixel(pixel_x, pixel_z)
    }

    fn region_pixel(&self, chunk: ChunkPos, local_x: u8, local_z: u8) -> Option<(u32, u32)> {
        let (region_x, region_z) = chunk_region_pixel_offset(self.chunk_region, chunk)?;
        let pixel_x = region_x.checked_add(u32::from(local_x))?;
        let pixel_z = region_z.checked_add(u32::from(local_z))?;
        Some((pixel_x, pixel_z))
    }

    fn color_at_region_pixel(&self, pixel_x: u32, pixel_z: u32) -> Option<RgbaColor> {
        match &self.payload {
            RegionBakePayload::Colors(plane) => plane.color_at(pixel_x, pixel_z),
            RegionBakePayload::Surface(plane) => plane.colors.color_at(pixel_x, pixel_z),
            RegionBakePayload::SurfaceAtlas(plane) => plane.colors.color_at(pixel_x, pixel_z),
            RegionBakePayload::HeightMap { colors, .. } => colors.color_at(pixel_x, pixel_z),
        }
    }

    fn height_at_region_pixel(&self, pixel_x: u32, pixel_z: u32) -> Option<i16> {
        match &self.payload {
            RegionBakePayload::Surface(plane) => plane.relief_heights.height_at(pixel_x, pixel_z),
            RegionBakePayload::SurfaceAtlas(plane) => {
                plane.relief_heights.height_at(pixel_x, pixel_z)
            }
            RegionBakePayload::HeightMap { heights, .. } => heights.height_at(pixel_x, pixel_z),
            RegionBakePayload::Colors(_) => None,
        }
    }

    fn water_depth_at_region_pixel(&self, pixel_x: u32, pixel_z: u32) -> u8 {
        match &self.payload {
            RegionBakePayload::Surface(plane) => plane.water_depths.depth_at(pixel_x, pixel_z),
            RegionBakePayload::SurfaceAtlas(plane) => plane.water_depths.depth_at(pixel_x, pixel_z),
            RegionBakePayload::Colors(_) | RegionBakePayload::HeightMap { .. } => 0,
        }
    }

    fn atlas_aux_at_region_pixel(&self, pixel_x: u32, pixel_z: u32) -> u32 {
        match &self.payload {
            RegionBakePayload::SurfaceAtlas(plane) => pack_atlas_aux(
                plane.water_depths.depth_at(pixel_x, pixel_z),
                SurfaceMaterialId::from_u8(plane.materials.depth_at(pixel_x, pixel_z)),
                plane.shape_flags.depth_at(pixel_x, pixel_z),
                plane.overlay_alpha.depth_at(pixel_x, pixel_z),
            ),
            _ => u32::from(self.water_depth_at_region_pixel(pixel_x, pixel_z)),
        }
    }
}

fn region_key_for_chunk(pos: ChunkPos, layout: RegionLayout, mode: RenderMode) -> RegionBakeKey {
    RegionBakeKey {
        coord: RegionCoord::from_chunk(pos, layout),
        mode,
    }
}

fn chunk_region_pixel_offset(region: ChunkRegion, chunk: ChunkPos) -> Option<(u32, u32)> {
    if chunk.dimension != region.dimension {
        return None;
    }
    let relative_chunk_x = chunk.x.checked_sub(region.min_chunk_x)?;
    let relative_chunk_z = chunk.z.checked_sub(region.min_chunk_z)?;
    let chunk_width = region
        .max_chunk_x
        .checked_sub(region.min_chunk_x)?
        .checked_add(1)?;
    let chunk_height = region
        .max_chunk_z
        .checked_sub(region.min_chunk_z)?
        .checked_add(1)?;
    if relative_chunk_x < 0
        || relative_chunk_z < 0
        || relative_chunk_x >= chunk_width
        || relative_chunk_z >= chunk_height
    {
        return None;
    }
    let pixel_x = u32::try_from(relative_chunk_x).ok()?.checked_mul(16)?;
    let pixel_z = u32::try_from(relative_chunk_z).ok()?.checked_mul(16)?;
    Some((pixel_x, pixel_z))
}

fn plane_len(width: u32, height: u32) -> Result<usize> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| BedrockRenderError::Validation("plane pixel count overflow".to_string()))
}

fn plane_index(width: u32, height: u32, pixel_x: u32, pixel_z: u32) -> Option<usize> {
    if pixel_x >= width || pixel_z >= height {
        return None;
    }
    usize::try_from(pixel_z.checked_mul(width)?.checked_add(pixel_x)?).ok()
}

/// Cache key for encoded tile images.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileCacheKey {
    /// Stable world identifier.
    pub world_id: String,
    /// World content signature.
    pub world_signature: String,
    /// Renderer cache schema version.
    pub renderer_version: u32,
    /// Palette data version.
    pub palette_version: u32,
    /// Bedrock dimension.
    pub dimension: Dimension,
    /// Render mode slug.
    pub mode: String,
    /// Chunks covered per tile edge.
    pub chunks_per_tile: u32,
    /// Blocks represented per output pixel.
    pub blocks_per_pixel: u32,
    /// Output pixels generated per source block.
    pub pixels_per_block: u32,
    /// Tile X coordinate.
    pub tile_x: i32,
    /// Tile Z coordinate.
    pub tile_z: i32,
    /// Encoded file extension without a leading dot.
    pub extension: String,
}

/// Computes the lightweight validation value stored in fast tile cache entries.
#[must_use]
pub fn tile_cache_validation_value(
    key: &TileCacheKey,
    region: &ChunkRegion,
    chunk_positions: &[ChunkPos],
    validation_seed: u64,
) -> u64 {
    let mut hash = FNV1A64_OFFSET;
    fnv1a_write_u64(&mut hash, validation_seed);
    fnv1a_write_str(&mut hash, &key.world_id);
    fnv1a_write_str(&mut hash, &key.world_signature);
    fnv1a_write_u32(&mut hash, key.renderer_version);
    fnv1a_write_u32(&mut hash, key.palette_version);
    fnv1a_write_i32(&mut hash, key.dimension.id());
    fnv1a_write_str(&mut hash, &key.mode);
    fnv1a_write_u32(&mut hash, key.chunks_per_tile);
    fnv1a_write_u32(&mut hash, key.blocks_per_pixel);
    fnv1a_write_u32(&mut hash, key.pixels_per_block);
    fnv1a_write_i32(&mut hash, key.tile_x);
    fnv1a_write_i32(&mut hash, key.tile_z);
    fnv1a_write_str(&mut hash, &key.extension);
    fnv1a_write_i32(&mut hash, region.dimension.id());
    fnv1a_write_i32(&mut hash, region.min_chunk_x);
    fnv1a_write_i32(&mut hash, region.min_chunk_z);
    fnv1a_write_i32(&mut hash, region.max_chunk_x);
    fnv1a_write_i32(&mut hash, region.max_chunk_z);

    let positions = canonical_chunk_positions(chunk_positions);
    fnv1a_write_u64(&mut hash, positions.len() as u64);
    for pos in positions.iter() {
        fnv1a_write_i32(&mut hash, pos.dimension.id());
        fnv1a_write_i32(&mut hash, pos.x);
        fnv1a_write_i32(&mut hash, pos.z);
    }
    hash
}

fn canonical_chunk_positions(chunk_positions: &[ChunkPos]) -> Cow<'_, [ChunkPos]> {
    if chunk_positions
        .windows(2)
        .all(|window| window[0] < window[1])
    {
        return Cow::Borrowed(chunk_positions);
    }
    let mut positions = chunk_positions.to_vec();
    positions.sort();
    positions.dedup();
    Cow::Owned(positions)
}

#[derive(Debug)]
struct RegionBakeMemoryCache {
    memory_limit: usize,
    memory_order: VecDeque<RegionBakeCacheKey>,
    memory: BTreeMap<RegionBakeCacheKey, Arc<RegionBake>>,
    renderer_version: u32,
    palette_version: u32,
}

impl RegionBakeMemoryCache {
    fn new(memory_limit: usize, renderer_version: u32, palette_version: u32) -> Self {
        Self {
            memory_limit: memory_limit.max(1),
            memory_order: VecDeque::new(),
            memory: BTreeMap::new(),
            renderer_version,
            palette_version,
        }
    }

    fn key_for(&self, key: RegionBakeKey, options: &RenderOptions) -> RegionBakeCacheKey {
        RegionBakeCacheKey {
            key,
            layout: options.region_layout,
            surface_hash: surface_options_hash(options.surface),
            renderer_version: self.renderer_version,
            palette_version: self.palette_version,
        }
    }

    fn get(&mut self, key: RegionBakeKey, options: &RenderOptions) -> Option<Arc<RegionBake>> {
        let key = self.key_for(key, options);
        self.memory.get(&key).cloned()
    }

    fn insert(&mut self, key: RegionBakeKey, bake: Arc<RegionBake>, options: &RenderOptions) {
        let key = self.key_for(key, options);
        self.memory_order.push_back(key.clone());
        self.memory.insert(key.clone(), bake);
        while self.memory.len() > self.memory_limit {
            let Some(old_key) = self.memory_order.pop_front() else {
                break;
            };
            if old_key != key {
                self.memory.remove(&old_key);
            }
        }
    }
}

/// Small memory and disk cache for rendered tiles.
#[derive(Debug)]
pub struct TileCache {
    root: PathBuf,
    authority: Arc<TileAuthorityCache>,
    memory_limit: usize,
    memory_order: VecDeque<TileCacheKey>,
    memory: BTreeMap<TileCacheKey, TileImage>,
}

struct TileCacheWrite {
    key: TileCacheKey,
    payload: TileCacheWritePayload,
}

enum TileCacheDiskRead {
    Hit(Vec<u8>),
    Miss,
    Corrupt,
}

enum TileCacheWritePayload {
    FastRgbaZstd {
        rgba: Arc<[u8]>,
        width: u32,
        height: u32,
        region: ChunkRegion,
        chunk_positions: Arc<[ChunkPos]>,
        validation_seed: u64,
        flags: u32,
    },
    EmptyNegative {
        tile_size: u32,
        region: ChunkRegion,
        validation_seed: u64,
    },
}

struct PreparedTileCacheCommit {
    authority_key: TileAuthorityCacheKey,
    commit: TileAuthorityCommit,
}

#[derive(Clone)]
enum TileAuthoritySnapshotState {
    Ready(Arc<TileAuthorityIndexSnapshot>),
    Missing,
    Corrupt,
}

type TileAuthoritySnapshotCache =
    Arc<Mutex<BTreeMap<TileAuthorityCacheKey, TileAuthoritySnapshotState>>>;

enum TileAuthorityBlobReaderState {
    Ready(Arc<TileAuthorityBlobReader>),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SessionTileMemoryKey {
    cache_key: TileCacheKey,
    validation_value: u64,
    pixel_format: TilePixelFormat,
}

struct SessionTileMemoryIndex {
    limit: usize,
    order: VecDeque<SessionTileMemoryKey>,
    entries: BTreeMap<SessionTileMemoryKey, TileImage>,
}

impl SessionTileMemoryIndex {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            order: VecDeque::new(),
            entries: BTreeMap::new(),
        }
    }

    fn get(&mut self, key: &SessionTileMemoryKey) -> Option<TileImage> {
        let tile = self.entries.get(key).cloned()?;
        self.order.retain(|existing| existing != key);
        self.order.push_back(key.clone());
        Some(tile)
    }

    fn insert(&mut self, key: SessionTileMemoryKey, tile: TileImage) {
        self.order.retain(|existing| existing != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key.clone(), tile);
        while self.entries.len() > self.limit {
            let Some(old_key) = self.order.pop_front() else {
                break;
            };
            if old_key != key {
                self.entries.remove(&old_key);
            }
        }
    }
}

fn session_tile_memory_key(
    cache_key: &TileCacheKey,
    planned: &PlannedTile,
    validation_seed: u64,
) -> Option<SessionTileMemoryKey> {
    let chunk_positions = planned.chunk_positions.as_deref()?;
    Some(SessionTileMemoryKey {
        cache_key: cache_key.clone(),
        validation_value: tile_cache_validation_value(
            cache_key,
            &planned.region,
            chunk_positions,
            validation_seed,
        ),
        pixel_format: TilePixelFormat::Rgba8,
    })
}

#[derive(Clone)]
struct TileCacheWriterPool {
    inner: Arc<TileCacheWriterPoolInner>,
}

struct TileCacheWriterPoolInner {
    sender: Mutex<Option<crossbeam_channel::Sender<TileCacheWrite>>>,
    encode_handles: Mutex<Vec<thread::JoinHandle<()>>>,
    commit_handle: Mutex<Option<thread::JoinHandle<()>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileCacheWriteQueueResult {
    Queued,
    Dropped,
}

impl Drop for TileCacheWriterPoolInner {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.get_mut() {
            sender.take();
        }
        let encode_handles = self
            .encode_handles
            .get_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let commit_handle = self.commit_handle.get_mut().ok().and_then(Option::take);
        if encode_handles.is_empty() && commit_handle.is_none() {
            return;
        }
        if let Err(error) = thread::Builder::new()
            .name("bedrock-tile-cache-retire".to_string())
            .spawn(move || {
                for handle in encode_handles {
                    if handle.join().is_err() {
                        log::warn!("tile cache encode worker thread panicked");
                    }
                }
                if let Some(handle) = commit_handle
                    && handle.join().is_err()
                {
                    log::warn!("tile cache commit thread panicked");
                }
            })
        {
            log::warn!("failed to spawn tile cache retirement thread: {error}");
        }
    }
}

impl TileCache {
    /// Creates a tile cache rooted at a filesystem path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, memory_limit: usize) -> Self {
        let root = root.into();
        Self {
            authority: Arc::new(TileAuthorityCache::new(root.clone())),
            root,
            memory_limit: memory_limit.max(1),
            memory_order: VecDeque::new(),
            memory: BTreeMap::new(),
        }
    }

    /// Returns a tile from the in-memory cache.
    #[must_use]
    pub fn get_memory(&mut self, key: &TileCacheKey) -> Option<TileImage> {
        self.memory.get(key).cloned()
    }

    fn load_authority_snapshot(
        &self,
        authority_key: &TileAuthorityCacheKey,
    ) -> Result<Option<Arc<TileAuthorityIndexSnapshot>>> {
        self.authority
            .load_index(authority_key)
            .map(|snapshot| snapshot.map(Arc::new))
    }

    fn get_disk_with_snapshot(
        authority: &TileAuthorityCache,
        key: &TileCacheKey,
        authority_key: &TileAuthorityCacheKey,
        snapshot: Option<&TileAuthorityIndexSnapshot>,
        blob_reader: Option<&TileAuthorityBlobReader>,
        blob_reader_known_missing: bool,
    ) -> TileCacheDiskRead {
        let Some(snapshot) = snapshot else {
            return TileCacheDiskRead::Miss;
        };
        let Some(entry) = snapshot.tile(key.tile_x, key.tile_z) else {
            return TileCacheDiskRead::Miss;
        };
        if entry.is_empty() {
            return match encode_fast_rgba_zstd_empty_negative(
                entry.width,
                entry.height,
                entry.validation_value,
            ) {
                Ok(encoded) => TileCacheDiskRead::Hit(encoded),
                Err(error) => {
                    log::debug!("tile authority empty entry rejected: {error}");
                    TileCacheDiskRead::Corrupt
                }
            };
        }
        let blob = match blob_reader {
            Some(reader) => reader.read_entry(entry),
            None if blob_reader_known_missing => return TileCacheDiskRead::Corrupt,
            None => authority.read_blob(authority_key, entry),
        };
        match blob {
            Ok(Some(blob)) => TileCacheDiskRead::Hit(blob),
            Ok(None) => TileCacheDiskRead::Corrupt,
            Err(error) => {
                log::debug!("tile authority blob rejected: {error}");
                TileCacheDiskRead::Corrupt
            }
        }
    }

    /// Inserts a tile into memory and writes encoded bytes to disk when present.
    ///
    /// # Errors
    ///
    /// Returns an error if encoded tile bytes cannot be written.
    #[allow(clippy::needless_pass_by_value)]
    pub fn insert(&mut self, key: TileCacheKey, tile: TileImage) -> Result<()> {
        if let Some(encoded) = tile.encoded.as_deref() {
            self.write_encoded(&key, encoded)?;
        }
        self.insert_memory(key, tile);
        Ok(())
    }

    /// Inserts a decoded tile into the in-memory LRU cache without writing disk cache bytes.
    #[allow(clippy::needless_pass_by_value)]
    pub fn insert_memory(&mut self, key: TileCacheKey, tile: TileImage) {
        self.memory_order.retain(|existing| existing != &key);
        self.memory_order.push_back(key.clone());
        self.memory.insert(key.clone(), tile);
        while self.memory.len() > self.memory_limit {
            let Some(old_key) = self.memory_order.pop_front() else {
                break;
            };
            if old_key != key {
                self.memory.remove(&old_key);
            }
        }
    }

    /// Writes encoded tile bytes to the deterministic disk-cache path.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created or the file cannot be written.
    pub fn write_encoded(&self, key: &TileCacheKey, encoded: &[u8]) -> Result<()> {
        self.write_fast_rgba_zstd_entry(key, encoded, None, None, 0)
    }

    fn write_fast_rgba_zstd_entry(
        &self,
        key: &TileCacheKey,
        encoded: &[u8],
        region: Option<ChunkRegion>,
        chunk_positions: Option<&[ChunkPos]>,
        validation_seed: u64,
    ) -> Result<()> {
        let commit =
            prepare_fast_rgba_zstd_commit(key, encoded, region, chunk_positions, validation_seed)?;
        self.commit_prepared_tile_cache_commits(vec![commit])
            .map(|_| ())
    }

    fn commit_prepared_tile_cache_commits(
        &self,
        commits: Vec<PreparedTileCacheCommit>,
    ) -> Result<Vec<(TileAuthorityCacheKey, TileAuthorityIndexSnapshot)>> {
        let mut commits_by_key = BTreeMap::<TileAuthorityCacheKey, Vec<TileAuthorityCommit>>::new();
        for commit in commits {
            commits_by_key
                .entry(commit.authority_key)
                .or_default()
                .push(commit.commit);
        }
        let mut committed_snapshots = Vec::with_capacity(commits_by_key.len());
        for (authority_key, commits) in commits_by_key {
            let previous = match self.authority.load_index(&authority_key) {
                Ok(previous) => previous,
                Err(error) => {
                    log::debug!("tile authority index ignored while writing new tile: {error}");
                    None
                }
            };
            let snapshot =
                self.authority
                    .commit_tiles(&authority_key, previous.as_ref(), commits, false)?;
            committed_snapshots.push((authority_key, snapshot));
        }
        Ok(committed_snapshots)
    }

    /// Returns the root authority-cache directory for a tile key.
    #[must_use]
    pub fn path_for_key(&self, key: &TileCacheKey) -> PathBuf {
        tile_authority_cache_key_from_tile_key(key).root_for_cache_root(&self.root)
    }
}

fn tile_authority_cache_key_from_tile_key(key: &TileCacheKey) -> TileAuthorityCacheKey {
    TileAuthorityCacheKey {
        world_id: key.world_id.clone(),
        world_signature: key.world_signature.clone(),
        renderer_signature: key.world_signature.clone(),
        mode_slug: key.mode.clone(),
        renderer_version: key.renderer_version,
        palette_version: key.palette_version,
        dimension: key.dimension,
        chunks_per_tile: key.chunks_per_tile,
        blocks_per_pixel: key.blocks_per_pixel,
        pixels_per_block: key.pixels_per_block,
    }
}

fn load_tile_authority_snapshots_for_batch(
    cache: &Arc<Mutex<TileCache>>,
    snapshot_cache: &TileAuthoritySnapshotCache,
    cache_keys: impl IntoIterator<Item = TileCacheKey>,
) -> Result<BTreeMap<TileAuthorityCacheKey, TileAuthoritySnapshotState>> {
    let mut authority_keys = cache_keys
        .into_iter()
        .map(|key| tile_authority_cache_key_from_tile_key(&key))
        .collect::<Vec<_>>();
    authority_keys.sort();
    authority_keys.dedup();

    let mut loaded = BTreeMap::new();
    let mut missing = Vec::new();
    if let Ok(snapshots) = snapshot_cache.lock() {
        for key in authority_keys {
            if let Some(snapshot) = snapshots.get(&key) {
                loaded.insert(key, snapshot.clone());
            } else {
                missing.push(key);
            }
        }
    } else {
        missing = authority_keys;
    }

    if missing.is_empty() {
        return Ok(loaded);
    }

    let cache = cache
        .lock()
        .map_err(|_| BedrockRenderError::Validation("tile cache lock was poisoned".to_string()))?;
    let mut newly_loaded = Vec::with_capacity(missing.len());
    for key in missing {
        let snapshot = match cache.load_authority_snapshot(&key) {
            Ok(Some(snapshot)) => TileAuthoritySnapshotState::Ready(snapshot),
            Ok(None) => TileAuthoritySnapshotState::Missing,
            Err(error) => {
                log::debug!("tile authority index rejected: {error}");
                TileAuthoritySnapshotState::Corrupt
            }
        };
        newly_loaded.push((key, snapshot));
    }
    drop(cache);

    if let Ok(mut snapshots) = snapshot_cache.lock() {
        for (key, snapshot) in &newly_loaded {
            snapshots.insert(key.clone(), snapshot.clone());
        }
    }
    for (key, snapshot) in newly_loaded {
        loaded.insert(key, snapshot);
    }
    Ok(loaded)
}

fn open_tile_authority_blob_readers_for_batch(
    cache: &Arc<Mutex<TileCache>>,
    snapshots: &BTreeMap<TileAuthorityCacheKey, TileAuthoritySnapshotState>,
) -> Result<BTreeMap<TileAuthorityCacheKey, TileAuthorityBlobReaderState>> {
    let authority = cache
        .lock()
        .map_err(|_| BedrockRenderError::Validation("tile cache lock was poisoned".to_string()))?
        .authority
        .clone();
    let mut readers = BTreeMap::new();
    for (key, snapshot) in snapshots {
        if !matches!(snapshot, TileAuthoritySnapshotState::Ready(_)) {
            continue;
        }
        match TileAuthorityBlobReader::open(&authority, key) {
            Ok(Some(reader)) => {
                readers.insert(
                    key.clone(),
                    TileAuthorityBlobReaderState::Ready(Arc::new(reader)),
                );
            }
            Ok(None) => {
                readers.insert(key.clone(), TileAuthorityBlobReaderState::Missing);
            }
            Err(error) => {
                log::debug!("tile authority blob reader unavailable: {error}");
                readers.insert(key.clone(), TileAuthorityBlobReaderState::Missing);
            }
        }
    }
    Ok(readers)
}

fn prepare_fast_rgba_zstd_commit(
    key: &TileCacheKey,
    encoded: &[u8],
    region: Option<ChunkRegion>,
    chunk_positions: Option<&[ChunkPos]>,
    validation_seed: u64,
) -> Result<PreparedTileCacheCommit> {
    let authority_key = tile_authority_cache_key_from_tile_key(key);
    let header = decode_fast_rgba_zstd_header(encoded)?;
    let validation_value = header.validation_value.unwrap_or_else(|| {
        region
            .zip(chunk_positions)
            .map_or(0, |(region, chunk_positions)| {
                tile_cache_validation_value(key, &region, chunk_positions, validation_seed)
            })
    });
    let flags = if header.is_empty_negative() {
        TILE_AUTHORITY_FLAG_EMPTY
    } else {
        TILE_AUTHORITY_FLAG_NON_EMPTY
    };
    let commit = TileAuthorityCommit {
        entry: TileAuthorityEntry {
            tile_x: key.tile_x,
            tile_z: key.tile_z,
            width: header.width,
            height: header.height,
            pixel_len: header.rgba_len,
            blob_offset: 0,
            blob_len: if header.is_empty_negative() {
                0
            } else {
                u64::try_from(encoded.len()).map_err(|_| {
                    BedrockRenderError::Validation(
                        "encoded tile length does not fit u64".to_string(),
                    )
                })?
            },
            payload_hash: if header.is_empty_negative() {
                0
            } else {
                xxh3_128(encoded)
            },
            validation_value,
            flags,
        },
        encoded_blob: if header.is_empty_negative() {
            Vec::new()
        } else {
            encoded.to_vec()
        },
        dependencies: tile_authority_dependencies_from_positions(
            key,
            region,
            chunk_positions,
            validation_seed,
        ),
        chunk_states: tile_authority_chunk_states_from_positions(
            key,
            region,
            chunk_positions,
            validation_seed,
        ),
        chunk_tile_refs: tile_authority_chunk_tile_refs_from_positions(key, chunk_positions),
    };
    Ok(PreparedTileCacheCommit {
        authority_key,
        commit,
    })
}

fn tile_authority_dependencies_from_positions(
    key: &TileCacheKey,
    region: Option<ChunkRegion>,
    chunk_positions: Option<&[ChunkPos]>,
    validation_seed: u64,
) -> Vec<TileAuthorityDependency> {
    let Some(chunk_positions) = chunk_positions else {
        return Vec::new();
    };
    let stamp = region.map_or(0, |region| {
        tile_cache_validation_value(key, &region, chunk_positions, validation_seed)
    });
    canonical_chunk_positions(chunk_positions)
        .iter()
        .copied()
        .map(|position| TileAuthorityDependency {
            tile_x: key.tile_x,
            tile_z: key.tile_z,
            position,
            revision: stamp,
            content_hash: tile_authority_chunk_stamp(stamp, position),
        })
        .collect()
}

fn tile_authority_chunk_states_from_positions(
    key: &TileCacheKey,
    region: Option<ChunkRegion>,
    chunk_positions: Option<&[ChunkPos]>,
    validation_seed: u64,
) -> Vec<TileAuthorityChunkState> {
    let Some(chunk_positions) = chunk_positions else {
        return Vec::new();
    };
    let stamp = region.map_or(0, |region| {
        tile_cache_validation_value(key, &region, chunk_positions, validation_seed)
    });
    canonical_chunk_positions(chunk_positions)
        .iter()
        .copied()
        .map(|position| TileAuthorityChunkState {
            position,
            revision: stamp,
            content_hash: tile_authority_chunk_stamp(stamp, position),
        })
        .collect()
}

fn tile_authority_chunk_tile_refs_from_positions(
    key: &TileCacheKey,
    chunk_positions: Option<&[ChunkPos]>,
) -> Vec<TileAuthorityChunkTileRef> {
    let Some(chunk_positions) = chunk_positions else {
        return Vec::new();
    };
    canonical_chunk_positions(chunk_positions)
        .iter()
        .copied()
        .map(|position| TileAuthorityChunkTileRef {
            position,
            tile_x: key.tile_x,
            tile_z: key.tile_z,
        })
        .collect()
}

fn tile_authority_chunk_stamp(seed: u64, position: ChunkPos) -> u128 {
    let mut bytes = [0u8; 20];
    bytes[0..8].copy_from_slice(&seed.to_le_bytes());
    bytes[8..12].copy_from_slice(&position.dimension.id().to_le_bytes());
    bytes[12..16].copy_from_slice(&position.x.to_le_bytes());
    bytes[16..20].copy_from_slice(&position.z.to_le_bytes());
    xxh3_128(&bytes)
}

fn spawn_tile_cache_writer(
    cache: Arc<Mutex<TileCache>>,
    authority_snapshots: TileAuthoritySnapshotCache,
    options: &RenderOptions,
    work_items: usize,
) -> Option<TileCacheWriterPool> {
    let work_items = work_items.max(64);
    let worker_count = tile_cache_writer_worker_count(options, work_items);
    let queue_capacity = tile_cache_writer_queue_capacity(options, work_items, worker_count);
    let (sender, receiver) = crossbeam_channel::bounded::<TileCacheWrite>(queue_capacity);
    let prepared_capacity = queue_capacity.max(worker_count.saturating_mul(2)).max(1);
    let (prepared_sender, prepared_receiver) =
        crossbeam_channel::bounded::<PreparedTileCacheCommit>(prepared_capacity);
    let commit_cache = Arc::clone(&cache);
    let commit_authority_snapshots = Arc::clone(&authority_snapshots);
    let commit_handle = match thread::Builder::new()
        .name("bedrock-render-tile-cache-commit".to_string())
        .spawn(move || {
            commit_tile_cache_entries(
                &commit_cache,
                &commit_authority_snapshots,
                prepared_receiver,
            );
        }) {
        Ok(handle) => handle,
        Err(error) => {
            log::warn!("failed to start rendered tile cache commit thread: {error}");
            return None;
        }
    };
    let mut encode_handles = Vec::with_capacity(worker_count);
    for worker_index in 0..worker_count {
        let receiver = receiver.clone();
        let prepared_sender = prepared_sender.clone();
        let handle = match thread::Builder::new()
            .name(format!("bedrock-render-tile-cache-encode-{worker_index}"))
            .spawn(move || {
                for write in &receiver {
                    prepare_tile_cache_entry(&prepared_sender, write);
                }
            }) {
            Ok(handle) => handle,
            Err(error) => {
                log::warn!(
                    "failed to start rendered tile cache encode worker {worker_index}: {error}"
                );
                break;
            }
        };
        encode_handles.push(handle);
    }
    drop(prepared_sender);
    if encode_handles.is_empty() {
        return None;
    }
    Some(TileCacheWriterPool {
        inner: Arc::new(TileCacheWriterPoolInner {
            sender: Mutex::new(Some(sender)),
            encode_handles: Mutex::new(encode_handles),
            commit_handle: Mutex::new(Some(commit_handle)),
        }),
    })
}

fn queue_tile_cache_write(
    writer: &Option<TileCacheWriterPool>,
    write: TileCacheWrite,
) -> TileCacheWriteQueueResult {
    let Some(writer) = writer else {
        log::warn!(
            "tile cache writer unavailable (dimension={:?}, tile=({}, {}))",
            write.key.dimension,
            write.key.tile_x,
            write.key.tile_z
        );
        return TileCacheWriteQueueResult::Dropped;
    };
    let tile_x = write.key.tile_x;
    let tile_z = write.key.tile_z;
    let dimension = write.key.dimension;
    let sender = match writer.inner.sender.lock() {
        Ok(sender) => sender.clone(),
        Err(_) => {
            log::warn!("tile cache writer sender lock was poisoned");
            None
        }
    };
    let Some(sender) = sender else {
        log::debug!(
            "tile cache writer already closed; dropping disk write (dimension={:?}, tile=({}, {}))",
            dimension,
            tile_x,
            tile_z
        );
        return TileCacheWriteQueueResult::Dropped;
    };
    match sender.try_send(write) {
        Ok(()) => {
            log::trace!(
                "tile cache write queued (dimension={:?}, tile=({}, {}))",
                dimension,
                tile_x,
                tile_z
            );
            TileCacheWriteQueueResult::Queued
        }
        Err(crossbeam_channel::TrySendError::Full(write)) => {
            log::debug!(
                "tile cache write queue full; dropping disk write (dimension={:?}, tile=({}, {}))",
                write.key.dimension,
                write.key.tile_x,
                write.key.tile_z
            );
            TileCacheWriteQueueResult::Dropped
        }
        Err(crossbeam_channel::TrySendError::Disconnected(write)) => {
            log::warn!(
                "tile cache writer disconnected (dimension={:?}, tile=({}, {}))",
                write.key.dimension,
                write.key.tile_x,
                write.key.tile_z
            );
            TileCacheWriteQueueResult::Dropped
        }
    }
}

fn tile_cache_writer_worker_count(options: &RenderOptions, work_items: usize) -> usize {
    let configured = options.cpu.encode_workers;
    if configured > 0 {
        return configured.min(work_items.max(1)).max(1);
    }
    resolve_cpu_worker_count(options, work_items.max(1))
        .map(|workers| workers.clamp(1, 4).min(work_items.max(1)))
        .unwrap_or(1)
}

fn tile_cache_writer_queue_capacity(
    options: &RenderOptions,
    work_items: usize,
    worker_count: usize,
) -> usize {
    options
        .pipeline_depth
        .max(worker_count.saturating_mul(2))
        .max(work_items.min(64))
        .max(1)
}

const TILE_CACHE_COMMIT_BATCH_SIZE: usize = 64;
const TILE_CACHE_COMMIT_FLUSH_MS: u64 = 50;

fn prepare_tile_cache_entry(
    sender: &crossbeam_channel::Sender<PreparedTileCacheCommit>,
    write: TileCacheWrite,
) {
    let tile_x = write.key.tile_x;
    let tile_z = write.key.tile_z;
    let dimension = write.key.dimension;
    let encoded = match encode_tile_cache_write_payload(&write.key, &write.payload) {
        Ok(encoded) => encoded,
        Err(error) => {
            log::warn!(
                "tile cache encode failed (dimension={:?}, tile=({}, {}), error={})",
                dimension,
                tile_x,
                tile_z,
                error
            );
            return;
        }
    };
    let prepared = match &write.payload {
        TileCacheWritePayload::FastRgbaZstd {
            region,
            chunk_positions,
            validation_seed,
            ..
        } => prepare_fast_rgba_zstd_commit(
            &write.key,
            &encoded,
            Some(*region),
            Some(chunk_positions.as_ref()),
            *validation_seed,
        ),
        TileCacheWritePayload::EmptyNegative {
            region,
            validation_seed,
            ..
        } => prepare_fast_rgba_zstd_commit(
            &write.key,
            &encoded,
            Some(*region),
            Some(&[]),
            *validation_seed,
        ),
    };
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            log::warn!(
                "tile cache commit prepare failed (dimension={:?}, tile=({}, {}), error={})",
                dimension,
                tile_x,
                tile_z,
                error
            );
            return;
        }
    };
    if sender.send(prepared).is_err() {
        log::debug!(
            "tile cache commit receiver closed (dimension={:?}, tile=({}, {}), bytes={})",
            dimension,
            tile_x,
            tile_z,
            encoded.len()
        );
    } else {
        log::trace!(
            "tile cache commit prepared (dimension={:?}, tile=({}, {}), bytes={})",
            dimension,
            tile_x,
            tile_z,
            encoded.len()
        );
    }
}

fn commit_tile_cache_entries(
    cache: &Arc<Mutex<TileCache>>,
    authority_snapshots: &TileAuthoritySnapshotCache,
    receiver: crossbeam_channel::Receiver<PreparedTileCacheCommit>,
) {
    let mut batch = Vec::with_capacity(TILE_CACHE_COMMIT_BATCH_SIZE);
    loop {
        match receiver.recv_timeout(Duration::from_millis(TILE_CACHE_COMMIT_FLUSH_MS)) {
            Ok(commit) => {
                batch.push(commit);
                while batch.len() < TILE_CACHE_COMMIT_BATCH_SIZE {
                    match receiver.try_recv() {
                        Ok(commit) => batch.push(commit),
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            flush_tile_cache_commit_batch(cache, authority_snapshots, &mut batch);
                            return;
                        }
                    }
                }
                if batch.len() >= TILE_CACHE_COMMIT_BATCH_SIZE {
                    flush_tile_cache_commit_batch(cache, authority_snapshots, &mut batch);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                flush_tile_cache_commit_batch(cache, authority_snapshots, &mut batch);
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                flush_tile_cache_commit_batch(cache, authority_snapshots, &mut batch);
                return;
            }
        }
    }
}

fn flush_tile_cache_commit_batch(
    cache: &Arc<Mutex<TileCache>>,
    authority_snapshots: &TileAuthoritySnapshotCache,
    batch: &mut Vec<PreparedTileCacheCommit>,
) {
    if batch.is_empty() {
        return;
    }
    let commits = std::mem::take(batch);
    let commit_count = commits.len();
    let cache = match cache.lock() {
        Ok(cache) => cache,
        Err(_) => {
            log::warn!("tile cache lock was poisoned while committing asynchronously");
            return;
        }
    };
    match cache.commit_prepared_tile_cache_commits(commits) {
        Ok(committed_snapshots) => {
            if let Ok(mut snapshots) = authority_snapshots.lock() {
                for (key, snapshot) in committed_snapshots {
                    snapshots.insert(key, TileAuthoritySnapshotState::Ready(Arc::new(snapshot)));
                }
            }
            log::trace!("tile cache batch commit complete (tiles={commit_count})");
        }
        Err(error) => {
            log::warn!(
                "tile cache batch commit failed (tiles={}, error={})",
                commit_count,
                error
            );
        }
    }
}

fn encode_tile_cache_write_payload(
    key: &TileCacheKey,
    payload: &TileCacheWritePayload,
) -> Result<Vec<u8>> {
    match payload {
        TileCacheWritePayload::FastRgbaZstd {
            rgba,
            width,
            height,
            region,
            chunk_positions,
            validation_seed,
            flags,
        } => {
            let validation_value =
                tile_cache_validation_value(key, region, chunk_positions, *validation_seed);
            if *flags == FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE {
                return encode_fast_rgba_zstd_empty_negative(*width, *height, validation_value);
            }
            encode_fast_rgba_zstd_with_validation_and_flags(
                rgba.as_ref(),
                *width,
                *height,
                validation_value,
                *flags,
            )
        }
        TileCacheWritePayload::EmptyNegative {
            tile_size,
            region,
            validation_seed,
        } => {
            let validation_value = tile_cache_validation_value(key, region, &[], *validation_seed);
            encode_fast_rgba_zstd_empty_negative(*tile_size, *tile_size, validation_value)
        }
    }
}

fn render_tile_stream_group_size(options: &RenderOptions, tile_count: usize) -> Result<usize> {
    if tile_count == 0 {
        return Ok(1);
    }
    if options.execution_profile != RenderExecutionProfile::Interactive {
        return Ok(tile_count.max(1));
    }
    let _worker_count = resolve_render_worker_count(options, tile_count)?;
    Ok(tile_count)
}

fn should_continue_after_stream_group_error(
    options: &RenderOptions,
    error: &BedrockRenderError,
) -> bool {
    options.execution_profile == RenderExecutionProfile::Interactive
        && !render_error_is_cancelled(error)
}

fn render_error_is_cancelled(error: &BedrockRenderError) -> bool {
    matches!(error, BedrockRenderError::Cancelled)
        || matches!(
            error,
            BedrockRenderError::World(world_error)
                if world_error.kind() == bedrock_world::BedrockWorldErrorKind::Cancelled
        )
}

fn region_wave_worker_split(
    options: &RenderOptions,
    worker_count: usize,
    region_count: usize,
) -> RegionWaveWorkerSplit {
    if region_count == 0 {
        return RegionWaveWorkerSplit {
            region_workers: 0,
            world_workers_per_region: 0,
        };
    }
    if matches!(options.threading, RenderThreadingOptions::Single) || worker_count <= 1 {
        return RegionWaveWorkerSplit {
            region_workers: 1,
            world_workers_per_region: 1,
        };
    }

    let region_workers = match options.execution_profile {
        RenderExecutionProfile::Interactive => {
            let max_regions =
                options
                    .cpu
                    .max_in_flight_regions
                    .max(if options.cpu.max_in_flight_regions == 0 {
                        2
                    } else {
                        1
                    });
            let max_bake = options
                .cpu
                .max_bake_workers
                .max(if options.cpu.max_bake_workers == 0 {
                    2
                } else {
                    1
                });
            (worker_count / 2)
                .max(1)
                .min(region_count)
                .min(max_regions)
                .min(max_bake)
        }
        RenderExecutionProfile::Export => worker_count.min(region_count),
    }
    .max(1);
    let mut world_workers_per_region = (worker_count / region_workers).max(1);
    if options.cpu.max_db_workers > 0 {
        world_workers_per_region = world_workers_per_region.min(options.cpu.max_db_workers);
    }
    RegionWaveWorkerSplit {
        region_workers,
        world_workers_per_region,
    }
}

fn resolve_render_worker_count(options: &RenderOptions, work_items: usize) -> Result<usize> {
    let max_threads = (options.cpu.max_total_threads > 0).then_some(options.cpu.max_total_threads);
    options.threading.resolve_for_profile_with_limits(
        options.execution_profile,
        work_items,
        max_threads,
        usize::from(options.execution_profile == RenderExecutionProfile::Interactive),
    )
}

fn resolve_cpu_worker_count(options: &RenderOptions, work_items: usize) -> Result<usize> {
    let max_threads = (options.cpu.max_total_threads > 0).then_some(options.cpu.max_total_threads);
    options.cpu_workers.resolve_for_profile_with_limits(
        options.execution_profile,
        work_items,
        max_threads,
        usize::from(options.execution_profile == RenderExecutionProfile::Interactive),
    )
}

/// Configuration for a reusable map render session.
#[derive(Debug, Clone)]
pub struct MapRenderSessionConfig {
    /// Root directory used for encoded tile cache files.
    pub cache_root: PathBuf,
    /// Maximum number of decoded tiles retained by the in-memory cache.
    pub tile_cache_memory_limit: usize,
    /// Stable world identifier used in cache paths.
    pub world_id: String,
    /// World content signature used to isolate incompatible cache entries.
    pub world_signature: String,
    /// Renderer cache schema version.
    pub renderer_version: u32,
    /// Palette version used by cache keys.
    pub palette_version: u32,
    /// Query exact render chunks for each cache miss before baking.
    pub cull_missing_chunks: bool,
    /// Maximum number of baked regions retained by the session memory cache.
    pub region_bake_cache_memory_limit: usize,
    /// Preferred GPU backend for the reusable session context.
    pub gpu_backend: RenderGpuBackend,
}

impl Default for MapRenderSessionConfig {
    fn default() -> Self {
        Self {
            cache_root: PathBuf::new(),
            tile_cache_memory_limit: 256,
            world_id: "world".to_string(),
            world_signature: "default".to_string(),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            cull_missing_chunks: true,
            region_bake_cache_memory_limit: 128,
            gpu_backend: RenderGpuBackend::Auto,
        }
    }
}

impl MapRenderSessionConfig {
    /// Creates a session config for the max-speed interactive profile.
    #[must_use]
    pub fn max_speed(
        cache_root: impl Into<PathBuf>,
        world_id: impl Into<String>,
        world_signature: impl Into<String>,
    ) -> Self {
        Self {
            cache_root: cache_root.into(),
            tile_cache_memory_limit: 512,
            world_id: world_id.into(),
            world_signature: world_signature.into(),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            cull_missing_chunks: true,
            region_bake_cache_memory_limit: 512,
            gpu_backend: RenderGpuBackend::Auto,
        }
    }
}

/// Source that produced a ready tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileReadySource {
    /// Decoded tile came from the session in-memory tile cache.
    MemoryCache,
    /// Decoded tile came from a validated disk cache entry.
    DiskCacheFresh,
    /// Decoded tile came from a disk cache entry without validation.
    DiskCacheStale,
    /// Tile was freshly rendered.
    Render,
    /// Tile is a fast preview.
    Preview,
}

/// Streaming events emitted by [`MapRenderSession`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum TileStreamEvent {
    /// Tile is decoded and ready to display.
    Ready {
        /// Planned tile that was satisfied.
        planned: PlannedTile,
        /// Decoded tile image.
        tile: TileImage,
        /// Source that produced the tile.
        source: TileReadySource,
    },
    /// Tile is known to contain no renderable content.
    Empty {
        /// Planned tile that was satisfied as empty.
        planned: PlannedTile,
    },
    /// Tile could not be rendered.
    Failed {
        /// Planned tile that failed.
        planned: PlannedTile,
        /// Human-readable failure message.
        error: String,
    },
    /// Aggregate progress update.
    Progress(RenderProgress),
    /// Stream finished.
    Complete {
        /// Aggregated diagnostics.
        diagnostics: RenderDiagnostics,
        /// Aggregated pipeline stats.
        stats: RenderPipelineStats,
    },
}

/// Streaming events emitted by the decoded v2 tile stream.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum TileStreamEventV2 {
    /// Tile is decoded and ready to display.
    Ready {
        /// Planned tile that was satisfied.
        planned: PlannedTile,
        /// Decoded tile image.
        tile: DecodedTileImage,
        /// Source that produced the tile.
        source: TileReadySource,
    },
    /// Tile is known to contain no renderable content.
    Empty {
        /// Planned tile that was satisfied as empty.
        planned: PlannedTile,
    },
    /// Tile could not be rendered.
    Failed {
        /// Planned tile that failed.
        planned: PlannedTile,
        /// Human-readable failure message.
        error: String,
    },
    /// Aggregate progress update.
    Progress(RenderProgress),
    /// Stream finished.
    Complete {
        /// Aggregated diagnostics.
        diagnostics: RenderDiagnostics,
        /// Aggregated pipeline stats.
        stats: RenderPipelineStats,
    },
}

/// Reusable render session that keeps world, renderer, and tile cache alive
/// across many viewport-driven tile requests.
#[derive(Clone)]
pub struct MapRenderSession<S = Arc<dyn WorldStorage>>
where
    S: WorldStorageHandle,
{
    renderer: MapRenderer<S>,
    cache: Arc<Mutex<TileCache>>,
    cache_writer: Arc<Mutex<Option<TileCacheWriterPool>>>,
    authority_snapshots: TileAuthoritySnapshotCache,
    session_tile_memory: Arc<Mutex<SessionTileMemoryIndex>>,
    gpu: Option<GpuRenderContext>,
    config: MapRenderSessionConfig,
}

impl MapRenderSession<Arc<dyn WorldStorage>> {
    /// Opens a Bedrock `LevelDB` world directly for read-only map rendering.
    ///
    /// # Errors
    ///
    /// Returns an error if `world_path/db` cannot be opened.
    pub fn open_leveldb_read_only(
        world_path: impl AsRef<Path>,
        config: MapRenderSessionConfig,
        palette: impl Into<Arc<RenderPalette>>,
    ) -> Result<Self> {
        let source = Arc::new(LevelDbRenderSource::open_read_only(world_path)?);
        let renderer = MapRenderer::from_source(source, palette);
        Ok(Self::new(renderer, config))
    }
}

impl<S> MapRenderSession<S>
where
    S: WorldStorageHandle,
{
    /// Creates a reusable session from an existing renderer.
    #[must_use]
    pub fn new(renderer: MapRenderer<S>, mut config: MapRenderSessionConfig) -> Self {
        if config.renderer_version < RENDERER_CACHE_VERSION {
            log::warn!(
                "renderer cache version {} is older than current {}; using current version to avoid stale tiles",
                config.renderer_version,
                RENDERER_CACHE_VERSION
            );
            config.renderer_version = RENDERER_CACHE_VERSION;
        }
        let cache = TileCache::new(
            config.cache_root.clone(),
            config.tile_cache_memory_limit.max(1),
        );
        let region_cache = Arc::new(Mutex::new(RegionBakeMemoryCache::new(
            config.region_bake_cache_memory_limit.max(1),
            config.renderer_version,
            config.palette_version,
        )));
        let gpu = GpuRenderContext::new(config.gpu_backend).ok();
        Self {
            renderer: renderer
                .with_region_bake_cache(region_cache)
                .with_gpu_context(gpu.clone()),
            cache: Arc::new(Mutex::new(cache)),
            cache_writer: Arc::new(Mutex::new(None)),
            authority_snapshots: Arc::new(Mutex::new(BTreeMap::new())),
            session_tile_memory: Arc::new(Mutex::new(SessionTileMemoryIndex::new(
                config.tile_cache_memory_limit.max(1),
            ))),
            gpu,
            config,
        }
    }

    fn tile_cache_writer(
        &self,
        options: &RenderOptions,
        work_items: usize,
    ) -> Option<TileCacheWriterPool> {
        let mut writer = match self.cache_writer.lock() {
            Ok(writer) => writer,
            Err(_) => {
                log::warn!("tile cache writer pool lock was poisoned");
                return None;
            }
        };
        if writer.is_none() {
            *writer = spawn_tile_cache_writer(
                Arc::clone(&self.cache),
                Arc::clone(&self.authority_snapshots),
                options,
                work_items,
            );
        }
        writer.clone()
    }

    /// Returns the underlying renderer.
    #[must_use]
    pub const fn renderer(&self) -> &MapRenderer<S> {
        &self.renderer
    }
    /// Returns true when this session has initialized a reusable GPU context.
    #[must_use]
    pub const fn gpu_available(&self) -> bool {
        self.gpu.is_some()
    }

    /// Renders planned web tiles and streams cache hits, rendered tiles, failures,
    /// and the final aggregate result to `sink`.
    ///
    /// # Errors
    ///
    /// Returns an error if rendering, cancellation, or the sink fails.
    ///
    /// Rendered tile cache writes are queued after the tile is emitted to the
    /// stream, and cache write failures are logged.
    #[allow(clippy::too_many_lines)]
    pub fn render_web_tiles_streaming_blocking<F>(
        &self,
        planned_tiles: &[PlannedTile],
        mut options: RenderOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(TileStreamEvent) -> Result<()> + Send + Sync + 'static,
    {
        let sink = Arc::new(sink);
        if planned_tiles.is_empty() {
            let result = RenderWebTilesResult {
                diagnostics: RenderDiagnostics::default(),
                stats: RenderPipelineStats::default(),
            };
            sink.as_ref()(TileStreamEvent::Complete {
                diagnostics: result.diagnostics.clone(),
                stats: result.stats.clone(),
            })?;
            return Ok(result);
        }

        let mut cached_diagnostics = RenderDiagnostics::default();
        let mut cached_stats = RenderPipelineStats {
            planned_tiles: planned_tiles.len(),
            ..RenderPipelineStats::default()
        };
        let mut render_tiles;
        let stream_started = Instant::now();
        let rendered_stream_count = Arc::new(AtomicUsize::new(0));
        let tile_cache_writer_dropped_count = Arc::new(AtomicUsize::new(0));
        let mut failed_stream_count = 0usize;

        log::debug!(
            "streaming web tiles start (tiles={}, backend={:?}, cache_policy={:?}, cull_missing={}, priority={:?})",
            planned_tiles.len(),
            options.backend,
            options.cache_policy,
            self.config.cull_missing_chunks,
            options.priority
        );

        let ordered_tiles = prioritized_planned_tiles(planned_tiles, options.priority);
        let cache_read_format = matches!(options.cache_policy, RenderCachePolicy::Use)
            .then_some(ImageFormat::FastRgbaZstd);
        let cache_write_format = matches!(
            options.cache_policy,
            RenderCachePolicy::Use | RenderCachePolicy::Refresh
        )
        .then_some(ImageFormat::FastRgbaZstd);
        if let Some(cache_format) = cache_read_format {
            let probe_workers = resolve_cpu_worker_count(&options, ordered_tiles.len())?.max(1);
            let cache = Arc::clone(&self.cache);
            let session_tile_memory = Arc::clone(&self.session_tile_memory);
            let authority_snapshots = Arc::clone(&self.authority_snapshots);
            let render_format = options.format;
            let validation_seed = options.tile_cache_validation_seed;
            cached_stats.peak_worker_threads = cached_stats.peak_worker_threads.max(probe_workers);

            let mut cache_misses = Vec::new();
            let mut cache_first_ready_recorded = false;
            let mut handle_probe = |ordinal: usize,
                                    probe: TileCacheProbe|
             -> std::result::Result<(), BedrockRenderError> {
                check_cancelled(&options)?;
                cached_stats.cache_probes = cached_stats.cache_probes.saturating_add(1);
                cached_stats.cache_read_ms =
                    cached_stats.cache_read_ms.saturating_add(probe.read_ms);
                cached_stats.cache_decode_ms =
                    cached_stats.cache_decode_ms.saturating_add(probe.decode_ms);
                cached_stats.tile_blob_decode_ms = cached_stats
                    .tile_blob_decode_ms
                    .saturating_add(probe.decode_ms);
                if probe.validation_mismatch {
                    cached_stats.cache_validation_mismatches =
                        cached_stats.cache_validation_mismatches.saturating_add(1);
                }
                if probe.corrupt_miss {
                    cached_stats.index_corrupt_misses =
                        cached_stats.index_corrupt_misses.saturating_add(1);
                }
                match probe.decision {
                    TileCacheProbeDecision::Ready { tile, source } => {
                        cached_diagnostics.cache_hits =
                            cached_diagnostics.cache_hits.saturating_add(1);
                        cached_stats.cache_hits = cached_stats.cache_hits.saturating_add(1);
                        match source {
                            TileReadySource::MemoryCache => {
                                cached_stats.cache_memory_hits =
                                    cached_stats.cache_memory_hits.saturating_add(1);
                            }
                            TileReadySource::DiskCacheFresh => {
                                cached_stats.cache_disk_fresh_hits =
                                    cached_stats.cache_disk_fresh_hits.saturating_add(1);
                            }
                            TileReadySource::DiskCacheStale => {
                                cached_stats.cache_disk_stale_hits =
                                    cached_stats.cache_disk_stale_hits.saturating_add(1);
                            }
                            TileReadySource::Render | TileReadySource::Preview => {}
                        }
                        if !cache_first_ready_recorded {
                            cache_first_ready_recorded = true;
                            cached_stats.cache_first_ready_ms =
                                stream_started.elapsed().as_millis();
                        }
                        log::trace!(
                            "tile cache hit (dimension={:?}, tile=({}, {}), source={:?}, bytes={})",
                            probe.planned.job.coord.dimension,
                            probe.planned.job.coord.x,
                            probe.planned.job.coord.z,
                            source,
                            tile.encoded.as_ref().map_or(0, Vec::len)
                        );
                        let planned = probe.planned;
                        sink.as_ref()(TileStreamEvent::Ready {
                            planned: planned.clone(),
                            tile,
                            source,
                        })?;
                    }
                    TileCacheProbeDecision::EmptyNegative => {
                        cached_diagnostics.cache_hits =
                            cached_diagnostics.cache_hits.saturating_add(1);
                        cached_stats.cache_hits = cached_stats.cache_hits.saturating_add(1);
                        cached_stats.cache_empty_negative_hits =
                            cached_stats.cache_empty_negative_hits.saturating_add(1);
                        sink.as_ref()(TileStreamEvent::Empty {
                            planned: probe.planned,
                        })?;
                    }
                    TileCacheProbeDecision::Miss => {
                        cached_diagnostics.cache_misses =
                            cached_diagnostics.cache_misses.saturating_add(1);
                        cached_stats.cache_misses = cached_stats.cache_misses.saturating_add(1);
                        cache_misses.push((ordinal, probe.planned));
                    }
                }
                Ok(())
            };

            let authority_snapshots_for_batch = load_tile_authority_snapshots_for_batch(
                &cache,
                &authority_snapshots,
                ordered_tiles
                    .iter()
                    .map(|planned| self.cache_key_for_planned(planned, cache_format)),
            )?;
            let blob_readers_for_batch =
                open_tile_authority_blob_readers_for_batch(&cache, &authority_snapshots_for_batch)?;

            if probe_workers <= 1 || ordered_tiles.len() <= 1 {
                let mut decode_scratch = TileCacheDecodeScratch::new();
                for (ordinal, planned) in ordered_tiles.iter().enumerate() {
                    let render_key = self.cache_key_for_planned(planned, render_format);
                    let cache_key = self.cache_key_for_planned(planned, cache_format);
                    let probe = resolve_tile_cache_entry(
                        &cache,
                        &session_tile_memory,
                        &authority_snapshots_for_batch,
                        &blob_readers_for_batch,
                        &render_key,
                        &cache_key,
                        planned,
                        cache_format,
                        validation_seed,
                        &mut decode_scratch,
                    )?;
                    handle_probe(ordinal, probe)?;
                }
            } else {
                struct TileCacheProbeWork {
                    ordinal: usize,
                    planned: PlannedTile,
                    render_key: TileCacheKey,
                    cache_key: TileCacheKey,
                }

                let ordered_tiles_len = ordered_tiles.len();
                let work_items = ordered_tiles
                    .into_iter()
                    .enumerate()
                    .map(|(ordinal, planned)| {
                        let render_key = self.cache_key_for_planned(&planned, render_format);
                        let cache_key = self.cache_key_for_planned(&planned, cache_format);
                        TileCacheProbeWork {
                            ordinal,
                            planned,
                            render_key,
                            cache_key,
                        }
                    })
                    .collect::<Vec<_>>();
                let queue_capacity = options
                    .pipeline_depth
                    .max(probe_workers.saturating_mul(2))
                    .max(1)
                    .min(ordered_tiles_len.max(1));
                let (work_sender, work_receiver) =
                    crossbeam_channel::bounded::<TileCacheProbeWork>(queue_capacity);
                let (result_sender, result_receiver) =
                    crossbeam_channel::bounded::<Result<(usize, TileCacheProbe)>>(queue_capacity);

                thread::scope(|scope| {
                    {
                        let work_sender = work_sender.clone();
                        scope.spawn(move || {
                            for work in work_items {
                                if work_sender.send(work).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    drop(work_sender);

                    for _ in 0..probe_workers {
                        let work_receiver = work_receiver.clone();
                        let result_sender = result_sender.clone();
                        let cache = Arc::clone(&cache);
                        let session_tile_memory = Arc::clone(&session_tile_memory);
                        let authority_snapshots_for_batch = &authority_snapshots_for_batch;
                        let blob_readers_for_batch = &blob_readers_for_batch;
                        scope.spawn(move || {
                            let mut decode_scratch = TileCacheDecodeScratch::new();
                            for work in &work_receiver {
                                let result = resolve_tile_cache_entry(
                                    &cache,
                                    &session_tile_memory,
                                    authority_snapshots_for_batch,
                                    blob_readers_for_batch,
                                    &work.render_key,
                                    &work.cache_key,
                                    &work.planned,
                                    cache_format,
                                    validation_seed,
                                    &mut decode_scratch,
                                )
                                .map(|probe| (work.ordinal, probe));
                                if result_sender.send(result).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    drop(result_sender);

                    let mut first_error = None;
                    let mut completed_probes = Vec::with_capacity(ordered_tiles_len);
                    for _ in 0..ordered_tiles_len {
                        let result = result_receiver.recv().map_err(|error| {
                            BedrockRenderError::Join(format!(
                                "tile cache probe worker stopped early: {error}"
                            ))
                        });
                        match result {
                            Ok(Ok((ordinal, probe))) => {
                                completed_probes.push((ordinal, probe));
                            }
                            Ok(Err(error)) | Err(error) => {
                                if first_error.is_none() {
                                    if let Some(cancel) = &options.cancel {
                                        cancel.cancel();
                                    }
                                    first_error = Some(error);
                                }
                            }
                        }
                    }
                    if let Some(error) = first_error {
                        return Err(error);
                    }
                    for (ordinal, probe) in order_tile_cache_probe_results(completed_probes) {
                        handle_probe(ordinal, probe)?;
                    }
                    Ok(())
                })?;
            }

            cache_misses.sort_by_key(|(ordinal, _)| *ordinal);
            render_tiles = cache_misses
                .into_iter()
                .map(|(_, planned)| planned)
                .collect();
        } else {
            render_tiles = ordered_tiles;
            if options.cache_policy != RenderCachePolicy::Refresh {
                cached_diagnostics.cache_misses = cached_diagnostics
                    .cache_misses
                    .saturating_add(render_tiles.len());
                cached_stats.cache_misses =
                    cached_stats.cache_misses.saturating_add(render_tiles.len());
            }
        }

        let cache_writer =
            cache_write_format.and_then(|_| self.tile_cache_writer(&options, render_tiles.len()));

        let mut culled_render_tiles = Vec::new();
        for planned in self.prepare_planned_tiles_for_render(&render_tiles, &options)? {
            if planned
                .chunk_positions
                .as_ref()
                .is_some_and(|positions| positions.is_empty())
            {
                if let Some(cache_format) = cache_write_format {
                    let cache_key = self.cache_key_for_planned(&planned, cache_format);
                    if let Some(write) = empty_negative_tile_cache_write(
                        &cache_key,
                        &planned,
                        cache_format,
                        options.tile_cache_validation_seed,
                    ) && queue_tile_cache_write(&cache_writer, write)
                        == TileCacheWriteQueueResult::Dropped
                    {
                        cached_stats.tile_cache_writer_dropped =
                            cached_stats.tile_cache_writer_dropped.saturating_add(1);
                    }
                }
                log::debug!(
                    "tile has no renderable chunks after cull (dimension={:?}, tile=({}, {}))",
                    planned.job.coord.dimension,
                    planned.job.coord.x,
                    planned.job.coord.z
                );
                sink.as_ref()(TileStreamEvent::Empty { planned })?;
            } else {
                culled_render_tiles.push(planned);
            }
        }
        render_tiles = culled_render_tiles;

        let progress_sink = RenderProgressSink::new({
            let sink = Arc::clone(&sink);
            move |progress| {
                if let Err(error) = sink.as_ref()(TileStreamEvent::Progress(progress)) {
                    log::warn!("tile stream progress sink failed: {error}");
                }
            }
        });
        options.progress = Some(match options.progress.take() {
            Some(existing) => RenderProgressSink::new(move |progress| {
                existing.emit(progress);
                progress_sink.emit(progress);
            }),
            None => progress_sink,
        });

        let mut final_diagnostics = cached_diagnostics.clone();
        let mut final_stats = cached_stats.clone();
        if !render_tiles.is_empty() {
            let render_format = options.format;
            let tile_cache_validation_seed = options.tile_cache_validation_seed;
            let render_group_size = render_tile_stream_group_size(&options, render_tiles.len())?;
            let sink = Arc::clone(&sink);
            let rendered_stream_count_for_sink = Arc::clone(&rendered_stream_count);
            let writer_dropped_for_sink = Arc::clone(&tile_cache_writer_dropped_count);
            for render_group in render_tiles.chunks(render_group_size) {
                let rendered = self.renderer.render_web_tiles_blocking(
                    render_group,
                    options.clone(),
                    |planned, tile| {
                        let render_key = self.cache_key_for_planned(&planned, render_format);
                        let session_memory_key =
                            session_tile_memory_key(&render_key, &planned, tile_cache_validation_seed);
                        let cache_write = cache_write_format.and_then(|cache_format| {
                            let Some(chunk_positions) = planned.chunk_positions.as_ref() else {
                                log::trace!(
                                    "skipping cache write without exact chunk positions (dimension={:?}, tile=({}, {}))",
                                    planned.job.coord.dimension,
                                    planned.job.coord.x,
                                    planned.job.coord.z
                                );
                                return None;
                            };
                            Some(TileCacheWrite {
                                key: self.cache_key_for_planned(&planned, cache_format),
                                payload: TileCacheWritePayload::FastRgbaZstd {
                                    rgba: Arc::clone(&tile.rgba),
                                    width: tile.width,
                                    height: tile.height,
                                    region: planned.region,
                                    chunk_positions: Arc::clone(chunk_positions),
                                    validation_seed: tile_cache_validation_seed,
                                    flags: if chunk_positions.is_empty() {
                                        FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE
                                    } else {
                                        FAST_RGBA_ZSTD_FLAG_NON_EMPTY
                                    },
                                },
                            })
                        });
                        rendered_stream_count_for_sink.fetch_add(1, Ordering::Relaxed);
                        log::trace!(
                            "tile rendered (dimension={:?}, tile=({}, {}), width={}, height={}, encoded_bytes={})",
                            planned.job.coord.dimension,
                            planned.job.coord.x,
                            planned.job.coord.z,
                            tile.width,
                            tile.height,
                            tile.rgba.len()
                        );
                        if let Ok(mut cache) = self.cache.lock() {
                            cache.insert_memory(
                                render_key,
                                TileImage {
                                    coord: tile.coord,
                                    width: tile.width,
                                    height: tile.height,
                                    rgba: Arc::clone(&tile.rgba),
                                    encoded: None,
                                },
                            );
                        } else {
                            log::warn!("tile cache lock was poisoned while storing rendered tile");
                        }
                        if let Some(session_memory_key) = session_memory_key {
                            if let Ok(mut session_memory) = self.session_tile_memory.lock() {
                                session_memory.insert(
                                    session_memory_key,
                                    TileImage {
                                        coord: tile.coord,
                                        width: tile.width,
                                        height: tile.height,
                                        rgba: Arc::clone(&tile.rgba),
                                        encoded: None,
                                    },
                                );
                            } else {
                                log::warn!(
                                    "session tile memory lock was poisoned while storing render hit"
                                );
                            }
                        }
                        sink.as_ref()(TileStreamEvent::Ready {
                            planned,
                            tile,
                            source: TileReadySource::Render,
                        })?;
                        if let Some(cache_write) = cache_write {
                            if queue_tile_cache_write(&cache_writer, cache_write)
                                == TileCacheWriteQueueResult::Dropped
                            {
                                writer_dropped_for_sink.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Ok(())
                    },
                );
                match rendered {
                    Ok(result) => {
                        final_diagnostics.add(result.diagnostics);
                        final_stats.add_pipeline_stats(result.stats);
                    }
                    Err(error) => {
                        let cancelled = render_error_is_cancelled(&error);
                        let continue_stream =
                            should_continue_after_stream_group_error(&options, &error);
                        if cancelled {
                            return Err(error);
                        }
                        let message = error.to_string();
                        for planned in render_group.iter().cloned() {
                            failed_stream_count = failed_stream_count.saturating_add(1);
                            log::warn!(
                                "tile render failed (dimension={:?}, tile=({}, {}), error={})",
                                planned.job.coord.dimension,
                                planned.job.coord.x,
                                planned.job.coord.z,
                                message
                            );
                            sink.as_ref()(TileStreamEvent::Failed {
                                planned,
                                error: message.clone(),
                            })?;
                        }
                        if !continue_stream {
                            return Err(error);
                        }
                    }
                }
            }
            final_stats.tile_cache_writer_dropped = final_stats
                .tile_cache_writer_dropped
                .saturating_add(tile_cache_writer_dropped_count.load(Ordering::Relaxed));
            final_stats.planned_tiles = planned_tiles.len();
        }

        let result = RenderWebTilesResult {
            diagnostics: final_diagnostics,
            stats: final_stats,
        };
        log::debug!(
            "streaming web tiles complete (tiles={}, cached={}, cache_misses={}, cache_probes={}, cache_validation_mismatches={}, cache_read_ms={}, cache_decode_ms={}, tile_blob_decode_ms={}, tile_index_trusted_hits={}, tile_index_validated_hits={}, tile_index_misses={}, tile_index_empty_hits={}, tile_index_read_ms={}, tile_dep_validation_ms={}, writer_dropped={}, index_corrupt_misses={}, rendered={}, failed={}, worker_threads={}, world_worker_threads={}, cpu_tiles={}, exact_get_batches={}, exact_keys_requested={}, exact_keys_found={}, render_prefix_scans={}, world_load_ms={}, db_read_ms={}, decode_ms={}, cpu_decode_ms={}, cpu_frame_pack_ms={}, biome_parse_ms={}, subchunk_parse_ms={}, surface_scan_ms={}, block_entity_parse_ms={}, full_reload_ms={}, region_copy_ms={}, elapsed_ms={})",
            planned_tiles.len(),
            cached_stats.cache_hits,
            cached_stats.cache_misses,
            cached_stats.cache_probes,
            cached_stats.cache_validation_mismatches,
            cached_stats.cache_read_ms,
            cached_stats.cache_decode_ms,
            result.stats.tile_blob_decode_ms,
            result.stats.tile_index_trusted_hits,
            result.stats.tile_index_validated_hits,
            result.stats.tile_index_misses,
            result.stats.tile_index_empty_hits,
            result.stats.tile_index_read_ms,
            result.stats.tile_dep_validation_ms,
            result.stats.tile_cache_writer_dropped,
            result.stats.index_corrupt_misses,
            rendered_stream_count.load(Ordering::Relaxed),
            failed_stream_count,
            result.stats.peak_worker_threads,
            result.stats.world_worker_threads,
            result.stats.cpu_tiles,
            result.stats.exact_get_batches,
            result.stats.exact_keys_requested,
            result.stats.exact_keys_found,
            result.stats.render_prefix_scans,
            result.stats.world_load_ms,
            result.stats.db_read_ms,
            result.stats.decode_ms,
            result.stats.cpu_decode_ms,
            result.stats.cpu_frame_pack_ms,
            result.stats.biome_parse_ms,
            result.stats.subchunk_parse_ms,
            result.stats.surface_scan_ms,
            result.stats.block_entity_parse_ms,
            result.stats.full_reload_ms,
            result.stats.region_copy_ms,
            stream_started.elapsed().as_millis()
        );
        sink.as_ref()(TileStreamEvent::Complete {
            diagnostics: result.diagnostics.clone(),
            stats: result.stats.clone(),
        })?;
        Ok(result)
    }

    /// Renders planned web tiles and streams decoded pixel buffers suitable for interactive UI
    /// display.
    ///
    /// # Errors
    ///
    /// Returns an error if rendering, cancellation, cache decoding, or the sink fails.
    #[allow(clippy::too_many_lines)]
    pub fn render_web_tiles_streaming_blocking_v2<F>(
        &self,
        planned_tiles: &[PlannedTile],
        mut options: RenderOptions,
        output: RenderTileOutputOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(TileStreamEventV2) -> Result<()> + Send + Sync + 'static,
    {
        options.format = ImageFormat::Rgba;
        let sink = Arc::new(sink);
        self.render_web_tiles_streaming_blocking(planned_tiles, options, {
            let sink = Arc::clone(&sink);
            move |event| match event {
                TileStreamEvent::Ready {
                    planned,
                    tile,
                    source,
                } => sink.as_ref()(TileStreamEventV2::Ready {
                    planned,
                    tile: decoded_tile_from_tile_image(tile, output),
                    source,
                }),
                TileStreamEvent::Empty { planned } => {
                    sink.as_ref()(TileStreamEventV2::Empty { planned })
                }
                TileStreamEvent::Failed { planned, error } => {
                    sink.as_ref()(TileStreamEventV2::Failed { planned, error })
                }
                TileStreamEvent::Progress(progress) => {
                    sink.as_ref()(TileStreamEventV2::Progress(progress))
                }
                TileStreamEvent::Complete { diagnostics, stats } => {
                    sink.as_ref()(TileStreamEventV2::Complete { diagnostics, stats })
                }
            }
        })
    }

    #[cfg(feature = "async")]
    /// Runs [`MapRenderSession::render_web_tiles_streaming_blocking`] on a Tokio
    /// blocking task.
    ///
    /// # Errors
    ///
    /// Returns an error if the blocking task fails or rendering fails.
    pub async fn render_web_tiles_streaming<F>(
        self: Arc<Self>,
        planned_tiles: Vec<PlannedTile>,
        options: RenderOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(TileStreamEvent) -> Result<()> + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
            self.render_web_tiles_streaming_blocking(&planned_tiles, options, sink)
        })
        .await
        .map_err(|error| BedrockRenderError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Runs [`MapRenderSession::render_web_tiles_streaming_blocking_v2`] on a Tokio
    /// blocking task.
    ///
    /// # Errors
    ///
    /// Returns an error if the blocking task fails or rendering fails.
    pub async fn render_web_tiles_streaming_v2<F>(
        self: Arc<Self>,
        planned_tiles: Vec<PlannedTile>,
        options: RenderOptions,
        output: RenderTileOutputOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(TileStreamEventV2) -> Result<()> + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
            self.render_web_tiles_streaming_blocking_v2(&planned_tiles, options, output, sink)
        })
        .await
        .map_err(|error| BedrockRenderError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Starts streaming web-map tiles on a Tokio blocking task and returns an
    /// async receiver for tile events.
    ///
    /// The task is detached after the receiver is created; render errors are
    /// delivered as `Failed` events when possible and are also logged.
    ///
    /// # Errors
    ///
    /// Returns an error only if the channel capacity is invalid.
    pub async fn render_web_tiles_streaming_channel(
        self: Arc<Self>,
        planned_tiles: Vec<PlannedTile>,
        options: RenderOptions,
        capacity: usize,
    ) -> Result<tokio::sync::mpsc::Receiver<TileStreamEvent>> {
        let capacity = capacity.max(1);
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let error_sender = sender.clone();
        let planned_tiles_for_error = planned_tiles.clone();
        tokio::task::spawn_blocking(move || {
            let send_event = move |event| {
                sender.blocking_send(event).map_err(|_| {
                    BedrockRenderError::Validation("tile stream receiver was dropped".to_string())
                })
            };
            if let Err(error) =
                self.render_web_tiles_streaming_blocking(&planned_tiles, options, send_event)
            {
                log::warn!("tile stream task failed: {error}");
                let message = error.to_string();
                for planned in planned_tiles_for_error {
                    if error_sender
                        .blocking_send(TileStreamEvent::Failed {
                            planned,
                            error: message.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Ok(receiver)
    }

    #[cfg(feature = "async")]
    /// Starts streaming decoded web-map tiles on a Tokio blocking task and returns an
    /// async receiver for tile events.
    ///
    /// # Errors
    ///
    /// Returns an error only if the channel capacity is invalid.
    pub async fn render_web_tiles_streaming_channel_v2(
        self: Arc<Self>,
        planned_tiles: Vec<PlannedTile>,
        options: RenderOptions,
        output: RenderTileOutputOptions,
        capacity: usize,
    ) -> Result<tokio::sync::mpsc::Receiver<TileStreamEventV2>> {
        let capacity = capacity.max(1);
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let error_sender = sender.clone();
        let planned_tiles_for_error = planned_tiles.clone();
        tokio::task::spawn_blocking(move || {
            let send_event = move |event| {
                sender.blocking_send(event).map_err(|_| {
                    BedrockRenderError::Validation(
                        "decoded tile stream receiver was dropped".to_string(),
                    )
                })
            };
            if let Err(error) = self.render_web_tiles_streaming_blocking_v2(
                &planned_tiles,
                options,
                output,
                send_event,
            ) {
                log::warn!("decoded tile stream task failed: {error}");
                let message = error.to_string();
                for planned in planned_tiles_for_error {
                    if error_sender
                        .blocking_send(TileStreamEventV2::Failed {
                            planned,
                            error: message.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Ok(receiver)
    }

    fn prepare_planned_tiles_for_render(
        &self,
        planned_tiles: &[PlannedTile],
        options: &RenderOptions,
    ) -> Result<Vec<PlannedTile>> {
        if planned_tiles.is_empty() || !self.config.cull_missing_chunks {
            return Ok(planned_tiles.to_vec());
        }
        if planned_tiles
            .iter()
            .all(|planned| planned.chunk_positions.is_some())
        {
            return Ok(planned_tiles.to_vec());
        }
        let started = Instant::now();
        let renderable_chunks =
            self.index_renderable_chunks_for_planned_tiles(planned_tiles, options)?;
        let mut prepared = Vec::with_capacity(planned_tiles.len());
        let mut total_selected_chunks = 0usize;
        for planned in planned_tiles {
            if let Some(positions) = &planned.chunk_positions {
                total_selected_chunks = total_selected_chunks.saturating_add(positions.len());
                prepared.push(planned.clone());
                continue;
            }
            let positions = tile_chunk_positions(&planned.job)?
                .into_iter()
                .filter(|pos| renderable_chunks.contains(pos))
                .collect::<Vec<_>>();
            total_selected_chunks = total_selected_chunks.saturating_add(positions.len());
            log::trace!(
                "tile cull complete (dimension={:?}, tile=({}, {}), renderable_chunks={})",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z,
                positions.len()
            );
            let mut planned = planned.clone();
            planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(positions));
            prepared.push(planned);
            check_cancelled(options)?;
        }
        log::debug!(
            "batch tile cull complete (tiles={}, renderable_chunks={}, selected_chunks={}, elapsed_ms={})",
            planned_tiles.len(),
            renderable_chunks.len(),
            total_selected_chunks,
            started.elapsed().as_millis()
        );
        Ok(prepared)
    }

    fn index_renderable_chunks_for_planned_tiles(
        &self,
        planned_tiles: &[PlannedTile],
        options: &RenderOptions,
    ) -> Result<BTreeSet<ChunkPos>> {
        let mut regions = BTreeMap::<Dimension, ChunkRegion>::new();
        for planned in planned_tiles
            .iter()
            .filter(|planned| planned.chunk_positions.is_none())
        {
            let region = ChunkRegion {
                dimension: planned.region.dimension,
                min_chunk_x: planned.region.min_chunk_x,
                min_chunk_z: planned.region.min_chunk_z,
                max_chunk_x: planned.region.max_chunk_x,
                max_chunk_z: planned.region.max_chunk_z,
            };
            regions
                .entry(region.dimension)
                .and_modify(|existing| merge_render_chunk_region(existing, region))
                .or_insert(region);
        }
        if regions.is_empty() {
            return Ok(BTreeSet::new());
        }

        let candidate_chunks = regions
            .values()
            .map(render_chunk_region_area)
            .try_fold(0usize, |total, area| {
                area.map(|area| total.saturating_add(area))
            })?;
        let threading = render_world_threading(options, candidate_chunks.max(planned_tiles.len()))?;
        let scan_options = WorldScanOptions {
            threading,
            pipeline: options.cpu.to_world_pipeline(),
            cancel: render_world_cancel(options),
            ..WorldScanOptions::default()
        };
        let use_full_index = should_use_full_render_chunk_index(candidate_chunks);
        log::debug!(
            "batch tile cull index start (tiles={}, dimensions={}, candidate_chunks={}, strategy={}, workers={:?})",
            planned_tiles.len(),
            regions.len(),
            candidate_chunks,
            if use_full_index {
                "full-index"
            } else {
                "region-index"
            },
            threading
        );

        let mut renderable_chunks = BTreeSet::new();
        if use_full_index {
            let all_positions = self
                .renderer
                .source
                .list_render_chunk_positions_blocking(scan_options)?;
            for pos in all_positions {
                if regions
                    .get(&pos.dimension)
                    .is_some_and(|region| render_chunk_region_contains(*region, pos))
                {
                    renderable_chunks.insert(pos);
                }
            }
        } else {
            for region in regions.values().copied() {
                for pos in self
                    .renderer
                    .source
                    .list_chunk_positions_in_region_blocking(region.into(), scan_options.clone())?
                {
                    renderable_chunks.insert(pos);
                }
            }
        }
        Ok(renderable_chunks)
    }

    fn cache_key_for_planned(&self, planned: &PlannedTile, format: ImageFormat) -> TileCacheKey {
        TileCacheKey {
            world_id: self.config.world_id.clone(),
            world_signature: self.config.world_signature.clone(),
            renderer_version: self.config.renderer_version,
            palette_version: self.config.palette_version,
            dimension: planned.job.coord.dimension,
            mode: mode_slug(planned.job.mode),
            chunks_per_tile: planned.layout.chunks_per_tile,
            blocks_per_pixel: planned.layout.blocks_per_pixel,
            pixels_per_block: planned.layout.pixels_per_block,
            tile_x: planned.job.coord.x,
            tile_z: planned.job.coord.z,
            extension: image_format_extension(format).to_string(),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileCacheEntryDecision {
    Image,
    EmptyNegative,
    Miss,
}

struct TileCacheProbe {
    planned: PlannedTile,
    decision: TileCacheProbeDecision,
    read_ms: u128,
    decode_ms: u128,
    validation_mismatch: bool,
    corrupt_miss: bool,
}

struct TileCacheDecodeScratch {
    decompressor: Option<zstd::bulk::Decompressor<'static>>,
    rgba: Vec<u8>,
}

impl TileCacheDecodeScratch {
    const fn new() -> Self {
        Self {
            decompressor: None,
            rgba: Vec::new(),
        }
    }

    fn decompress_into_scratch(&mut self, payload: &[u8], expected_len: usize) -> Result<()> {
        if self.decompressor.is_none() {
            self.decompressor =
                Some(zstd::bulk::Decompressor::new().map_err(|error| {
                    BedrockRenderError::io("failed to create zstd decoder", error)
                })?);
        }
        let Some(decompressor) = self.decompressor.as_mut() else {
            return Err(BedrockRenderError::Validation(
                "zstd decoder was not initialized".to_string(),
            ));
        };
        self.rgba.clear();
        if self.rgba.capacity() < expected_len {
            self.rgba.reserve_exact(expected_len - self.rgba.capacity());
        }
        let written = decompressor
            .decompress_to_buffer(payload, &mut self.rgba)
            .map_err(|error| BedrockRenderError::io("failed to decode zstd tile", error))?;
        if written != expected_len || self.rgba.len() != expected_len {
            return Err(BedrockRenderError::Validation(format!(
                "fast tile decoded length mismatch: expected {expected_len}, got {}",
                self.rgba.len()
            )));
        }
        Ok(())
    }

    fn decompress_to_vec(&mut self, payload: &[u8], expected_len: usize) -> Result<Vec<u8>> {
        self.decompress_into_scratch(payload, expected_len)?;
        Ok(self.rgba.clone())
    }

    fn decompress_to_arc(&mut self, payload: &[u8], expected_len: usize) -> Result<Arc<[u8]>> {
        self.decompress_into_scratch(payload, expected_len)?;
        Ok(Arc::<[u8]>::from(&self.rgba[..]))
    }
}

enum TileCacheProbeDecision {
    Ready {
        tile: TileImage,
        source: TileReadySource,
    },
    EmptyNegative,
    Miss,
}

fn tile_cache_probe_miss(
    planned: &PlannedTile,
    read_ms: u128,
    decode_ms: u128,
    validation_mismatch: bool,
    corrupt_miss: bool,
) -> TileCacheProbe {
    TileCacheProbe {
        planned: planned.clone(),
        decision: TileCacheProbeDecision::Miss,
        read_ms,
        decode_ms,
        validation_mismatch,
        corrupt_miss,
    }
}

fn resolve_tile_cache_entry(
    cache: &Arc<Mutex<TileCache>>,
    session_tile_memory: &Arc<Mutex<SessionTileMemoryIndex>>,
    authority_snapshots: &BTreeMap<TileAuthorityCacheKey, TileAuthoritySnapshotState>,
    blob_readers: &BTreeMap<TileAuthorityCacheKey, TileAuthorityBlobReaderState>,
    render_key: &TileCacheKey,
    cache_key: &TileCacheKey,
    planned: &PlannedTile,
    format: ImageFormat,
    validation_seed: u64,
    scratch: &mut TileCacheDecodeScratch,
) -> Result<TileCacheProbe> {
    let mut read_ms = 0;
    let mut decode_ms = 0;
    let session_memory_key = session_tile_memory_key(render_key, planned, validation_seed);
    if let Some(memory_key) = session_memory_key.as_ref()
        && let Some(tile) = session_tile_memory
            .lock()
            .map_err(|_| {
                BedrockRenderError::Validation("session tile memory lock was poisoned".to_string())
            })?
            .get(memory_key)
    {
        return Ok(TileCacheProbe {
            planned: planned.clone(),
            decision: TileCacheProbeDecision::Ready {
                tile,
                source: TileReadySource::MemoryCache,
            },
            read_ms,
            decode_ms,
            validation_mismatch: false,
            corrupt_miss: false,
        });
    }

    if let Some(tile) = cache
        .lock()
        .map_err(|_| BedrockRenderError::Validation("tile cache lock was poisoned".to_string()))?
        .get_memory(render_key)
    {
        if let Some(memory_key) = session_memory_key {
            if let Ok(mut session_memory) = session_tile_memory.lock() {
                session_memory.insert(memory_key, tile.clone());
            } else {
                log::warn!("session tile memory lock was poisoned while storing cache hit");
            }
        }
        return Ok(TileCacheProbe {
            planned: planned.clone(),
            decision: TileCacheProbeDecision::Ready {
                tile,
                source: TileReadySource::MemoryCache,
            },
            read_ms,
            decode_ms,
            validation_mismatch: false,
            corrupt_miss: false,
        });
    }

    if format != ImageFormat::FastRgbaZstd {
        return Ok(tile_cache_probe_miss(
            planned, read_ms, decode_ms, false, false,
        ));
    }

    let read_started = Instant::now();
    let authority_key = tile_authority_cache_key_from_tile_key(cache_key);
    let disk_read = match authority_snapshots.get(&authority_key) {
        Some(TileAuthoritySnapshotState::Ready(authority_snapshot)) => {
            let blob_reader = match blob_readers.get(&authority_key) {
                Some(TileAuthorityBlobReaderState::Ready(reader)) => Some(Arc::as_ref(reader)),
                Some(TileAuthorityBlobReaderState::Missing) => None,
                None => None,
            };
            let blob_reader_known_missing = matches!(
                blob_readers.get(&authority_key),
                Some(TileAuthorityBlobReaderState::Missing)
            );
            let authority = cache.lock().map_err(|_| {
                BedrockRenderError::Validation("tile cache lock was poisoned".to_string())
            })?;
            let authority = Arc::clone(&authority.authority);
            TileCache::get_disk_with_snapshot(
                authority.as_ref(),
                cache_key,
                &authority_key,
                Some(authority_snapshot),
                blob_reader,
                blob_reader_known_missing,
            )
        }
        Some(TileAuthoritySnapshotState::Corrupt) => TileCacheDiskRead::Corrupt,
        Some(TileAuthoritySnapshotState::Missing) | None => TileCacheDiskRead::Miss,
    };
    let encoded = match disk_read {
        TileCacheDiskRead::Hit(encoded) => encoded,
        TileCacheDiskRead::Miss => {
            read_ms = read_started.elapsed().as_millis();
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, false, false,
            ));
        }
        TileCacheDiskRead::Corrupt => {
            read_ms = read_started.elapsed().as_millis();
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, false, true,
            ));
        }
    };
    read_ms = read_started.elapsed().as_millis();

    let header = match decode_fast_rgba_zstd_header(&encoded) {
        Ok(header) => header,
        Err(error) => {
            log::debug!(
                "fast tile cache header rejected (dimension={:?}, tile=({}, {}), error={})",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z,
                error
            );
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, false, true,
            ));
        }
    };
    if header.width != planned.job.tile_size || header.height != planned.job.tile_size {
        return Ok(tile_cache_probe_miss(
            planned, read_ms, decode_ms, false, true,
        ));
    }

    let chunk_positions = planned.chunk_positions.as_deref();
    if validation_seed != 0 {
        let Some(chunk_positions) = chunk_positions else {
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, false, false,
            ));
        };
        let expected = tile_cache_validation_value(
            cache_key,
            &planned.region,
            chunk_positions,
            validation_seed,
        );
        if header.validation_value != Some(expected) {
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, true, false,
            ));
        }
    }

    if header.is_empty_negative() {
        return Ok(TileCacheProbe {
            planned: planned.clone(),
            decision: if chunk_positions.is_some_and(<[ChunkPos]>::is_empty) {
                TileCacheProbeDecision::EmptyNegative
            } else {
                TileCacheProbeDecision::Miss
            },
            read_ms,
            decode_ms,
            validation_mismatch: false,
            corrupt_miss: !chunk_positions.is_some_and(<[ChunkPos]>::is_empty),
        });
    }

    let decode_started = Instant::now();
    let decoded = match decode_fast_rgba_zstd_shared_with_header(&encoded, header, scratch) {
        Ok(decoded) => decoded,
        Err(error) => {
            decode_ms = decode_started.elapsed().as_millis();
            log::debug!(
                "fast tile cache payload rejected (dimension={:?}, tile=({}, {}), error={})",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z,
                error
            );
            return Ok(tile_cache_probe_miss(
                planned, read_ms, decode_ms, false, true,
            ));
        }
    };
    decode_ms = decode_started.elapsed().as_millis();

    if decoded.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        return Ok(TileCacheProbe {
            planned: planned.clone(),
            decision: if chunk_positions.is_some_and(<[ChunkPos]>::is_empty) {
                TileCacheProbeDecision::EmptyNegative
            } else {
                TileCacheProbeDecision::Miss
            },
            read_ms,
            decode_ms,
            validation_mismatch: false,
            corrupt_miss: !chunk_positions.is_some_and(<[ChunkPos]>::is_empty),
        });
    }

    let tile = TileImage {
        coord: planned.job.coord,
        width: decoded.width,
        height: decoded.height,
        rgba: decoded.rgba,
        encoded: Some(encoded),
    };
    let source = if header.validation_value.is_some() {
        TileReadySource::DiskCacheFresh
    } else {
        TileReadySource::DiskCacheStale
    };
    cache
        .lock()
        .map_err(|_| BedrockRenderError::Validation("tile cache lock was poisoned".to_string()))?
        .insert_memory(
            render_key.clone(),
            TileImage {
                coord: tile.coord,
                width: tile.width,
                height: tile.height,
                rgba: Arc::clone(&tile.rgba),
                encoded: None,
            },
        );
    if let Some(memory_key) = session_memory_key {
        if let Ok(mut session_memory) = session_tile_memory.lock() {
            session_memory.insert(
                memory_key,
                TileImage {
                    coord: tile.coord,
                    width: tile.width,
                    height: tile.height,
                    rgba: Arc::clone(&tile.rgba),
                    encoded: None,
                },
            );
        } else {
            log::warn!("session tile memory lock was poisoned while storing disk hit");
        }
    }
    Ok(TileCacheProbe {
        planned: planned.clone(),
        decision: TileCacheProbeDecision::Ready { tile, source },
        read_ms,
        decode_ms,
        validation_mismatch: false,
        corrupt_miss: false,
    })
}

fn empty_negative_tile_cache_write(
    key: &TileCacheKey,
    planned: &PlannedTile,
    format: ImageFormat,
    validation_seed: u64,
) -> Option<TileCacheWrite> {
    if format != ImageFormat::FastRgbaZstd || validation_seed == 0 {
        return None;
    }
    let chunk_positions = planned.chunk_positions.as_deref()?;
    if !chunk_positions.is_empty() {
        return None;
    }
    Some(TileCacheWrite {
        key: key.clone(),
        payload: TileCacheWritePayload::EmptyNegative {
            tile_size: planned.job.tile_size,
            region: planned.region,
            validation_seed,
        },
    })
}

#[cfg(test)]
fn tile_cache_entry_decision(
    encoded: &[u8],
    key: &TileCacheKey,
    planned: &PlannedTile,
    format: ImageFormat,
    validation_seed: u64,
) -> Result<TileCacheEntryDecision> {
    if format != ImageFormat::FastRgbaZstd {
        return Ok(TileCacheEntryDecision::Image);
    }
    let header = match decode_fast_rgba_zstd_header(encoded) {
        Ok(header) => header,
        Err(error) => {
            log::debug!(
                "fast tile cache header rejected (dimension={:?}, tile=({}, {}), error={})",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z,
                error
            );
            return Ok(TileCacheEntryDecision::Miss);
        }
    };
    if header.width != planned.job.tile_size || header.height != planned.job.tile_size {
        return Ok(TileCacheEntryDecision::Miss);
    }
    let chunk_positions = planned.chunk_positions.as_deref();
    if validation_seed != 0 {
        let Some(chunk_positions) = chunk_positions else {
            log::trace!(
                "tile cache validation skipped without exact chunk positions (dimension={:?}, tile=({}, {}))",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z
            );
            return Ok(TileCacheEntryDecision::Miss);
        };
        let expected =
            tile_cache_validation_value(key, &planned.region, chunk_positions, validation_seed);
        if header.validation_value != Some(expected) {
            return Ok(TileCacheEntryDecision::Miss);
        }
    }
    if header.is_empty_negative() {
        return if chunk_positions.is_some_and(<[ChunkPos]>::is_empty) {
            Ok(TileCacheEntryDecision::EmptyNegative)
        } else {
            Ok(TileCacheEntryDecision::Miss)
        };
    }
    if header.is_non_empty() {
        return Ok(TileCacheEntryDecision::Image);
    }

    match decode_fast_rgba_zstd(encoded) {
        Ok(decoded) if decoded.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0) => {
            Ok(TileCacheEntryDecision::Image)
        }
        Ok(_) if chunk_positions.is_some_and(<[ChunkPos]>::is_empty) => {
            Ok(TileCacheEntryDecision::EmptyNegative)
        }
        Ok(_) => Ok(TileCacheEntryDecision::Miss),
        Err(error) => {
            log::debug!(
                "legacy fast tile cache payload rejected (dimension={:?}, tile=({}, {}), error={})",
                planned.job.coord.dimension,
                planned.job.coord.x,
                planned.job.coord.z,
                error
            );
            Ok(TileCacheEntryDecision::Miss)
        }
    }
}

/// Renderer handle for Bedrock world tile rendering.
#[derive(Clone)]
pub struct MapRenderer<S = Arc<dyn WorldStorage>>
where
    S: WorldStorageHandle,
{
    source: Arc<dyn RenderChunkSource>,
    palette: Arc<RenderPalette>,
    region_bake_cache: Option<Arc<Mutex<RegionBakeMemoryCache>>>,
    gpu: Option<GpuRenderContext>,
    _marker: PhantomData<fn() -> S>,
}

impl<S> MapRenderer<S>
where
    S: WorldStorageHandle,
{
    /// Creates a renderer from a world handle and palette.
    #[must_use]
    pub fn new(world: Arc<BedrockWorld<S>>, palette: impl Into<Arc<RenderPalette>>) -> Self {
        Self::from_source(world, palette)
    }

    /// Creates a renderer from a render chunk source and palette.
    #[must_use]
    pub fn from_source<T>(source: Arc<T>, palette: impl Into<Arc<RenderPalette>>) -> Self
    where
        T: RenderChunkSource + 'static,
    {
        Self {
            source,
            palette: palette.into(),
            region_bake_cache: None,
            gpu: None,
            _marker: PhantomData,
        }
    }
    fn with_region_bake_cache(mut self, cache: Arc<Mutex<RegionBakeMemoryCache>>) -> Self {
        self.region_bake_cache = Some(cache);
        self
    }

    fn with_gpu_context(mut self, gpu: Option<GpuRenderContext>) -> Self {
        self.gpu = gpu;
        self
    }

    /// Renders one tile with default options.
    ///
    /// # Errors
    ///
    /// Returns an error if the job is invalid, world reads fail, rendering is cancelled,
    /// or the requested output cannot be encoded.
    pub fn render_tile_blocking(&self, job: RenderJob) -> Result<TileImage> {
        self.render_tile_with_options_blocking(job, &RenderOptions::default())
    }

    /// Renders one tile with explicit options.
    ///
    /// # Errors
    ///
    /// Returns an error if the job/options are invalid, world reads fail, rendering is
    /// cancelled, or the requested output cannot be encoded.
    pub fn render_tile_with_options_blocking(
        &self,
        job: RenderJob,
        options: &RenderOptions,
    ) -> Result<TileImage> {
        validate_job(&job)?;
        self.render_tile_from_bake_blocking(job, options)
    }

    /// Renders a batch of tiles.
    ///
    /// # Errors
    ///
    /// Returns an error if any tile fails, rendering is cancelled, or the thread options
    /// are invalid.
    #[allow(clippy::needless_pass_by_value)]
    pub fn render_tiles_blocking(
        &self,
        jobs: impl IntoIterator<Item = RenderJob>,
        options: RenderOptions,
    ) -> Result<Vec<TileImage>> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let total_tiles = jobs.len();
        let worker_count = resolve_render_worker_count(&options, total_tiles)?;
        if worker_count == 1 {
            let mut tiles = Vec::with_capacity(total_tiles);
            for job in jobs {
                check_cancelled(&options)?;
                tiles.push(self.render_tile_with_options_blocking(job, &options)?);
                emit_progress(&options, tiles.len(), total_tiles);
            }
            return Ok(tiles);
        }

        self.render_tiles_from_shared_bakes_blocking(&jobs, &options)
    }

    #[allow(clippy::too_many_lines)]
    fn render_tiles_from_shared_bakes_blocking(
        &self,
        jobs: &[RenderJob],
        options: &RenderOptions,
    ) -> Result<Vec<TileImage>> {
        let total_tiles = jobs.len();
        let mut tile_dependencies = Vec::with_capacity(total_tiles);
        let mut dependents = BTreeMap::<ChunkBakeKey, Vec<usize>>::new();
        let mut bake_key_set = BTreeSet::<ChunkBakeKey>::new();

        for (tile_index, job) in jobs.iter().enumerate() {
            validate_job(job)?;
            let mut dependencies = tile_chunk_positions(job)?
                .into_iter()
                .map(|pos| ChunkBakeKey {
                    pos,
                    mode: job.mode,
                })
                .collect::<Vec<_>>();
            dependencies.sort();
            dependencies.dedup();
            for key in dependencies.iter().copied() {
                bake_key_set.insert(key);
                dependents.entry(key).or_default().push(tile_index);
            }
            tile_dependencies.push(dependencies);
        }

        let bake_keys = bake_key_set.into_iter().collect::<Vec<_>>();
        if bake_keys.is_empty() {
            return Ok(Vec::new());
        }

        let worker_count = resolve_render_worker_count(options, bake_keys.len().max(total_tiles))?;
        let queue_capacity = pipeline_queue_capacity(options, worker_count);
        let loader_count =
            pipeline_loader_count_for_options(options, worker_count, bake_keys.len());
        let compose_count =
            pipeline_compose_count_for_options(options, worker_count, loader_count, total_tiles);
        let bake_count = pipeline_bake_count_for_options(
            options,
            worker_count,
            loader_count,
            compose_count,
            bake_keys.len(),
        );
        let (load_sender, load_receiver) = crossbeam_channel::bounded(queue_capacity);
        let (loaded_sender, loaded_receiver) = crossbeam_channel::bounded(queue_capacity);
        let (baked_sender, baked_receiver) = crossbeam_channel::bounded(queue_capacity);
        let (compose_sender, compose_receiver) =
            crossbeam_channel::bounded::<TileComposeTask>(queue_capacity);
        let (tile_sender, tile_receiver) =
            crossbeam_channel::bounded::<Result<TileComposeResult>>(queue_capacity);

        let pool = render_cpu_pool(worker_count)?;
        pool.scope(|scope| {
            {
                let load_sender = load_sender.clone();
                let bake_keys = &bake_keys;
                scope.spawn(move |_| {
                    for key in bake_keys.iter().copied() {
                        if load_sender.send(key).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(load_sender);

            for _ in 0..loader_count {
                let load_receiver = load_receiver.clone();
                let loaded_sender = loaded_sender.clone();
                let renderer = self.clone();
                let options = options.clone();
                scope.spawn(move |_| {
                    for key in load_receiver {
                        if check_cancelled(&options).is_err() {
                            let _ = loaded_sender.send(Err(BedrockRenderError::Cancelled));
                            return;
                        }
                        let result = renderer
                            .load_render_chunk_data_with_options_blocking(
                                key.pos, key.mode, &options,
                            )
                            .map(|data| (key, data));
                        if loaded_sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(loaded_sender);

            for _ in 0..bake_count {
                let loaded_receiver = loaded_receiver.clone();
                let baked_sender = baked_sender.clone();
                let renderer = self.clone();
                let options = options.clone();
                scope.spawn(move |_| {
                    for message in loaded_receiver {
                        let (key, data) = match message {
                            Ok(value) => value,
                            Err(error) => {
                                let _ = baked_sender.send(Err(error));
                                return;
                            }
                        };
                        if check_cancelled(&options).is_err() {
                            let _ = baked_sender.send(Err(BedrockRenderError::Cancelled));
                            return;
                        }
                        let result = renderer
                            .bake_chunk_data(
                                data,
                                BakeOptions {
                                    mode: key.mode,
                                    surface: options.surface,
                                },
                            )
                            .map(|bake| (key, bake));
                        if baked_sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(baked_sender);

            for _ in 0..compose_count {
                let compose_receiver = compose_receiver.clone();
                let tile_sender = tile_sender.clone();
                let renderer = self.clone();
                let options = options.clone();
                scope.spawn(move |_| {
                    for task in compose_receiver {
                        let result = renderer
                            .render_tile_from_chunk_bakes_blocking(
                                task.job,
                                &options,
                                &task.bakes,
                                task.diagnostics,
                            )
                            .map(|tile| TileComposeResult {
                                tile_index: task.tile_index,
                                tile,
                            });
                        if tile_sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(compose_receiver);
            drop(tile_sender);

            let mut bakes = BTreeMap::<ChunkBakeKey, ChunkBake>::new();
            let mut remaining_dependencies =
                tile_dependencies.iter().map(Vec::len).collect::<Vec<_>>();
            let mut tiles = vec![None; total_tiles];
            let mut submitted_tiles = 0usize;
            let mut completed_tiles = 0usize;

            for message in baked_receiver {
                let (key, bake) = message?;
                bakes.insert(key, bake);
                if let Some(tile_indexes) = dependents.get(&key) {
                    for tile_index in tile_indexes.iter().copied() {
                        let Some(remaining) = remaining_dependencies.get_mut(tile_index) else {
                            return Err(BedrockRenderError::Validation(
                                "tile dependency index is out of range".to_string(),
                            ));
                        };
                        *remaining = remaining.saturating_sub(1);
                        if *remaining != 0 {
                            continue;
                        }
                        let (tile_bakes, diagnostics) =
                            collect_tile_chunk_bakes(&tile_dependencies[tile_index], &bakes)?;
                        if compose_count == 0 {
                            let tile = self.render_tile_from_chunk_bakes_blocking(
                                jobs[tile_index].clone(),
                                options,
                                &tile_bakes,
                                diagnostics,
                            )?;
                            store_tile_compose_result(
                                Ok(TileComposeResult { tile_index, tile }),
                                &mut tiles,
                                &mut completed_tiles,
                                total_tiles,
                                options,
                            )?;
                            submitted_tiles = submitted_tiles.saturating_add(1);
                            if submitted_tiles == total_tiles {
                                break;
                            }
                            continue;
                        }
                        let mut task = TileComposeTask {
                            tile_index,
                            job: jobs[tile_index].clone(),
                            bakes: tile_bakes,
                            diagnostics,
                        };
                        loop {
                            match compose_sender.try_send(task) {
                                Ok(()) => {
                                    submitted_tiles = submitted_tiles.saturating_add(1);
                                    break;
                                }
                                Err(crossbeam_channel::TrySendError::Full(returned_task)) => {
                                    task = returned_task;
                                    let message = tile_receiver.recv().map_err(|_| {
                                        BedrockRenderError::Validation(
                                            "tile compose pipeline stopped early".to_string(),
                                        )
                                    })?;
                                    store_tile_compose_result(
                                        message,
                                        &mut tiles,
                                        &mut completed_tiles,
                                        total_tiles,
                                        options,
                                    )?;
                                }
                                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                    return Err(BedrockRenderError::Validation(
                                        "tile compose worker stopped early".to_string(),
                                    ));
                                }
                            }
                        }
                        if submitted_tiles == total_tiles {
                            break;
                        }
                    }
                }
                if submitted_tiles == total_tiles {
                    break;
                }
            }
            drop(compose_sender);

            if submitted_tiles != total_tiles {
                return Err(BedrockRenderError::Validation(
                    "shared tile bake pipeline stopped before all tiles completed".to_string(),
                ));
            }
            while completed_tiles < total_tiles {
                let message = tile_receiver.recv().map_err(|_| {
                    BedrockRenderError::Validation(
                        "tile compose pipeline stopped before all tiles completed".to_string(),
                    )
                })?;
                store_tile_compose_result(
                    message,
                    &mut tiles,
                    &mut completed_tiles,
                    total_tiles,
                    options,
                )?;
            }

            tiles
                .into_iter()
                .map(|tile| {
                    tile.ok_or_else(|| {
                        BedrockRenderError::Validation(
                            "rendered tile result is missing".to_string(),
                        )
                    })
                })
                .collect()
        })
    }

    /// Plans all tiles intersecting a chunk region.
    ///
    /// # Errors
    ///
    /// Returns an error if the region or layout is invalid or the resulting plan is too large.
    pub fn plan_region_tiles(
        region: ChunkRegion,
        mode: RenderMode,
        layout: ChunkTileLayout,
    ) -> Result<Vec<PlannedTile>> {
        validate_region(region)?;
        validate_layout(layout)?;
        let chunks_per_tile = i32::try_from(layout.chunks_per_tile).map_err(|_| {
            BedrockRenderError::Validation("chunks_per_tile is outside i32 range".to_string())
        })?;
        let min_tile_x = floor_div(region.min_chunk_x, chunks_per_tile);
        let max_tile_x = floor_div(region.max_chunk_x, chunks_per_tile);
        let min_tile_z = floor_div(region.min_chunk_z, chunks_per_tile);
        let max_tile_z = floor_div(region.max_chunk_z, chunks_per_tile);
        let x_tile_count = i64::from(max_tile_x) - i64::from(min_tile_x) + 1;
        let z_tile_count = i64::from(max_tile_z) - i64::from(min_tile_z) + 1;
        let capacity =
            usize::try_from(x_tile_count.saturating_mul(z_tile_count)).map_err(|_| {
                BedrockRenderError::Validation("planned tile count is too large".to_string())
            })?;
        let mut tiles = Vec::with_capacity(capacity);
        for z in min_tile_z..=max_tile_z {
            for x in min_tile_x..=max_tile_x {
                let job = RenderJob::chunk_tile(
                    TileCoord {
                        x,
                        z,
                        dimension: region.dimension,
                    },
                    mode,
                    layout,
                )?;
                tiles.push(PlannedTile {
                    job,
                    region,
                    layout,
                    chunk_positions: None,
                });
            }
        }
        Ok(tiles)
    }

    /// Renders all tiles intersecting a region and returns them as a tile set.
    ///
    /// # Errors
    ///
    /// Returns an error if planning, rendering, encoding, cancellation, or result
    /// aggregation fails.
    pub fn render_region_tiles_blocking(
        &self,
        region: ChunkRegion,
        mode: RenderMode,
        layout: ChunkTileLayout,
        options: RenderOptions,
    ) -> Result<TileSet> {
        let planned_tiles = Self::plan_region_tiles(region, mode, layout)?;
        let tiles = Arc::new(Mutex::new(Vec::with_capacity(planned_tiles.len())));
        self.render_tiles_from_regions_blocking(&planned_tiles, options, {
            let tiles = Arc::clone(&tiles);
            move |_planned, tile| {
                tiles
                    .lock()
                    .map_err(|_| {
                        BedrockRenderError::Validation(
                            "render region tile result lock failed".to_string(),
                        )
                    })?
                    .push(tile);
                Ok(())
            }
        })?;
        let tiles = Arc::try_unwrap(tiles)
            .map_err(|_| {
                BedrockRenderError::Validation(
                    "render region tile results still shared".to_string(),
                )
            })?
            .into_inner()
            .map_err(|_| {
                BedrockRenderError::Validation("render region tile result lock failed".to_string())
            })?;
        Ok(TileSet { tiles })
    }

    /// Renders an arbitrary iterator of jobs and wraps the result in a tile set.
    ///
    /// # Errors
    ///
    /// Returns an error if any tile render fails.
    pub fn render_region_blocking(
        &self,
        jobs: impl IntoIterator<Item = RenderJob>,
        options: RenderOptions,
    ) -> Result<TileSet> {
        Ok(TileSet {
            tiles: self.render_tiles_blocking(jobs, options)?,
        })
    }

    /// Bakes all chunks in a region.
    ///
    /// # Errors
    ///
    /// Returns an error if the region layout is invalid, world reads fail, or baking is cancelled.
    pub fn bake_region_blocking(
        &self,
        coord: RegionCoord,
        options: &RenderOptions,
        mode: RenderMode,
    ) -> Result<RegionBake> {
        options.region_layout.validate()?;
        let chunk_region = coord.chunk_region(options.region_layout);
        self.bake_region_chunk_region_blocking(coord, chunk_region, options, mode)
    }

    fn bake_region_chunk_region_blocking(
        &self,
        coord: RegionCoord,
        chunk_region: ChunkRegion,
        options: &RenderOptions,
        mode: RenderMode,
    ) -> Result<RegionBake> {
        options.region_layout.validate()?;
        let world_region = WorldChunkQueryRegion {
            dimension: chunk_region.dimension,
            min_chunk_x: chunk_region.min_chunk_x,
            min_chunk_z: chunk_region.min_chunk_z,
            max_chunk_x: chunk_region.max_chunk_x,
            max_chunk_z: chunk_region.max_chunk_z,
        };
        let threading = render_world_threading(options, render_chunk_region_area(&chunk_region)?)?;
        let data = self.source.query_chunk_region_blocking(
            world_region,
            WorldChunkQueryRegionLoadOptions {
                data_request: render_chunk_request_for_options(mode, options),
                subchunk_decode: render_subchunk_decode_mode(mode),
                threading,
                pipeline: options.cpu.to_world_pipeline(),
                cancel: render_world_cancel(options),
                priority: render_chunk_priority_for_region(options, chunk_region),
                ..WorldChunkQueryRegionLoadOptions::default()
            },
        )?;
        let load_stats = data.stats;
        self.bake_loaded_region_chunks(
            coord,
            chunk_region,
            data.chunks,
            Some(load_stats),
            options,
            mode,
        )
    }

    fn bake_region_chunk_positions_blocking(
        &self,
        coord: RegionCoord,
        chunk_region: ChunkRegion,
        chunk_positions: &[ChunkPos],
        options: &RenderOptions,
        mode: RenderMode,
        world_workers: usize,
    ) -> Result<RegionBake> {
        options.region_layout.validate()?;
        let threading =
            render_world_threading_with_budget(options, chunk_positions.len(), world_workers);
        let (data, load_stats) = self.source.query_chunk_data_with_stats_blocking(
            chunk_positions,
            ChunkLoadOptions {
                data_request: render_chunk_request_for_options(mode, options),
                subchunk_decode: render_subchunk_decode_mode(mode),
                threading,
                pipeline: options.cpu.to_world_pipeline(),
                cancel: render_world_cancel(options),
                priority: render_chunk_priority_for_region(options, chunk_region),
                storage_cache_policy: render_storage_cache_policy(options),
                ..ChunkLoadOptions::default()
            },
        )?;
        self.bake_loaded_region_chunks_with_cached(
            coord,
            chunk_region,
            data,
            Some(load_stats),
            options,
            mode,
        )
    }

    fn bake_loaded_region_chunks(
        &self,
        coord: RegionCoord,
        chunk_region: ChunkRegion,
        chunks: Vec<ChunkData>,
        load_stats: Option<ChunkLoadStats>,
        options: &RenderOptions,
        mode: RenderMode,
    ) -> Result<RegionBake> {
        self.bake_loaded_region_chunks_with_cached(
            coord,
            chunk_region,
            chunks,
            load_stats,
            options,
            mode,
        )
    }

    fn bake_loaded_region_chunks_with_cached(
        &self,
        coord: RegionCoord,
        chunk_region: ChunkRegion,
        chunks: Vec<ChunkData>,
        load_stats: Option<ChunkLoadStats>,
        options: &RenderOptions,
        mode: RenderMode,
    ) -> Result<RegionBake> {
        let width = options.region_layout.chunks_per_region.saturating_mul(16);
        let height = width;
        let mut payload = empty_region_payload(
            mode,
            options.surface,
            width,
            height,
            self.palette.missing_chunk_color(),
        )?;
        let mut diagnostics = RenderDiagnostics::default();
        let base_region = coord.chunk_region(options.region_layout);
        let mut chunks_copied = 0usize;
        let mut chunks_out_of_bounds = 0usize;
        let copy_started = Instant::now();
        for chunk_data in chunks {
            check_cancelled(options)?;
            let bake = self.bake_chunk_data(
                chunk_data,
                BakeOptions {
                    mode,
                    surface: options.surface,
                },
            )?;
            diagnostics.add(bake.diagnostics.clone());
            if copy_chunk_bake_to_region_checked(&bake, &mut payload, base_region) {
                chunks_copied = chunks_copied.saturating_add(1);
            } else {
                chunks_out_of_bounds = chunks_out_of_bounds.saturating_add(1);
            }
        }
        let copy_ms = copy_started.elapsed().as_millis();
        log::debug!(
            "region payload copy complete (coord=({}, {:?}, {}), mode={:?}, chunks_copied={}, chunks_out_of_bounds={}, copy_ms={})",
            coord.x,
            coord.dimension,
            coord.z,
            mode,
            chunks_copied,
            chunks_out_of_bounds,
            copy_ms
        );
        Ok(RegionBake {
            coord,
            layout: options.region_layout,
            mode,
            covered_chunk_region: chunk_region,
            chunk_region: base_region,
            payload,
            diagnostics,
            load_stats,
            copy_ms,
            chunks_copied,
            chunks_out_of_bounds,
        })
    }

    fn load_cached_region_bake(
        &self,
        key: RegionBakeKey,
        options: &RenderOptions,
    ) -> Result<Option<Arc<RegionBake>>> {
        let Some(cache) = &self.region_bake_cache else {
            return Ok(None);
        };
        let mut cache = cache.lock().map_err(|_| {
            BedrockRenderError::Validation("region bake cache lock was poisoned".to_string())
        })?;
        Ok(cache.get(key, options))
    }

    fn store_region_bake_cache(
        &self,
        key: RegionBakeKey,
        bake: &Arc<RegionBake>,
        options: &RenderOptions,
    ) -> Result<()> {
        let Some(cache) = &self.region_bake_cache else {
            return Ok(());
        };
        let mut cache = cache.lock().map_err(|_| {
            BedrockRenderError::Validation("region bake cache lock was poisoned".to_string())
        })?;
        cache.insert(key, Arc::clone(bake), options);
        Ok(())
    }

    /// Renders planned tiles from region bakes and streams each tile to a sink.
    ///
    /// # Errors
    ///
    /// Returns an error if region baking, tile composition, encoding, cancellation,
    /// or the sink callback fails.
    #[allow(clippy::needless_pass_by_value)]
    pub fn render_tiles_from_regions_blocking<F>(
        &self,
        planned_tiles: &[PlannedTile],
        options: RenderOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(PlannedTile, TileImage) -> Result<()> + Send + Sync,
    {
        self.render_web_regions_blocking(planned_tiles, &options, sink)
    }

    /// Renders web-map planned tiles and streams each tile to a sink.
    ///
    /// # Errors
    ///
    /// Returns an error if region baking, tile composition, encoding, cancellation,
    /// or the sink callback fails.
    #[allow(clippy::needless_pass_by_value)]
    pub fn render_web_tiles_blocking<F>(
        &self,
        planned_tiles: &[PlannedTile],
        options: RenderOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(PlannedTile, TileImage) -> Result<()> + Send + Sync,
    {
        self.render_web_regions_blocking(planned_tiles, &options, sink)
    }

    fn render_web_regions_blocking<F>(
        &self,
        planned_tiles: &[PlannedTile],
        options: &RenderOptions,
        sink: F,
    ) -> Result<RenderWebTilesResult>
    where
        F: Fn(PlannedTile, TileImage) -> Result<()> + Send + Sync,
    {
        let render_started = Instant::now();
        if planned_tiles.is_empty() {
            return Ok(RenderWebTilesResult {
                diagnostics: RenderDiagnostics::default(),
                stats: RenderPipelineStats::default(),
            });
        }
        options.region_layout.validate()?;
        let region_plans = collect_region_plans(planned_tiles, options.region_layout)?;
        let planned_chunk_count = region_plans.iter().fold(0usize, |total, plan| {
            total.saturating_add(plan.chunk_positions.len())
        });
        let mut stats = RenderPipelineStats {
            planned_tiles: planned_tiles.len(),
            planned_regions: region_plans.len(),
            unique_chunks: planned_chunk_count,
            ..RenderPipelineStats::default()
        };
        let region_plan_by_key = region_plans
            .iter()
            .map(|plan| (plan.key, plan.clone()))
            .collect::<BTreeMap<_, _>>();
        let tile_region_plans = planned_tiles
            .iter()
            .map(|planned| tile_region_plans(planned, options.region_layout))
            .collect::<Result<Vec<_>>>()?;
        let memory_budget = options
            .memory_budget
            .resolve_bytes(options.execution_profile)
            .unwrap_or(usize::MAX);
        let mut pending_tiles = (0..planned_tiles.len()).collect::<Vec<_>>();
        let mut diagnostics = RenderDiagnostics::default();
        let worker_count = resolve_render_worker_count(options, planned_tiles.len())?;
        stats.active_tasks_peak = 1;
        stats.peak_worker_threads = worker_count;

        while !pending_tiles.is_empty() {
            check_cancelled(options)?;
            let wave_keys = select_region_wave(
                &pending_tiles,
                &tile_region_plans,
                &region_plan_by_key,
                memory_budget,
            )?;
            let wave_plans = wave_keys
                .iter()
                .map(|key| region_plan_by_key.get(key).cloned())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    BedrockRenderError::Validation("planned tile references missing region".into())
                })?;
            let wave_cache_bytes = wave_plans.iter().fold(0usize, |total, plan| {
                total.saturating_add(
                    plan.chunk_positions
                        .len()
                        .saturating_mul(REGION_BAKE_ESTIMATED_BYTES_PER_CHUNK),
                )
            });
            stats.peak_cache_bytes = stats.peak_cache_bytes.max(wave_cache_bytes);
            let wave_result = self.render_web_region_wave_streaming(
                planned_tiles,
                &pending_tiles,
                &tile_region_plans,
                &wave_plans,
                options,
                worker_count,
                &sink,
            )?;
            diagnostics.add(wave_result.diagnostics);
            stats.add_pipeline_stats(wave_result.stats);
            stats.baked_chunks = diagnostics.baked_chunks;
            pending_tiles.retain(|index| !wave_result.rendered_tiles.contains(index));
        }
        finalize_pipeline_throughput(&mut stats, render_started.elapsed());
        Ok(RenderWebTilesResult { diagnostics, stats })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_web_region_wave_streaming<F>(
        &self,
        planned_tiles: &[PlannedTile],
        pending_tiles: &[usize],
        tile_region_plans: &[Vec<RegionPlan>],
        wave_plans: &[RegionPlan],
        options: &RenderOptions,
        worker_count: usize,
        sink: &F,
    ) -> Result<RegionWaveRenderResult>
    where
        F: Fn(PlannedTile, TileImage) -> Result<()> + Send + Sync,
    {
        if wave_plans.is_empty() {
            return Ok(RegionWaveRenderResult {
                rendered_tiles: BTreeSet::new(),
                diagnostics: RenderDiagnostics::default(),
                stats: RenderPipelineStats::default(),
            });
        }

        let worker_split = region_wave_worker_split(options, worker_count, wave_plans.len());
        let region_worker_count = worker_split.region_workers.max(1);
        let queue_capacity = pipeline_queue_capacity(options, region_worker_count);
        let next_plan = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = crossbeam_channel::bounded(queue_capacity);
        log::debug!(
            "region wave render start (regions={}, worker_threads={}, region_workers={}, world_worker_threads={}, queue_capacity={})",
            wave_plans.len(),
            worker_count,
            region_worker_count,
            worker_split.world_workers_per_region,
            queue_capacity
        );

        let mut cached_regions = RegionBakeMap::new();
        let mut cache_seed_diagnostics = RenderDiagnostics::default();
        let mut cache_seed_stats = RenderPipelineStats::default();
        let mut miss_plans = Vec::new();
        for plan in wave_plans {
            check_cancelled(options)?;
            match self.load_cached_region_bake(plan.key, options)? {
                Some(region) if region_bake_covers_plan(&region, plan) => {
                    log::trace!(
                        "region bake cache hit (coord=({}, {:?}, {}), mode={:?})",
                        plan.key.coord.x,
                        plan.key.coord.dimension,
                        plan.key.coord.z,
                        plan.key.mode
                    );
                    cache_seed_diagnostics.add(region.diagnostics.clone());
                    cache_seed_stats.region_cache_hits =
                        cache_seed_stats.region_cache_hits.saturating_add(1);
                    cached_regions.insert(plan.key, region);
                }
                Some(region) => {
                    log::trace!(
                        "region bake cache partial hit rejected (coord=({}, {:?}, {}), mode={:?}, cached=({},{}..{},{}), needed=({},{}..{},{}))",
                        plan.key.coord.x,
                        plan.key.coord.dimension,
                        plan.key.coord.z,
                        plan.key.mode,
                        region.covered_chunk_region.min_chunk_x,
                        region.covered_chunk_region.min_chunk_z,
                        region.covered_chunk_region.max_chunk_x,
                        region.covered_chunk_region.max_chunk_z,
                        plan.region.min_chunk_x,
                        plan.region.min_chunk_z,
                        plan.region.max_chunk_x,
                        plan.region.max_chunk_z
                    );
                    cache_seed_stats.region_cache_misses =
                        cache_seed_stats.region_cache_misses.saturating_add(1);
                    miss_plans.push(plan.clone());
                }
                None => {
                    cache_seed_stats.region_cache_misses =
                        cache_seed_stats.region_cache_misses.saturating_add(1);
                    miss_plans.push(plan.clone());
                }
            }
        }
        let miss_plans = Arc::new(miss_plans);

        let pool = render_cpu_pool(region_worker_count)?;
        pool.scope(|scope| {
            for _ in 0..region_worker_count {
                let next_plan = Arc::clone(&next_plan);
                let sender = sender.clone();
                let renderer = self.clone();
                let options = options.clone();
                let miss_plans = Arc::clone(&miss_plans);
                scope.spawn(move |_| {
                    loop {
                        if check_cancelled(&options).is_err() {
                            let _ = sender.send(Err(BedrockRenderError::Cancelled));
                            return;
                        }
                        let index = next_plan.fetch_add(1, Ordering::Relaxed);
                        let Some(plan) = miss_plans.get(index).cloned() else {
                            return;
                        };
                        let started = Instant::now();
                        let result = renderer
                            .bake_region_chunk_positions_blocking(
                                plan.key.coord,
                                plan.region,
                                &plan.chunk_positions,
                                &options,
                                plan.key.mode,
                                worker_split.world_workers_per_region,
                            )
                            .map(|bake| (plan.key, bake, started.elapsed().as_millis()));
                        if sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(sender);

            let mut regions = cached_regions;
            let mut rendered_tiles = BTreeSet::new();
            let mut diagnostics = cache_seed_diagnostics;
            let mut stats = cache_seed_stats;
            stats.active_tasks_peak = region_worker_count;
            stats.peak_worker_threads = worker_count;
            let mut completed_regions = 0usize;

            let ready_tile_indexes = ready_web_tile_indexes(
                pending_tiles,
                tile_region_plans,
                &regions,
                &rendered_tiles,
            )?;
            if !ready_tile_indexes.is_empty() {
                let compose_start = Instant::now();
                let compose_stats = render_web_tile_indexes(
                    self,
                    planned_tiles,
                    &ready_tile_indexes,
                    options,
                    &regions,
                    worker_count,
                    self.gpu.as_ref(),
                    sink,
                )?;
                stats.add_tile_compose_stats(compose_stats);
                let compose_ms = compose_start.elapsed().as_millis();
                stats.tile_compose_ms = stats.tile_compose_ms.saturating_add(compose_ms);
                stats.encode_ms = stats.encode_ms.saturating_add(compose_ms);
                rendered_tiles.extend(ready_tile_indexes);
            }

            for message in receiver {
                let (key, region, region_bake_ms) = message?;
                let region = Arc::new(region);
                diagnostics.add(region.diagnostics.clone());
                if let Some(load_stats) = &region.load_stats {
                    stats.add_render_load_stats(load_stats);
                }
                stats.region_bake_ms = stats.region_bake_ms.saturating_add(region_bake_ms);
                stats.bake_ms = stats.bake_ms.saturating_add(region_bake_ms);
                stats.region_copy_ms = stats.region_copy_ms.saturating_add(region.copy_ms);
                stats.cpu_frame_pack_ms = stats.cpu_frame_pack_ms.saturating_add(region.copy_ms);
                stats.baked_regions = stats.baked_regions.saturating_add(1);
                stats.region_chunks_copied = stats
                    .region_chunks_copied
                    .saturating_add(region.chunks_copied);
                stats.region_chunks_out_of_bounds = stats
                    .region_chunks_out_of_bounds
                    .saturating_add(region.chunks_out_of_bounds);
                stats.baked_chunks = diagnostics.baked_chunks;
                if region_bake_covers_full_region(&region, options.region_layout) {
                    self.store_region_bake_cache(key, &region, options)?;
                } else {
                    log::trace!(
                        "skipping partial region bake cache store (coord=({}, {:?}, {}), mode={:?}, covered=({},{}..{},{}))",
                        key.coord.x,
                        key.coord.dimension,
                        key.coord.z,
                        key.mode,
                        region.covered_chunk_region.min_chunk_x,
                        region.covered_chunk_region.min_chunk_z,
                        region.covered_chunk_region.max_chunk_x,
                        region.covered_chunk_region.max_chunk_z
                    );
                }
                regions.insert(key, region);
                completed_regions = completed_regions.saturating_add(1);

                let ready_tile_indexes = ready_web_tile_indexes(
                    pending_tiles,
                    tile_region_plans,
                    &regions,
                    &rendered_tiles,
                )?;
                if !ready_tile_indexes.is_empty() {
                    let compose_start = Instant::now();
                    let compose_stats = render_web_tile_indexes(
                        self,
                        planned_tiles,
                        &ready_tile_indexes,
                        options,
                        &regions,
                        worker_count,
                        self.gpu.as_ref(),
                        sink,
                    )?;
                    stats.add_tile_compose_stats(compose_stats);
                    let compose_ms = compose_start.elapsed().as_millis();
                    stats.tile_compose_ms = stats.tile_compose_ms.saturating_add(compose_ms);
                    stats.encode_ms = stats.encode_ms.saturating_add(compose_ms);
                    rendered_tiles.extend(ready_tile_indexes);
                }

                if completed_regions == miss_plans.len() {
                    break;
                }
            }

            if completed_regions != miss_plans.len() {
                return Err(BedrockRenderError::Validation(
                    "region bake pipeline stopped before all regions completed".to_string(),
                ));
            }

            Ok(RegionWaveRenderResult {
                rendered_tiles,
                diagnostics,
                stats,
            })
        })
    }

    fn render_tile_from_cached_regions_with_gpu_stats(
        &self,
        job: RenderJob,
        options: &RenderOptions,
        regions: &RegionBakeMap,
        work_items: usize,
        gpu: Option<&GpuRenderContext>,
    ) -> Result<(TileImage, TileComposeStats)> {
        validate_job(&job)?;
        check_cancelled(options)?;
        let pixel_count = usize::try_from(job.tile_size)
            .ok()
            .and_then(|size| size.checked_mul(size))
            .ok_or_else(|| {
                BedrockRenderError::Validation("tile pixel count overflow".to_string())
            })?;
        let (mut rgba, diagnostics) =
            compose_region_tile_cpu(self, &job, options, regions, pixel_count)?;
        let mut stats = TileComposeStats::cpu();
        stats.tile_missing_region_samples = diagnostics.missing_chunks;
        if let Some(sink) = &options.diagnostics {
            sink.emit(diagnostics);
        }

        if let Some(gpu) =
            gpu.filter(|_| should_process_tile_on_gpu(options, job.tile_size, work_items))
        {
            let processed = process_tile_rgba_on_gpu(gpu, &rgba, options)?;
            if processed.diagnostics.tiles == 0 {
                stats.gpu.add(processed.diagnostics);
            } else {
                rgba = processed.rgba;
                stats = TileComposeStats::gpu(processed.diagnostics);
            }
        }

        let encoded = encode_image(&rgba, job.tile_size, job.tile_size, options.format)?;
        Ok((
            TileImage {
                coord: job.coord,
                width: job.tile_size,
                height: job.tile_size,
                rgba: Arc::<[u8]>::from(rgba),
                encoded,
            },
            stats,
        ))
    }

    /// Bakes one chunk into reusable render payloads.
    ///
    /// # Errors
    ///
    /// Returns an error if the chunk cannot be loaded or decoded for the requested mode.
    pub fn bake_chunk_blocking(&self, pos: ChunkPos, options: BakeOptions) -> Result<ChunkBake> {
        let data = self.load_render_chunk_data_blocking(pos, options.mode)?;
        self.bake_chunk_data(data, options)
    }

    fn load_render_chunk_data_blocking(
        &self,
        pos: ChunkPos,
        mode: RenderMode,
    ) -> Result<ChunkData> {
        self.source
            .query_chunk_data_blocking(pos, render_chunk_load_options(mode))
    }

    fn load_render_chunk_data_with_options_blocking(
        &self,
        pos: ChunkPos,
        mode: RenderMode,
        options: &RenderOptions,
    ) -> Result<ChunkData> {
        self.source.query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: render_chunk_request_for_options(mode, options),
                subchunk_decode: render_subchunk_decode_mode(mode),
                threading: render_world_threading(options, 1)?,
                pipeline: options.cpu.to_world_pipeline(),
                cancel: render_world_cancel(options),
                priority: render_chunk_priority_for_tile(options, pos),
                storage_cache_policy: render_storage_cache_policy(options),
                ..ChunkLoadOptions::default()
            },
        )
    }

    fn bake_chunk_data(&self, data: ChunkData, options: BakeOptions) -> Result<ChunkBake> {
        let mode = options.mode;
        let pos = data.pos;
        let block_entity_index = block_entity_index(&data);
        let mut context = ChunkBakeContext {
            palette: &self.palette,
            options,
            data,
            block_entity_index,
            block_class_cache: HashMap::with_capacity(64),
            diagnostics: RenderDiagnostics::default(),
        };
        let payload = context.bake_payload()?;
        let mut diagnostics = context.diagnostics;
        diagnostics.baked_chunks = diagnostics.baked_chunks.saturating_add(1);
        Ok(ChunkBake {
            pos,
            mode,
            payload,
            diagnostics,
        })
    }

    /// Renders a tile by baking the tile's chunks first.
    ///
    /// # Errors
    ///
    /// Returns an error if the job is invalid, chunk loading or baking fails, rendering is
    /// cancelled, or image encoding fails.
    #[allow(clippy::needless_pass_by_value)]
    pub fn render_tile_from_bake_blocking(
        &self,
        job: RenderJob,
        options: &RenderOptions,
    ) -> Result<TileImage> {
        validate_job(&job)?;
        check_cancelled(options)?;
        let positions = tile_chunk_positions(&job)?;
        let Some(first_position) = positions.first().copied() else {
            return Err(BedrockRenderError::Validation(
                "tile plan did not include any chunks".to_string(),
            ));
        };
        let bake_options = BakeOptions {
            mode: job.mode,
            surface: options.surface,
        };
        let load_options = ChunkLoadOptions {
            data_request: render_chunk_request_for_options(job.mode, options),
            subchunk_decode: render_subchunk_decode_mode(job.mode),
            threading: render_world_threading(options, positions.len())?,
            pipeline: options.cpu.to_world_pipeline(),
            cancel: render_world_cancel(options),
            priority: render_chunk_priority_for_tile(options, first_position),
            ..ChunkLoadOptions::default()
        };
        let (chunk_data, _stats) = self
            .source
            .query_chunk_data_with_stats_blocking(&positions, load_options)?;
        let mut diagnostics = RenderDiagnostics::default();
        let mut bakes = BTreeMap::new();
        for data in chunk_data {
            check_cancelled(options)?;
            let bake = self.bake_chunk_data(data, bake_options.clone())?;
            diagnostics.add(bake.diagnostics.clone());
            bakes.insert(bake.pos, bake);
        }
        self.render_tile_from_chunk_bakes_blocking(job, options, &bakes, diagnostics)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn render_tile_from_chunk_bakes_blocking(
        &self,
        job: RenderJob,
        options: &RenderOptions,
        bakes: &BTreeMap<ChunkPos, ChunkBake>,
        mut diagnostics: RenderDiagnostics,
    ) -> Result<TileImage> {
        validate_job(&job)?;
        check_cancelled(options)?;
        let pixel_count = usize::try_from(job.tile_size)
            .ok()
            .and_then(|size| size.checked_mul(size))
            .ok_or_else(|| {
                BedrockRenderError::Validation("tile pixel count overflow".to_string())
            })?;
        let rgba = compose_tile_from_chunk_bakes_cpu(
            self,
            &job,
            options,
            bakes,
            &mut diagnostics,
            pixel_count,
        )?;
        if let Some(sink) = &options.diagnostics {
            sink.emit(diagnostics);
        }
        let encoded = encode_image(&rgba, job.tile_size, job.tile_size, options.format)?;
        Ok(TileImage {
            coord: job.coord,
            width: job.tile_size,
            height: job.tile_size,
            rgba: Arc::<[u8]>::from(rgba),
            encoded,
        })
    }

    #[cfg(feature = "async")]
    /// Asynchronously renders one tile by running the blocking renderer on a Tokio
    /// blocking task.
    ///
    /// # Errors
    ///
    /// Returns an error if the blocking task fails or if tile rendering fails.
    pub async fn render_tile(&self, job: RenderJob) -> Result<TileImage> {
        let renderer = self.clone();
        tokio::task::spawn_blocking(move || renderer.render_tile_blocking(job))
            .await
            .map_err(|error| BedrockRenderError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Asynchronously renders a batch of tiles on a Tokio blocking task.
    ///
    /// # Errors
    ///
    /// Returns an error if the blocking task fails or if batch rendering fails.
    pub async fn render_tiles(
        &self,
        jobs: Vec<RenderJob>,
        options: RenderOptions,
    ) -> Result<Vec<TileImage>> {
        let renderer = self.clone();
        tokio::task::spawn_blocking(move || renderer.render_tiles_blocking(jobs, options))
            .await
            .map_err(|error| BedrockRenderError::Join(error.to_string()))?
    }
}

struct ChunkBakeContext<'a> {
    palette: &'a RenderPalette,
    options: BakeOptions,
    data: ChunkData,
    block_entity_index: BTreeMap<(u8, i32, u8), usize>,
    block_class_cache: HashMap<String, SurfaceBlockClass>,
    diagnostics: RenderDiagnostics,
}

#[derive(Debug, Clone, Copy)]
struct SurfaceSample {
    color: RgbaColor,
    height: Option<i16>,
    underwater_height: Option<i16>,
    water_depth: u8,
    is_water: bool,
    material: SurfaceMaterialId,
    shape_flags: u8,
    overlay_alpha: u8,
}

#[derive(Debug, Clone, Copy)]
struct SurfaceOverlay {
    color: RgbaColor,
    alpha: u8,
}

#[derive(Debug, Clone, Copy)]
struct SurfaceBlockClass {
    has_color: bool,
    is_water: bool,
    material: SurfaceMaterialId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BiomeSample {
    Id(u32),
    Legacy { id: u8, color: RgbaColor },
}

impl BiomeSample {
    const fn biome_id(self) -> Option<u32> {
        match self {
            Self::Id(id) => Some(id),
            Self::Legacy { id, .. } => Some(id as u32),
        }
    }

    const fn legacy_color(self) -> Option<RgbaColor> {
        match self {
            Self::Id(_) => None,
            Self::Legacy { color, .. } => Some(color),
        }
    }
}

impl ChunkBakeContext<'_> {
    fn bake_payload(&mut self) -> Result<ChunkBakePayload> {
        match self.options.mode {
            RenderMode::SurfaceBlocks => self.bake_surface_payload(),
            RenderMode::HeightMap | RenderMode::RawHeightMap => Ok(self.bake_heightmap_payload()),
            RenderMode::Biome { .. }
            | RenderMode::RawBiomeLayer { .. }
            | RenderMode::LayerBlocks { .. }
            | RenderMode::CaveSlice { .. } => self.bake_color_payload(),
        }
    }

    fn bake_color_payload(&mut self) -> Result<ChunkBakePayload> {
        let mut colors = RgbaPlane::new(16, 16, self.palette.missing_chunk_color())?;
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                colors.set_color(
                    u32::from(local_x),
                    u32::from(local_z),
                    self.color_at(local_x, local_z),
                );
            }
        }
        Ok(ChunkBakePayload::Colors(colors))
    }

    fn bake_surface_payload(&mut self) -> Result<ChunkBakePayload> {
        if surface_gbuffer_enabled(self.options.surface) {
            return self.bake_surface_payload_v2();
        }
        let mut colors = RgbaPlane::new(16, 16, self.palette.missing_chunk_color())?;
        let mut heights = HeightPlane::new(16, 16)?;
        let mut relief_heights = HeightPlane::new(16, 16)?;
        let mut water_depths = DepthPlane::new(16, 16)?;
        let column_samples = self.data.column_samples.take();
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let sample =
                    self.surface_sample_at_from_columns(column_samples.as_ref(), local_x, local_z);
                colors.set_color(u32::from(local_x), u32::from(local_z), sample.color);
                if let Some(height) = sample.height {
                    heights.set_height(u32::from(local_x), u32::from(local_z), height);
                    relief_heights.set_height(
                        u32::from(local_x),
                        u32::from(local_z),
                        sample.underwater_height.unwrap_or(height),
                    );
                }
                if sample.is_water {
                    water_depths.set_depth(
                        u32::from(local_x),
                        u32::from(local_z),
                        sample.water_depth,
                    );
                }
            }
        }
        self.data.column_samples = column_samples;
        Ok(ChunkBakePayload::Surface(SurfacePlane {
            colors,
            heights,
            relief_heights,
            water_depths,
        }))
    }

    fn bake_surface_payload_v2(&mut self) -> Result<ChunkBakePayload> {
        let mut colors = RgbaPlane::new(16, 16, self.palette.missing_chunk_color())?;
        let mut heights = HeightPlane::new(16, 16)?;
        let mut relief_heights = HeightPlane::new(16, 16)?;
        let mut water_depths = DepthPlane::new(16, 16)?;
        let mut materials = DepthPlane::new(16, 16)?;
        let mut shape_flags = DepthPlane::new(16, 16)?;
        let mut overlay_alpha = DepthPlane::new(16, 16)?;
        let column_samples = self.data.column_samples.take();
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let sample =
                    self.surface_sample_at_from_columns(column_samples.as_ref(), local_x, local_z);
                let x = u32::from(local_x);
                let z = u32::from(local_z);
                colors.set_color(x, z, sample.color);
                if let Some(height) = sample.height {
                    heights.set_height(x, z, height);
                    relief_heights.set_height(x, z, sample.underwater_height.unwrap_or(height));
                }
                water_depths.set_depth(x, z, sample.water_depth);
                materials.set_depth(x, z, sample.material.as_u8());
                shape_flags.set_depth(x, z, sample.shape_flags);
                overlay_alpha.set_depth(x, z, sample.overlay_alpha);
            }
        }
        self.data.column_samples = column_samples;
        Ok(ChunkBakePayload::SurfaceAtlas(SurfacePlaneAtlas {
            colors,
            heights,
            relief_heights,
            water_depths,
            materials,
            shape_flags,
            overlay_alpha,
        }))
    }

    fn bake_heightmap_payload(&mut self) -> ChunkBakePayload {
        let mut colors = RgbaPlane {
            width: 16,
            height: 16,
            pixels: Vec::with_capacity(256),
        };
        let mut heights = HeightPlane {
            width: 16,
            height: 16,
            heights: Vec::with_capacity(256),
        };
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let height = match self.options.mode {
                    RenderMode::RawHeightMap => self.raw_height_at(local_x, local_z).unwrap_or(-64),
                    _ => self.surface_height_at(local_x, local_z).unwrap_or(-64),
                };
                let (min_y, max_y) = self.height_palette_range();
                colors
                    .pixels
                    .push(self.palette.height_color(height, min_y, max_y));
                heights.heights.push(height);
            }
        }
        ChunkBakePayload::HeightMap { colors, heights }
    }

    fn color_at(&mut self, local_x: u8, local_z: u8) -> RgbaColor {
        if !self.data.is_loaded {
            self.diagnostics.missing_chunks = self.diagnostics.missing_chunks.saturating_add(1);
            self.diagnostics.record_transparent_pixel();
            return self.palette.missing_chunk_color();
        }
        match self.options.mode {
            RenderMode::SurfaceBlocks => self.surface_sample_at(local_x, local_z).color,
            RenderMode::HeightMap => self.height_color_at(local_x, local_z),
            RenderMode::RawHeightMap => self.raw_height_color_at(local_x, local_z),
            RenderMode::Biome { y } => self.biome_color_at(local_x, local_z, y, false),
            RenderMode::RawBiomeLayer { y } => self.biome_color_at(local_x, local_z, y, true),
            RenderMode::LayerBlocks { y } => self.layer_color_at(local_x, local_z, y),
            RenderMode::CaveSlice { y } => self.cave_color_at(local_x, local_z, y),
        }
    }

    fn surface_sample_at(&mut self, local_x: u8, local_z: u8) -> SurfaceSample {
        let column = self.data.column_sample_at(local_x, local_z).cloned();
        self.surface_sample_from_optional_column(local_x, local_z, column.as_ref())
    }

    fn surface_sample_at_from_columns(
        &mut self,
        column_samples: Option<&TerrainColumnSamples>,
        local_x: u8,
        local_z: u8,
    ) -> SurfaceSample {
        let column = column_samples.and_then(|columns| columns.get(local_x, local_z));
        self.surface_sample_from_optional_column(local_x, local_z, column)
    }

    fn surface_sample_from_optional_column(
        &mut self,
        local_x: u8,
        local_z: u8,
        column: Option<&TerrainColumnSample>,
    ) -> SurfaceSample {
        if !self.data.is_loaded {
            self.diagnostics.missing_chunks = self.diagnostics.missing_chunks.saturating_add(1);
            self.diagnostics.record_transparent_pixel();
            return SurfaceSample {
                color: self.palette.missing_chunk_color(),
                height: None,
                underwater_height: None,
                water_depth: 0,
                is_water: false,
                material: SurfaceMaterialId::Unknown,
                shape_flags: 0,
                overlay_alpha: 0,
            };
        }
        if let Some(column) = column {
            return self.surface_sample_from_column(local_x, local_z, column);
        }

        self.diagnostics.record_transparent_pixel();
        SurfaceSample {
            color: self.palette.missing_chunk_color(),
            height: None,
            underwater_height: None,
            water_depth: 0,
            is_water: false,
            material: SurfaceMaterialId::Unknown,
            shape_flags: 0,
            overlay_alpha: 0,
        }
    }

    fn surface_sample_from_column(
        &mut self,
        local_x: u8,
        local_z: u8,
        sample: &TerrainColumnSample,
    ) -> SurfaceSample {
        let state = normalize_legacy_block_state(&sample.surface_block_state);
        let name = state.name.as_str();
        let block_entity = self
            .block_entity_at(local_x, i32::from(sample.surface_y), local_z)
            .cloned();
        let biome = sample.biome.map(terrain_column_biome_to_render_sample);
        let block_class = self.surface_block_class(name);
        if !block_class.has_color {
            self.diagnostics.record_unknown_block(name);
            if !self.options.surface.render_unknown_blocks {
                self.diagnostics.record_transparent_pixel();
                return SurfaceSample {
                    color: self.palette.void_color(),
                    height: Some(sample.surface_y),
                    underwater_height: Some(sample.relief_y),
                    water_depth: 0,
                    is_water: false,
                    material: SurfaceMaterialId::Unknown,
                    shape_flags: 0,
                    overlay_alpha: 0,
                };
            }
        }
        let water = sample.water.as_ref();
        let is_water = water.is_some() || block_class.is_water;
        let underwater_state = water
            .and_then(|water| water.underwater_block_state.as_ref())
            .map(normalize_legacy_block_state);
        let water_depth = if is_water && self.options.surface.transparent_water {
            water.map_or(0, |water| water.depth)
        } else {
            0
        };
        let color = if is_water && self.options.surface.transparent_water {
            self.palette.transparent_water_color(
                name,
                underwater_state
                    .as_ref()
                    .map(|state| state.as_ref().name.as_str()),
                biome.and_then(BiomeSample::biome_id),
                water_depth,
                self.options.surface.biome_tint,
            )
        } else {
            special_surface_block_color(
                self.palette,
                state.as_ref(),
                block_entity.as_ref(),
                biome,
                self.options.surface.biome_tint,
            )
        };
        let overlay = sample
            .overlay
            .as_ref()
            .and_then(|overlay| self.surface_overlay_from_column(local_x, local_z, overlay, biome));
        let color = blend_surface_overlay(color, overlay);
        let material = if is_water {
            SurfaceMaterialId::Water
        } else {
            block_class.material
        };
        let shape_flags = atlas_shape_flags_from_material(
            block_class.material,
            sample.overlay.is_some(),
            is_water,
        );
        SurfaceSample {
            color,
            height: Some(sample.surface_y),
            underwater_height: if is_water && self.options.surface.transparent_water {
                water.and_then(|water| water.underwater_y)
            } else {
                None
            }
            .or((!is_water).then_some(sample.relief_y)),
            water_depth,
            is_water,
            material,
            shape_flags,
            overlay_alpha: overlay.map_or(0, |overlay| overlay.alpha),
        }
    }

    fn surface_overlay_from_column(
        &mut self,
        local_x: u8,
        local_z: u8,
        overlay: &TerrainColumnOverlay,
        fallback_biome: Option<BiomeSample>,
    ) -> Option<SurfaceOverlay> {
        let state = normalize_legacy_block_state(&overlay.block_state);
        let name = state.name.as_str();
        let alpha = surface_overlay_alpha(name)?;
        if !self.surface_block_class(name).has_color {
            self.diagnostics.record_unknown_block(name);
        }
        let block_entity = self
            .block_entity_at(local_x, i32::from(overlay.y), local_z)
            .cloned();
        let biome = self
            .biome_sample_at_or_top(local_x, local_z, i32::from(overlay.y))
            .or(fallback_biome);
        Some(SurfaceOverlay {
            color: special_surface_block_color(
                self.palette,
                state.as_ref(),
                block_entity.as_ref(),
                biome,
                self.options.surface.biome_tint,
            ),
            alpha,
        })
    }

    fn surface_block_class(&mut self, name: &str) -> SurfaceBlockClass {
        if let Some(class) = self.block_class_cache.get(name).copied() {
            return class;
        }
        let class = SurfaceBlockClass {
            has_color: self.palette.has_block_color(name),
            is_water: self.palette.is_water_block(name),
            material: classify_surface_material(name),
        };
        self.block_class_cache.insert(name.to_string(), class);
        class
    }

    fn height_color_at(&mut self, local_x: u8, local_z: u8) -> RgbaColor {
        let height = self.surface_height_at(local_x, local_z).unwrap_or(-64);
        let (min_y, max_y) = self.height_palette_range();
        self.palette.height_color(height, min_y, max_y)
    }

    fn surface_height_at(&mut self, local_x: u8, local_z: u8) -> Option<i16> {
        if !self.data.is_loaded {
            self.diagnostics.missing_chunks = self.diagnostics.missing_chunks.saturating_add(1);
            self.diagnostics.record_transparent_pixel();
            return None;
        }
        let height = self.data.column_sample_at(local_x, local_z)?.surface_y;
        Some(height)
    }

    fn raw_height_color_at(&self, local_x: u8, local_z: u8) -> RgbaColor {
        let height = self.raw_height_at(local_x, local_z).unwrap_or(-64);
        let (min_y, max_y) = self.height_palette_range();
        self.palette.height_color(height, min_y, max_y)
    }

    fn raw_height_at(&self, local_x: u8, local_z: u8) -> Option<i16> {
        self.data.height_map.as_ref()?[usize::from(local_z)][usize::from(local_x)]
    }

    fn biome_color_at(&self, local_x: u8, local_z: u8, y: i32, raw: bool) -> RgbaColor {
        let Some(sample) = self.biome_sample_at_or_top(local_x, local_z, y) else {
            return self.palette.unknown_biome_color();
        };
        match sample {
            BiomeSample::Id(id) => {
                if raw {
                    self.palette.raw_biome_color(id)
                } else {
                    self.palette.biome_color(id)
                }
            }
            BiomeSample::Legacy { id, color } => {
                let id = u32::from(id);
                if raw {
                    if self.palette.has_biome_color(id) {
                        self.palette.raw_biome_color(id)
                    } else {
                        color
                    }
                } else if self.palette.has_biome_color(id) {
                    self.palette.biome_color(id)
                } else {
                    self.palette.unknown_biome_color()
                }
            }
        }
    }

    fn layer_color_at(&mut self, local_x: u8, local_z: u8, y: i32) -> RgbaColor {
        let Some(state) = self.block_state_at(local_x, y, local_z) else {
            self.diagnostics.record_transparent_pixel();
            return self.palette.missing_chunk_color();
        };
        let name = state.name.clone();
        if !self.palette.has_block_color(&name) {
            self.diagnostics.record_unknown_block(&name);
        }
        state_block_color(self.palette, &state)
    }

    fn cave_color_at(&self, local_x: u8, local_z: u8, y: i32) -> RgbaColor {
        let block_name = self
            .block_state_at(local_x, y, local_z)
            .map(|state| state.name.clone());
        self.palette.cave_color(block_name.as_deref())
    }

    fn block_state_at(&self, local_x: u8, y: i32, local_z: u8) -> Option<BlockState> {
        let subchunk_y = block_y_to_subchunk_y(y).ok()?;
        let local_y = u8::try_from(y - i32::from(subchunk_y) * 16).ok()?;
        if let Some(subchunk) = self.data.subchunks.get(&subchunk_y) {
            if let Some(state) = subchunk.visible_block_state_at(local_x, local_y, local_z) {
                return Some(state.clone());
            }
            if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z) {
                let data = subchunk
                    .legacy_block_data_at(local_x, local_y, local_z)
                    .unwrap_or(0);
                return Some(legacy_block_state(id, data));
            }
        }
        let terrain = self.data.legacy_terrain.as_ref()?;
        if !(0..=127).contains(&y) {
            return None;
        }
        let legacy_y = u8::try_from(y).ok()?;
        let id = terrain.block_id_at(local_x, legacy_y, local_z)?;
        let data = terrain
            .block_data_at(local_x, legacy_y, local_z)
            .unwrap_or(0);
        Some(legacy_block_state(id, data))
    }

    fn block_entity_at(&self, local_x: u8, y: i32, local_z: u8) -> Option<&ChunkBlockEntity> {
        self.block_entity_index
            .get(&(local_x, y, local_z))
            .and_then(|index| self.data.block_entities.get(*index))
    }

    fn biome_id_at(&self, local_x: u8, local_z: u8, y: i32) -> Option<u32> {
        let storage = self
            .data
            .biome_data
            .get(&biome_storage_bucket_y(y))
            .or_else(|| self.data.biome_data.values().next())?;
        non_empty_biome_id(storage.biome_id_at(local_x, local_biome_y(storage, y).ok()?, local_z))
    }

    fn biome_id_at_or_top(&self, local_x: u8, local_z: u8, y: i32) -> Option<u32> {
        if let Some(id) = self.biome_id_at(local_x, local_z, y) {
            return Some(id);
        }
        self.top_biome_id_at(local_x, local_z)
    }

    fn biome_sample_at_or_top(&self, local_x: u8, local_z: u8, y: i32) -> Option<BiomeSample> {
        if let Some(sample) = self.legacy_biome_sample_at(local_x, local_z) {
            return Some(sample);
        }
        if let Some(id) = self.biome_id_at_or_top(local_x, local_z, y) {
            return Some(BiomeSample::Id(id));
        }
        None
    }

    fn legacy_biome_sample_at(&self, local_x: u8, local_z: u8) -> Option<BiomeSample> {
        let sample = self.data.legacy_biomes.as_ref()?[usize::from(local_z)][usize::from(local_x)]?;
        Some(legacy_biome_sample_to_render_sample(sample))
    }

    fn top_biome_id_at(&self, local_x: u8, local_z: u8) -> Option<u32> {
        for storage in self.data.biome_data.values().rev() {
            if storage.y.is_none() {
                if let Some(id) = non_empty_biome_id(storage.biome_id_at(local_x, 0, local_z)) {
                    return Some(id);
                }
                continue;
            }
            for local_y in (0..16_u8).rev() {
                if let Some(id) = non_empty_biome_id(storage.biome_id_at(local_x, local_y, local_z))
                {
                    return Some(id);
                }
            }
        }
        None
    }

    fn height_palette_range(&self) -> (i16, i16) {
        if self.uses_legacy_terrain_only() {
            (0, 127)
        } else {
            (-64, 320)
        }
    }

    fn uses_legacy_terrain_only(&self) -> bool {
        self.data.legacy_terrain.is_some() && self.data.subchunks.is_empty()
    }
}

fn compose_region_tile_cpu<S>(
    renderer: &MapRenderer<S>,
    job: &RenderJob,
    options: &RenderOptions,
    regions: &RegionBakeMap,
    pixel_count: usize,
) -> Result<(Vec<u8>, RenderDiagnostics)>
where
    S: WorldStorageHandle,
{
    if block_volume_enabled_for(job.mode, options.surface, job)
        || atlas_enabled_for(job.mode, options.surface, job)
    {
        let prepared = prepare_region_tile_compose(
            renderer.palette.missing_chunk_color(),
            job,
            options,
            regions,
        )?;
        return Ok((
            compose_region_tile_from_prepared(&renderer.palette, &prepared, job, options.surface),
            prepared.diagnostics,
        ));
    }
    let mut diagnostics = RenderDiagnostics::default();
    let mut rgba = vec![0; pixel_count.saturating_mul(4)];
    let mut pixel_index = 0usize;
    for pixel_z in 0..job.tile_size {
        for pixel_x in 0..job.tile_size {
            if (pixel_count > 4096) && pixel_index.is_multiple_of(4096) {
                check_cancelled(options)?;
            }
            let color = region_tile_pixel_color(
                &renderer.palette,
                job,
                options,
                regions,
                pixel_x,
                pixel_z,
                &mut diagnostics,
            )?;
            write_rgba_pixel(&mut rgba, pixel_index, color);
            pixel_index += 1;
        }
    }
    Ok((rgba, diagnostics))
}

fn compose_tile_from_chunk_bakes_cpu<S>(
    renderer: &MapRenderer<S>,
    job: &RenderJob,
    options: &RenderOptions,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    diagnostics: &mut RenderDiagnostics,
    pixel_count: usize,
) -> Result<Vec<u8>>
where
    S: WorldStorageHandle,
{
    if block_volume_enabled_for(job.mode, options.surface, job)
        || atlas_enabled_for(job.mode, options.surface, job)
    {
        let prepared = prepare_chunk_tile_compose(
            renderer.palette.missing_chunk_color(),
            job,
            options,
            bakes,
        )?;
        diagnostics.add(prepared.diagnostics.clone());
        return Ok(compose_region_tile_from_prepared(
            &renderer.palette,
            &prepared,
            job,
            options.surface,
        ));
    }
    let mut rgba = vec![0; pixel_count.saturating_mul(4)];
    let mut pixel_index = 0usize;
    for pixel_z in 0..job.tile_size {
        for pixel_x in 0..job.tile_size {
            if (pixel_count > 4096) && pixel_index.is_multiple_of(4096) {
                check_cancelled(options)?;
            }
            let color = baked_tile_pixel_color(
                &renderer.palette,
                job,
                options,
                bakes,
                pixel_x,
                pixel_z,
                diagnostics,
            )?;
            write_rgba_pixel(&mut rgba, pixel_index, color);
            pixel_index += 1;
        }
    }
    Ok(rgba)
}

fn region_tile_pixel_color(
    palette: &RenderPalette,
    job: &RenderJob,
    options: &RenderOptions,
    regions: &RegionBakeMap,
    pixel_x: u32,
    pixel_z: u32,
    diagnostics: &mut RenderDiagnostics,
) -> Result<RgbaColor> {
    let bounds = tile_pixel_block_bounds(job, pixel_x, pixel_z)?;
    if bounds.covers_single_block() {
        return Ok(region_shaded_color_at_block(
            palette,
            regions,
            options.region_layout,
            job.coord.dimension,
            job.mode,
            bounds.min_x,
            bounds.min_z,
            options.surface,
            Some(BlockBoundaryContext::new(job, pixel_x, pixel_z)),
        )
        .unwrap_or_else(|| {
            diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
            diagnostics.record_transparent_pixel();
            palette.missing_chunk_color()
        }));
    }

    let mut average = RgbaAccumulator::default();
    for block_z in bounds.min_z..=bounds.max_z {
        for block_x in bounds.min_x..=bounds.max_x {
            if let Some(color) = region_shaded_color_at_block(
                palette,
                regions,
                options.region_layout,
                job.coord.dimension,
                job.mode,
                block_x,
                block_z,
                options.surface,
                None,
            ) {
                average.add(color);
            } else {
                diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
                diagnostics.record_transparent_pixel();
                average.add(palette.missing_chunk_color());
            }
        }
    }
    Ok(average
        .average()
        .unwrap_or_else(|| palette.missing_chunk_color()))
}

fn prepare_region_tile_compose(
    missing_color: RgbaColor,
    job: &RenderJob,
    options: &RenderOptions,
    regions: &RegionBakeMap,
) -> Result<PreparedTileCompose> {
    if let Some(prepared) =
        try_prepare_region_tile_compose_fast(missing_color, job, options, regions)?
    {
        return Ok(prepared);
    }
    let pixel_count = usize::try_from(job.tile_size)
        .ok()
        .and_then(|size| size.checked_mul(size))
        .ok_or_else(|| BedrockRenderError::Validation("tile pixel count overflow".to_string()))?;
    let lighting_enabled = lighting_enabled_for(job.mode, options.surface);
    let atlas_enabled = atlas_enabled_for(job.mode, options.surface, job);
    let gbuffer_enabled = atlas_enabled;
    let block_volume_enabled = block_volume_enabled_for(job.mode, options.surface, job);
    let (
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
    ) = if block_volume_enabled || gbuffer_enabled {
        prepare_region_block_height_grid(job, options, regions, atlas_enabled)?
    } else {
        (Vec::new(), Vec::new(), 0, 0, 0)
    };
    let mut diagnostics = RenderDiagnostics::default();
    let mut colors = Vec::with_capacity(pixel_count);
    let mut water_depths = Vec::with_capacity(pixel_count);
    let mut heights = Vec::with_capacity(pixel_count.saturating_mul(9));

    for pixel_z in 0..job.tile_size {
        for pixel_x in 0..job.tile_size {
            let pixel_index = colors.len();
            if (pixel_count > 4096) && pixel_index.is_multiple_of(4096) {
                check_cancelled(options)?;
            }
            let (block_x, block_z) = tile_pixel_to_block(job, pixel_x, pixel_z)?;
            if let Some(color) = region_color_at_block(
                regions,
                options.region_layout,
                job.coord.dimension,
                job.mode,
                block_x,
                block_z,
            ) {
                colors.push(pack_rgba_color(color));
                let aux = region_atlas_aux_at_block(
                    regions,
                    options.region_layout,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                    gbuffer_enabled,
                );
                water_depths.push(aux);
                push_region_height_neighborhood(
                    &mut heights,
                    regions,
                    options.region_layout,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                    lighting_enabled,
                );
            } else {
                diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
                diagnostics.record_transparent_pixel();
                colors.push(pack_rgba_color(missing_color));
                water_depths.push(0);
                push_missing_height_neighborhood(&mut heights);
            }
        }
    }

    Ok(PreparedTileCompose {
        colors,
        heights,
        water_depths,
        atlas_enabled,
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
        block_volume_enabled,
        diagnostics,
        lighting_enabled,
    })
}

fn try_prepare_region_tile_compose_fast(
    missing_color: RgbaColor,
    job: &RenderJob,
    options: &RenderOptions,
    regions: &RegionBakeMap,
) -> Result<Option<PreparedTileCompose>> {
    if job.scale != 1 || job.pixels_per_block == 0 || job.tile_size % job.pixels_per_block != 0 {
        return Ok(None);
    }
    let source_blocks = job.tile_size / job.pixels_per_block;
    if source_blocks == 0 {
        return Ok(None);
    }
    let (start_block_x, start_block_z) = tile_pixel_to_block(job, 0, 0)?;
    let (end_block_x, end_block_z) =
        tile_pixel_to_block(job, job.tile_size - 1, job.tile_size - 1)?;
    let start_chunk = BlockPos {
        x: start_block_x,
        y: 0,
        z: start_block_z,
    }
    .to_chunk_pos(job.coord.dimension);
    let end_chunk = BlockPos {
        x: end_block_x,
        y: 0,
        z: end_block_z,
    }
    .to_chunk_pos(job.coord.dimension);
    let region_key = region_key_for_chunk(start_chunk, options.region_layout, job.mode);
    if region_key != region_key_for_chunk(end_chunk, options.region_layout, job.mode) {
        return Ok(None);
    }
    let Some(region) = regions.get(&region_key) else {
        return Ok(None);
    };
    let (local_x, _, local_z) = BlockPos {
        x: start_block_x,
        y: 0,
        z: start_block_z,
    }
    .in_chunk_offset();
    let Some((region_start_x, region_start_z)) = region.region_pixel(start_chunk, local_x, local_z)
    else {
        return Ok(None);
    };
    if !region_payload_contains_square(region, region_start_x, region_start_z, source_blocks) {
        return Ok(None);
    }

    let pixel_count = usize::try_from(job.tile_size)
        .ok()
        .and_then(|size| size.checked_mul(size))
        .ok_or_else(|| BedrockRenderError::Validation("tile pixel count overflow".to_string()))?;
    let lighting_enabled = lighting_enabled_for(job.mode, options.surface);
    let atlas_enabled = atlas_enabled_for(job.mode, options.surface, job);
    let gbuffer_enabled = atlas_enabled;
    let block_volume_enabled = block_volume_enabled_for(job.mode, options.surface, job);
    let (
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
    ) = if block_volume_enabled || gbuffer_enabled {
        prepare_region_block_height_grid(job, options, regions, atlas_enabled)?
    } else {
        (Vec::new(), Vec::new(), 0, 0, 0)
    };
    let mut diagnostics = RenderDiagnostics::default();
    let mut colors = Vec::with_capacity(pixel_count);
    let mut water_depths = Vec::with_capacity(pixel_count);
    let mut heights = Vec::with_capacity(pixel_count.saturating_mul(9));
    let packed_missing = pack_rgba_color(missing_color);

    for source_z in 0..source_blocks {
        if source_z.is_multiple_of(64) {
            check_cancelled(options)?;
        }
        let region_z = region_start_z + source_z;
        for _repeat_z in 0..job.pixels_per_block {
            for source_x in 0..source_blocks {
                let region_x = region_start_x + source_x;
                let color = region
                    .color_at_region_pixel(region_x, region_z)
                    .map(pack_rgba_color)
                    .unwrap_or_else(|| {
                        diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
                        diagnostics.record_transparent_pixel();
                        packed_missing
                    });
                let water_depth = if gbuffer_enabled {
                    region.atlas_aux_at_region_pixel(region_x, region_z)
                } else {
                    u32::from(region.water_depth_at_region_pixel(region_x, region_z))
                };
                let mut height_neighborhood = [i32::from(MISSING_HEIGHT); 9];
                if lighting_enabled {
                    push_fast_region_height_neighborhood(
                        &mut height_neighborhood,
                        region,
                        region_x,
                        region_z,
                    );
                }
                for _repeat_x in 0..job.pixels_per_block {
                    colors.push(color);
                    water_depths.push(water_depth);
                    heights.extend_from_slice(&height_neighborhood);
                }
            }
        }
    }

    Ok(Some(PreparedTileCompose {
        colors,
        heights,
        water_depths,
        atlas_enabled,
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
        block_volume_enabled,
        diagnostics,
        lighting_enabled,
    }))
}

fn region_payload_contains_square(
    region: &RegionBake,
    start_x: u32,
    start_z: u32,
    width: u32,
) -> bool {
    let Some(end_x) = start_x.checked_add(width.saturating_sub(1)) else {
        return false;
    };
    let Some(end_z) = start_z.checked_add(width.saturating_sub(1)) else {
        return false;
    };
    match &region.payload {
        RegionBakePayload::Colors(plane) => end_x < plane.width && end_z < plane.height,
        RegionBakePayload::Surface(surface) => {
            end_x < surface.colors.width && end_z < surface.colors.height
        }
        RegionBakePayload::SurfaceAtlas(surface) => {
            end_x < surface.colors.width && end_z < surface.colors.height
        }
        RegionBakePayload::HeightMap { colors, .. } => {
            end_x < colors.width && end_z < colors.height
        }
    }
}

fn push_fast_region_height_neighborhood(
    target: &mut [i32; 9],
    region: &RegionBake,
    region_x: u32,
    region_z: u32,
) {
    let Some(center) = region.height_at_region_pixel(region_x, region_z) else {
        return;
    };
    let height_at = |dx: i32, dz: i32| -> i32 {
        let x = i64::from(region_x) + i64::from(dx);
        let z = i64::from(region_z) + i64::from(dz);
        if x < 0 || z < 0 || x > i64::from(u32::MAX) || z > i64::from(u32::MAX) {
            return i32::from(center);
        }
        i32::from(
            region
                .height_at_region_pixel(
                    u32::try_from(x).unwrap_or(0),
                    u32::try_from(z).unwrap_or(0),
                )
                .unwrap_or(center),
        )
    };
    *target = [
        i32::from(center),
        height_at(-1, -1),
        height_at(0, -1),
        height_at(1, -1),
        height_at(-1, 0),
        height_at(1, 0),
        height_at(-1, 1),
        height_at(0, 1),
        height_at(1, 1),
    ];
}

fn prepare_chunk_tile_compose(
    missing_color: RgbaColor,
    job: &RenderJob,
    options: &RenderOptions,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
) -> Result<PreparedTileCompose> {
    let pixel_count = usize::try_from(job.tile_size)
        .ok()
        .and_then(|size| size.checked_mul(size))
        .ok_or_else(|| BedrockRenderError::Validation("tile pixel count overflow".to_string()))?;
    let lighting_enabled = lighting_enabled_for(job.mode, options.surface);
    let atlas_enabled = atlas_enabled_for(job.mode, options.surface, job);
    let gbuffer_enabled = atlas_enabled;
    let block_volume_enabled = block_volume_enabled_for(job.mode, options.surface, job);
    let (
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
    ) = if block_volume_enabled || gbuffer_enabled {
        prepare_chunk_block_height_grid(job, options, bakes, atlas_enabled)?
    } else {
        (Vec::new(), Vec::new(), 0, 0, 0)
    };
    let mut diagnostics = RenderDiagnostics::default();
    let mut colors = Vec::with_capacity(pixel_count);
    let mut water_depths = Vec::with_capacity(pixel_count);
    let mut heights = Vec::with_capacity(pixel_count.saturating_mul(9));

    for pixel_z in 0..job.tile_size {
        for pixel_x in 0..job.tile_size {
            let pixel_index = colors.len();
            if (pixel_count > 4096) && pixel_index.is_multiple_of(4096) {
                check_cancelled(options)?;
            }
            let (block_x, block_z) = tile_pixel_to_block(job, pixel_x, pixel_z)?;
            if let Some((chunk_pos, local_x, local_z, color)) =
                chunk_color_at_block(bakes, job.coord.dimension, job.mode, block_x, block_z)
            {
                colors.push(pack_rgba_color(color));
                let aux = bakes.get(&chunk_pos).map_or(0, |bake| {
                    if gbuffer_enabled {
                        chunk_bake_atlas_aux(bake, u32::from(local_x), u32::from(local_z))
                    } else {
                        u32::from(chunk_bake_water_depth(
                            bake,
                            u32::from(local_x),
                            u32::from(local_z),
                        ))
                    }
                });
                water_depths.push(aux);
                push_chunk_height_neighborhood(
                    &mut heights,
                    bakes,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                    lighting_enabled,
                );
            } else {
                diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
                diagnostics.record_transparent_pixel();
                colors.push(pack_rgba_color(missing_color));
                water_depths.push(0);
                push_missing_height_neighborhood(&mut heights);
            }
        }
    }

    Ok(PreparedTileCompose {
        colors,
        heights,
        water_depths,
        atlas_enabled,
        block_heights,
        block_aux,
        block_height_grid_width,
        block_height_grid_height,
        block_height_grid_padding,
        block_volume_enabled,
        diagnostics,
        lighting_enabled,
    })
}

type BlockHeightGridData = (Vec<i32>, Vec<u32>, u32, u32, u32);

fn block_volume_grid_padding(surface: SurfaceRenderOptions) -> u32 {
    let mut padding = surface.block_volume.cast_shadow_max_blocks.max(1);
    if atlas_enabled(surface) {
        padding = padding.max(MAP_ATLAS_CAST_SHADOW_RADIUS_BLOCKS);
    }
    padding
}

fn tile_source_block_span(job: &RenderJob) -> Result<(i32, i32, u32)> {
    if job.scale != 1 || job.pixels_per_block == 0 || job.tile_size % job.pixels_per_block != 0 {
        return Err(BedrockRenderError::Validation(
            "block-volume compose requires one-block pixels and integer pixels_per_block"
                .to_string(),
        ));
    }
    let (start_block_x, start_block_z) = tile_pixel_to_block(job, 0, 0)?;
    Ok((
        start_block_x,
        start_block_z,
        job.tile_size / job.pixels_per_block,
    ))
}

fn prepare_region_block_height_grid(
    job: &RenderJob,
    options: &RenderOptions,
    regions: &RegionBakeMap,
    include_aux: bool,
) -> Result<BlockHeightGridData> {
    let (start_block_x, start_block_z, source_blocks) = tile_source_block_span(job)?;
    let padding = block_volume_grid_padding(options.surface);
    let grid_width = source_blocks
        .checked_add(padding.saturating_mul(2))
        .ok_or_else(|| BedrockRenderError::Validation("block-volume grid overflow".to_string()))?;
    let grid_height = grid_width;
    let capacity = usize::try_from(grid_width)
        .ok()
        .and_then(|width| {
            usize::try_from(grid_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| BedrockRenderError::Validation("block-volume grid overflow".to_string()))?;
    let mut heights = Vec::with_capacity(capacity);
    let mut aux_values = include_aux
        .then(|| Vec::with_capacity(capacity))
        .unwrap_or_default();
    let padding_i32 = i32::try_from(padding)
        .map_err(|_| BedrockRenderError::Validation("block-volume padding overflow".to_string()))?;
    let source_blocks_i32 = i32::try_from(source_blocks).map_err(|_| {
        BedrockRenderError::Validation("block-volume source span overflow".to_string())
    })?;
    for dz in -padding_i32..source_blocks_i32 + padding_i32 {
        for dx in -padding_i32..source_blocks_i32 + padding_i32 {
            let block_x = start_block_x.saturating_add(dx);
            let block_z = start_block_z.saturating_add(dz);
            heights.push(
                region_height_at_block(
                    regions,
                    options.region_layout,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                )
                .map(i32::from)
                .unwrap_or(i32::from(MISSING_HEIGHT)),
            );
            if include_aux {
                aux_values.push(region_atlas_aux_at_block(
                    regions,
                    options.region_layout,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                    true,
                ));
            }
        }
    }
    Ok((heights, aux_values, grid_width, grid_height, padding))
}

fn prepare_chunk_block_height_grid(
    job: &RenderJob,
    options: &RenderOptions,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    include_aux: bool,
) -> Result<BlockHeightGridData> {
    let (start_block_x, start_block_z, source_blocks) = tile_source_block_span(job)?;
    let padding = block_volume_grid_padding(options.surface);
    let grid_width = source_blocks
        .checked_add(padding.saturating_mul(2))
        .ok_or_else(|| BedrockRenderError::Validation("block-volume grid overflow".to_string()))?;
    let grid_height = grid_width;
    let capacity = usize::try_from(grid_width)
        .ok()
        .and_then(|width| {
            usize::try_from(grid_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| BedrockRenderError::Validation("block-volume grid overflow".to_string()))?;
    let mut heights = Vec::with_capacity(capacity);
    let mut aux_values = include_aux
        .then(|| Vec::with_capacity(capacity))
        .unwrap_or_default();
    let padding_i32 = i32::try_from(padding)
        .map_err(|_| BedrockRenderError::Validation("block-volume padding overflow".to_string()))?;
    let source_blocks_i32 = i32::try_from(source_blocks).map_err(|_| {
        BedrockRenderError::Validation("block-volume source span overflow".to_string())
    })?;
    for dz in -padding_i32..source_blocks_i32 + padding_i32 {
        for dx in -padding_i32..source_blocks_i32 + padding_i32 {
            let block_x = start_block_x.saturating_add(dx);
            let block_z = start_block_z.saturating_add(dz);
            heights.push(
                chunk_bake_height_at_block(bakes, job.coord.dimension, job.mode, block_x, block_z)
                    .map(i32::from)
                    .unwrap_or(i32::from(MISSING_HEIGHT)),
            );
            if include_aux {
                aux_values.push(chunk_bake_atlas_aux_at_block(
                    bakes,
                    job.coord.dimension,
                    job.mode,
                    block_x,
                    block_z,
                ));
            }
        }
    }
    Ok((heights, aux_values, grid_width, grid_height, padding))
}

#[allow(clippy::too_many_arguments)]
fn push_region_height_neighborhood(
    heights: &mut Vec<i32>,
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    lighting_enabled: bool,
) {
    if !lighting_enabled {
        push_missing_height_neighborhood(heights);
        return;
    }
    let Some(center) =
        region_height_at_block(regions, region_layout, dimension, mode, block_x, block_z)
    else {
        push_missing_height_neighborhood(heights);
        return;
    };
    let height_at = |x, z| {
        i32::from(
            region_height_at_block(regions, region_layout, dimension, mode, x, z).unwrap_or(center),
        )
    };
    heights.extend_from_slice(&[
        i32::from(center),
        height_at(block_x - 1, block_z - 1),
        height_at(block_x, block_z - 1),
        height_at(block_x + 1, block_z - 1),
        height_at(block_x - 1, block_z),
        height_at(block_x + 1, block_z),
        height_at(block_x - 1, block_z + 1),
        height_at(block_x, block_z + 1),
        height_at(block_x + 1, block_z + 1),
    ]);
}

fn push_missing_height_neighborhood(heights: &mut Vec<i32>) {
    heights.extend_from_slice(&[i32::from(MISSING_HEIGHT); 9]);
}

fn push_chunk_height_neighborhood(
    heights: &mut Vec<i32>,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    lighting_enabled: bool,
) {
    if !lighting_enabled {
        push_missing_height_neighborhood(heights);
        return;
    }
    let Some(center) = chunk_bake_height_at_block(bakes, dimension, mode, block_x, block_z) else {
        push_missing_height_neighborhood(heights);
        return;
    };
    let height_at = |x, z| {
        i32::from(chunk_bake_height_at_block(bakes, dimension, mode, x, z).unwrap_or(center))
    };
    heights.extend_from_slice(&[
        i32::from(center),
        height_at(block_x - 1, block_z - 1),
        height_at(block_x, block_z - 1),
        height_at(block_x + 1, block_z - 1),
        height_at(block_x - 1, block_z),
        height_at(block_x + 1, block_z),
        height_at(block_x - 1, block_z + 1),
        height_at(block_x, block_z + 1),
        height_at(block_x + 1, block_z + 1),
    ]);
}

fn compose_region_tile_from_prepared(
    palette: &RenderPalette,
    prepared: &PreparedTileCompose,
    job: &RenderJob,
    surface: SurfaceRenderOptions,
) -> Vec<u8> {
    let pixel_count = usize::try_from(job.tile_size)
        .ok()
        .and_then(|size| size.checked_mul(size))
        .unwrap_or(0);
    let mut rgba = vec![0; pixel_count.saturating_mul(4)];
    for (pixel_index, packed_color) in prepared.colors.iter().copied().enumerate() {
        let mut color = unpack_rgba_color(packed_color);
        if prepared.lighting_enabled {
            let offset = pixel_index.saturating_mul(9);
            if let Some(heights) = prepared.heights.get(offset..offset.saturating_add(9))
                && heights.first().copied() != Some(i32::from(MISSING_HEIGHT))
            {
                let neighborhood = TerrainHeightNeighborhood {
                    center: i16_from_i32(heights[0]),
                    north_west: i16_from_i32(heights[1]),
                    north: i16_from_i32(heights[2]),
                    north_east: i16_from_i32(heights[3]),
                    west: i16_from_i32(heights[4]),
                    east: i16_from_i32(heights[5]),
                    south_west: i16_from_i32(heights[6]),
                    south: i16_from_i32(heights[7]),
                    south_east: i16_from_i32(heights[8]),
                };
                let aux = prepared.water_depths.get(pixel_index).copied().unwrap_or(0);
                let water_depth = atlas_aux_water_depth(aux);
                let pixel_x = u32::try_from(pixel_index)
                    .ok()
                    .map_or(0, |index| index % job.tile_size);
                let pixel_z = u32::try_from(pixel_index)
                    .ok()
                    .map_or(0, |index| index / job.tile_size);
                let volume = || {
                    (prepared.block_volume_enabled || prepared.atlas_enabled).then(|| {
                        BlockVolumeContext::new(
                            job,
                            pixel_x,
                            pixel_z,
                            &prepared.block_heights,
                            &prepared.block_aux,
                            prepared.block_height_grid_width,
                            prepared.block_height_grid_height,
                            prepared.block_height_grid_padding,
                        )
                    })
                };
                color = if prepared.atlas_enabled {
                    apply_atlas_shading(color, neighborhood, water_depth, surface, aux, volume())
                } else {
                    surface_lit_color(
                        palette,
                        color,
                        neighborhood,
                        water_depth,
                        surface,
                        volume(),
                        Some(BlockBoundaryContext::new(job, pixel_x, pixel_z)),
                    )
                };
            }
        }
        write_rgba_pixel(&mut rgba, pixel_index, color);
    }
    rgba
}

fn pack_rgba_color(color: RgbaColor) -> u32 {
    u32::from(color.red)
        | (u32::from(color.green) << 8)
        | (u32::from(color.blue) << 16)
        | (u32::from(color.alpha) << 24)
}

fn unpack_rgba_color(color: u32) -> RgbaColor {
    RgbaColor::new(
        u8_from_u32(color & 0xff),
        u8_from_u32((color >> 8) & 0xff),
        u8_from_u32((color >> 16) & 0xff),
        u8_from_u32((color >> 24) & 0xff),
    )
}

fn validate_job(job: &RenderJob) -> Result<()> {
    if job.tile_size == 0 {
        return Err(BedrockRenderError::Validation(
            "tile_size must be greater than zero".to_string(),
        ));
    }
    if job.scale == 0 {
        return Err(BedrockRenderError::Validation(
            "scale must be greater than zero".to_string(),
        ));
    }
    if job.pixels_per_block == 0 {
        return Err(BedrockRenderError::Validation(
            "pixels_per_block must be greater than zero".to_string(),
        ));
    }
    if job.tile_size > MAX_TILE_SIZE_PIXELS {
        return Err(BedrockRenderError::Validation(format!(
            "tile_size must be <= {MAX_TILE_SIZE_PIXELS}"
        )));
    }
    let tile_scaled_pixels = job
        .tile_size
        .checked_mul(job.scale)
        .ok_or_else(|| BedrockRenderError::Validation("tile block span overflow".to_string()))?;
    if tile_scaled_pixels % job.pixels_per_block != 0 {
        return Err(BedrockRenderError::Validation(
            "tile_size * scale must be divisible by pixels_per_block".to_string(),
        ));
    }
    Ok(())
}

fn validate_layout(layout: ChunkTileLayout) -> Result<()> {
    if layout.chunks_per_tile == 0 {
        return Err(BedrockRenderError::Validation(
            "chunks_per_tile must be greater than zero".to_string(),
        ));
    }
    if layout.blocks_per_pixel == 0 {
        return Err(BedrockRenderError::Validation(
            "blocks_per_pixel must be greater than zero".to_string(),
        ));
    }
    if layout.pixels_per_block == 0 {
        return Err(BedrockRenderError::Validation(
            "pixels_per_block must be greater than zero".to_string(),
        ));
    }
    if layout.tile_size().is_none() {
        return Err(BedrockRenderError::Validation(format!(
            "chunks_per_tile * 16 * pixels_per_block must be divisible by blocks_per_pixel and produce a tile <= {MAX_TILE_SIZE_PIXELS}px"
        )));
    }
    Ok(())
}

fn validate_region(region: ChunkRegion) -> Result<()> {
    if region.min_chunk_x > region.max_chunk_x || region.min_chunk_z > region.max_chunk_z {
        return Err(BedrockRenderError::Validation(format!(
            "invalid chunk region: min=({}, {}) max=({}, {})",
            region.min_chunk_x, region.min_chunk_z, region.max_chunk_x, region.max_chunk_z
        )));
    }
    Ok(())
}

fn floor_div(value: i32, divisor: i32) -> i32 {
    value.div_euclid(divisor)
}

fn dimension_slug(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "overworld".to_string(),
        Dimension::Nether => "nether".to_string(),
        Dimension::End => "end".to_string(),
        Dimension::Unknown(id) => format!("dimension-{id}"),
    }
}

fn mode_slug(mode: RenderMode) -> String {
    match mode {
        RenderMode::Biome { y } => format!("biome-y{y}"),
        RenderMode::RawBiomeLayer { y } => format!("raw-biome-y{y}"),
        RenderMode::SurfaceBlocks => "surface".to_string(),
        RenderMode::LayerBlocks { y } => format!("layer-y{y}"),
        RenderMode::HeightMap => "heightmap".to_string(),
        RenderMode::RawHeightMap => "raw-heightmap".to_string(),
        RenderMode::CaveSlice { y } => format!("cave-y{y}"),
    }
}

fn surface_options_hash(surface: SurfaceRenderOptions) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in format!("{surface:?}").as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const fn image_format_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::WebP => "webp",
        ImageFormat::Png => "png",
        ImageFormat::FastRgbaZstd => "brtile",
        ImageFormat::Rgba => "rgba",
    }
}

fn tile_pixel_to_block(job: &RenderJob, pixel_x: u32, pixel_z: u32) -> Result<(i32, i32)> {
    let bounds = tile_pixel_block_bounds(job, pixel_x, pixel_z)?;
    Ok((bounds.min_x, bounds.min_z))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TilePixelBlockBounds {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

impl TilePixelBlockBounds {
    const fn covers_single_block(self) -> bool {
        self.min_x == self.max_x && self.min_z == self.max_z
    }
}

fn tile_pixel_block_bounds(
    job: &RenderJob,
    pixel_x: u32,
    pixel_z: u32,
) -> Result<TilePixelBlockBounds> {
    let (min_x, max_x) = tile_axis_block_bounds(job.coord.x, pixel_x, job, "x")?;
    let (min_z, max_z) = tile_axis_block_bounds(job.coord.z, pixel_z, job, "z")?;
    Ok(TilePixelBlockBounds {
        min_x,
        max_x,
        min_z,
        max_z,
    })
}

fn tile_axis_block_bounds(
    tile_coord: i32,
    pixel: u32,
    job: &RenderJob,
    axis: &str,
) -> Result<(i32, i32)> {
    let scale = i64::from(job.scale);
    let pixels_per_block = i64::from(job.pixels_per_block);
    let tile_span = i64::from(job.tile_size)
        .checked_mul(scale)
        .and_then(|value| value.checked_div(pixels_per_block))
        .ok_or_else(|| {
            BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
        })?;
    let origin = i64::from(tile_coord)
        .checked_mul(tile_span)
        .ok_or_else(|| {
            BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
        })?;
    let pixel_start = i64::from(pixel)
        .checked_mul(scale)
        .and_then(|value| value.checked_div(pixels_per_block))
        .ok_or_else(|| {
            BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
        })?;
    let pixel_end = i64::from(pixel)
        .checked_add(1)
        .and_then(|value| value.checked_mul(scale))
        .and_then(|value| value.checked_sub(1))
        .and_then(|value| value.checked_div(pixels_per_block))
        .ok_or_else(|| {
            BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
        })?;
    let min = origin.checked_add(pixel_start).ok_or_else(|| {
        BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
    })?;
    let max = origin.checked_add(pixel_end).ok_or_else(|| {
        BedrockRenderError::Validation(format!("tile {axis} coordinate overflow"))
    })?;
    let min = i32::try_from(min).map_err(|_| {
        BedrockRenderError::Validation(format!("tile {axis} coordinate is outside i32 range"))
    })?;
    let max = i32::try_from(max).map_err(|_| {
        BedrockRenderError::Validation(format!("tile {axis} coordinate is outside i32 range"))
    })?;
    Ok((min, max))
}

#[derive(Debug, Clone, Copy, Default)]
struct RgbaAccumulator {
    red: u64,
    green: u64,
    blue: u64,
    alpha: u64,
    samples: u64,
}

impl RgbaAccumulator {
    fn add(&mut self, color: RgbaColor) {
        let alpha = u64::from(color.alpha);
        self.red = self.red.saturating_add(u64::from(color.red) * alpha);
        self.green = self.green.saturating_add(u64::from(color.green) * alpha);
        self.blue = self.blue.saturating_add(u64::from(color.blue) * alpha);
        self.alpha = self.alpha.saturating_add(alpha);
        self.samples = self.samples.saturating_add(1);
    }

    fn average(self) -> Option<RgbaColor> {
        if self.samples == 0 {
            return None;
        }
        if self.alpha == 0 {
            return Some(RgbaColor::new(0, 0, 0, 0));
        }
        let alpha = (self.alpha + self.samples / 2) / self.samples;
        Some(RgbaColor::new(
            u8_from_u64((self.red + self.alpha / 2) / self.alpha),
            u8_from_u64((self.green + self.alpha / 2) / self.alpha),
            u8_from_u64((self.blue + self.alpha / 2) / self.alpha),
            u8_from_u64(alpha),
        ))
    }
}

fn write_rgba_pixel(rgba: &mut [u8], pixel_index: usize, color: RgbaColor) {
    let Some(offset) = pixel_index.checked_mul(4) else {
        return;
    };
    let Some(pixel) = rgba.get_mut(offset..offset.saturating_add(4)) else {
        return;
    };
    pixel[0] = color.red;
    pixel[1] = color.green;
    pixel[2] = color.blue;
    pixel[3] = color.alpha;
}

fn lighting_enabled_for(mode: RenderMode, surface: SurfaceRenderOptions) -> bool {
    surface.height_shading
        && surface.lighting.enabled
        && matches!(mode, RenderMode::SurfaceBlocks | RenderMode::HeightMap)
}

const fn atlas_enabled(surface: SurfaceRenderOptions) -> bool {
    surface.atlas.enabled
}

const fn surface_gbuffer_enabled(surface: SurfaceRenderOptions) -> bool {
    atlas_enabled(surface)
}

fn atlas_enabled_for(mode: RenderMode, surface: SurfaceRenderOptions, job: &RenderJob) -> bool {
    surface.height_shading
        && surface.lighting.enabled
        && atlas_enabled(surface)
        && job.scale == 1
        && job.pixels_per_block > 0
        && matches!(mode, RenderMode::SurfaceBlocks)
}

fn block_volume_enabled_for(
    mode: RenderMode,
    surface: SurfaceRenderOptions,
    job: &RenderJob,
) -> bool {
    surface.height_shading
        && surface.block_volume.enabled
        && job.scale == 1
        && job.pixels_per_block > 0
        && matches!(mode, RenderMode::SurfaceBlocks | RenderMode::HeightMap)
}

fn render_chunk_request(mode: RenderMode) -> ChunkDataRequest {
    match mode {
        RenderMode::SurfaceBlocks => ChunkDataRequest::new()
            .surface_columns(ExactSurfaceSubchunkPolicy::Full)
            .biome(BiomeDataRequirement::SurfaceColumns)
            .block_entities(),
        RenderMode::HeightMap => ChunkDataRequest::new()
            .surface_columns(ExactSurfaceSubchunkPolicy::Full)
            .height_map(),
        RenderMode::RawHeightMap => ChunkDataRequest::new().height_map(),
        RenderMode::LayerBlocks { y } => ChunkDataRequest::new().layer(y),
        RenderMode::CaveSlice { y } => ChunkDataRequest::new().cave_slice(y),
        RenderMode::Biome { y } | RenderMode::RawBiomeLayer { y } => {
            ChunkDataRequest::new().biome(BiomeDataRequirement::Layer(y))
        }
    }
}

fn render_chunk_request_for_options(mode: RenderMode, options: &RenderOptions) -> ChunkDataRequest {
    let surface_subchunks = match options.performance.surface_load {
        RenderSurfaceLoadPolicy::Full => ExactSurfaceSubchunkPolicy::Full,
        RenderSurfaceLoadPolicy::HintThenVerify => ExactSurfaceSubchunkPolicy::HintThenVerify,
    };
    match mode {
        RenderMode::SurfaceBlocks => ChunkDataRequest::new()
            .surface_columns(surface_subchunks)
            .biome(BiomeDataRequirement::SurfaceColumns)
            .block_entities(),
        RenderMode::HeightMap => ChunkDataRequest::new()
            .surface_columns(surface_subchunks)
            .height_map(),
        other => render_chunk_request(other),
    }
}

const fn render_subchunk_decode_mode(mode: RenderMode) -> SubChunkDecodeMode {
    match mode {
        RenderMode::SurfaceBlocks | RenderMode::HeightMap => SubChunkDecodeMode::SurfaceColumns,
        RenderMode::RawHeightMap
        | RenderMode::LayerBlocks { .. }
        | RenderMode::CaveSlice { .. }
        | RenderMode::Biome { .. }
        | RenderMode::RawBiomeLayer { .. } => SubChunkDecodeMode::FullIndices,
    }
}

fn render_chunk_load_options(mode: RenderMode) -> ChunkLoadOptions {
    ChunkLoadOptions {
        data_request: render_chunk_request(mode),
        subchunk_decode: render_subchunk_decode_mode(mode),
        threading: WorldThreadingOptions::Single,
        ..ChunkLoadOptions::default()
    }
}

fn render_world_cancel(options: &RenderOptions) -> Option<WorldCancelFlag> {
    options
        .cancel
        .as_ref()
        .map(|cancel| WorldCancelFlag::from_shared(Arc::clone(&cancel.0)))
}

const fn render_storage_cache_policy(options: &RenderOptions) -> StorageCachePolicy {
    match options.cache_policy {
        RenderCachePolicy::Bypass => StorageCachePolicy::Bypass,
        RenderCachePolicy::Use | RenderCachePolicy::Refresh => StorageCachePolicy::Use,
    }
}

fn render_world_threading(
    options: &RenderOptions,
    work_items: usize,
) -> Result<WorldThreadingOptions> {
    let workers = resolve_render_worker_count(options, work_items.max(1))?;
    Ok(if workers <= 1 {
        WorldThreadingOptions::Single
    } else {
        WorldThreadingOptions::Fixed(workers)
    })
}

fn render_world_threading_with_budget(
    options: &RenderOptions,
    work_items: usize,
    worker_budget: usize,
) -> WorldThreadingOptions {
    if work_items == 0
        || worker_budget <= 1
        || matches!(options.threading, RenderThreadingOptions::Single)
    {
        WorldThreadingOptions::Single
    } else {
        WorldThreadingOptions::Fixed(worker_budget.min(work_items).max(1))
    }
}

fn merge_render_chunk_region(target: &mut ChunkRegion, other: ChunkRegion) {
    target.min_chunk_x = target.min_chunk_x.min(other.min_chunk_x);
    target.min_chunk_z = target.min_chunk_z.min(other.min_chunk_z);
    target.max_chunk_x = target.max_chunk_x.max(other.max_chunk_x);
    target.max_chunk_z = target.max_chunk_z.max(other.max_chunk_z);
}

fn render_chunk_region_area(region: &ChunkRegion) -> Result<usize> {
    let x_count = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
    let z_count = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
    usize::try_from(x_count.saturating_mul(z_count))
        .map_err(|_| BedrockRenderError::Validation("render cull region is too large".to_string()))
}

const fn should_use_full_render_chunk_index(candidate_chunks: usize) -> bool {
    candidate_chunks > SESSION_BATCH_CULL_FULL_INDEX_THRESHOLD_CHUNKS
}

fn render_chunk_region_contains(region: ChunkRegion, pos: ChunkPos) -> bool {
    pos.dimension == region.dimension
        && pos.x >= region.min_chunk_x
        && pos.x <= region.max_chunk_x
        && pos.z >= region.min_chunk_z
        && pos.z <= region.max_chunk_z
}
fn render_chunk_priority_for_region(
    options: &RenderOptions,
    region: ChunkRegion,
) -> ChunkLoadPriority {
    match options.priority {
        RenderTilePriority::RowMajor => ChunkLoadPriority::RowMajor,
        RenderTilePriority::DistanceFrom { tile_x, tile_z } => {
            let chunks_per_tile = i32::try_from(region.max_chunk_x - region.min_chunk_x + 1)
                .unwrap_or(1)
                .max(1);
            ChunkLoadPriority::DistanceFrom {
                chunk_x: tile_x.saturating_mul(chunks_per_tile),
                chunk_z: tile_z.saturating_mul(chunks_per_tile),
            }
        }
    }
}

fn render_chunk_priority_for_tile(options: &RenderOptions, pos: ChunkPos) -> ChunkLoadPriority {
    match options.priority {
        RenderTilePriority::RowMajor => ChunkLoadPriority::RowMajor,
        RenderTilePriority::DistanceFrom { .. } => ChunkLoadPriority::DistanceFrom {
            chunk_x: pos.x,
            chunk_z: pos.z,
        },
    }
}

fn block_entity_index(data: &ChunkData) -> BTreeMap<(u8, i32, u8), usize> {
    let mut index = BTreeMap::new();
    for (entity_index, block_entity) in data.block_entities.iter().enumerate() {
        let Some([x, y, z]) = block_entity.position else {
            continue;
        };
        if x.div_euclid(16) != data.pos.x || z.div_euclid(16) != data.pos.z {
            continue;
        }
        let Ok(local_x) = u8::try_from(x.rem_euclid(16)) else {
            continue;
        };
        let Ok(local_z) = u8::try_from(z.rem_euclid(16)) else {
            continue;
        };
        index.insert((local_x, y, local_z), entity_index);
    }
    index
}

fn baked_tile_pixel_color(
    palette: &RenderPalette,
    job: &RenderJob,
    options: &RenderOptions,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    pixel_x: u32,
    pixel_z: u32,
    diagnostics: &mut RenderDiagnostics,
) -> Result<RgbaColor> {
    let bounds = tile_pixel_block_bounds(job, pixel_x, pixel_z)?;
    if bounds.covers_single_block() {
        return Ok(chunk_shaded_color_at_block(
            palette,
            bakes,
            job.coord.dimension,
            job.mode,
            bounds.min_x,
            bounds.min_z,
            options.surface,
            Some(BlockBoundaryContext::new(job, pixel_x, pixel_z)),
        )
        .unwrap_or_else(|| {
            diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
            diagnostics.record_transparent_pixel();
            palette.missing_chunk_color()
        }));
    }

    let mut average = RgbaAccumulator::default();
    for block_z in bounds.min_z..=bounds.max_z {
        for block_x in bounds.min_x..=bounds.max_x {
            if let Some(color) = chunk_shaded_color_at_block(
                palette,
                bakes,
                job.coord.dimension,
                job.mode,
                block_x,
                block_z,
                options.surface,
                None,
            ) {
                average.add(color);
            } else {
                diagnostics.missing_chunks = diagnostics.missing_chunks.saturating_add(1);
                diagnostics.record_transparent_pixel();
                average.add(palette.missing_chunk_color());
            }
        }
    }
    Ok(average
        .average()
        .unwrap_or_else(|| palette.missing_chunk_color()))
}

#[allow(clippy::too_many_arguments)]
fn region_shaded_color_at_block(
    palette: &RenderPalette,
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    surface: SurfaceRenderOptions,
    boundary: Option<BlockBoundaryContext>,
) -> Option<RgbaColor> {
    let color = region_color_at_block(regions, region_layout, dimension, mode, block_x, block_z)?;
    Some(shade_region_color(
        palette,
        color,
        regions,
        region_layout,
        dimension,
        mode,
        block_x,
        block_z,
        surface,
        boundary,
    ))
}

#[allow(clippy::too_many_arguments)]
fn chunk_shaded_color_at_block(
    palette: &RenderPalette,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    surface: SurfaceRenderOptions,
    boundary: Option<BlockBoundaryContext>,
) -> Option<RgbaColor> {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let bake = bakes.get(&chunk_pos)?;
    if bake.mode != mode {
        return None;
    }
    let color = chunk_bake_color(bake, u32::from(local_x), u32::from(local_z))?;
    Some(shade_chunk_bake_color(
        palette, color, bakes, chunk_pos, local_x, local_z, surface, boundary,
    ))
}

fn chunk_color_at_block(
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> Option<(ChunkPos, u8, u8, RgbaColor)> {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let bake = bakes.get(&chunk_pos)?;
    if bake.mode != mode {
        return None;
    }
    let color = chunk_bake_color(bake, u32::from(local_x), u32::from(local_z))?;
    Some((chunk_pos, local_x, local_z, color))
}

#[allow(clippy::too_many_arguments)]
fn shade_region_color(
    palette: &RenderPalette,
    color: RgbaColor,
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    surface: SurfaceRenderOptions,
    boundary: Option<BlockBoundaryContext>,
) -> RgbaColor {
    if !lighting_enabled_for(mode, surface) {
        return color;
    }
    let Some(height) =
        region_height_at_block(regions, region_layout, dimension, mode, block_x, block_z)
    else {
        return color;
    };
    let heights = region_height_neighborhood_at_block(
        regions,
        region_layout,
        dimension,
        mode,
        block_x,
        block_z,
        height,
    );
    let water_depth =
        region_water_depth_at_block(regions, region_layout, dimension, mode, block_x, block_z);
    surface_lit_color(
        palette,
        color,
        heights,
        water_depth,
        surface,
        None,
        boundary,
    )
}

#[allow(clippy::too_many_arguments)]
fn shade_chunk_bake_color(
    palette: &RenderPalette,
    color: RgbaColor,
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    chunk_pos: ChunkPos,
    local_x: u8,
    local_z: u8,
    surface: SurfaceRenderOptions,
    boundary: Option<BlockBoundaryContext>,
) -> RgbaColor {
    let Some(bake) = bakes.get(&chunk_pos) else {
        return color;
    };
    if !lighting_enabled_for(bake.mode, surface) {
        return color;
    }
    let Some(height) = chunk_bake_relief_height(bake, u32::from(local_x), u32::from(local_z))
    else {
        return color;
    };
    let block_x = chunk_pos.x.saturating_mul(16) + i32::from(local_x);
    let block_z = chunk_pos.z.saturating_mul(16) + i32::from(local_z);
    let heights = chunk_bake_height_neighborhood_at_block(
        bakes,
        chunk_pos.dimension,
        bake.mode,
        block_x,
        block_z,
        height,
    );
    let water_depth = chunk_bake_water_depth(bake, u32::from(local_x), u32::from(local_z));
    surface_lit_color(
        palette,
        color,
        heights,
        water_depth,
        surface,
        None,
        boundary,
    )
}

fn region_color_at_block(
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> Option<RgbaColor> {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let region_key = region_key_for_chunk(chunk_pos, region_layout, mode);
    regions
        .get(&region_key)
        .and_then(|region| region.color_at_chunk_local(chunk_pos, local_x, local_z))
}

fn region_height_at_block(
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> Option<i16> {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let region_key = region_key_for_chunk(chunk_pos, region_layout, mode);
    regions
        .get(&region_key)
        .and_then(|region| region.height_at_chunk_local(chunk_pos, local_x, local_z))
}

fn region_height_neighborhood_at_block(
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    fallback: i16,
) -> TerrainHeightNeighborhood {
    let height_at = |x, z| {
        region_height_at_block(regions, region_layout, dimension, mode, x, z).unwrap_or(fallback)
    };
    TerrainHeightNeighborhood {
        center: fallback,
        north_west: height_at(block_x - 1, block_z - 1),
        north: height_at(block_x, block_z - 1),
        north_east: height_at(block_x + 1, block_z - 1),
        west: height_at(block_x - 1, block_z),
        east: height_at(block_x + 1, block_z),
        south_west: height_at(block_x - 1, block_z + 1),
        south: height_at(block_x, block_z + 1),
        south_east: height_at(block_x + 1, block_z + 1),
    }
}

fn region_water_depth_at_block(
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> u8 {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let region_key = region_key_for_chunk(chunk_pos, region_layout, mode);
    regions
        .get(&region_key)
        .and_then(|region| {
            let (pixel_x, pixel_z) = region.region_pixel(chunk_pos, local_x, local_z)?;
            Some(region.water_depth_at_region_pixel(pixel_x, pixel_z))
        })
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn region_atlas_aux_at_block(
    regions: &RegionBakeMap,
    region_layout: RegionLayout,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    gbuffer_enabled: bool,
) -> u32 {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    let region_key = region_key_for_chunk(chunk_pos, region_layout, mode);
    regions
        .get(&region_key)
        .and_then(|region| {
            let (pixel_x, pixel_z) = region.region_pixel(chunk_pos, local_x, local_z)?;
            if gbuffer_enabled {
                Some(region.atlas_aux_at_region_pixel(pixel_x, pixel_z))
            } else {
                Some(u32::from(
                    region.water_depth_at_region_pixel(pixel_x, pixel_z),
                ))
            }
        })
        .unwrap_or(0)
}

fn chunk_bake_height_at_block(
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> Option<i16> {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    bakes.get(&chunk_pos).and_then(|bake| {
        (bake.mode == mode)
            .then(|| chunk_bake_relief_height(bake, u32::from(local_x), u32::from(local_z)))
            .flatten()
    })
}

fn chunk_bake_atlas_aux_at_block(
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
) -> u32 {
    let block_pos = BlockPos {
        x: block_x,
        y: 0,
        z: block_z,
    };
    let chunk_pos = block_pos.to_chunk_pos(dimension);
    let (local_x, _, local_z) = block_pos.in_chunk_offset();
    bakes.get(&chunk_pos).map_or(0, |bake| {
        if bake.mode == mode {
            chunk_bake_atlas_aux(bake, u32::from(local_x), u32::from(local_z))
        } else {
            0
        }
    })
}

fn chunk_bake_height_neighborhood_at_block(
    bakes: &BTreeMap<ChunkPos, ChunkBake>,
    dimension: Dimension,
    mode: RenderMode,
    block_x: i32,
    block_z: i32,
    fallback: i16,
) -> TerrainHeightNeighborhood {
    let height_at =
        |x, z| chunk_bake_height_at_block(bakes, dimension, mode, x, z).unwrap_or(fallback);
    TerrainHeightNeighborhood {
        center: fallback,
        north_west: height_at(block_x - 1, block_z - 1),
        north: height_at(block_x, block_z - 1),
        north_east: height_at(block_x + 1, block_z - 1),
        west: height_at(block_x - 1, block_z),
        east: height_at(block_x + 1, block_z),
        south_west: height_at(block_x - 1, block_z + 1),
        south: height_at(block_x, block_z + 1),
        south_east: height_at(block_x + 1, block_z + 1),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn surface_lit_color(
    palette: &RenderPalette,
    color: RgbaColor,
    heights: TerrainHeightNeighborhood,
    water_depth: u8,
    surface: SurfaceRenderOptions,
    volume: Option<BlockVolumeContext<'_>>,
    boundary: Option<BlockBoundaryContext>,
) -> RgbaColor {
    if shallow_water_skips_lighting(water_depth) {
        return color;
    }
    let lit = terrain_lit_color(color, heights, water_depth, surface.lighting);
    let moderated = moderate_surface_lighting(palette, color, lit, water_depth, surface);
    let volumetric = apply_block_volume_shading(moderated, heights, water_depth, surface, volume);
    apply_block_boundary_shading(volumetric, heights, water_depth, surface, boundary)
}

fn moderate_surface_lighting(
    palette: &RenderPalette,
    base: RgbaColor,
    lit: RgbaColor,
    water_depth: u8,
    surface: SurfaceRenderOptions,
) -> RgbaColor {
    if water_depth > 0 {
        return alpha_blend_surface(base, lit, 72);
    }
    if surface.biome_tint && palette.is_biome_surface_color(base) {
        return alpha_blend_surface(base, lit, 104);
    }
    lit
}

fn shallow_water_skips_lighting(water_depth: u8) -> bool {
    matches!(water_depth, 1 | 2)
}

fn apply_atlas_shading(
    color: RgbaColor,
    heights: TerrainHeightNeighborhood,
    water_depth: u8,
    surface: SurfaceRenderOptions,
    aux: u32,
    volume: Option<BlockVolumeContext<'_>>,
) -> RgbaColor {
    if color.alpha == 0 || !surface.height_shading || !atlas_enabled(surface) {
        return color;
    }
    if heights.center == MISSING_HEIGHT {
        return color;
    }
    let options = surface.atlas;
    let material = atlas_aux_material(aux);
    let shape_flags = atlas_aux_shape_flags(aux);
    let (local_x, local_z) = atlas_local_pixel_position(volume.as_ref());
    let neighborhood = map_atlas_material_neighborhood(material, volume.as_ref());
    let mut base = map_atlas_material_color(color, material, neighborhood, options);
    let lighting = surface.lighting;
    let azimuth = lighting.light_azimuth_degrees.to_radians();
    let elevation = lighting
        .light_elevation_degrees
        .to_radians()
        .clamp(0.01, 1.55);
    let light_horizontal = elevation.cos();
    let light_x = azimuth.sin() * light_horizontal;
    let light_y = elevation.sin();
    let light_z = -azimuth.cos() * light_horizontal;
    let detail = map_atlas_pixel_detail(
        material,
        shape_flags,
        heights,
        neighborhood,
        local_x,
        local_z,
        light_x,
        light_z,
        options,
    );
    let detail_limit = map_atlas_material_detail_limit(material);
    base = shade_color_percent(
        base,
        detail
            .color_factor
            .round()
            .clamp(-detail_limit, detail_limit) as i32,
    );

    let (mut dx, mut dz) = heights.sobel_gradient();
    dx = compress_land_slope(dx, 5.5) + detail.normal_dx;
    dz = compress_land_slope(dz, 5.5) + detail.normal_dz;
    let normal_length = (dx.mul_add(dx, dz.mul_add(dz, 4.0)))
        .sqrt()
        .max(f32::EPSILON);
    let dot_light = (-dx / normal_length).mul_add(
        light_x,
        (2.0 / normal_length).mul_add(light_y, (-dz / normal_length) * light_z),
    );
    let relative_light = dot_light - light_y;
    let slope_light_weight = map_atlas_slope_light_material_weight(material);
    let mut light_accumulator = if relative_light >= 0.0 {
        relative_light * lighting.highlight_strength * 36.0 * slope_light_weight
    } else {
        relative_light * lighting.shadow_strength * 64.0 * slope_light_weight
    };
    let relief = (dx.abs() + dz.abs()).min(28.0) / 28.0;
    light_accumulator -= relief * options.ambient_occlusion_strength * 52.0 * slope_light_weight;
    light_accumulator +=
        map_atlas_neighbor_relief(heights, light_x, light_z, options) * slope_light_weight;
    light_accumulator += detail.light_factor;
    if let Some(context) = volume.as_ref() {
        light_accumulator -=
            map_atlas_cast_shadow(context, material, heights.center, light_x, light_z, options);
    }
    if matches!(material, SurfaceMaterialId::Water)
        || water_depth > 0
        || (shape_flags & ATLAS_SHAPE_WATER) != 0
    {
        light_accumulator *= 0.42;
    }
    if matches!(material, SurfaceMaterialId::Lava) {
        light_accumulator *= 0.18;
    }
    let max_shadow = if matches!(material, SurfaceMaterialId::Water) {
        42.0
    } else {
        72.0
    };
    let max_highlight = if matches!(material, SurfaceMaterialId::Snow) {
        28.0
    } else {
        40.0
    };
    let factor = light_accumulator.round().clamp(-max_shadow, max_highlight) as i32;
    if factor == 0 {
        base
    } else {
        shade_color_percent(base, factor)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct MapAtlasMaterialNeighborhood {
    foliage_count: u8,
    water_count: u8,
    stone_count: u8,
    snow_count: u8,
    west_material_edge: f32,
    east_material_edge: f32,
    north_material_edge: f32,
    south_material_edge: f32,
    shoreline: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct MapAtlasPixelDetail {
    color_factor: f32,
    normal_dx: f32,
    normal_dz: f32,
    light_factor: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct MapAtlasHeightEdgeDetail {
    edge_factor: f32,
    shadow: f32,
    highlight: f32,
    color_factor: f32,
    normal_dx: f32,
    normal_dz: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct MapAtlasCanopyDetail {
    edge_factor: f32,
    lit_edge: f32,
    shadow_edge: f32,
    high_edge: f32,
    normal_dx: f32,
    normal_dz: f32,
}

fn map_atlas_material_neighborhood(
    material: SurfaceMaterialId,
    volume: Option<&BlockVolumeContext<'_>>,
) -> MapAtlasMaterialNeighborhood {
    let Some(context) = volume else {
        return MapAtlasMaterialNeighborhood {
            foliage_count: u8::from(matches!(material, SurfaceMaterialId::Foliage)),
            water_count: u8::from(matches!(material, SurfaceMaterialId::Water)),
            stone_count: u8::from(matches!(material, SurfaceMaterialId::Stone)),
            snow_count: u8::from(matches!(material, SurfaceMaterialId::Snow)),
            west_material_edge: 0.0,
            east_material_edge: 0.0,
            north_material_edge: 0.0,
            south_material_edge: 0.0,
            shoreline: 0.0,
        };
    };
    let source_x = i32::try_from(context.pixel_x / context.pixels_per_block.max(1)).unwrap_or(0);
    let source_z = i32::try_from(context.pixel_z / context.pixels_per_block.max(1)).unwrap_or(0);
    let mut foliage_count = 0_u8;
    let mut water_count = 0_u8;
    let mut stone_count = 0_u8;
    let mut snow_count = 0_u8;
    let mut west_material_edge = 0.0_f32;
    let mut east_material_edge = 0.0_f32;
    let mut north_material_edge = 0.0_f32;
    let mut south_material_edge = 0.0_f32;
    for dz in -1_i32..=1 {
        for dx in -1_i32..=1 {
            let neighbor_material = context
                .aux_at(source_x + dx, source_z + dz)
                .map(atlas_aux_material)
                .unwrap_or(material);
            foliage_count = foliage_count.saturating_add(u8::from(matches!(
                neighbor_material,
                SurfaceMaterialId::Foliage
            )));
            water_count = water_count.saturating_add(u8::from(matches!(
                neighbor_material,
                SurfaceMaterialId::Water
            )));
            stone_count = stone_count.saturating_add(u8::from(matches!(
                neighbor_material,
                SurfaceMaterialId::Stone
            )));
            snow_count = snow_count.saturating_add(u8::from(matches!(
                neighbor_material,
                SurfaceMaterialId::Snow
            )));
            if dx.abs() + dz.abs() == 1 {
                if neighbor_material != material {
                    match (dx, dz) {
                        (-1, 0) => west_material_edge = 1.0,
                        (1, 0) => east_material_edge = 1.0,
                        (0, -1) => north_material_edge = 1.0,
                        (0, 1) => south_material_edge = 1.0,
                        _ => {}
                    }
                }
            }
        }
    }
    let shoreline = if matches!(material, SurfaceMaterialId::Water) {
        f32::from(9_u8.saturating_sub(water_count)) / 9.0
    } else {
        f32::from(water_count) / 9.0
    };
    MapAtlasMaterialNeighborhood {
        foliage_count,
        water_count,
        stone_count,
        snow_count,
        west_material_edge,
        east_material_edge,
        north_material_edge,
        south_material_edge,
        shoreline,
    }
}

fn map_atlas_material_color(
    color: RgbaColor,
    material: SurfaceMaterialId,
    neighborhood: MapAtlasMaterialNeighborhood,
    options: AtlasRenderOptions,
) -> RgbaColor {
    let target = match material {
        SurfaceMaterialId::Grass | SurfaceMaterialId::Plant => RgbaColor::new(88, 128, 76, 255),
        SurfaceMaterialId::Foliage => {
            if neighborhood.foliage_count >= 6 {
                RgbaColor::new(20, 104, 24, 255)
            } else {
                RgbaColor::new(42, 130, 44, 255)
            }
        }
        SurfaceMaterialId::Snow => RgbaColor::new(238, 242, 242, 255),
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => RgbaColor::new(126, 132, 126, 255),
        SurfaceMaterialId::Dirt => RgbaColor::new(112, 96, 62, 255),
        SurfaceMaterialId::Sand => RgbaColor::new(190, 178, 118, 255),
        SurfaceMaterialId::Water => RgbaColor::new(44, 70, 172, 255),
        SurfaceMaterialId::Lava => RgbaColor::new(255, 82, 14, 255),
        SurfaceMaterialId::Wood | SurfaceMaterialId::Built => RgbaColor::new(126, 104, 74, 255),
        SurfaceMaterialId::Unknown => color,
    };
    let alpha = (options.texture_detail_strength.clamp(0.0, 1.0) * 92.0).round() as u8;
    alpha_blend_surface(target, color, alpha)
}

fn map_atlas_pixel_detail(
    material: SurfaceMaterialId,
    shape_flags: u8,
    heights: TerrainHeightNeighborhood,
    neighborhood: MapAtlasMaterialNeighborhood,
    local_x: f32,
    local_z: f32,
    light_x: f32,
    light_z: f32,
    options: AtlasRenderOptions,
) -> MapAtlasPixelDetail {
    let centered_x = local_x - 0.5;
    let centered_z = local_z - 0.5;
    let (slope_x, slope_z) = heights.sobel_gradient();
    let relief = (slope_x.abs() + slope_z.abs()).min(24.0) / 24.0;
    let texture = map_atlas_texture_for(material, neighborhood);
    let atlas = map_atlas_texture_sample(texture, local_x, local_z);
    let contour = map_atlas_contour_factor(heights, local_x, local_z, options);
    let block_edge = map_atlas_block_edge_factor(local_x, local_z);
    let raw_material_edge = map_atlas_directional_material_edge(local_x, local_z, neighborhood);
    let material_edge = raw_material_edge * options.material_edge_strength;
    let height_edge =
        map_atlas_height_edge_detail(local_x, local_z, heights, material, light_x, light_z);
    let contour_weight = map_atlas_contour_material_weight(material);
    let connected_edge = raw_material_edge.max(height_edge.edge_factor);
    let chunk_grid = block_edge * connected_edge * options.chunk_grid_strength;
    let water_grid = if matches!(material, SurfaceMaterialId::Water) {
        block_edge * options.water_grid_strength
    } else {
        0.0
    };
    let shoreline = neighborhood.shoreline * options.shoreline_shadow_strength;
    let hatch = map_atlas_hatch_factor(local_x, local_z) * relief * options.slope_hatching_strength;
    let mut detail = MapAtlasPixelDetail {
        color_factor: atlas * options.texture_detail_strength,
        normal_dx: height_edge.normal_dx,
        normal_dz: height_edge.normal_dz,
        light_factor: -contour * options.height_contour_strength * 24.0 * contour_weight
            - material_edge * 30.0
            - height_edge.shadow
            + height_edge.highlight
            - connected_edge * block_edge * 10.0
            - water_grid * 22.0
            - chunk_grid * 18.0
            - shoreline * 24.0,
    };
    detail.color_factor += height_edge.color_factor;
    if matches!(material, SurfaceMaterialId::Foliage) || (shape_flags & ATLAS_SHAPE_FOLIAGE) != 0 {
        let density = f32::from(neighborhood.foliage_count) / 9.0;
        let canopy =
            map_atlas_canopy_detail(local_x, local_z, heights, neighborhood, light_x, light_z);
        let sparse_dome = (1.0 - density).clamp(0.0, 1.0)
            * (1.0 - centered_x.mul_add(centered_x, centered_z * centered_z) / 0.46)
                .clamp(0.0, 1.0)
            * 0.18;
        detail.color_factor += (canopy.lit_edge * 18.0 - canopy.shadow_edge * 14.0 - density * 5.0
            + sparse_dome * 10.0)
            * options.forest_canopy_strength;
        detail.light_factor += (canopy.lit_edge * 20.0 - canopy.shadow_edge * 22.0 - density * 5.0
            + canopy.high_edge * 8.0
            + sparse_dome * 8.0)
            * options.forest_canopy_strength;
        detail.normal_dx += canopy.normal_dx * options.forest_canopy_strength;
        detail.normal_dz += canopy.normal_dz * options.forest_canopy_strength;
    } else if matches!(material, SurfaceMaterialId::Snow) {
        let snow_density = f32::from(neighborhood.snow_count) / 9.0;
        detail.color_factor -= (contour * 13.0 + hatch * 16.0) * options.snow_ridge_strength;
        detail.color_factor += snow_density * 3.0;
        detail.light_factor -= (contour * 22.0 + hatch * 20.0) * options.snow_ridge_strength;
        detail.normal_dx += centered_x * 0.12 * options.snow_ridge_strength;
        detail.normal_dz += centered_z * 0.12 * options.snow_ridge_strength;
    } else if matches!(
        material,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal
    ) {
        let stone_density = f32::from(neighborhood.stone_count) / 9.0;
        detail.color_factor -= contour * 12.0 + hatch * 13.0;
        detail.color_factor -= (1.0 - stone_density) * 3.0;
        detail.light_factor -= contour * 20.0 + hatch * 17.0;
        detail.normal_dx += centered_x * 0.16;
        detail.normal_dz += centered_z * 0.16;
    } else if matches!(
        material,
        SurfaceMaterialId::Grass | SurfaceMaterialId::Plant
    ) {
        detail.color_factor += relief * 4.0 - connected_edge * 1.5 - contour * 2.0;
        detail.light_factor -= contour * options.height_contour_strength * 8.0;
    } else if matches!(material, SurfaceMaterialId::Water) {
        let water_density = f32::from(neighborhood.water_count) / 9.0;
        detail.color_factor -= shoreline * 9.0 + water_grid * 8.0;
        detail.color_factor += water_density * 2.0;
        detail.light_factor -= shoreline * 18.0;
    } else if matches!(material, SurfaceMaterialId::Lava) {
        detail.color_factor += 18.0;
        detail.light_factor += 10.0;
    }
    detail
}

fn map_atlas_texture_for(
    material: SurfaceMaterialId,
    neighborhood: MapAtlasMaterialNeighborhood,
) -> MapAtlasTextureId {
    match material {
        SurfaceMaterialId::Grass | SurfaceMaterialId::Plant => MapAtlasTextureId::Plains,
        SurfaceMaterialId::Foliage if neighborhood.foliage_count >= 5 => {
            MapAtlasTextureId::DenseCanopy
        }
        SurfaceMaterialId::Foliage => MapAtlasTextureId::SparseCanopy,
        SurfaceMaterialId::Snow => MapAtlasTextureId::SnowRidge,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => MapAtlasTextureId::StoneContour,
        SurfaceMaterialId::Dirt => MapAtlasTextureId::DirtPatch,
        SurfaceMaterialId::Sand => MapAtlasTextureId::SandRipples,
        SurfaceMaterialId::Water => MapAtlasTextureId::WaterGrid,
        SurfaceMaterialId::Lava => MapAtlasTextureId::LavaAccent,
        SurfaceMaterialId::Wood | SurfaceMaterialId::Built => MapAtlasTextureId::Structure,
        SurfaceMaterialId::Unknown => MapAtlasTextureId::Plains,
    }
}

fn map_atlas_texture_sample(texture: MapAtlasTextureId, local_x: f32, local_z: f32) -> f32 {
    let centered_x = local_x - 0.5;
    let centered_z = local_z - 0.5;
    let radius = centered_x.mul_add(centered_x, centered_z * centered_z);
    let edge = centered_x.abs().max(centered_z.abs());
    let crown = (1.0 - radius / 0.34).clamp(0.0, 1.0);
    let rim = ((edge - 0.30) / 0.20).clamp(0.0, 1.0);
    let diagonal = map_atlas_soft_band(local_x + local_z, 1.0, 0.28);
    let cross =
        map_atlas_soft_band(local_x, 0.5, 0.22).max(map_atlas_soft_band(local_z, 0.5, 0.22));
    match texture {
        MapAtlasTextureId::Plains | MapAtlasTextureId::WaterGrid => 0.0,
        MapAtlasTextureId::DenseCanopy | MapAtlasTextureId::SparseCanopy => 0.0,
        MapAtlasTextureId::SnowRidge => -diagonal * 5.0,
        MapAtlasTextureId::StoneContour => -diagonal * 5.0 - rim * 2.0,
        MapAtlasTextureId::DirtPatch => (0.5 - edge) * 2.0,
        MapAtlasTextureId::SandRipples => map_atlas_soft_band(local_z, 0.5, 0.34) * 2.0 - 1.0,
        MapAtlasTextureId::LavaAccent => crown * 18.0 - rim * 4.0,
        MapAtlasTextureId::Structure => cross * 2.0 - rim * 5.0,
    }
}

fn map_atlas_contour_factor(
    heights: TerrainHeightNeighborhood,
    local_x: f32,
    local_z: f32,
    options: AtlasRenderOptions,
) -> f32 {
    let interval = i16::try_from(options.height_contour_interval.max(1)).unwrap_or(1);
    let band = |height: i16| height.div_euclid(interval);
    let center_band = band(heights.center);
    let edge_width = 0.22_f32;
    let crosses = [
        (heights.west, local_x),
        (heights.east, 1.0 - local_x),
        (heights.north, local_z),
        (heights.south, 1.0 - local_z),
    ]
    .into_iter()
    .filter_map(|(neighbor, distance)| {
        (band(neighbor) != center_band).then_some((edge_width - distance).max(0.0) / edge_width)
    })
    .fold(0.0_f32, f32::max);
    let (slope_x, slope_z) = heights.sobel_gradient();
    let relief = (slope_x.abs() + slope_z.abs()).min(20.0) / 20.0;
    let interval_f = f32::from(interval.max(1));
    let projected_height =
        f32::from(heights.center) + (slope_x * (local_x - 0.5) + slope_z * (local_z - 0.5)) * 0.42;
    let phase = projected_height.rem_euclid(interval_f);
    let distance_to_band = phase.min(interval_f - phase);
    let slope_band_width = (0.36 + relief * 0.18).min(interval_f * 0.34);
    let slope_band = (1.0 - distance_to_band / slope_band_width.max(f32::EPSILON)).clamp(0.0, 1.0)
        * relief.powf(0.85);
    let cardinal_average = (f32::from(heights.west)
        + f32::from(heights.east)
        + f32::from(heights.north)
        + f32::from(heights.south))
        * 0.25;
    let curvature = ((cardinal_average - f32::from(heights.center))
        .abs()
        .min(8.0)
        / 8.0)
        * relief.max(0.35);
    (crosses * (0.58 + relief * 0.65) + slope_band.powf(1.25) * 0.48 + curvature * 0.22)
        .clamp(0.0, 1.0)
}

fn map_atlas_block_edge_factor(local_x: f32, local_z: f32) -> f32 {
    let edge_distance = local_x.min(1.0 - local_x).min(local_z.min(1.0 - local_z));
    ((0.17 - edge_distance) / 0.17).clamp(0.0, 1.0)
}

fn map_atlas_directional_material_edge(
    local_x: f32,
    local_z: f32,
    neighborhood: MapAtlasMaterialNeighborhood,
) -> f32 {
    let west = map_atlas_edge_proximity(local_x) * neighborhood.west_material_edge;
    let east = map_atlas_edge_proximity(1.0 - local_x) * neighborhood.east_material_edge;
    let north = map_atlas_edge_proximity(local_z) * neighborhood.north_material_edge;
    let south = map_atlas_edge_proximity(1.0 - local_z) * neighborhood.south_material_edge;
    west.max(east).max(north).max(south)
}

fn map_atlas_height_edge_detail(
    local_x: f32,
    local_z: f32,
    heights: TerrainHeightNeighborhood,
    material: SurfaceMaterialId,
    light_x: f32,
    light_z: f32,
) -> MapAtlasHeightEdgeDetail {
    let center = f32::from(heights.center);
    let weight = map_atlas_height_edge_material_weight(material);
    [
        (heights.west, -1.0, 0.0, map_atlas_edge_proximity(local_x)),
        (
            heights.east,
            1.0,
            0.0,
            map_atlas_edge_proximity(1.0 - local_x),
        ),
        (heights.north, 0.0, -1.0, map_atlas_edge_proximity(local_z)),
        (
            heights.south,
            0.0,
            1.0,
            map_atlas_edge_proximity(1.0 - local_z),
        ),
    ]
    .into_iter()
    .fold(MapAtlasHeightEdgeDetail::default(), |mut detail, side| {
        let (neighbor, normal_x, normal_z, proximity) = side;
        let delta = center - f32::from(neighbor);
        let amount = map_atlas_height_amount(delta.abs()) * proximity * weight;
        if amount <= 0.0 {
            return detail;
        }
        detail.edge_factor = detail.edge_factor.max(amount);
        let facing_light = (normal_x * light_x + normal_z * light_z).clamp(-1.0, 1.0);
        if delta > 0.0 {
            let lit = facing_light.max(0.0);
            let unlit = (-facing_light).max(0.0);
            let top_highlight = amount * (7.0 + 23.0 * lit);
            let face_shadow = amount * (18.0 + 34.0 * unlit);
            let face_highlight = top_highlight;
            detail.shadow += face_shadow;
            detail.highlight += face_highlight;
            detail.color_factor += face_highlight * 0.12 - face_shadow * 0.05;
            detail.normal_dx -= normal_x * amount * 0.34;
            detail.normal_dz -= normal_z * amount * 0.34;
        } else {
            let contact_shadow = amount * 38.0;
            detail.shadow += contact_shadow;
            detail.color_factor -= contact_shadow * 0.06;
            detail.normal_dx += normal_x * amount * 0.14;
            detail.normal_dz += normal_z * amount * 0.14;
        }
        detail
    })
}

fn map_atlas_canopy_detail(
    local_x: f32,
    local_z: f32,
    heights: TerrainHeightNeighborhood,
    neighborhood: MapAtlasMaterialNeighborhood,
    light_x: f32,
    light_z: f32,
) -> MapAtlasCanopyDetail {
    let center = f32::from(heights.center);
    [
        (
            heights.west,
            -1.0,
            0.0,
            neighborhood.west_material_edge,
            map_atlas_edge_proximity(local_x),
        ),
        (
            heights.east,
            1.0,
            0.0,
            neighborhood.east_material_edge,
            map_atlas_edge_proximity(1.0 - local_x),
        ),
        (
            heights.north,
            0.0,
            -1.0,
            neighborhood.north_material_edge,
            map_atlas_edge_proximity(local_z),
        ),
        (
            heights.south,
            0.0,
            1.0,
            neighborhood.south_material_edge,
            map_atlas_edge_proximity(1.0 - local_z),
        ),
    ]
    .into_iter()
    .fold(MapAtlasCanopyDetail::default(), |mut detail, side| {
        let (neighbor_height, normal_x, normal_z, material_edge, proximity) = side;
        if material_edge <= 0.0 || proximity <= 0.0 {
            return detail;
        }
        let height_delta = (center - f32::from(neighbor_height)).max(0.0);
        let height_exposure = 0.42 + map_atlas_height_amount(height_delta) * 0.58;
        let amount = material_edge * proximity * height_exposure;
        let facing_light = (normal_x * light_x + normal_z * light_z).clamp(-1.0, 1.0);
        detail.edge_factor = detail.edge_factor.max(amount);
        detail.high_edge = detail
            .high_edge
            .max(map_atlas_height_amount(height_delta) * amount);
        if facing_light >= 0.0 {
            detail.lit_edge += amount * (0.45 + facing_light * 0.55);
            detail.shadow_edge += amount * 0.12;
        } else {
            detail.lit_edge += amount * 0.16;
            detail.shadow_edge += amount * (0.42 + (-facing_light) * 0.48);
        }
        detail.normal_dx -= normal_x * amount * 0.18;
        detail.normal_dz -= normal_z * amount * 0.18;
        detail
    })
}

fn map_atlas_height_edge_material_weight(material: SurfaceMaterialId) -> f32 {
    match material {
        SurfaceMaterialId::Water => 0.34,
        SurfaceMaterialId::Foliage => 0.82,
        SurfaceMaterialId::Plant => 0.62,
        SurfaceMaterialId::Snow => 0.84,
        SurfaceMaterialId::Lava => 0.35,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => 1.12,
        SurfaceMaterialId::Dirt | SurfaceMaterialId::Sand => 1.04,
        SurfaceMaterialId::Grass => 0.74,
        SurfaceMaterialId::Wood | SurfaceMaterialId::Built => 1.18,
        _ => 1.0,
    }
}

fn map_atlas_contour_material_weight(material: SurfaceMaterialId) -> f32 {
    match material {
        SurfaceMaterialId::Water | SurfaceMaterialId::Lava => 0.0,
        SurfaceMaterialId::Foliage => 0.18,
        SurfaceMaterialId::Plant => 0.24,
        SurfaceMaterialId::Grass => 0.30,
        SurfaceMaterialId::Dirt | SurfaceMaterialId::Sand => 0.55,
        SurfaceMaterialId::Snow => 0.82,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => 1.0,
        SurfaceMaterialId::Wood | SurfaceMaterialId::Built => 0.38,
        SurfaceMaterialId::Unknown => 0.45,
    }
}

fn map_atlas_slope_light_material_weight(material: SurfaceMaterialId) -> f32 {
    match material {
        SurfaceMaterialId::Water => 0.20,
        SurfaceMaterialId::Lava => 0.12,
        SurfaceMaterialId::Foliage => 0.36,
        SurfaceMaterialId::Plant => 0.26,
        SurfaceMaterialId::Grass => 0.22,
        SurfaceMaterialId::Dirt | SurfaceMaterialId::Sand => 0.48,
        SurfaceMaterialId::Snow => 0.72,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => 1.0,
        SurfaceMaterialId::Wood | SurfaceMaterialId::Built => 0.62,
        SurfaceMaterialId::Unknown => 0.58,
    }
}

fn map_atlas_edge_proximity(distance_to_edge: f32) -> f32 {
    ((0.28 - distance_to_edge) / 0.28).clamp(0.0, 1.0)
}

fn map_atlas_hatch_factor(local_x: f32, local_z: f32) -> f32 {
    map_atlas_soft_band(local_x + local_z, 1.0, 0.24)
}

fn map_atlas_soft_band(value: f32, center: f32, half_width: f32) -> f32 {
    (1.0 - (value - center).abs() / half_width.max(f32::EPSILON)).clamp(0.0, 1.0)
}

fn map_atlas_material_detail_limit(material: SurfaceMaterialId) -> f32 {
    match material {
        SurfaceMaterialId::Foliage => 34.0,
        SurfaceMaterialId::Snow => 28.0,
        SurfaceMaterialId::Stone | SurfaceMaterialId::Metal => 30.0,
        SurfaceMaterialId::Water => 18.0,
        SurfaceMaterialId::Lava => 36.0,
        _ => 20.0,
    }
}

fn map_atlas_neighbor_relief(
    heights: TerrainHeightNeighborhood,
    light_x: f32,
    light_z: f32,
    options: AtlasRenderOptions,
) -> f32 {
    let center = f32::from(heights.center);
    let mut accumulator = 0.0;
    for (neighbor, normal_x, normal_z) in [
        (heights.west, -1.0, 0.0),
        (heights.east, 1.0, 0.0),
        (heights.north, 0.0, -1.0),
        (heights.south, 0.0, 1.0),
    ] {
        let delta = center - f32::from(neighbor);
        let amount = map_atlas_height_amount(delta.abs());
        if amount <= 0.0 {
            continue;
        }
        if delta > 0.0 {
            let facing_light = normal_x * light_x + normal_z * light_z;
            if facing_light > 0.0 {
                accumulator += amount * 12.0 * facing_light;
            } else {
                accumulator -= amount * 24.0 * (0.35 + (-facing_light).clamp(0.0, 1.0));
            }
        } else {
            accumulator -= amount * options.ambient_occlusion_strength * 28.0;
        }
    }
    accumulator
}

fn map_atlas_cast_shadow(
    context: &BlockVolumeContext<'_>,
    receiver_material: SurfaceMaterialId,
    center_height: i16,
    light_x: f32,
    light_z: f32,
    options: AtlasRenderOptions,
) -> f32 {
    if options.cast_shadow_strength <= 0.0 {
        return 0.0;
    }
    let source_x = i32::try_from(context.pixel_x / context.pixels_per_block.max(1)).unwrap_or(0);
    let source_z = i32::try_from(context.pixel_z / context.pixels_per_block.max(1)).unwrap_or(0);
    let step_x = if light_x > 0.25 {
        1
    } else if light_x < -0.25 {
        -1
    } else {
        0
    };
    let step_z = if light_z > 0.25 {
        1
    } else if light_z < -0.25 {
        -1
    } else {
        0
    };
    if step_x == 0 && step_z == 0 {
        return 0.0;
    }
    let receiver_factor = if matches!(receiver_material, SurfaceMaterialId::Water) {
        0.45
    } else {
        1.0
    };
    let mut shadow = 0.0_f32;
    for distance in 1..=MAP_ATLAS_CAST_SHADOW_RADIUS_BLOCKS {
        let distance_i32 = i32::try_from(distance).unwrap_or(i32::MAX);
        let sample_x = source_x.saturating_add(step_x * distance_i32);
        let sample_z = source_z.saturating_add(step_z * distance_i32);
        let Some(blocker_height) = context.height_at(sample_x, sample_z) else {
            continue;
        };
        let blocker_material = context
            .aux_at(sample_x, sample_z)
            .map(atlas_aux_material)
            .unwrap_or(SurfaceMaterialId::Unknown);
        let opacity = match blocker_material {
            SurfaceMaterialId::Foliage | SurfaceMaterialId::Plant => 0.56,
            SurfaceMaterialId::Water => 0.20,
            SurfaceMaterialId::Lava => 0.35,
            _ => 1.0,
        };
        let delta = blocker_height as f32 - f32::from(center_height) - distance as f32 * 0.8;
        shadow = shadow.max(map_atlas_height_amount(delta) * opacity / distance as f32);
    }
    shadow * options.cast_shadow_strength * 58.0 * receiver_factor
}

fn map_atlas_height_amount(delta: f32) -> f32 {
    if delta <= 0.75 {
        return 0.0;
    }
    let adjusted = delta - 0.75;
    adjusted / (adjusted + 4.5)
}

fn atlas_local_pixel_position(volume: Option<&BlockVolumeContext<'_>>) -> (f32, f32) {
    let Some(context) = volume else {
        return (0.5, 0.5);
    };
    let pixels_per_block = context.pixels_per_block.max(1);
    let local_x = (context.pixel_x % pixels_per_block) as f32 + 0.5;
    let local_z = (context.pixel_z % pixels_per_block) as f32 + 0.5;
    (
        local_x / pixels_per_block as f32,
        local_z / pixels_per_block as f32,
    )
}

#[allow(clippy::cast_possible_truncation)]
fn apply_block_volume_shading(
    color: RgbaColor,
    heights: TerrainHeightNeighborhood,
    water_depth: u8,
    surface: SurfaceRenderOptions,
    volume: Option<BlockVolumeContext<'_>>,
) -> RgbaColor {
    let options = surface.block_volume;
    if color.alpha == 0 || !surface.height_shading || !options.enabled {
        return color;
    }
    let Some(context) = volume else {
        return color;
    };
    if context.blocks_per_pixel != 1 || context.pixels_per_block == 0 {
        return color;
    }
    let source_x = i32::try_from(context.pixel_x / context.pixels_per_block).unwrap_or(0);
    let source_z = i32::try_from(context.pixel_z / context.pixels_per_block).unwrap_or(0);
    let Some(center_height) = context
        .height_at(source_x, source_z)
        .or(Some(i32::from(heights.center)))
    else {
        return color;
    };
    let local_x = (context.pixel_x % context.pixels_per_block) as f32 + 0.5;
    let local_z = (context.pixel_z % context.pixels_per_block) as f32 + 0.5;
    let pixels_per_block = context.pixels_per_block as f32;
    let width = options
        .face_width_pixels
        .clamp(0.25, pixels_per_block.max(0.25));
    let edge_softness = options.softness.clamp(0.25, 2.0);
    let west_factor = block_volume_edge_factor(local_x, width, edge_softness);
    let east_factor = block_volume_edge_factor(pixels_per_block - local_x, width, edge_softness);
    let north_factor = block_volume_edge_factor(local_z, width, edge_softness);
    let south_factor = block_volume_edge_factor(pixels_per_block - local_z, width, edge_softness);
    let azimuth = surface.lighting.light_azimuth_degrees.to_radians();
    let light_x = azimuth.sin();
    let light_z = -azimuth.cos();
    let mut factor = 0.0_f32;
    factor += block_volume_neighbor_factor(
        center_height,
        i32::from(heights.west),
        west_factor,
        -1.0,
        0.0,
        light_x,
        light_z,
        options,
    );
    factor += block_volume_neighbor_factor(
        center_height,
        i32::from(heights.east),
        east_factor,
        1.0,
        0.0,
        light_x,
        light_z,
        options,
    );
    factor += block_volume_neighbor_factor(
        center_height,
        i32::from(heights.north),
        north_factor,
        0.0,
        -1.0,
        light_x,
        light_z,
        options,
    );
    factor += block_volume_neighbor_factor(
        center_height,
        i32::from(heights.south),
        south_factor,
        0.0,
        1.0,
        light_x,
        light_z,
        options,
    );
    factor -= block_volume_cast_shadow(
        source_x,
        source_z,
        center_height,
        &context,
        options,
        light_x,
        light_z,
    );
    if water_depth > 0 {
        factor *= 0.45;
    }
    let factor = factor
        .round()
        .clamp(-options.max_shadow.max(0.0), options.max_highlight.max(0.0))
        as i32;
    if factor == 0 {
        return color;
    }
    shade_color_percent(color, factor)
}

fn block_volume_edge_factor(distance: f32, width: f32, softness: f32) -> f32 {
    ((width + softness - distance) / softness).clamp(0.0, 1.0)
}

fn block_volume_amount(delta: f32, options: BlockVolumeRenderOptions) -> f32 {
    let threshold = options.height_threshold.max(0.0);
    if delta <= threshold {
        return 0.0;
    }
    let delta = delta - threshold;
    delta / (delta + options.softness.max(0.001))
}

#[allow(clippy::too_many_arguments)]
fn block_volume_neighbor_factor(
    center_height: i32,
    neighbor_height: i32,
    edge_factor: f32,
    normal_x: f32,
    normal_z: f32,
    light_x: f32,
    light_z: f32,
    options: BlockVolumeRenderOptions,
) -> f32 {
    if edge_factor <= 0.0 || neighbor_height == i32::from(MISSING_HEIGHT) {
        return 0.0;
    }
    let delta = (center_height - neighbor_height) as f32;
    if delta > options.height_threshold.max(0.0) {
        let amount = block_volume_amount(delta, options) * edge_factor;
        let facing_light = normal_x.mul_add(light_x, normal_z * light_z);
        if facing_light > 0.0 {
            amount * options.highlight_strength.max(0.0) * options.max_highlight.max(0.0)
        } else {
            -amount
                * options.face_shadow_strength.max(0.0)
                * options.max_shadow.max(0.0)
                * (0.35 + (-facing_light).clamp(0.0, 1.0))
        }
    } else if -delta > options.height_threshold.max(0.0) {
        -block_volume_amount(-delta, options)
            * edge_factor
            * options.contact_shadow_strength.max(0.0)
            * options.max_shadow.max(0.0)
    } else {
        0.0
    }
}

fn block_volume_cast_shadow(
    source_x: i32,
    source_z: i32,
    center_height: i32,
    context: &BlockVolumeContext<'_>,
    options: BlockVolumeRenderOptions,
    light_x: f32,
    light_z: f32,
) -> f32 {
    let max_blocks = options.cast_shadow_max_blocks;
    if max_blocks == 0 || options.cast_shadow_strength <= 0.0 {
        return 0.0;
    }
    let step_x = if light_x > 0.25 {
        1
    } else if light_x < -0.25 {
        -1
    } else {
        0
    };
    let step_z = if light_z > 0.25 {
        1
    } else if light_z < -0.25 {
        -1
    } else {
        0
    };
    if step_x == 0 && step_z == 0 {
        return 0.0;
    }
    let mut shadow = 0.0_f32;
    for distance in 1..=max_blocks {
        let distance_i32 = i32::try_from(distance).unwrap_or(i32::MAX);
        let sample_x = source_x.saturating_add(step_x * distance_i32);
        let sample_z = source_z.saturating_add(step_z * distance_i32);
        let Some(blocker_height) = context.height_at(sample_x, sample_z) else {
            continue;
        };
        let projected_drop = distance as f32 * options.cast_shadow_height_scale.max(0.0);
        let delta = blocker_height as f32 - center_height as f32 - projected_drop;
        let amount = block_volume_amount(delta, options) / distance as f32;
        shadow = shadow.max(amount);
    }
    shadow * options.cast_shadow_strength.max(0.0) * options.max_shadow.max(0.0)
}

#[allow(clippy::cast_possible_truncation)]
fn apply_block_boundary_shading(
    color: RgbaColor,
    heights: TerrainHeightNeighborhood,
    water_depth: u8,
    surface: SurfaceRenderOptions,
    boundary: Option<BlockBoundaryContext>,
) -> RgbaColor {
    let boundary_options = surface.block_boundaries;
    if color.alpha == 0
        || !surface.height_shading
        || !boundary_options.enabled
        || boundary_options.strength <= 0.0
    {
        return color;
    }
    let Some(context) = boundary else {
        return color;
    };
    let Some(line_factor) = block_boundary_line_factor(context, boundary_options) else {
        return color;
    };
    let relief =
        heights.block_boundary_relief(boundary_options.height_threshold, boundary_options.softness);
    let water_factor = if water_depth == 0 { 1.0 } else { 0.45 };
    let flat_shadow = if context.pixels_per_block > 1 {
        line_factor * boundary_options.flat_strength.max(0.0)
    } else {
        0.0
    };
    let edge_shadow = relief.shadow * line_factor * boundary_options.strength.max(0.0);
    let edge_highlight = relief.highlight
        * line_factor
        * boundary_options.strength.max(0.0)
        * boundary_options.highlight_strength.max(0.0)
        * 100.0
        * water_factor;
    let shadow = (flat_shadow + edge_shadow) * boundary_options.max_shadow.max(0.0) * water_factor;
    let factor = (edge_highlight - shadow)
        .round()
        .clamp(-boundary_options.max_shadow.max(0.0), 35.0) as i32;
    if factor == 0 {
        return color;
    }
    shade_color_percent(color, factor)
}

#[allow(clippy::cast_precision_loss)]
fn block_boundary_line_factor(
    context: BlockBoundaryContext,
    options: BlockBoundaryRenderOptions,
) -> Option<f32> {
    if context.blocks_per_pixel != 1 {
        return None;
    }
    if context.pixels_per_block <= 1 {
        return Some(1.0);
    }
    let pixels_per_block = context.pixels_per_block as f32;
    let local_x = (context.pixel_x % context.pixels_per_block) as f32 + 0.5;
    let local_z = (context.pixel_z % context.pixels_per_block) as f32 + 0.5;
    let edge_distance = local_x
        .min(pixels_per_block - local_x)
        .min(local_z.min(pixels_per_block - local_z));
    let width = options
        .line_width_pixels
        .clamp(0.25, (pixels_per_block * 0.5).max(0.25));
    let softness = options.softness.clamp(0.25, 2.0);
    Some(((width + softness - edge_distance) / softness).clamp(0.0, 1.0))
}

#[allow(clippy::cast_possible_truncation)]
fn terrain_lit_color(
    color: RgbaColor,
    heights: TerrainHeightNeighborhood,
    water_depth: u8,
    lighting: TerrainLightingOptions,
) -> RgbaColor {
    if color.alpha == 0 || !lighting.enabled {
        return color;
    }
    let mut normal_strength = lighting.normal_strength.max(0.0);
    let mut shadow_strength = lighting.shadow_strength;
    let mut highlight_strength = lighting.highlight_strength;
    let mut ambient_occlusion = lighting.ambient_occlusion;
    let mut edge_relief_strength = lighting.edge_relief_strength.max(0.0);
    if water_depth > 0 {
        if !lighting.underwater_relief_enabled {
            return color;
        }
        let fade = underwater_depth_factor(water_depth, lighting);
        let minimum_light = lighting.underwater_min_light.clamp(0.0, 1.0);
        let light_factor = fade.max(minimum_light);
        normal_strength *= lighting.underwater_relief_strength.max(0.0) * fade;
        shadow_strength *= light_factor;
        highlight_strength *= light_factor;
        ambient_occlusion *= fade;
        edge_relief_strength *= fade;
    }
    let max_shadow = if water_depth > 0 {
        lighting.max_shadow.clamp(0.0, 26.0)
    } else {
        lighting.max_shadow.max(0.0)
    };
    if normal_strength == 0.0 {
        return color;
    }
    let (mut dx, mut dz) = heights.sobel_gradient();
    if water_depth == 0 {
        dx = compress_land_slope(dx, lighting.land_slope_softness);
        dz = compress_land_slope(dz, lighting.land_slope_softness);
    }
    let dx = dx * normal_strength;
    let dz = dz * normal_strength;
    let normal_length = (dx.mul_add(dx, dz.mul_add(dz, 4.0)))
        .sqrt()
        .max(f32::EPSILON);
    let normal_x = -dx / normal_length;
    let normal_y = 2.0 / normal_length;
    let normal_z = -dz / normal_length;
    let azimuth = lighting.light_azimuth_degrees.to_radians();
    let elevation = lighting
        .light_elevation_degrees
        .to_radians()
        .clamp(0.01, 1.55);
    let light_horizontal = elevation.cos();
    let light_x = azimuth.sin() * light_horizontal;
    let light_y = elevation.sin();
    let light_z = -azimuth.cos() * light_horizontal;
    let dot = normal_x.mul_add(light_x, normal_y.mul_add(light_y, normal_z * light_z));
    let flat_dot = light_y;
    let relief = (dx.abs() + dz.abs()).min(24.0) / 24.0;
    let relative_light = dot - flat_dot;
    let mut factor = if relative_light >= 0.0 {
        relative_light * highlight_strength * 100.0
    } else {
        relative_light * shadow_strength * 100.0
    };
    factor -= relief * ambient_occlusion * 100.0;
    if edge_relief_strength > 0.0 {
        let edge = heights.edge_relief(lighting.edge_relief_threshold);
        let edge_shadow =
            edge.shadow * edge_relief_strength * lighting.edge_relief_max_shadow.max(0.0);
        let edge_highlight = if relative_light >= 0.0 {
            edge.highlight * edge_relief_strength * lighting.edge_relief_highlight.max(0.0) * 100.0
        } else {
            0.0
        };
        factor += edge_highlight;
        factor -= edge_shadow;
    }
    shade_color_percent(color, factor.round().clamp(-max_shadow, 55.0) as i32)
}

fn compress_land_slope(delta: f32, softness: f32) -> f32 {
    let softness = softness.max(0.0);
    if softness <= f32::EPSILON {
        return 0.0;
    }
    delta / (1.0 + delta.abs() / softness)
}

fn underwater_depth_factor(water_depth: u8, lighting: TerrainLightingOptions) -> f32 {
    if water_depth == 0 {
        return 1.0;
    }
    let fade = lighting.underwater_depth_fade.max(1.0);
    (1.0 - (f32::from(water_depth.saturating_sub(1)) / fade)).clamp(0.0, 1.0)
}

fn shade_color_percent(color: RgbaColor, factor: i32) -> RgbaColor {
    RgbaColor::new(
        shade_channel_percent(color.red, factor),
        shade_channel_percent(color.green, factor),
        shade_channel_percent(color.blue, factor),
        color.alpha,
    )
}

fn shade_channel_percent(channel: u8, factor: i32) -> u8 {
    if factor >= 0 {
        let value = i32::from(channel) + ((255 - i32::from(channel)) * factor / 100);
        u8_from_i32(value.clamp(0, 255))
    } else {
        let value = i32::from(channel) * (100 + factor) / 100;
        u8_from_i32(value.clamp(0, 255))
    }
}

fn u8_from_u32(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn u8_from_u64(value: u64) -> u8 {
    u8::try_from(value.min(u64::from(u8::MAX))).unwrap_or(u8::MAX)
}

fn u8_from_i32(value: i32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn i16_from_i32(value: i32) -> i16 {
    i16::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        }
    })
}

fn tile_chunk_positions(job: &RenderJob) -> Result<Vec<ChunkPos>> {
    let start = tile_pixel_block_bounds(job, 0, 0)?;
    let last_pixel = job.tile_size.checked_sub(1).ok_or_else(|| {
        BedrockRenderError::Validation("tile_size must be greater than zero".to_string())
    })?;
    let end = tile_pixel_block_bounds(job, last_pixel, last_pixel)?;
    let min_x = start.min_x.min(end.min_x);
    let max_x = start.max_x.max(end.max_x);
    let min_z = start.min_z.min(end.min_z);
    let max_z = start.max_z.max(end.max_z);
    let min_chunk = BlockPos {
        x: min_x,
        y: 0,
        z: min_z,
    }
    .to_chunk_pos(job.coord.dimension);
    let max_chunk = BlockPos {
        x: max_x,
        y: 0,
        z: max_z,
    }
    .to_chunk_pos(job.coord.dimension);
    let x_chunk_count = i64::from(max_chunk.x) - i64::from(min_chunk.x) + 1;
    let z_chunk_count = i64::from(max_chunk.z) - i64::from(min_chunk.z) + 1;
    let capacity = usize::try_from(x_chunk_count.saturating_mul(z_chunk_count)).map_err(|_| {
        BedrockRenderError::Validation("tile chunk coverage is too large".to_string())
    })?;
    let mut positions = Vec::with_capacity(capacity);
    for z in min_chunk.z..=max_chunk.z {
        for x in min_chunk.x..=max_chunk.x {
            positions.push(ChunkPos {
                x,
                z,
                dimension: job.coord.dimension,
            });
        }
    }
    Ok(positions)
}

fn collect_region_plans(
    planned_tiles: &[PlannedTile],
    region_layout: RegionLayout,
) -> Result<Vec<RegionPlan>> {
    region_layout.validate()?;
    let mut region_indexes = BTreeMap::<RegionBakeKey, usize>::new();
    let mut regions = Vec::<RegionPlan>::new();
    for planned in planned_tiles {
        for pos in planned_chunk_positions(planned)?.iter().copied() {
            let key = region_key_for_chunk(pos, region_layout, planned.job.mode);
            if let Some(index) = region_indexes.get(&key).copied() {
                if let Some(plan) = regions.get_mut(index) {
                    plan.region.min_chunk_x = plan.region.min_chunk_x.min(pos.x);
                    plan.region.min_chunk_z = plan.region.min_chunk_z.min(pos.z);
                    plan.region.max_chunk_x = plan.region.max_chunk_x.max(pos.x);
                    plan.region.max_chunk_z = plan.region.max_chunk_z.max(pos.z);
                    plan.chunk_positions.push(pos);
                }
            } else {
                let index = regions.len();
                regions.push(RegionPlan {
                    key,
                    region: ChunkRegion::new(pos.dimension, pos.x, pos.z, pos.x, pos.z),
                    chunk_positions: vec![pos],
                });
                region_indexes.insert(key, index);
            }
        }
    }
    Ok(regions
        .into_iter()
        .map(|mut plan| {
            plan.chunk_positions.sort();
            plan.chunk_positions.dedup();
            plan
        })
        .collect())
}

fn region_bake_covers_plan(region: &RegionBake, plan: &RegionPlan) -> bool {
    region.coord == plan.key.coord
        && region.mode == plan.key.mode
        && chunk_region_contains(region.covered_chunk_region, plan.region)
}

fn region_bake_covers_full_region(region: &RegionBake, layout: RegionLayout) -> bool {
    chunk_region_contains(
        region.covered_chunk_region,
        region.coord.chunk_region(layout),
    )
}

fn chunk_region_contains(outer: ChunkRegion, inner: ChunkRegion) -> bool {
    outer.dimension == inner.dimension
        && outer.min_chunk_x <= inner.min_chunk_x
        && outer.min_chunk_z <= inner.min_chunk_z
        && outer.max_chunk_x >= inner.max_chunk_x
        && outer.max_chunk_z >= inner.max_chunk_z
}

fn planned_chunk_positions(planned: &PlannedTile) -> Result<Cow<'_, [ChunkPos]>> {
    if let Some(chunk_positions) = &planned.chunk_positions {
        return Ok(Cow::Borrowed(chunk_positions));
    }
    tile_chunk_positions(&planned.job).map(Cow::Owned)
}

fn prioritized_planned_tiles(
    planned_tiles: &[PlannedTile],
    priority: RenderTilePriority,
) -> Vec<PlannedTile> {
    let mut tiles = planned_tiles.to_vec();
    match priority {
        RenderTilePriority::RowMajor => {}
        RenderTilePriority::DistanceFrom { tile_x, tile_z } => {
            if should_parallel_sort_tile_priorities(tiles.len()) {
                tiles.par_sort_by_key(|planned| {
                    tile_distance_sort_key(planned.job.coord.x, planned.job.coord.z, tile_x, tile_z)
                });
            } else {
                tiles.sort_by_key(|planned| {
                    tile_distance_sort_key(planned.job.coord.x, planned.job.coord.z, tile_x, tile_z)
                });
            }
        }
    }
    tiles
}

fn should_parallel_sort_tile_priorities(tile_count: usize) -> bool {
    tile_count >= PARALLEL_TILE_PRIORITY_SORT_THRESHOLD
}

fn order_tile_cache_probe_results<T>(mut completed_probes: Vec<(usize, T)>) -> Vec<(usize, T)> {
    completed_probes.sort_by_key(|(ordinal, _)| *ordinal);
    completed_probes
}

fn tile_distance_sort_key(
    x: i32,
    z: i32,
    center_x: i32,
    center_z: i32,
) -> (i64, i64, i64, i32, i32) {
    let dx = i64::from(x) - i64::from(center_x);
    let dz = i64::from(z) - i64::from(center_z);
    let absolute_x = dx.abs();
    let absolute_z = dz.abs();
    (
        absolute_x.max(absolute_z),
        dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
        absolute_x.saturating_add(absolute_z),
        z,
        x,
    )
}

fn tile_region_plans(
    planned: &PlannedTile,
    region_layout: RegionLayout,
) -> Result<Vec<RegionPlan>> {
    let mut regions = BTreeMap::<RegionBakeKey, RegionPlan>::new();
    for pos in planned_chunk_positions(planned)?.iter().copied() {
        let key = region_key_for_chunk(pos, region_layout, planned.job.mode);
        regions
            .entry(key)
            .and_modify(|plan| {
                plan.region.min_chunk_x = plan.region.min_chunk_x.min(pos.x);
                plan.region.min_chunk_z = plan.region.min_chunk_z.min(pos.z);
                plan.region.max_chunk_x = plan.region.max_chunk_x.max(pos.x);
                plan.region.max_chunk_z = plan.region.max_chunk_z.max(pos.z);
                plan.chunk_positions.push(pos);
            })
            .or_insert_with(|| RegionPlan {
                key,
                region: ChunkRegion::new(pos.dimension, pos.x, pos.z, pos.x, pos.z),
                chunk_positions: vec![pos],
            });
    }
    Ok(regions
        .into_values()
        .map(|mut plan| {
            plan.chunk_positions.sort();
            plan.chunk_positions.dedup();
            plan
        })
        .collect())
}

fn select_region_wave(
    pending_tiles: &[usize],
    tile_region_plans: &[Vec<RegionPlan>],
    region_plan_by_key: &BTreeMap<RegionBakeKey, RegionPlan>,
    memory_budget: usize,
) -> Result<BTreeSet<RegionBakeKey>> {
    let Some(first_tile) = pending_tiles.first().copied() else {
        return Ok(BTreeSet::new());
    };
    let mut selected = BTreeSet::new();
    let mut selected_bytes = 0usize;

    for plan in tile_region_plans
        .get(first_tile)
        .ok_or_else(|| BedrockRenderError::Validation("missing tile region plans".to_string()))?
    {
        selected_bytes =
            selected_bytes.saturating_add(region_estimated_bytes(plan.key, region_plan_by_key)?);
        selected.insert(plan.key);
    }

    for tile_index in pending_tiles.iter().copied().skip(1) {
        let plans = tile_region_plans.get(tile_index).ok_or_else(|| {
            BedrockRenderError::Validation("missing tile region plans".to_string())
        })?;
        let missing_bytes = plans.iter().try_fold(0usize, |total, plan| {
            if selected.contains(&plan.key) {
                Ok::<usize, BedrockRenderError>(total)
            } else {
                Ok::<usize, BedrockRenderError>(
                    total.saturating_add(region_estimated_bytes(plan.key, region_plan_by_key)?),
                )
            }
        })?;
        if selected_bytes.saturating_add(missing_bytes) > memory_budget {
            continue;
        }
        for plan in plans {
            selected.insert(plan.key);
        }
        selected_bytes = selected_bytes.saturating_add(missing_bytes);
    }

    Ok(selected)
}

fn ready_web_tile_indexes(
    pending_tiles: &[usize],
    tile_region_plans: &[Vec<RegionPlan>],
    regions: &RegionBakeMap,
    rendered_tiles: &BTreeSet<usize>,
) -> Result<Vec<usize>> {
    let mut ready = Vec::new();
    for index in pending_tiles.iter().copied() {
        if rendered_tiles.contains(&index) {
            continue;
        }
        let plans = tile_region_plans.get(index).ok_or_else(|| {
            BedrockRenderError::Validation("missing tile region plans".to_string())
        })?;
        if plans.iter().all(|plan| {
            regions
                .get(&plan.key)
                .is_some_and(|region| region_bake_covers_plan(region, plan))
        }) {
            ready.push(index);
        }
    }
    Ok(ready)
}

fn region_estimated_bytes(
    key: RegionBakeKey,
    region_plan_by_key: &BTreeMap<RegionBakeKey, RegionPlan>,
) -> Result<usize> {
    let plan = region_plan_by_key.get(&key).ok_or_else(|| {
        BedrockRenderError::Validation("planned tile references missing region".to_string())
    })?;
    Ok(plan
        .chunk_positions
        .len()
        .saturating_mul(REGION_BAKE_ESTIMATED_BYTES_PER_CHUNK))
}

fn render_web_tile_indexes<S, F>(
    renderer: &MapRenderer<S>,
    planned_tiles: &[PlannedTile],
    tile_indexes: &[usize],
    options: &RenderOptions,
    regions: &RegionBakeMap,
    worker_count: usize,
    gpu: Option<&GpuRenderContext>,
    sink: &F,
) -> Result<TileComposeStats>
where
    S: WorldStorageHandle,
    F: Fn(PlannedTile, TileImage) -> Result<()> + Send + Sync,
{
    if tile_indexes.is_empty() {
        return Ok(TileComposeStats::default());
    }
    if worker_count == 1 {
        let mut stats = TileComposeStats::default();
        for index in tile_indexes {
            let planned = planned_tiles.get(*index).ok_or_else(|| {
                BedrockRenderError::Validation("planned tile index is out of range".to_string())
            })?;
            let (tile, tile_stats) = renderer.render_tile_from_cached_regions_with_gpu_stats(
                planned.job.clone(),
                options,
                regions,
                tile_indexes.len(),
                gpu,
            )?;
            stats.add(tile_stats);
            sink(planned.clone(), tile)?;
        }
        return Ok(stats);
    }

    let next_tile = Arc::new(AtomicUsize::new(0));
    let queue_capacity = options
        .pipeline_depth
        .max(worker_count.saturating_mul(2))
        .max(1);
    let (sender, receiver) =
        mpsc::sync_channel::<Result<(PlannedTile, TileImage, TileComposeStats)>>(queue_capacity);
    let pool = render_cpu_pool(worker_count)?;
    pool.scope(|scope| {
        for _ in 0..worker_count {
            let next_tile = Arc::clone(&next_tile);
            let sender = sender.clone();
            let renderer = renderer.clone();
            let options = options.clone();
            let gpu = gpu.cloned();
            scope.spawn(move |_| {
                loop {
                    if check_cancelled(&options).is_err() {
                        let send_result = sender.send(Err(BedrockRenderError::Cancelled));
                        drop(send_result);
                        return;
                    }
                    let index = next_tile.fetch_add(1, Ordering::Relaxed);
                    let Some(tile_index) = tile_indexes.get(index).copied() else {
                        return;
                    };
                    let Some(planned) = planned_tiles.get(tile_index).cloned() else {
                        let send_result = sender.send(Err(BedrockRenderError::Validation(
                            "planned tile index is out of range".to_string(),
                        )));
                        drop(send_result);
                        return;
                    };
                    let tile_result = renderer
                        .render_tile_from_cached_regions_with_gpu_stats(
                            planned.job.clone(),
                            &options,
                            regions,
                            tile_indexes.len(),
                            gpu.as_ref(),
                        )
                        .map(|(tile, stats)| (planned, tile, stats));
                    if sender.send(tile_result).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        let mut stats = TileComposeStats::default();
        for message in receiver {
            let (planned, tile, tile_stats) = message?;
            stats.add(tile_stats);
            sink(planned, tile)?;
        }
        Ok::<TileComposeStats, BedrockRenderError>(stats)
    })
}

fn process_tile_rgba_on_gpu(
    gpu: &GpuRenderContext,
    rgba: &[u8],
    options: &RenderOptions,
) -> Result<GpuProcessResult> {
    gpu.process_rgba(rgba, options.gpu.backend, options.gpu.fallback_policy)
}

fn should_process_tile_on_gpu(options: &RenderOptions, tile_size: u32, work_items: usize) -> bool {
    if !render_backend_uses_gpu(options.backend) {
        return false;
    }
    let tile_pixels = usize::try_from(tile_size)
        .ok()
        .and_then(|size| size.checked_mul(size))
        .unwrap_or(0);
    if tile_pixels == 0 {
        return false;
    }
    let effective_items = work_items.max(1);
    let batch_pixels = if options.gpu.batch_pixels == 0 {
        tile_pixels.saturating_mul(4)
    } else {
        options.gpu.batch_pixels
    };
    tile_pixels.saturating_mul(effective_items) >= batch_pixels
}

fn render_backend_uses_gpu(backend: RenderBackend) -> bool {
    matches!(backend, RenderBackend::Auto | RenderBackend::Wgpu)
}

fn collect_tile_chunk_bakes(
    dependencies: &[ChunkBakeKey],
    bakes: &BTreeMap<ChunkBakeKey, ChunkBake>,
) -> Result<(BTreeMap<ChunkPos, ChunkBake>, RenderDiagnostics)> {
    let mut tile_bakes = BTreeMap::new();
    let mut diagnostics = RenderDiagnostics::default();
    for key in dependencies {
        let bake = bakes.get(key).ok_or_else(|| {
            BedrockRenderError::Validation("tile dependency was not baked".to_string())
        })?;
        diagnostics.add(bake.diagnostics.clone());
        tile_bakes.insert(key.pos, bake.clone());
    }
    Ok((tile_bakes, diagnostics))
}

fn store_tile_compose_result(
    message: Result<TileComposeResult>,
    tiles: &mut [Option<TileImage>],
    completed_tiles: &mut usize,
    total_tiles: usize,
    options: &RenderOptions,
) -> Result<()> {
    let result = message?;
    let slot = tiles.get_mut(result.tile_index).ok_or_else(|| {
        BedrockRenderError::Validation("tile compose result index is out of range".to_string())
    })?;
    if slot.is_some() {
        return Err(BedrockRenderError::Validation(
            "duplicate tile compose result".to_string(),
        ));
    }
    *slot = Some(result.tile);
    *completed_tiles = completed_tiles.saturating_add(1);
    emit_progress(options, *completed_tiles, total_tiles);
    Ok(())
}

fn pipeline_queue_capacity(options: &RenderOptions, worker_count: usize) -> usize {
    options
        .cpu
        .resolve_queue_depth(worker_count, worker_count)
        .max(
            options
                .pipeline_depth
                .max(worker_count.saturating_mul(2))
                .max(1),
        )
}

fn render_cpu_pool(worker_count: usize) -> Result<rayon::ThreadPool> {
    ThreadPoolBuilder::new()
        .num_threads(worker_count.max(1).saturating_add(1))
        .thread_name(|index| format!("bedrock-render-cpu-{index}"))
        .build()
        .map_err(|error| {
            BedrockRenderError::Validation(format!("failed to build render CPU pool: {error}"))
        })
}

fn pipeline_loader_count(worker_count: usize, work_items: usize) -> usize {
    if work_items == 0 {
        return 0;
    }
    if worker_count < 3 {
        return 1.min(work_items);
    }
    (worker_count / 2)
        .clamp(1, worker_count.saturating_sub(2))
        .min(work_items)
}

fn pipeline_loader_count_for_options(
    options: &RenderOptions,
    worker_count: usize,
    work_items: usize,
) -> usize {
    let count = pipeline_loader_count(worker_count, work_items);
    if options.cpu.max_db_workers > 0 {
        count
            .min(options.cpu.max_db_workers)
            .max(usize::from(work_items > 0))
    } else {
        count
    }
}

fn pipeline_compose_count(worker_count: usize, loader_count: usize, work_items: usize) -> usize {
    if work_items == 0 || worker_count <= loader_count.saturating_add(1) {
        return 0;
    }
    worker_count
        .saturating_sub(loader_count)
        .saturating_div(2)
        .max(1)
        .min(worker_count.saturating_sub(loader_count).saturating_sub(1))
        .min(work_items)
}

fn pipeline_compose_count_for_options(
    options: &RenderOptions,
    worker_count: usize,
    loader_count: usize,
    work_items: usize,
) -> usize {
    let count = pipeline_compose_count(worker_count, loader_count, work_items);
    if options.cpu.max_compose_workers > 0 {
        count.min(options.cpu.max_compose_workers)
    } else {
        count
    }
}

fn pipeline_bake_count(
    worker_count: usize,
    loader_count: usize,
    compose_count: usize,
    work_items: usize,
) -> usize {
    if work_items == 0 {
        return 0;
    }
    worker_count
        .saturating_sub(loader_count)
        .saturating_sub(compose_count)
        .max(1)
        .min(work_items)
}

fn pipeline_bake_count_for_options(
    options: &RenderOptions,
    worker_count: usize,
    loader_count: usize,
    compose_count: usize,
    work_items: usize,
) -> usize {
    let count = pipeline_bake_count(worker_count, loader_count, compose_count, work_items);
    if options.cpu.max_bake_workers > 0 {
        count
            .min(options.cpu.max_bake_workers)
            .max(usize::from(work_items > 0))
    } else {
        count
    }
}

fn empty_region_payload(
    mode: RenderMode,
    surface_options: SurfaceRenderOptions,
    width: u32,
    height: u32,
    missing_color: RgbaColor,
) -> Result<RegionBakePayload> {
    Ok(match mode {
        RenderMode::SurfaceBlocks => {
            if surface_gbuffer_enabled(surface_options) {
                RegionBakePayload::SurfaceAtlas(SurfacePlaneAtlas {
                    colors: RgbaPlane::new(width, height, missing_color)?,
                    heights: HeightPlane::new(width, height)?,
                    relief_heights: HeightPlane::new(width, height)?,
                    water_depths: DepthPlane::new(width, height)?,
                    materials: DepthPlane::new(width, height)?,
                    shape_flags: DepthPlane::new(width, height)?,
                    overlay_alpha: DepthPlane::new(width, height)?,
                })
            } else {
                RegionBakePayload::Surface(SurfacePlane {
                    colors: RgbaPlane::new(width, height, missing_color)?,
                    heights: HeightPlane::new(width, height)?,
                    relief_heights: HeightPlane::new(width, height)?,
                    water_depths: DepthPlane::new(width, height)?,
                })
            }
        }
        RenderMode::HeightMap | RenderMode::RawHeightMap => RegionBakePayload::HeightMap {
            colors: RgbaPlane::new(width, height, missing_color)?,
            heights: HeightPlane::new(width, height)?,
        },
        RenderMode::Biome { .. }
        | RenderMode::RawBiomeLayer { .. }
        | RenderMode::LayerBlocks { .. }
        | RenderMode::CaveSlice { .. } => {
            RegionBakePayload::Colors(RgbaPlane::new(width, height, missing_color)?)
        }
    })
}

fn copy_chunk_bake_to_region_checked(
    bake: &ChunkBake,
    payload: &mut RegionBakePayload,
    region: ChunkRegion,
) -> bool {
    let Some((region_x, region_z)) = chunk_region_pixel_offset(region, bake.pos) else {
        log::warn!(
            "skipping baked chunk outside region payload (chunk=({}, {:?}, {}), region=({}, {:?}, {})..({}, {:?}, {}))",
            bake.pos.x,
            bake.pos.dimension,
            bake.pos.z,
            region.min_chunk_x,
            region.dimension,
            region.min_chunk_z,
            region.max_chunk_x,
            region.dimension,
            region.max_chunk_z
        );
        return false;
    };
    copy_chunk_bake_to_region(bake, payload, region_x, region_z);
    true
}

fn copy_chunk_bake_to_region(
    bake: &ChunkBake,
    payload: &mut RegionBakePayload,
    region_x: u32,
    region_z: u32,
) {
    if let ChunkBakePayload::Colors(source) = &bake.payload {
        match payload {
            RegionBakePayload::Colors(target) => {
                if target.copy_16x16_from(region_x, region_z, source) {
                    return;
                }
            }
            RegionBakePayload::HeightMap { colors, .. } => {
                if colors.copy_16x16_from(region_x, region_z, source) {
                    return;
                }
            }
            RegionBakePayload::Surface(_) | RegionBakePayload::SurfaceAtlas(_) => {}
        }
    }
    for local_z in 0..16_u32 {
        for local_x in 0..16_u32 {
            let color = chunk_bake_color(bake, local_x, local_z);
            let height = chunk_bake_height(bake, local_x, local_z);
            let dst_x = region_x + local_x;
            let dst_z = region_z + local_z;
            match payload {
                RegionBakePayload::Colors(colors) => {
                    if let Some(color) = color {
                        colors.set_color(dst_x, dst_z, color);
                    }
                }
                RegionBakePayload::Surface(surface) => {
                    if let Some(color) = color {
                        surface.colors.set_color(dst_x, dst_z, color);
                    }
                    if let Some(height) = height {
                        surface.heights.set_height(dst_x, dst_z, height);
                    }
                    if let Some(relief_height) = chunk_bake_relief_height(bake, local_x, local_z) {
                        surface
                            .relief_heights
                            .set_height(dst_x, dst_z, relief_height);
                    }
                    let depth = chunk_bake_water_depth(bake, local_x, local_z);
                    if depth > 0 {
                        surface.water_depths.set_depth(dst_x, dst_z, depth);
                    }
                }
                RegionBakePayload::SurfaceAtlas(surface) => {
                    if let Some(color) = color {
                        surface.colors.set_color(dst_x, dst_z, color);
                    }
                    if let Some(height) = height {
                        surface.heights.set_height(dst_x, dst_z, height);
                    }
                    if let Some(relief_height) = chunk_bake_relief_height(bake, local_x, local_z) {
                        surface
                            .relief_heights
                            .set_height(dst_x, dst_z, relief_height);
                    }
                    let depth = chunk_bake_water_depth(bake, local_x, local_z);
                    if depth > 0 {
                        surface.water_depths.set_depth(dst_x, dst_z, depth);
                    }
                    surface.materials.set_depth(
                        dst_x,
                        dst_z,
                        chunk_bake_atlas_material(bake, local_x, local_z).as_u8(),
                    );
                    surface.shape_flags.set_depth(
                        dst_x,
                        dst_z,
                        chunk_bake_atlas_shape_flags(bake, local_x, local_z),
                    );
                    surface.overlay_alpha.set_depth(
                        dst_x,
                        dst_z,
                        chunk_bake_atlas_overlay_alpha(bake, local_x, local_z),
                    );
                }
                RegionBakePayload::HeightMap { colors, heights } => {
                    if let Some(color) = color {
                        colors.set_color(dst_x, dst_z, color);
                    }
                    if let Some(height) = height {
                        heights.set_height(dst_x, dst_z, height);
                    }
                }
            }
        }
    }
}

fn chunk_bake_color(bake: &ChunkBake, local_x: u32, local_z: u32) -> Option<RgbaColor> {
    match &bake.payload {
        ChunkBakePayload::Colors(colors) | ChunkBakePayload::HeightMap { colors, .. } => {
            colors.color_at(local_x, local_z)
        }
        ChunkBakePayload::Surface(surface) => surface.colors.color_at(local_x, local_z),
        ChunkBakePayload::SurfaceAtlas(surface) => surface.colors.color_at(local_x, local_z),
    }
}

fn chunk_bake_height(bake: &ChunkBake, local_x: u32, local_z: u32) -> Option<i16> {
    match &bake.payload {
        ChunkBakePayload::Surface(surface) => surface.heights.height_at(local_x, local_z),
        ChunkBakePayload::SurfaceAtlas(surface) => surface.heights.height_at(local_x, local_z),
        ChunkBakePayload::HeightMap { heights, .. } => heights.height_at(local_x, local_z),
        ChunkBakePayload::Colors(_) => None,
    }
}

fn chunk_bake_relief_height(bake: &ChunkBake, local_x: u32, local_z: u32) -> Option<i16> {
    match &bake.payload {
        ChunkBakePayload::Surface(surface) => surface.relief_heights.height_at(local_x, local_z),
        ChunkBakePayload::SurfaceAtlas(surface) => {
            surface.relief_heights.height_at(local_x, local_z)
        }
        ChunkBakePayload::HeightMap { heights, .. } => heights.height_at(local_x, local_z),
        ChunkBakePayload::Colors(_) => None,
    }
}

fn chunk_bake_water_depth(bake: &ChunkBake, local_x: u32, local_z: u32) -> u8 {
    match &bake.payload {
        ChunkBakePayload::Surface(surface) => surface.water_depths.depth_at(local_x, local_z),
        ChunkBakePayload::SurfaceAtlas(surface) => surface.water_depths.depth_at(local_x, local_z),
        ChunkBakePayload::Colors(_) | ChunkBakePayload::HeightMap { .. } => 0,
    }
}

fn chunk_bake_atlas_material(bake: &ChunkBake, local_x: u32, local_z: u32) -> SurfaceMaterialId {
    match &bake.payload {
        ChunkBakePayload::SurfaceAtlas(surface) => {
            SurfaceMaterialId::from_u8(surface.materials.depth_at(local_x, local_z))
        }
        _ => SurfaceMaterialId::Unknown,
    }
}

fn chunk_bake_atlas_shape_flags(bake: &ChunkBake, local_x: u32, local_z: u32) -> u8 {
    match &bake.payload {
        ChunkBakePayload::SurfaceAtlas(surface) => surface.shape_flags.depth_at(local_x, local_z),
        _ => 0,
    }
}

fn chunk_bake_atlas_overlay_alpha(bake: &ChunkBake, local_x: u32, local_z: u32) -> u8 {
    match &bake.payload {
        ChunkBakePayload::SurfaceAtlas(surface) => surface.overlay_alpha.depth_at(local_x, local_z),
        _ => 0,
    }
}

fn chunk_bake_atlas_aux(bake: &ChunkBake, local_x: u32, local_z: u32) -> u32 {
    pack_atlas_aux(
        chunk_bake_water_depth(bake, local_x, local_z),
        chunk_bake_atlas_material(bake, local_x, local_z),
        chunk_bake_atlas_shape_flags(bake, local_x, local_z),
        chunk_bake_atlas_overlay_alpha(bake, local_x, local_z),
    )
}

const ATLAS_SHAPE_FOLIAGE: u8 = 1;
const ATLAS_SHAPE_THIN: u8 = 1 << 1;
const ATLAS_SHAPE_SOLID: u8 = 1 << 2;
const ATLAS_SHAPE_WATER: u8 = 1 << 3;
const ATLAS_SHAPE_OVERLAY: u8 = 1 << 4;
const MAP_ATLAS_CAST_SHADOW_RADIUS_BLOCKS: u32 = 6;

fn pack_atlas_aux(
    water_depth: u8,
    material: SurfaceMaterialId,
    shape_flags: u8,
    overlay_alpha: u8,
) -> u32 {
    u32::from(water_depth)
        | (u32::from(material.as_u8()) << 8)
        | (u32::from(shape_flags) << 16)
        | (u32::from(overlay_alpha) << 24)
}

fn atlas_aux_water_depth(aux: u32) -> u8 {
    u8_from_u32(aux & 0xff)
}

fn atlas_aux_material(aux: u32) -> SurfaceMaterialId {
    SurfaceMaterialId::from_u8(u8_from_u32((aux >> 8) & 0xff))
}

fn atlas_aux_shape_flags(aux: u32) -> u8 {
    u8_from_u32((aux >> 16) & 0xff)
}

fn classify_surface_material(name: &str) -> SurfaceMaterialId {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if name.contains("leaves") || name.contains("azalea") || name.contains("mangrove_roots") {
        SurfaceMaterialId::Foliage
    } else if name.contains("grass") || name.contains("moss") || name.contains("vine") {
        SurfaceMaterialId::Grass
    } else if name.contains("snow") || name.contains("ice") || name.contains("powder_snow") {
        SurfaceMaterialId::Snow
    } else if name.contains("stone")
        || name.contains("deepslate")
        || name.contains("ore")
        || name.contains("basalt")
        || name.contains("tuff")
        || name.contains("cobble")
    {
        SurfaceMaterialId::Stone
    } else if name.contains("dirt") || name.contains("mud") || name.contains("podzol") {
        SurfaceMaterialId::Dirt
    } else if name.contains("sand") || name.contains("gravel") {
        SurfaceMaterialId::Sand
    } else if name.contains("log")
        || name.contains("wood")
        || name.contains("planks")
        || name.contains("stem")
        || name.contains("hyphae")
    {
        SurfaceMaterialId::Wood
    } else if name.contains("water") {
        SurfaceMaterialId::Water
    } else if name.contains("lava") {
        SurfaceMaterialId::Lava
    } else if name.contains("iron")
        || name.contains("gold")
        || name.contains("copper")
        || name.contains("chain")
    {
        SurfaceMaterialId::Metal
    } else if name.contains("flower")
        || name.contains("sapling")
        || name.contains("bamboo")
        || name.contains("fern")
        || name.contains("crop")
    {
        SurfaceMaterialId::Plant
    } else {
        SurfaceMaterialId::Built
    }
}

fn atlas_shape_flags_from_material(
    material: SurfaceMaterialId,
    has_overlay: bool,
    is_water: bool,
) -> u8 {
    let mut flags = if is_water {
        ATLAS_SHAPE_WATER
    } else {
        ATLAS_SHAPE_SOLID
    };
    if matches!(material, SurfaceMaterialId::Foliage) {
        flags |= ATLAS_SHAPE_FOLIAGE;
    }
    if matches!(
        material,
        SurfaceMaterialId::Plant | SurfaceMaterialId::Grass
    ) {
        flags |= ATLAS_SHAPE_THIN;
    }
    if has_overlay {
        flags |= ATLAS_SHAPE_OVERLAY;
    }
    flags
}

fn state_block_color(palette: &RenderPalette, state: &BlockState) -> RgbaColor {
    palette.block_state_color(canonical_material_state_for_color(state).as_ref())
}

fn legacy_block_state(id: u8, data: u8) -> BlockState {
    let mut states = BTreeMap::new();
    states.insert("data".to_string(), NbtTag::Byte(data as i8));
    BlockState {
        name: legacy_block_name(id, data).into_owned(),
        states,
        version: None,
    }
}

fn legacy_block_name(id: u8, data: u8) -> Cow<'static, str> {
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
        5 => legacy_wood_name(data, "planks"),
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
        17 => legacy_wood_name(data, "log"),
        18 => legacy_wood_name(data, "leaves"),
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
        35 => legacy_wool_name(data),
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
        159 => "minecraft:terracotta",
        161 => legacy_wood_name(data.saturating_add(4), "leaves"),
        162 => legacy_wood_name(data.saturating_add(4), "log"),
        169 => "minecraft:sea_lantern",
        170 => "minecraft:hay_block",
        171 => "minecraft:white_carpet",
        172 => "minecraft:terracotta",
        173 => "minecraft:coal_block",
        174 => "minecraft:packed_ice",
        175 => "minecraft:sunflower",
        _ => return Cow::Owned(format!("legacy:{id}")),
    };
    Cow::Borrowed(name)
}

fn legacy_wood_name(data: u8, suffix: &'static str) -> &'static str {
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

fn legacy_wool_name(data: u8) -> &'static str {
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

const fn legacy_biome_sample_to_rgba(sample: LegacyBiomeSample) -> RgbaColor {
    RgbaColor::new(sample.red, sample.green, sample.blue, 255)
}

const fn legacy_biome_sample_to_render_sample(sample: LegacyBiomeSample) -> BiomeSample {
    BiomeSample::Legacy {
        id: sample.biome_id,
        color: legacy_biome_sample_to_rgba(sample),
    }
}

const fn terrain_column_biome_to_render_sample(sample: TerrainColumnBiome) -> BiomeSample {
    match sample {
        TerrainColumnBiome::Id(id) => BiomeSample::Id(id),
        TerrainColumnBiome::Legacy(sample) => legacy_biome_sample_to_render_sample(sample),
    }
}

fn normalize_legacy_block_state(state: &BlockState) -> Cow<'_, BlockState> {
    let Some(id_text) = state.name.strip_prefix("legacy:") else {
        return Cow::Borrowed(state);
    };
    let Ok(id) = id_text.parse::<u8>() else {
        return Cow::Borrowed(state);
    };
    let data = state
        .states
        .get("data")
        .and_then(|tag| match tag {
            NbtTag::Byte(value) => Some(*value as u8),
            NbtTag::Short(value) => u8::try_from(*value).ok(),
            NbtTag::Int(value) => u8::try_from(*value).ok(),
            _ => None,
        })
        .unwrap_or(0);
    Cow::Owned(legacy_block_state(id, data))
}

fn special_surface_block_color(
    palette: &RenderPalette,
    state: &BlockState,
    block_entity: Option<&ChunkBlockEntity>,
    biome: Option<BiomeSample>,
    biome_tint: bool,
) -> RgbaColor {
    let base = state_surface_block_color_with_biome(palette, state, biome, biome_tint);
    let short_name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    match short_name {
        "pistonArmCollision" | "piston_arm_collision" => palette
            .block_variant_color(&state.name, "piston_arm")
            .map(|color| with_opaque_alpha(color))
            .unwrap_or(base),
        "stickyPistonArmCollision" | "sticky_piston_arm_collision" => palette
            .block_variant_color(&state.name, "sticky_piston_arm")
            .or_else(|| palette.block_variant_color("minecraft:pistonArmCollision", "piston_arm"))
            .map(|color| with_opaque_alpha(color))
            .unwrap_or(base),
        "movingBlock" | "moving_block" => block_entity
            .and_then(moving_block_state_from_entity)
            .map(|moving_state| {
                state_surface_block_color_with_biome(palette, &moving_state, biome, biome_tint)
            })
            .or_else(|| {
                palette
                    .block_variant_color("minecraft:pistonArmCollision", "piston_arm")
                    .map(with_opaque_alpha)
            })
            .unwrap_or(base),
        "standing_banner" | "wall_banner" => {
            banner_surface_color(palette, &state.name, base, block_entity)
        }
        "bed" => bed_surface_color(palette, &state.name, base, block_entity),
        "decorated_pot" => decorated_pot_surface_color(palette, &state.name, base, block_entity),
        _ => base,
    }
}

fn state_surface_block_color_with_biome(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
    biome_tint: bool,
) -> RgbaColor {
    let state = canonical_material_state_for_color(state);
    palette.surface_block_state_color_with_legacy_biome(
        state.as_ref(),
        biome.and_then(BiomeSample::biome_id),
        biome.and_then(BiomeSample::legacy_color),
        biome_tint,
    )
}

fn canonical_material_state_for_color(state: &BlockState) -> Cow<'_, BlockState> {
    let Some(name) = canonical_slab_material_name(state) else {
        return Cow::Borrowed(state);
    };
    let mut canonical = state.clone();
    canonical.name = name.to_string();
    Cow::Owned(canonical)
}

fn canonical_slab_material_name(state: &BlockState) -> Option<&'static str> {
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if let Some(material) = canonical_dedicated_slab_material_name(name) {
        return Some(material);
    }
    let (state_key, value_map): (&str, &[(&str, &str)]) = match name {
        "stone_slab" => (
            "stone_slab_type",
            &[
                ("smooth_stone", "minecraft:smooth_stone"),
                ("sandstone", "minecraft:sandstone"),
                ("wood", "minecraft:oak_planks"),
                ("cobblestone", "minecraft:cobblestone"),
                ("brick", "minecraft:bricks"),
                ("stone_brick", "minecraft:stone_bricks"),
                ("quartz", "minecraft:quartz_block"),
                ("nether_brick", "minecraft:nether_bricks"),
            ],
        ),
        "stone_slab2" => (
            "stone_slab_type_2",
            &[
                ("red_sandstone", "minecraft:red_sandstone"),
                ("purpur", "minecraft:purpur_block"),
                ("prismarine_rough", "minecraft:prismarine"),
                ("prismarine_dark", "minecraft:dark_prismarine"),
                ("prismarine_brick", "minecraft:prismarine_bricks"),
                ("mossy_cobblestone", "minecraft:mossy_cobblestone"),
                ("smooth_sandstone", "minecraft:smooth_sandstone"),
                ("red_nether_brick", "minecraft:red_nether_brick"),
            ],
        ),
        "stone_slab3" => (
            "stone_slab_type_3",
            &[
                ("end_stone_brick", "minecraft:end_stone_bricks"),
                ("smooth_red_sandstone", "minecraft:smooth_red_sandstone"),
                ("polished_andesite", "minecraft:polished_andesite"),
                ("andesite", "minecraft:andesite"),
                ("diorite", "minecraft:diorite"),
                ("polished_diorite", "minecraft:polished_diorite"),
                ("granite", "minecraft:granite"),
                ("polished_granite", "minecraft:polished_granite"),
            ],
        ),
        "stone_slab4" => (
            "stone_slab_type_4",
            &[
                ("mossy_stone_brick", "minecraft:mossy_stone_bricks"),
                ("smooth_quartz", "minecraft:smooth_quartz"),
                ("stone", "minecraft:stone"),
                ("cut_sandstone", "minecraft:cut_sandstone"),
                ("cut_red_sandstone", "minecraft:cut_red_sandstone"),
            ],
        ),
        _ => return None,
    };
    let value = block_state_string(state, state_key)?;
    value_map
        .iter()
        .find_map(|(candidate, canonical)| (*candidate == value).then_some(*canonical))
}

fn canonical_dedicated_slab_material_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "andesite_slab" => "minecraft:andesite",
        "blackstone_slab" => "minecraft:blackstone",
        "brick_slab" => "minecraft:bricks",
        "cobbled_deepslate_slab" => "minecraft:cobbled_deepslate",
        "cobblestone_slab" => "minecraft:cobblestone",
        "cut_sandstone_slab" => "minecraft:cut_sandstone",
        "cut_red_sandstone_slab" => "minecraft:cut_red_sandstone",
        "dark_prismarine_slab" => "minecraft:dark_prismarine",
        "deepslate_brick_slab" => "minecraft:deepslate_bricks",
        "deepslate_tile_slab" => "minecraft:deepslate_tiles",
        "diorite_slab" => "minecraft:diorite",
        "end_stone_brick_slab" => "minecraft:end_stone_bricks",
        "granite_slab" => "minecraft:granite",
        "mossy_cobblestone_slab" => "minecraft:mossy_cobblestone",
        "mossy_stone_brick_slab" => "minecraft:mossy_stone_bricks",
        "mud_brick_slab" => "minecraft:mud_bricks",
        "nether_brick_slab" => "minecraft:nether_bricks",
        "normal_stone_slab" => "minecraft:stone",
        "polished_andesite_slab" => "minecraft:polished_andesite",
        "polished_blackstone_slab" => "minecraft:polished_blackstone",
        "polished_blackstone_brick_slab" => "minecraft:polished_blackstone_bricks",
        "polished_deepslate_slab" => "minecraft:polished_deepslate",
        "polished_diorite_slab" => "minecraft:polished_diorite",
        "polished_granite_slab" => "minecraft:polished_granite",
        "prismarine_slab" => "minecraft:prismarine",
        "prismarine_brick_slab" => "minecraft:prismarine_bricks",
        "purpur_slab" => "minecraft:purpur_block",
        "quartz_slab" => "minecraft:quartz_block",
        "red_nether_brick_slab" => "minecraft:red_nether_bricks",
        "red_sandstone_slab" => "minecraft:red_sandstone",
        "sandstone_slab" => "minecraft:sandstone",
        "smooth_quartz_slab" => "minecraft:smooth_quartz",
        "smooth_red_sandstone_slab" => "minecraft:smooth_red_sandstone",
        "smooth_sandstone_slab" => "minecraft:smooth_sandstone",
        "smooth_stone_slab" => "minecraft:smooth_stone",
        "stone_brick_slab" => "minecraft:stone_bricks",
        "tuff_slab" => "minecraft:tuff",
        "tuff_brick_slab" => "minecraft:tuff_bricks",
        "polished_tuff_slab" => "minecraft:polished_tuff",
        _ => return None,
    })
}

fn block_state_string<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    let tag = state
        .states
        .get(key)
        .or_else(|| state.states.get(&format!("minecraft:{key}")))?;
    match tag {
        NbtTag::String(value) => Some(value),
        _ => None,
    }
}

fn banner_surface_color(
    palette: &RenderPalette,
    block_name: &str,
    base: RgbaColor,
    block_entity: Option<&ChunkBlockEntity>,
) -> RgbaColor {
    let mut color = block_entity
        .and_then(banner_base_variant)
        .and_then(|variant| palette.block_variant_color(block_name, &variant))
        .or_else(|| palette.block_variant_color(block_name, "banner_base_white"))
        .map(with_opaque_alpha)
        .unwrap_or(base);
    if let Some(block_entity) = block_entity {
        for variant in banner_pattern_variants(block_entity).into_iter().take(8) {
            if let Some(pattern_color) = palette.block_variant_color(block_name, &variant) {
                color = alpha_blend_surface(with_opaque_alpha(pattern_color), color, 54);
            }
        }
    }
    color
}

fn decorated_pot_surface_color(
    palette: &RenderPalette,
    block_name: &str,
    base: RgbaColor,
    block_entity: Option<&ChunkBlockEntity>,
) -> RgbaColor {
    let mut color = palette
        .block_variant_color(block_name, "decorated_pot_base")
        .map(with_opaque_alpha)
        .unwrap_or(base);
    if let Some(block_entity) = block_entity {
        for variant in decorated_pot_sherd_variants(block_entity)
            .into_iter()
            .take(4)
        {
            if let Some(sherd_color) = palette.block_variant_color(block_name, &variant) {
                color = alpha_blend_surface(with_opaque_alpha(sherd_color), color, 70);
            }
        }
    }
    color
}

fn bed_surface_color(
    palette: &RenderPalette,
    block_name: &str,
    base: RgbaColor,
    block_entity: Option<&ChunkBlockEntity>,
) -> RgbaColor {
    block_entity
        .and_then(bed_variant)
        .and_then(|variant| palette.block_variant_color(block_name, &variant))
        .or_else(|| palette.block_variant_color(block_name, "bed_red"))
        .or_else(|| palette.block_variant_color(block_name, "bed_white"))
        .map(with_opaque_alpha)
        .unwrap_or(base)
}

fn with_opaque_alpha(color: RgbaColor) -> RgbaColor {
    RgbaColor::new(color.red, color.green, color.blue, 255)
}

fn moving_block_state_from_entity(block_entity: &ChunkBlockEntity) -> Option<BlockState> {
    find_block_state_in_nbt(&block_entity.nbt, 0)
}

fn find_block_state_in_nbt(tag: &NbtTag, depth: usize) -> Option<BlockState> {
    if depth > 6 {
        return None;
    }
    let root = nbt_compound(tag)?;
    for key in [
        "movingBlock",
        "moving_block",
        "blockState",
        "BlockState",
        "block_state",
        "Block",
        "block",
    ] {
        if let Some(candidate) = root.get(key).and_then(|tag| block_state_from_nbt(tag)) {
            return Some(candidate);
        }
    }
    if let Some(candidate) = block_state_from_nbt(tag) {
        return Some(candidate);
    }
    for value in root.values() {
        if let Some(candidate) = find_block_state_in_nbt(value, depth + 1) {
            return Some(candidate);
        }
    }
    None
}

fn block_state_from_nbt(tag: &NbtTag) -> Option<BlockState> {
    let root = nbt_compound(tag)?;
    let name = nbt_string_field(root, "name")
        .or_else(|| nbt_string_field(root, "Name"))
        .or_else(|| nbt_string_field(root, "identifier"))
        .or_else(|| nbt_string_field(root, "Identifier"))?;
    let states = root
        .get("states")
        .or_else(|| root.get("States"))
        .and_then(nbt_compound)
        .map(|states| {
            states
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    let version = nbt_int_field(root, "version").or_else(|| nbt_int_field(root, "Version"));
    Some(BlockState {
        name: normalize_nbt_block_name(name),
        states,
        version,
    })
}

fn banner_base_variant(block_entity: &ChunkBlockEntity) -> Option<String> {
    let root = nbt_compound(&block_entity.nbt)?;
    nbt_string_field(root, "Base")
        .or_else(|| nbt_string_field(root, "base"))
        .or_else(|| nbt_string_field(root, "BaseColor"))
        .or_else(|| nbt_string_field(root, "Color"))
        .or_else(|| nbt_string_field(root, "color"))
        .map(banner_variant_from_color_name)
        .or_else(|| {
            nbt_int_field(root, "Base")
                .or_else(|| nbt_int_field(root, "base"))
                .or_else(|| nbt_int_field(root, "BaseColor"))
                .or_else(|| nbt_int_field(root, "Color"))
                .or_else(|| nbt_int_field(root, "color"))
                .and_then(banner_variant_from_color_id)
        })
}

fn bed_variant(block_entity: &ChunkBlockEntity) -> Option<String> {
    let root = nbt_compound(&block_entity.nbt)?;
    nbt_string_field(root, "Color")
        .or_else(|| nbt_string_field(root, "color"))
        .or_else(|| nbt_string_field(root, "BedColor"))
        .or_else(|| nbt_string_field(root, "bed_color"))
        .map(bed_variant_from_color_name)
        .or_else(|| {
            nbt_int_field(root, "Color")
                .or_else(|| nbt_int_field(root, "color"))
                .or_else(|| nbt_int_field(root, "BedColor"))
                .or_else(|| nbt_int_field(root, "bed_color"))
                .and_then(bed_variant_from_color_id)
        })
}

fn banner_pattern_variants(block_entity: &ChunkBlockEntity) -> Vec<String> {
    let Some(root) = nbt_compound(&block_entity.nbt) else {
        return Vec::new();
    };
    let Some(NbtTag::List(patterns)) = root.get("Patterns").or_else(|| root.get("patterns")) else {
        return Vec::new();
    };
    patterns
        .iter()
        .filter_map(|pattern| {
            let pattern = nbt_compound(pattern)?;
            nbt_int_field(pattern, "Color")
                .or_else(|| nbt_int_field(pattern, "color"))
                .and_then(banner_variant_from_color_id)
                .or_else(|| {
                    nbt_string_field(pattern, "Color")
                        .or_else(|| nbt_string_field(pattern, "color"))
                        .map(banner_variant_from_color_name)
                })
        })
        .collect()
}

fn decorated_pot_sherd_variants(block_entity: &ChunkBlockEntity) -> Vec<String> {
    let Some(root) = nbt_compound(&block_entity.nbt) else {
        return Vec::new();
    };
    let Some(NbtTag::List(sherds)) = root
        .get("sherds")
        .or_else(|| root.get("Sherds"))
        .or_else(|| root.get("shards"))
        .or_else(|| root.get("Shards"))
    else {
        return Vec::new();
    };
    sherds
        .iter()
        .filter_map(|sherd| match sherd {
            NbtTag::String(value) => Some(decorated_pot_variant_from_sherd(value)),
            NbtTag::Compound(root) => nbt_string_field(root, "Name")
                .or_else(|| nbt_string_field(root, "name"))
                .map(decorated_pot_variant_from_sherd),
            _ => None,
        })
        .collect()
}

fn banner_variant_from_color_id(id: i32) -> Option<String> {
    let color = match id {
        0 => "white",
        1 => "orange",
        2 => "magenta",
        3 => "light_blue",
        4 => "yellow",
        5 => "lime",
        6 => "pink",
        7 => "gray",
        8 => "light_gray",
        9 => "cyan",
        10 => "purple",
        11 => "blue",
        12 => "brown",
        13 => "green",
        14 => "red",
        15 => "black",
        _ => return None,
    };
    Some(format!("banner_base_{color}"))
}

fn bed_variant_from_color_id(id: i32) -> Option<String> {
    dye_color_name_from_id(id).map(|color| format!("bed_{color}"))
}

fn banner_variant_from_color_name(name: &str) -> String {
    format!("banner_base_{}", normalize_variant_token(name))
}

fn bed_variant_from_color_name(name: &str) -> String {
    format!("bed_{}", normalize_variant_token(name))
}

fn dye_color_name_from_id(id: i32) -> Option<&'static str> {
    Some(match id {
        0 => "white",
        1 => "orange",
        2 => "magenta",
        3 => "light_blue",
        4 => "yellow",
        5 => "lime",
        6 => "pink",
        7 => "gray",
        8 => "light_gray",
        9 => "cyan",
        10 => "purple",
        11 => "blue",
        12 => "brown",
        13 => "green",
        14 => "red",
        15 => "black",
        _ => return None,
    })
}

fn decorated_pot_variant_from_sherd(name: &str) -> String {
    let token = normalize_variant_token(name)
        .trim_end_matches("_pottery_sherd")
        .trim_end_matches("_sherd")
        .to_string();
    format!("decorated_pot_{token}")
}

fn normalize_variant_token(name: &str) -> String {
    name.strip_prefix("minecraft:")
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', ':'], "_")
}

fn normalize_nbt_block_name(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

fn nbt_compound(tag: &NbtTag) -> Option<&indexmap::IndexMap<String, NbtTag>> {
    match tag {
        NbtTag::Compound(root) => Some(root),
        _ => None,
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

fn surface_overlay_alpha(name: &str) -> Option<u8> {
    terrain_surface_overlay_alpha(name)
}

fn blend_surface_overlay(base: RgbaColor, overlay: Option<SurfaceOverlay>) -> RgbaColor {
    let Some(overlay) = overlay else {
        return base;
    };
    alpha_blend_surface(overlay.color, base, overlay.alpha)
}

fn alpha_blend_surface(foreground: RgbaColor, background: RgbaColor, alpha: u8) -> RgbaColor {
    let alpha = u16::from(alpha);
    let inverse = 255_u16.saturating_sub(alpha);
    RgbaColor::new(
        blend_surface_channel(foreground.red, background.red, alpha, inverse),
        blend_surface_channel(foreground.green, background.green, alpha, inverse),
        blend_surface_channel(foreground.blue, background.blue, alpha, inverse),
        255,
    )
}

fn blend_surface_channel(foreground: u8, background: u8, alpha: u16, inverse: u16) -> u8 {
    let value = ((u16::from(foreground) * alpha) + (u16::from(background) * inverse)) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn block_y_to_subchunk_y(y: i32) -> Result<i8> {
    i8::try_from(y.div_euclid(16)).map_err(|_| {
        BedrockRenderError::Validation(format!(
            "block y={y} cannot be represented as a Bedrock subchunk index"
        ))
    })
}

#[allow(dead_code)]
fn block_name_at(subchunk: &SubChunk, block_pos: BlockPos) -> Option<&str> {
    block_state_at(subchunk, block_pos).map(|state| state.name.as_str())
}

fn block_state_at(subchunk: &SubChunk, block_pos: BlockPos) -> Option<&BlockState> {
    let (local_x, y, local_z) = block_pos.in_chunk_offset();
    let local_y = u8::try_from(y - i32::from(subchunk.y) * 16).ok()?;
    if local_y >= 16 {
        return None;
    }
    subchunk.block_state_at(local_x, local_y, local_z)
}

fn biome_storage_bucket_y(y: i32) -> i32 {
    y.div_euclid(16) * 16
}

fn local_biome_y(storage: &bedrock_world::ParsedBiomeStorage, y: i32) -> Result<u8> {
    let local_y = if let Some(start_y) = storage.y {
        y - start_y
    } else {
        0
    };
    u8::try_from(local_y).map_err(|_| {
        BedrockRenderError::Validation(format!("biome y={y} is outside biome storage bounds"))
    })
}

fn non_empty_biome_id(id: Option<u32>) -> Option<u32> {
    id.filter(|id| *id != u32::MAX)
}

fn encode_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    format: ImageFormat,
) -> Result<Option<Vec<u8>>> {
    match format {
        ImageFormat::Rgba => Ok(None),
        ImageFormat::WebP => encode_webp(rgba, width, height).map(Some),
        ImageFormat::Png => encode_png(rgba, width, height).map(Some),
        ImageFormat::FastRgbaZstd => encode_fast_rgba_zstd(rgba, width, height).map(Some),
    }
}

fn rgba_to_bgra(mut rgba: Vec<u8>) -> Vec<u8> {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    rgba
}

fn decoded_tile_from_tile_image(
    tile: TileImage,
    output: RenderTileOutputOptions,
) -> DecodedTileImage {
    let pixels = match output.pixel_format {
        TilePixelFormat::Rgba8 => tile.rgba,
        TilePixelFormat::Bgra8 => Arc::from(rgba_to_bgra(tile.rgba.as_ref().to_vec())),
    };
    DecodedTileImage {
        coord: tile.coord,
        width: tile.width,
        height: tile.height,
        pixels,
        pixel_format: output.pixel_format,
    }
}

/// Encodes raw RGBA tile bytes into the fast zstd-backed tile cache format.
///
/// # Errors
///
/// Returns an error if the RGBA buffer length does not match `width * height * 4`, or if zstd
/// compression fails.
pub fn encode_fast_rgba_zstd(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    encode_fast_rgba_zstd_inner(
        rgba,
        width,
        height,
        FAST_RGBA_ZSTD_VALIDATION_KIND_NONE,
        fast_rgba_content_flags(rgba),
        0,
    )
}

/// Encodes RGBA tile bytes as fast zstd-backed cache bytes with a tile validation value.
///
/// # Errors
///
/// Returns an error if the RGBA buffer length does not match `width * height * 4`, or if zstd
/// compression fails.
pub fn encode_fast_rgba_zstd_with_validation(
    rgba: &[u8],
    width: u32,
    height: u32,
    validation_value: u64,
) -> Result<Vec<u8>> {
    encode_fast_rgba_zstd_with_validation_and_flags(
        rgba,
        width,
        height,
        validation_value,
        fast_rgba_content_flags(rgba),
    )
}

fn encode_fast_rgba_zstd_with_validation_and_flags(
    rgba: &[u8],
    width: u32,
    height: u32,
    validation_value: u64,
    flags: u32,
) -> Result<Vec<u8>> {
    encode_fast_rgba_zstd_inner(
        rgba,
        width,
        height,
        FAST_RGBA_ZSTD_VALIDATION_KIND_SIMPLE_TILE,
        flags,
        validation_value,
    )
}

fn encode_fast_rgba_zstd_empty_negative(
    width: u32,
    height: u32,
    validation_value: u64,
) -> Result<Vec<u8>> {
    let expected_len = fast_rgba_byte_len(width, height)?;
    encode_fast_rgba_zstd_header(
        width,
        height,
        expected_len,
        FAST_RGBA_ZSTD_VALIDATION_KIND_SIMPLE_TILE,
        FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE,
        validation_value,
        0,
    )
}

fn encode_fast_rgba_zstd_inner(
    rgba: &[u8],
    width: u32,
    height: u32,
    validation_kind: u32,
    flags: u32,
    validation_value: u64,
) -> Result<Vec<u8>> {
    let expected_len = fast_rgba_byte_len(width, height)?;
    if rgba.len() != expected_len {
        return Err(BedrockRenderError::Validation(format!(
            "fast tile buffer length mismatch: expected {expected_len}, got {}",
            rgba.len()
        )));
    }

    let compressed = zstd::bulk::compress(rgba, FAST_RGBA_ZSTD_LEVEL)
        .map_err(|error| BedrockRenderError::io("failed to encode zstd tile", error))?;
    let mut output = encode_fast_rgba_zstd_header(
        width,
        height,
        expected_len,
        validation_kind,
        flags,
        validation_value,
        compressed.len(),
    )?;
    output.extend_from_slice(&compressed);
    Ok(output)
}

fn encode_fast_rgba_zstd_header(
    width: u32,
    height: u32,
    rgba_len: usize,
    validation_kind: u32,
    flags: u32,
    validation_value: u64,
    payload_len: usize,
) -> Result<Vec<u8>> {
    validate_fast_rgba_flags(flags)?;
    let mut output = Vec::with_capacity(FAST_RGBA_ZSTD_HEADER_LEN.saturating_add(payload_len));
    output.extend_from_slice(FAST_RGBA_ZSTD_MAGIC);
    output.extend_from_slice(&FAST_RGBA_ZSTD_VERSION.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(
        &u64::try_from(rgba_len)
            .map_err(|_| {
                BedrockRenderError::Validation("fast tile length is too large".to_string())
            })?
            .to_le_bytes(),
    );
    output.extend_from_slice(&validation_kind.to_le_bytes());
    output.extend_from_slice(&flags.to_le_bytes());
    output.extend_from_slice(&validation_value.to_le_bytes());
    Ok(output)
}

fn validate_fast_rgba_flags(flags: u32) -> Result<()> {
    if flags & !FAST_RGBA_ZSTD_KNOWN_FLAGS != 0 {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported fast RGBA tile flags {flags:#x}"
        )));
    }
    if flags & FAST_RGBA_ZSTD_KNOWN_FLAGS == FAST_RGBA_ZSTD_KNOWN_FLAGS {
        return Err(BedrockRenderError::Validation(
            "fast RGBA tile cannot be both non-empty and empty".to_string(),
        ));
    }
    Ok(())
}

fn fast_rgba_content_flags(rgba: &[u8]) -> u32 {
    if rgba.chunks_exact(4).any(|pixel| pixel[3] != 0) {
        FAST_RGBA_ZSTD_FLAG_NON_EMPTY
    } else {
        FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE
    }
}

#[cfg(test)]
fn encode_fast_rgba_zstd_v1_for_test(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let expected_len = fast_rgba_byte_len(width, height)?;
    if rgba.len() != expected_len {
        return Err(BedrockRenderError::Validation(format!(
            "fast RGBA tile buffer length mismatch: expected {expected_len}, got {}",
            rgba.len()
        )));
    }

    let compressed = zstd::bulk::compress(rgba, FAST_RGBA_ZSTD_LEVEL)
        .map_err(|error| BedrockRenderError::io("failed to encode zstd RGBA tile", error))?;
    let mut output =
        Vec::with_capacity(FAST_RGBA_ZSTD_V1_HEADER_LEN.saturating_add(compressed.len()));
    output.extend_from_slice(FAST_RGBA_ZSTD_MAGIC);
    output.extend_from_slice(&FAST_RGBA_ZSTD_V1_VERSION.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(
        &u64::try_from(expected_len)
            .map_err(|_| {
                BedrockRenderError::Validation("fast RGBA tile length is too large".to_string())
            })?
            .to_le_bytes(),
    );
    output.extend_from_slice(&compressed);
    Ok(output)
}

/// Decodes a fast zstd-backed tile cache entry.
///
/// # Errors
///
/// Returns an error if the header is invalid, the dimensions are inconsistent, or zstd
/// decompression fails.
pub fn decode_fast_rgba_zstd(bytes: &[u8]) -> Result<FastRgbaZstdTile> {
    let header = decode_fast_rgba_zstd_header(bytes)?;
    let mut scratch = TileCacheDecodeScratch::new();
    decode_fast_rgba_zstd_with_header(bytes, header, &mut scratch)
}

/// Decodes only the header from a fast zstd-backed tile cache entry.
///
/// # Errors
///
/// Returns an error if the header is truncated, invalid, or declares inconsistent dimensions.
pub fn decode_fast_rgba_zstd_header(bytes: &[u8]) -> Result<FastRgbaZstdHeader> {
    if bytes.len() < FAST_RGBA_ZSTD_V1_HEADER_LEN {
        return Err(BedrockRenderError::Validation(
            "fast tile cache entry is truncated".to_string(),
        ));
    }
    if &bytes[..FAST_RGBA_ZSTD_MAGIC.len()] != FAST_RGBA_ZSTD_MAGIC {
        return Err(BedrockRenderError::Validation(
            "fast tile cache entry has an invalid magic".to_string(),
        ));
    }

    let version = read_le_u32(bytes, 4)?;
    let width = read_le_u32(bytes, 8)?;
    let height = read_le_u32(bytes, 12)?;
    let stored_len = read_le_u64(bytes, 16)?;
    let expected_len = fast_rgba_byte_len(width, height)?;
    if stored_len
        != u64::try_from(expected_len).map_err(|_| {
            BedrockRenderError::Validation("fast RGBA tile length is too large".to_string())
        })?
    {
        return Err(BedrockRenderError::Validation(format!(
            "fast tile payload length mismatch: expected {expected_len}, got {stored_len}"
        )));
    }

    match version {
        FAST_RGBA_ZSTD_V1_VERSION => Ok(FastRgbaZstdHeader {
            version,
            width,
            height,
            rgba_len: stored_len,
            header_len: FAST_RGBA_ZSTD_V1_HEADER_LEN,
            validation_kind: FAST_RGBA_ZSTD_VALIDATION_KIND_NONE,
            flags: 0,
            validation_value: None,
        }),
        FAST_RGBA_ZSTD_VERSION => {
            if bytes.len() < FAST_RGBA_ZSTD_HEADER_LEN {
                return Err(BedrockRenderError::Validation(
                    "fast v2 tile cache entry header is truncated".to_string(),
                ));
            }
            let validation_kind = read_le_u32(bytes, 24)?;
            let flags = read_le_u32(bytes, 28)?;
            validate_fast_rgba_flags(flags)?;
            let validation_value = read_le_u64(bytes, 32)?;
            let validation_value = match validation_kind {
                FAST_RGBA_ZSTD_VALIDATION_KIND_NONE => None,
                FAST_RGBA_ZSTD_VALIDATION_KIND_SIMPLE_TILE => Some(validation_value),
                _ => {
                    return Err(BedrockRenderError::Validation(format!(
                        "unsupported fast RGBA tile validation kind {validation_kind}"
                    )));
                }
            };
            Ok(FastRgbaZstdHeader {
                version,
                width,
                height,
                rgba_len: stored_len,
                header_len: FAST_RGBA_ZSTD_HEADER_LEN,
                validation_kind,
                flags,
                validation_value,
            })
        }
        _ => Err(BedrockRenderError::Validation(format!(
            "unsupported fast tile cache version {version}"
        ))),
    }
}

fn decode_fast_rgba_zstd_pixels_with_header(
    bytes: &[u8],
    header: FastRgbaZstdHeader,
    scratch: &mut TileCacheDecodeScratch,
) -> Result<Vec<u8>> {
    let expected_len = usize::try_from(header.rgba_len).map_err(|_| {
        BedrockRenderError::Validation(format!(
            "fast tile byte length does not fit usize: {}",
            header.rgba_len
        ))
    })?;

    let pixels = scratch.decompress_to_vec(&bytes[header.header_len..], expected_len)?;
    if pixels.len() != expected_len {
        return Err(BedrockRenderError::Validation(format!(
            "fast tile decoded length mismatch: expected {expected_len}, got {}",
            pixels.len()
        )));
    }
    Ok(pixels)
}

fn decode_fast_rgba_zstd_pixels_arc_with_header(
    bytes: &[u8],
    header: FastRgbaZstdHeader,
    scratch: &mut TileCacheDecodeScratch,
) -> Result<Arc<[u8]>> {
    let expected_len = usize::try_from(header.rgba_len).map_err(|_| {
        BedrockRenderError::Validation(format!(
            "fast tile byte length does not fit usize: {}",
            header.rgba_len
        ))
    })?;
    scratch.decompress_to_arc(&bytes[header.header_len..], expected_len)
}

fn decode_fast_rgba_zstd_with_header(
    bytes: &[u8],
    header: FastRgbaZstdHeader,
    scratch: &mut TileCacheDecodeScratch,
) -> Result<FastRgbaZstdTile> {
    let rgba = decode_fast_rgba_zstd_pixels_with_header(bytes, header, scratch)?;
    Ok(FastRgbaZstdTile {
        width: header.width,
        height: header.height,
        validation_value: header.validation_value,
        rgba,
    })
}

fn decode_fast_rgba_zstd_shared_with_header(
    bytes: &[u8],
    header: FastRgbaZstdHeader,
    scratch: &mut TileCacheDecodeScratch,
) -> Result<FastRgbaZstdSharedTile> {
    let rgba = decode_fast_rgba_zstd_pixels_arc_with_header(bytes, header, scratch)?;
    Ok(FastRgbaZstdSharedTile {
        width: header.width,
        height: header.height,
        rgba,
    })
}

fn fast_rgba_byte_len(width: u32, height: u32) -> Result<usize> {
    let pixels = width.checked_mul(height).ok_or_else(|| {
        BedrockRenderError::Validation(format!(
            "fast RGBA tile dimensions overflow: {width}x{height}"
        ))
    })?;
    let bytes = pixels.checked_mul(4).ok_or_else(|| {
        BedrockRenderError::Validation(format!(
            "fast RGBA tile byte length overflow: {width}x{height}"
        ))
    })?;
    usize::try_from(bytes).map_err(|_| {
        BedrockRenderError::Validation(format!(
            "fast RGBA tile byte length does not fit usize: {width}x{height}"
        ))
    })
}

fn fnv1a_write(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
}

fn fnv1a_write_str(hash: &mut u64, value: &str) {
    fnv1a_write_u64(hash, value.len() as u64);
    fnv1a_write(hash, value.as_bytes());
}

fn fnv1a_write_u32(hash: &mut u64, value: u32) {
    fnv1a_write(hash, &value.to_le_bytes());
}

fn fnv1a_write_i32(hash: &mut u64, value: i32) {
    fnv1a_write(hash, &value.to_le_bytes());
}

fn fnv1a_write_u64(hash: &mut u64, value: u64) {
    fnv1a_write(hash, &value.to_le_bytes());
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.saturating_add(4);
    let slice = bytes.get(offset..end).ok_or_else(|| {
        BedrockRenderError::Validation("fast RGBA tile header is truncated".to_string())
    })?;
    let array: [u8; 4] = slice.try_into().map_err(|_| {
        BedrockRenderError::Validation("fast RGBA tile header is invalid".to_string())
    })?;
    Ok(u32::from_le_bytes(array))
}

fn read_le_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let end = offset.saturating_add(8);
    let slice = bytes.get(offset..end).ok_or_else(|| {
        BedrockRenderError::Validation("fast RGBA tile header is truncated".to_string())
    })?;
    let array: [u8; 8] = slice.try_into().map_err(|_| {
        BedrockRenderError::Validation("fast RGBA tile header is invalid".to_string())
    })?;
    Ok(u64::from_le_bytes(array))
}

#[cfg(feature = "webp")]
fn encode_webp(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    WebPEncoder::new_lossless(&mut output)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| BedrockRenderError::image("failed to encode WebP tile", error))?;
    Ok(output)
}

#[cfg(not(feature = "webp"))]
fn encode_webp(_rgba: &[u8], _width: u32, _height: u32) -> Result<Vec<u8>> {
    Err(BedrockRenderError::UnsupportedMode(
        "webp feature is disabled".to_string(),
    ))
}

#[cfg(feature = "png")]
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    PngEncoder::new(&mut output)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| BedrockRenderError::image("failed to encode PNG tile", error))?;
    Ok(output)
}

#[cfg(not(feature = "png"))]
fn encode_png(_rgba: &[u8], _width: u32, _height: u32) -> Result<Vec<u8>> {
    Err(BedrockRenderError::UnsupportedMode(
        "png feature is disabled".to_string(),
    ))
}

fn check_cancelled(options: &RenderOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(RenderCancelFlag::is_cancelled)
    {
        return Err(BedrockRenderError::Cancelled);
    }
    Ok(())
}

fn emit_progress(options: &RenderOptions, completed_tiles: usize, total_tiles: usize) {
    if let Some(progress) = &options.progress {
        progress.emit(RenderProgress {
            completed_tiles,
            total_tiles,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_leveldb::{Db, OpenOptions as LevelDbOpenOptions};
    use bedrock_world::{
        ChunkKey, ChunkRecordTag, MemoryStorage, NbtTag, OpenOptions, WorldStorage,
        block_storage_index,
    };
    use indexmap::IndexMap;
    use std::fs;

    #[test]
    fn decoded_rgba_tile_image_reuses_shared_payload() {
        let pixels = Arc::<[u8]>::from([1, 2, 3, 255]);
        let tile = TileImage {
            coord: TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            width: 1,
            height: 1,
            rgba: pixels.clone(),
            encoded: None,
        };
        let decoded = decoded_tile_from_tile_image(
            tile,
            RenderTileOutputOptions {
                pixel_format: TilePixelFormat::Rgba8,
            },
        );

        assert!(Arc::ptr_eq(&decoded.pixels, &pixels));
    }

    #[test]
    fn decoded_bgra_tile_image_freezes_converted_pixels() {
        let pixels = Arc::<[u8]>::from([1, 2, 3, 255]);
        let tile = TileImage {
            coord: TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            width: 1,
            height: 1,
            rgba: pixels.clone(),
            encoded: None,
        };
        let decoded = decoded_tile_from_tile_image(
            tile,
            RenderTileOutputOptions {
                pixel_format: TilePixelFormat::Bgra8,
            },
        );

        assert!(!Arc::ptr_eq(&decoded.pixels, &pixels));
        assert_eq!(decoded.pixels.as_ref(), &[3, 2, 1, 255]);
    }

    fn cardinal_heights(west: i16, east: i16, north: i16, south: i16) -> TerrainHeightNeighborhood {
        fn average(first: i16, second: i16) -> i16 {
            i16_from_i32(i32::midpoint(i32::from(first), i32::from(second)))
        }

        TerrainHeightNeighborhood {
            center: 64,
            north_west: average(west, north),
            north,
            north_east: average(east, north),
            west,
            east,
            south_west: average(west, south),
            south,
            south_east: average(east, south),
        }
    }

    fn uniform_neighbor_heights(center: i16, neighbor: i16) -> TerrainHeightNeighborhood {
        TerrainHeightNeighborhood {
            center,
            north_west: neighbor,
            north: neighbor,
            north_east: neighbor,
            west: neighbor,
            east: neighbor,
            south_west: neighbor,
            south: neighbor,
            south_east: neighbor,
        }
    }

    fn planned_tile_at(x: i32, z: i32) -> PlannedTile {
        let layout = ChunkTileLayout::default();
        let job = RenderJob::chunk_tile(
            TileCoord {
                x,
                z,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("render job");
        PlannedTile {
            job,
            region: ChunkRegion::new(
                Dimension::Overworld,
                x.saturating_mul(16),
                z.saturating_mul(16),
                x.saturating_mul(16).saturating_add(15),
                z.saturating_mul(16).saturating_add(15),
            ),
            layout,
            chunk_positions: None,
        }
    }

    fn planned_tile_with_layout(x: i32, z: i32, layout: ChunkTileLayout) -> PlannedTile {
        let job = RenderJob::chunk_tile(
            TileCoord {
                x,
                z,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("render job");
        let chunks = tile_chunk_positions(&job).expect("tile chunk positions");
        let first = chunks.first().copied().expect("non-empty tile chunks");
        let region = chunks.iter().copied().fold(
            ChunkRegion::new(first.dimension, first.x, first.z, first.x, first.z),
            |mut region, pos| {
                region.min_chunk_x = region.min_chunk_x.min(pos.x);
                region.min_chunk_z = region.min_chunk_z.min(pos.z);
                region.max_chunk_x = region.max_chunk_x.max(pos.x);
                region.max_chunk_z = region.max_chunk_z.max(pos.z);
                region
            },
        );
        PlannedTile {
            job,
            region,
            layout,
            chunk_positions: Some(Arc::<[ChunkPos]>::from(chunks)),
        }
    }

    #[test]
    fn region_plan_collection_preserves_request_order() {
        let layout = ChunkTileLayout::default();
        let plans = collect_region_plans(
            &[
                planned_tile_with_layout(4, 0, layout),
                planned_tile_with_layout(0, 0, layout),
            ],
            RegionLayout::default(),
        )
        .expect("region plans");

        assert_eq!(plans[0].key.coord.x, 2);
        assert_eq!(plans[1].key.coord.x, 0);
    }

    fn temp_world_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bedrock-render-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove stale temp world");
        }
        path
    }
    #[test]
    fn render_chunk_index_strategy_switches_after_measured_region_threshold() {
        assert!(!should_use_full_render_chunk_index(
            SESSION_BATCH_CULL_FULL_INDEX_THRESHOLD_CHUNKS
        ));
        assert!(should_use_full_render_chunk_index(
            SESSION_BATCH_CULL_FULL_INDEX_THRESHOLD_CHUNKS + 1
        ));
    }

    #[test]
    fn leveldb_render_source_reuses_full_render_chunk_index() {
        let world_path = temp_world_dir("leveldb-render-source-full-index");
        let db_path = world_path.join("db");
        let db = Db::open(&db_path, LevelDbOpenOptions::default()).expect("open db");
        let key = bedrock_leveldb::ChunkKey::new(
            bedrock_leveldb::ChunkCoordinates::new(0, 0),
            bedrock_leveldb::Dimension::Overworld,
            bedrock_leveldb::ChunkRecordTag::Data2D,
        )
        .encode();
        db.put(key, vec![1], Default::default())
            .expect("write render key");
        drop(db);

        let source = LevelDbRenderSource::open_read_only(&world_path).expect("open render source");
        let region = ChunkRegion {
            dimension: Dimension::Overworld,
            min_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_x: i32::try_from(SESSION_BATCH_CULL_FULL_INDEX_THRESHOLD_CHUNKS)
                .expect("threshold"),
            max_chunk_z: 0,
        };
        let first = source
            .list_chunk_positions_in_region_blocking(region.into(), WorldScanOptions::default())
            .expect("first full index query");
        let first_index = Arc::clone(
            source
                .full_render_chunk_index
                .get()
                .expect("cached full index"),
        );
        let second = source
            .list_chunk_positions_in_region_blocking(region.into(), WorldScanOptions::default())
            .expect("second full index query");
        let second_index = source
            .full_render_chunk_index
            .get()
            .expect("reused full index");

        assert_eq!(first, second);
        assert!(Arc::ptr_eq(&first_index, second_index));
        drop(source);
        fs::remove_dir_all(&world_path).expect("remove temp world");
    }

    #[test]
    fn tile_cache_writer_honors_single_configured_encoder() {
        let mut options = RenderOptions::max_speed_interactive();
        options.cpu.encode_workers = 1;

        assert_eq!(tile_cache_writer_worker_count(&options, 64), 1);
    }

    fn tile_cache_key_for_test(planned: &PlannedTile) -> TileCacheKey {
        TileCacheKey {
            world_id: "test-world".to_string(),
            world_signature: "test-signature".to_string(),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            dimension: planned.job.coord.dimension,
            mode: mode_slug(RenderMode::SurfaceBlocks),
            chunks_per_tile: planned.layout.chunks_per_tile,
            blocks_per_pixel: planned.layout.blocks_per_pixel,
            pixels_per_block: planned.layout.pixels_per_block,
            tile_x: planned.job.coord.x,
            tile_z: planned.job.coord.z,
            extension: image_format_extension(ImageFormat::FastRgbaZstd).to_string(),
        }
    }

    fn authority_snapshots_for_test(
        cache: &Arc<Mutex<TileCache>>,
        key: &TileCacheKey,
    ) -> BTreeMap<TileAuthorityCacheKey, TileAuthoritySnapshotState> {
        let authority_key = tile_authority_cache_key_from_tile_key(key);
        let snapshot = cache
            .lock()
            .expect("cache lock")
            .load_authority_snapshot(&authority_key)
            .expect("load authority snapshot");
        let snapshot = snapshot.map_or(TileAuthoritySnapshotState::Missing, |snapshot| {
            TileAuthoritySnapshotState::Ready(snapshot)
        });
        BTreeMap::from([(authority_key, snapshot)])
    }

    fn blob_readers_for_test(
        cache: &Arc<Mutex<TileCache>>,
        key: &TileCacheKey,
    ) -> BTreeMap<TileAuthorityCacheKey, TileAuthorityBlobReaderState> {
        let authority_key = tile_authority_cache_key_from_tile_key(key);
        open_tile_authority_blob_readers_for_batch(
            cache,
            &BTreeMap::from([(
                authority_key.clone(),
                TileAuthoritySnapshotState::Ready(Arc::new(TileAuthorityIndexSnapshot::default())),
            )]),
        )
        .expect("open blob readers")
    }

    fn session_tile_memory_for_test(limit: usize) -> Arc<Mutex<SessionTileMemoryIndex>> {
        Arc::new(Mutex::new(SessionTileMemoryIndex::new(limit)))
    }

    #[test]
    fn session_culls_dense_tiles_to_renderable_chunks() {
        let storage = Arc::new(MemoryStorage::new());
        let loaded = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(loaded, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 4),
            )
            .expect("put renderable chunk marker");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let session = MapRenderSession::new(
            MapRenderer::new(world, RenderPalette::default()),
            MapRenderSessionConfig {
                cull_missing_chunks: true,
                ..MapRenderSessionConfig::default()
            },
        );
        let planned = vec![planned_tile_at(0, 0), planned_tile_at(1, 0)];

        let prepared = session
            .prepare_planned_tiles_for_render(
                &planned,
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("batch cull");

        assert_eq!(prepared[0].chunk_positions.as_deref(), Some(&[loaded][..]));
        assert_eq!(prepared[1].chunk_positions.as_deref(), Some(&[][..]));
    }

    #[test]
    fn render_threading_validates_fixed_range_and_auto_is_not_capped_to_eight() {
        let expected_auto = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(10_000);
        assert_eq!(
            RenderThreadingOptions::Auto
                .resolve_checked(10_000)
                .expect("auto threads"),
            expected_auto
        );
        assert_eq!(
            RenderThreadingOptions::Fixed(MAX_RENDER_THREADS)
                .resolve_checked(10_000)
                .expect("max fixed threads"),
            MAX_RENDER_THREADS
        );
        assert!(
            RenderThreadingOptions::Fixed(0)
                .resolve_checked(10)
                .is_err()
        );
        assert!(
            RenderThreadingOptions::Fixed(MAX_RENDER_THREADS + 1)
                .resolve_checked(10)
                .is_err()
        );
        let expected_interactive = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .div_euclid(2)
            .clamp(1, 6)
            .min(10_000);
        assert_eq!(
            RenderThreadingOptions::Auto
                .resolve_for_profile_checked(RenderExecutionProfile::Interactive, 10_000)
                .expect("interactive threads"),
            expected_interactive
        );
    }

    #[test]
    fn distance_priority_orders_planned_tiles_from_view_center() {
        let planned = vec![
            planned_tile_at(8, 0),
            planned_tile_at(1, 1),
            planned_tile_at(0, -1),
            planned_tile_at(1, 0),
            planned_tile_at(-2, 0),
            planned_tile_at(0, 0),
        ];
        let ordered = prioritized_planned_tiles(
            &planned,
            RenderTilePriority::DistanceFrom {
                tile_x: 0,
                tile_z: 0,
            },
        );

        let coords = ordered
            .iter()
            .map(|planned| (planned.job.coord.x, planned.job.coord.z))
            .collect::<Vec<_>>();
        assert_eq!(
            coords,
            vec![(0, 0), (0, -1), (1, 0), (1, 1), (-2, 0), (8, 0)]
        );
    }

    #[test]
    fn parallel_cache_probe_results_keep_tile_priority_order() {
        let completed_out_of_order = vec![(2, "far"), (0, "center"), (1, "near")];

        let ordered = order_tile_cache_probe_results(completed_out_of_order);

        assert_eq!(ordered, vec![(0, "center"), (1, "near"), (2, "far")]);
    }

    #[test]
    fn large_tile_priority_sorts_use_parallel_workers_without_penalizing_drag_batches() {
        assert!(!should_parallel_sort_tile_priorities(4));
        assert!(should_parallel_sort_tile_priorities(128));
    }

    #[test]
    fn surface_render_modes_use_packed_surface_columns_only() {
        assert_eq!(
            render_subchunk_decode_mode(RenderMode::SurfaceBlocks),
            SubChunkDecodeMode::SurfaceColumns
        );
        assert_eq!(
            render_subchunk_decode_mode(RenderMode::HeightMap),
            SubChunkDecodeMode::SurfaceColumns
        );
        assert_eq!(
            render_subchunk_decode_mode(RenderMode::CaveSlice { y: 32 }),
            SubChunkDecodeMode::FullIndices
        );
    }

    #[test]
    fn render_modes_declare_composable_world_data_requirements() {
        use bedrock_world::SubchunkDataRequirement;

        let surface = render_chunk_request(RenderMode::SurfaceBlocks);
        assert!(surface.block_entities);
        assert!(matches!(
            surface.subchunks.as_slice(),
            [SubchunkDataRequirement::SurfaceColumns(
                ExactSurfaceSubchunkPolicy::Full
            )]
        ));

        let height_map = render_chunk_request_for_options(
            RenderMode::HeightMap,
            &RenderOptions::max_speed_interactive(),
        );
        assert!(height_map.height_map);
        assert!(matches!(
            height_map.subchunks.as_slice(),
            [SubchunkDataRequirement::SurfaceColumns(
                ExactSurfaceSubchunkPolicy::HintThenVerify
            )]
        ));

        let cave = render_chunk_request(RenderMode::CaveSlice { y: -16 });
        assert!(matches!(
            cave.subchunks.as_slice(),
            [SubchunkDataRequirement::CaveSlice(-16)]
        ));
    }

    #[test]
    fn interactive_stream_keeps_one_shared_batch_for_all_visible_tiles() {
        let cpu_interactive = RenderOptions::max_speed_interactive();
        assert_eq!(
            render_tile_stream_group_size(&cpu_interactive, 12).expect("group size"),
            12
        );
        assert_eq!(
            render_tile_stream_group_size(&cpu_interactive, 70).expect("group size"),
            70
        );

        let interactive = RenderOptions {
            backend: RenderBackend::Cpu,
            execution_profile: RenderExecutionProfile::Interactive,
            threading: RenderThreadingOptions::Fixed(6),
            ..RenderOptions::default()
        };
        assert_eq!(
            render_tile_stream_group_size(&interactive, 8).expect("group size"),
            8
        );
        assert_eq!(
            render_tile_stream_group_size(&interactive, 2).expect("group size"),
            2
        );

        let single = RenderOptions {
            backend: RenderBackend::Cpu,
            execution_profile: RenderExecutionProfile::Interactive,
            threading: RenderThreadingOptions::Single,
            ..RenderOptions::default()
        };
        assert_eq!(
            render_tile_stream_group_size(&single, 8).expect("group size"),
            8
        );

        let export = RenderOptions {
            execution_profile: RenderExecutionProfile::Export,
            threading: RenderThreadingOptions::Fixed(6),
            ..RenderOptions::default()
        };
        assert_eq!(
            render_tile_stream_group_size(&export, 8).expect("group size"),
            8
        );
    }

    #[test]
    fn interactive_stream_continues_after_non_cancelled_group_error() {
        let options = RenderOptions::max_speed_interactive();
        let error = BedrockRenderError::Validation("damaged tile group".to_string());

        assert!(should_continue_after_stream_group_error(&options, &error));
    }

    #[test]
    fn interactive_stream_stops_after_cancellation() {
        let options = RenderOptions::max_speed_interactive();

        assert!(!should_continue_after_stream_group_error(
            &options,
            &BedrockRenderError::Cancelled
        ));
    }

    #[test]
    fn export_stream_stops_after_group_error() {
        let options = RenderOptions {
            execution_profile: RenderExecutionProfile::Export,
            ..RenderOptions::default()
        };
        let error = BedrockRenderError::Validation("damaged tile group".to_string());

        assert!(!should_continue_after_stream_group_error(&options, &error));
    }

    #[test]
    fn interactive_region_worker_split_feeds_multiple_regions_and_world_decoders() {
        let interactive = RenderOptions {
            execution_profile: RenderExecutionProfile::Interactive,
            threading: RenderThreadingOptions::Auto,
            ..RenderOptions::default()
        };
        assert_eq!(
            region_wave_worker_split(&interactive, 6, 4),
            RegionWaveWorkerSplit {
                region_workers: 2,
                world_workers_per_region: 3,
            }
        );
        assert_eq!(
            region_wave_worker_split(&interactive, 4, 8),
            RegionWaveWorkerSplit {
                region_workers: 2,
                world_workers_per_region: 2,
            }
        );
        assert_eq!(
            region_wave_worker_split(&interactive, 6, 1),
            RegionWaveWorkerSplit {
                region_workers: 1,
                world_workers_per_region: 6,
            }
        );

        let single = RenderOptions {
            execution_profile: RenderExecutionProfile::Interactive,
            threading: RenderThreadingOptions::Single,
            ..RenderOptions::default()
        };
        assert_eq!(
            region_wave_worker_split(&single, 6, 4),
            RegionWaveWorkerSplit {
                region_workers: 1,
                world_workers_per_region: 1,
            }
        );
    }

    #[test]
    fn max_speed_interactive_enables_hint_cpu_preview_and_tile_cache_policy() {
        let options = RenderOptions::max_speed_interactive();
        assert_eq!(options.backend, RenderBackend::Cpu);
        assert_eq!(
            options.execution_profile,
            RenderExecutionProfile::Interactive
        );
        assert_eq!(options.cache_policy, RenderCachePolicy::Use);
        assert_eq!(
            options.performance.profile,
            RenderPerformanceProfile::MaxSpeed
        );
        assert!(options.performance.progressive_preview);
        assert_eq!(
            options.performance.surface_load,
            RenderSurfaceLoadPolicy::HintThenVerify
        );
    }

    #[test]
    fn max_speed_export_enables_cpu_defaults() {
        let options = RenderOptions::max_speed_export();
        assert_eq!(options.backend, RenderBackend::Cpu);
        assert_eq!(options.execution_profile, RenderExecutionProfile::Export);
        assert_eq!(options.cache_policy, RenderCachePolicy::Use);
        assert_eq!(
            options.performance.profile,
            RenderPerformanceProfile::MaxSpeed
        );
        assert!(!options.performance.progressive_preview);
        assert_eq!(
            options.performance.surface_load,
            RenderSurfaceLoadPolicy::HintThenVerify
        );
    }

    #[test]
    fn max_speed_session_config_uses_final_tile_cache_metadata() {
        let config = MapRenderSessionConfig::max_speed("cache-root", "world-a", "sig-a");
        assert_eq!(config.cache_root, PathBuf::from("cache-root"));
        assert_eq!(config.world_id, "world-a");
        assert_eq!(config.world_signature, "sig-a");
        assert!(config.region_bake_cache_memory_limit >= 128);
        assert!(config.cull_missing_chunks);
    }

    #[test]
    fn region_bake_memory_cache_keys_by_layout_and_surface() {
        let key = RegionBakeKey {
            coord: RegionCoord {
                x: 1,
                z: -2,
                dimension: Dimension::Overworld,
            },
            mode: RenderMode::SurfaceBlocks,
        };
        let mut options = RenderOptions::max_speed_interactive();
        options.region_layout = RegionLayout {
            chunks_per_region: 32,
        };
        let region = RegionBake {
            coord: key.coord,
            layout: options.region_layout,
            mode: key.mode,
            covered_chunk_region: key.coord.chunk_region(options.region_layout),
            chunk_region: key.coord.chunk_region(options.region_layout),
            payload: empty_region_payload(
                RenderMode::SurfaceBlocks,
                options.surface,
                512,
                512,
                RgbaColor::new(0, 0, 0, 0),
            )
            .expect("payload"),
            diagnostics: RenderDiagnostics::default(),
            load_stats: None,
            copy_ms: 0,
            chunks_copied: 0,
            chunks_out_of_bounds: 0,
        };
        let mut cache =
            RegionBakeMemoryCache::new(4, RENDERER_CACHE_VERSION, DEFAULT_PALETTE_VERSION);

        assert!(cache.get(key, &options).is_none());
        cache.insert(key, Arc::new(region), &options);
        let first = cache.get(key, &options).expect("first cache hit");
        let second = cache.get(key, &options).expect("second cache hit");
        assert!(Arc::ptr_eq(&first, &second));

        let mut different_layout = options.clone();
        different_layout.region_layout = RegionLayout {
            chunks_per_region: 16,
        };
        assert!(cache.get(key, &different_layout).is_none());

        let mut different_surface = options;
        different_surface.surface.transparent_water = !different_surface.surface.transparent_water;
        assert!(cache.get(key, &different_surface).is_none());
    }

    #[test]
    fn fast_region_prepare_uses_contiguous_region_planes_for_scaled_tiles() {
        let layout = RegionLayout {
            chunks_per_region: 2,
        };
        let mode = RenderMode::SurfaceBlocks;
        let coord = RegionCoord {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let chunk_region = coord.chunk_region(layout);
        let mut region = RegionBake {
            coord,
            layout,
            mode,
            covered_chunk_region: chunk_region,
            chunk_region,
            payload: empty_region_payload(
                mode,
                SurfaceRenderOptions::default(),
                32,
                32,
                RgbaColor::new(0, 0, 0, 0),
            )
            .expect("payload"),
            diagnostics: RenderDiagnostics::default(),
            load_stats: None,
            copy_ms: 0,
            chunks_copied: 0,
            chunks_out_of_bounds: 0,
        };
        if let RegionBakePayload::SurfaceAtlas(surface) = &mut region.payload {
            surface
                .colors
                .set_color(0, 0, RgbaColor::new(10, 20, 30, 255));
            surface.relief_heights.set_height(0, 0, 70);
            surface.relief_heights.set_height(1, 0, 71);
            surface.relief_heights.set_height(0, 1, 72);
            surface.water_depths.set_depth(0, 0, 4);
        }
        let mut regions = BTreeMap::new();
        regions.insert(RegionBakeKey { coord, mode }, Arc::new(region));
        let job = RenderJob {
            coord: TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            mode,
            tile_size: 64,
            scale: 1,
            pixels_per_block: 2,
        };
        let options = RenderOptions {
            region_layout: layout,
            ..RenderOptions::default()
        };
        let prepared = try_prepare_region_tile_compose_fast(
            RgbaColor::new(0, 0, 0, 0),
            &job,
            &options,
            &regions,
        )
        .expect("fast prepare")
        .expect("fast path");
        assert_eq!(prepared.colors.len(), 64 * 64);
        assert_eq!(
            prepared.colors[0],
            pack_rgba_color(RgbaColor::new(10, 20, 30, 255))
        );
        assert_eq!(prepared.water_depths[0], 4);
        assert_eq!(prepared.heights[0], 70);
    }

    #[test]
    fn pipeline_stats_records_world_and_cpu_compose_timing() {
        let mut stats = RenderPipelineStats::default();
        stats.add_render_load_stats(&ChunkLoadStats {
            worker_threads: 3,
            load_ms: 11,
            db_read_ms: 5,
            decode_ms: 6,
            ..ChunkLoadStats::default()
        });
        stats.add_tile_compose_stats(TileComposeStats::cpu());

        assert_eq!(stats.world_worker_threads, 3);
        assert_eq!(stats.world_load_ms, 11);
        assert_eq!(stats.db_read_ms, 5);
        assert_eq!(stats.decode_ms, 6);
        assert_eq!(stats.cpu_decode_ms, 6);
        assert_eq!(stats.chunk_frame_decode_ms, 6);
        assert_eq!(stats.cpu_tiles, 1);
        assert_eq!(stats.tile_compose_ms, 0);
    }

    #[test]
    fn pipeline_stats_finalize_throughput_and_worker_utilization() {
        let mut stats = RenderPipelineStats {
            planned_tiles: 20,
            unique_chunks: 80,
            peak_worker_threads: 4,
            world_load_ms: 100,
            decode_ms: 100,
            region_bake_ms: 100,
            tile_compose_ms: 100,
            ..RenderPipelineStats::default()
        };

        finalize_pipeline_throughput(&mut stats, Duration::from_secs(2));

        assert_eq!(stats.tiles_per_second, 10);
        assert_eq!(stats.chunks_per_second, 40);
        assert_eq!(stats.cpu_worker_utilization_per_mille, 50);
    }

    #[test]
    fn block_state_overrides_crop_growth_and_farmland_moisture() {
        let young_wheat =
            test_block_state("minecraft:wheat", [("growth", NbtTag::Int(0))].into_iter());
        let mature_wheat =
            test_block_state("minecraft:wheat", [("growth", NbtTag::Int(7))].into_iter());
        let dry_farmland = test_block_state(
            "minecraft:farmland",
            [("moisturized_amount", NbtTag::Int(0))].into_iter(),
        );
        let wet_farmland = test_block_state(
            "minecraft:farmland",
            [("moisturized_amount", NbtTag::Int(7))].into_iter(),
        );

        let young = state_block_color(&RenderPalette::default(), &young_wheat);
        let mature = state_block_color(&RenderPalette::default(), &mature_wheat);
        let dry = state_block_color(&RenderPalette::default(), &dry_farmland);
        let wet = state_block_color(&RenderPalette::default(), &wet_farmland);

        assert!(mature.red > young.red);
        assert!(mature.green >= young.green);
        assert!(mature.blue > young.blue);
        assert!(rgba_color_distance(young, mature) >= 120);
        assert!(dry.red > wet.red);
        assert!(dry.green > wet.green);
        assert!(rgba_color_distance(dry, wet) >= 100);
    }

    #[test]
    fn block_state_overrides_horizontal_log_axis() {
        let vertical = test_block_state(
            "minecraft:oak_log",
            [("pillar_axis", NbtTag::String("y".to_string()))].into_iter(),
        );
        let horizontal = test_block_state(
            "minecraft:oak_log",
            [("pillar_axis", NbtTag::String("x".to_string()))].into_iter(),
        );

        let palette = RenderPalette::default();
        let vertical_color = state_block_color(&palette, &vertical);
        let horizontal_color = state_block_color(&palette, &horizontal);

        assert_ne!(vertical_color, horizontal_color);
        assert!(rgba_color_distance(vertical_color, horizontal_color) >= 40);
    }

    #[test]
    fn slabs_use_matching_full_block_material_colors() {
        let palette = RenderPalette::default();
        let old_cobblestone_slab = test_block_state(
            "minecraft:stone_slab",
            [
                ("stone_slab_type", NbtTag::String("cobblestone".to_string())),
                ("top_slot_bit", NbtTag::Byte(0)),
            ]
            .into_iter(),
        );
        let modern_cobblestone_slab = test_block_state(
            "minecraft:cobblestone_slab",
            [(
                "minecraft:vertical_half",
                NbtTag::String("bottom".to_string()),
            )]
            .into_iter(),
        );
        let smooth_stone_slab = test_block_state(
            "minecraft:stone_slab",
            [(
                "stone_slab_type",
                NbtTag::String("smooth_stone".to_string()),
            )]
            .into_iter(),
        );
        let normal_stone_slab = test_block_state(
            "minecraft:stone_slab4",
            [("stone_slab_type_4", NbtTag::String("stone".to_string()))].into_iter(),
        );

        let old_cobblestone = state_block_color(&palette, &old_cobblestone_slab);
        let modern_cobblestone = state_block_color(&palette, &modern_cobblestone_slab);
        let smooth_stone = state_block_color(&palette, &smooth_stone_slab);
        let normal_stone = state_block_color(&palette, &normal_stone_slab);

        assert_eq!(old_cobblestone, modern_cobblestone);
        assert_eq!(
            old_cobblestone,
            palette.block_color("minecraft:cobblestone")
        );
        assert_eq!(
            modern_cobblestone,
            palette.block_color("minecraft:cobblestone")
        );
        assert_eq!(smooth_stone, palette.block_color("minecraft:smooth_stone"));
        assert_eq!(normal_stone, palette.block_color("minecraft:stone"));
        assert_ne!(smooth_stone, old_cobblestone);
    }

    #[test]
    fn render_memory_budget_auto_has_stable_profile_caps() {
        let export_budget = RenderMemoryBudget::Auto
            .resolve_bytes(RenderExecutionProfile::Export)
            .expect("export budget");
        let interactive_budget = RenderMemoryBudget::Auto
            .resolve_bytes(RenderExecutionProfile::Interactive)
            .expect("interactive budget");
        assert!(export_budget >= MIN_AUTO_MEMORY_BUDGET_BYTES);
        assert!(export_budget <= MAX_AUTO_MEMORY_BUDGET_BYTES);
        assert!(interactive_budget <= DEFAULT_INTERACTIVE_MEMORY_BUDGET_BYTES);
        assert_eq!(
            RenderMemoryBudget::FixedBytes(64 * 1024 * 1024)
                .resolve_bytes(RenderExecutionProfile::Export),
            Some(64 * 1024 * 1024)
        );
        assert_eq!(
            RenderMemoryBudget::Disabled.resolve_bytes(RenderExecutionProfile::Export),
            None
        );
    }

    #[test]
    fn render_layout_supports_pixels_per_block() {
        let layout = RenderLayout {
            chunks_per_tile: 16,
            blocks_per_pixel: 1,
            pixels_per_block: 2,
        };
        assert_eq!(layout.tile_size(), Some(512));
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::HeightMap,
            layout,
        )
        .expect("chunk tile");
        assert_eq!(job.tile_size, 512);
        assert_eq!(job.scale, 1);
        assert_eq!(job.pixels_per_block, 2);
    }

    #[test]
    fn render_layout_supports_ui_eight_by_eight_tiles() {
        let layout = RenderLayout {
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 2,
        };
        assert_eq!(layout.tile_size(), Some(256));

        let job = RenderJob::chunk_tile(
            TileCoord {
                x: -1,
                z: 2,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("8x8 ui tile");
        let chunks = tile_chunk_positions(&job).expect("tile chunk positions");

        assert_eq!(job.tile_size, 256);
        assert_eq!(job.scale, 1);
        assert_eq!(job.pixels_per_block, 2);
        assert_eq!(chunks.len(), 64);
        assert!(chunks.contains(&ChunkPos {
            x: -8,
            z: 16,
            dimension: Dimension::Overworld,
        }));
        assert!(chunks.contains(&ChunkPos {
            x: -1,
            z: 23,
            dimension: Dimension::Overworld,
        }));
        assert!(job.scale <= job.pixels_per_block);
    }

    #[test]
    fn partial_eight_by_eight_region_bake_does_not_cover_neighbor_tile() {
        let tile_layout = RenderLayout {
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 4,
        };
        let region_layout = RegionLayout {
            chunks_per_region: 32,
        };
        let mut first_plans = collect_region_plans(
            &[planned_tile_with_layout(0, 0, tile_layout)],
            region_layout,
        )
        .expect("first region plan");
        let mut second_plans = collect_region_plans(
            &[planned_tile_with_layout(1, 0, tile_layout)],
            region_layout,
        )
        .expect("second region plan");
        let first_plan = first_plans.pop().expect("first plan");
        let second_plan = second_plans.pop().expect("second plan");

        assert_eq!(first_plan.key, second_plan.key);
        assert_eq!(first_plan.chunk_positions.len(), 64);
        assert_eq!(second_plan.chunk_positions.len(), 64);
        assert_eq!(first_plan.region.min_chunk_x, 0);
        assert_eq!(first_plan.region.max_chunk_x, 7);
        assert_eq!(second_plan.region.min_chunk_x, 8);
        assert_eq!(second_plan.region.max_chunk_x, 15);

        let partial_region = RegionBake {
            coord: first_plan.key.coord,
            layout: region_layout,
            mode: first_plan.key.mode,
            covered_chunk_region: first_plan.region,
            chunk_region: first_plan.key.coord.chunk_region(region_layout),
            payload: empty_region_payload(
                first_plan.key.mode,
                SurfaceRenderOptions::default(),
                512,
                512,
                RgbaColor::new(0, 0, 0, 0),
            )
            .expect("payload"),
            diagnostics: RenderDiagnostics::default(),
            load_stats: None,
            copy_ms: 0,
            chunks_copied: first_plan.chunk_positions.len(),
            chunks_out_of_bounds: 0,
        };

        assert!(region_bake_covers_plan(&partial_region, &first_plan));
        assert!(!region_bake_covers_plan(&partial_region, &second_plan));
        assert!(!region_bake_covers_full_region(
            &partial_region,
            region_layout
        ));
    }

    #[test]
    fn adjacent_eight_by_eight_tiles_merge_region_plan_chunks() {
        let tile_layout = RenderLayout {
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 4,
        };
        let region_layout = RegionLayout {
            chunks_per_region: 32,
        };
        let plans = collect_region_plans(
            &[
                planned_tile_with_layout(0, 0, tile_layout),
                planned_tile_with_layout(1, 0, tile_layout),
            ],
            region_layout,
        )
        .expect("merged region plan");

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].chunk_positions.len(), 128);
        assert_eq!(plans[0].region.min_chunk_x, 0);
        assert_eq!(plans[0].region.max_chunk_x, 15);
        assert_eq!(plans[0].region.min_chunk_z, 0);
        assert_eq!(plans[0].region.max_chunk_z, 7);
    }

    #[test]
    fn partial_eight_by_eight_region_bake_does_not_mark_neighbor_ready() {
        let tile_layout = RenderLayout {
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 4,
        };
        let region_layout = RegionLayout {
            chunks_per_region: 32,
        };
        let first_tile = planned_tile_with_layout(0, 0, tile_layout);
        let second_tile = planned_tile_with_layout(1, 0, tile_layout);
        let tile_region_plans = vec![
            tile_region_plans(&first_tile, region_layout).expect("first tile plans"),
            tile_region_plans(&second_tile, region_layout).expect("second tile plans"),
        ];
        let first_plan = tile_region_plans[0].first().cloned().expect("first plan");
        let second_plan = tile_region_plans[1].first().cloned().expect("second plan");
        assert_eq!(first_plan.key, second_plan.key);

        let partial_region = RegionBake {
            coord: first_plan.key.coord,
            layout: region_layout,
            mode: first_plan.key.mode,
            covered_chunk_region: first_plan.region,
            chunk_region: first_plan.key.coord.chunk_region(region_layout),
            payload: empty_region_payload(
                first_plan.key.mode,
                SurfaceRenderOptions::default(),
                512,
                512,
                RgbaColor::new(0, 0, 0, 0),
            )
            .expect("payload"),
            diagnostics: RenderDiagnostics::default(),
            load_stats: None,
            copy_ms: 0,
            chunks_copied: first_plan.chunk_positions.len(),
            chunks_out_of_bounds: 0,
        };
        let mut regions = BTreeMap::new();
        regions.insert(first_plan.key, Arc::new(partial_region));

        let ready = ready_web_tile_indexes(&[0, 1], &tile_region_plans, &regions, &BTreeSet::new())
            .expect("ready tiles");

        assert_eq!(ready, vec![0]);
    }

    #[test]
    fn render_layout_rejects_invalid_pixel_sizes() {
        assert_eq!(
            RenderLayout {
                chunks_per_tile: 16,
                blocks_per_pixel: 3,
                pixels_per_block: 2,
            }
            .tile_size(),
            None
        );
        assert_eq!(
            RenderLayout {
                chunks_per_tile: 256,
                blocks_per_pixel: 1,
                pixels_per_block: 2,
            }
            .tile_size(),
            None
        );
    }

    #[test]
    fn web_tile_pipeline_bakes_shared_chunks_once() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let layout = ChunkTileLayout::default();
        let region = ChunkRegion::new(Dimension::Overworld, 0, 0, 15, 15);
        let planned = MapRenderer::<Arc<dyn WorldStorage>>::plan_region_tiles(
            region,
            RenderMode::HeightMap,
            layout,
        )
        .expect("plan");
        let planned = vec![planned[0].clone(), planned[0].clone()];
        let emitted_tiles = AtomicUsize::new(0);
        let result = renderer
            .render_web_tiles_blocking(
                &planned,
                RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Fixed(4),
                    memory_budget: RenderMemoryBudget::Disabled,
                    ..RenderOptions::default()
                },
                |_planned, tile| {
                    assert_eq!(tile.rgba.len(), 256 * 256 * 4);
                    emitted_tiles.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            )
            .expect("render web tiles");
        assert_eq!(emitted_tiles.load(Ordering::Relaxed), 2);
        assert_eq!(result.stats.planned_tiles, 2);
        assert_eq!(result.stats.planned_regions, 1);
        assert_eq!(result.stats.baked_regions, 1);
        assert_eq!(result.stats.unique_chunks, 256);
        assert_eq!(result.stats.baked_chunks, 256);
    }

    #[test]
    fn tile_coordinate_handles_negative_world_blocks() {
        let job = RenderJob {
            coord: TileCoord {
                x: -1,
                z: 2,
                dimension: Dimension::Overworld,
            },
            tile_size: 256,
            scale: 2,
            ..RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::Biome { y: 64 },
            )
        };
        assert_eq!(
            tile_pixel_to_block(&job, 0, 0).expect("coord"),
            (-512, 1024)
        );
        assert_eq!(
            tile_pixel_to_block(&job, 1, 1).expect("coord"),
            (-510, 1026)
        );
    }

    #[test]
    fn render_rgba_tile_has_expected_size() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let job = RenderJob {
            tile_size: 4,
            ..RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::Biome { y: 64 },
            )
        };
        let tile = renderer
            .render_tile_with_options_blocking(
                job,
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    ..RenderOptions::default()
                },
            )
            .expect("render");
        assert_eq!(tile.width, 4);
        assert_eq!(tile.height, 4);
        assert_eq!(tile.rgba.len(), 4 * 4 * 4);
        assert!(tile.encoded.is_none());
    }

    #[test]
    fn legacy_terrain_renders_surface_height_layer_and_cave() {
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
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());

        for mode in [
            RenderMode::SurfaceBlocks,
            RenderMode::HeightMap,
            RenderMode::LayerBlocks { y: 65 },
            RenderMode::CaveSlice { y: 65 },
        ] {
            let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
            let diagnostics_sink = RenderDiagnosticsSink::new({
                let diagnostics = Arc::clone(&diagnostics);
                move |value| diagnostics.lock().expect("diagnostics lock").add(value)
            });
            let tile = renderer
                .render_tile_with_options_blocking(
                    RenderJob {
                        tile_size: 16,
                        ..RenderJob::new(
                            TileCoord {
                                x: 0,
                                z: 0,
                                dimension: Dimension::Overworld,
                            },
                            mode,
                        )
                    },
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        diagnostics: Some(diagnostics_sink),
                        threading: RenderThreadingOptions::Single,
                        backend: RenderBackend::Cpu,
                        ..RenderOptions::default()
                    },
                )
                .expect("render legacy tile");
            assert_eq!(tile.rgba.len(), 16 * 16 * 4);
            assert!(
                tile.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0),
                "legacy {mode:?} should render visible pixels"
            );
            assert_eq!(
                diagnostics.lock().expect("diagnostics lock").missing_chunks,
                0,
                "legacy {mode:?} should not report missing chunks"
            );
        }
    }

    #[test]
    fn legacy_terrain_surface_uses_top_grass_and_legacy_biome_tint() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_grass_over_stone_bytes(65, 0x0020_c840),
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    surface: SurfaceRenderOptions {
                        biome_tint: true,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy grass surface");
        let pixel = pixel_rgba(&tile.rgba, tile.width, 8, 8);
        assert!(
            pixel[1] > pixel[0].saturating_add(20) && pixel[1] > pixel[2].saturating_add(20),
            "legacy grass should render green, got {pixel:?}"
        );
    }

    #[test]
    fn legacy_terrain_biome_mode_uses_palette_viewport_colors() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut terrain = test_legacy_terrain_bytes(2, 65);
        write_legacy_biome_sample(&mut terrain, 0, 0, 4, 0x0011_2233);
        write_legacy_biome_sample(&mut terrain, 1, 0, 1, 0x007f_b238);
        write_legacy_biome_sample(&mut terrain, 0, 1, 2, 0x00aa_4411);
        write_legacy_biome_sample(&mut terrain, 15, 15, 12, 0x0044_5566);
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &terrain,
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let palette = RenderPalette::default();
        let forest = palette.biome_color(4).to_array();
        let plains = palette.biome_color(1).to_array();
        let desert = palette.biome_color(2).to_array();
        let ice_plains = palette.biome_color(12).to_array();
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::Biome { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy biome");

        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 0, 0), forest);
        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 1, 0), plains);
        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 0, 1), desert);
        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 15, 15), ice_plains);
    }

    #[test]
    fn legacy_terrain_biome_mode_prefers_legacy_biome_id_over_conflicting_data2d() {
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
            .expect("put conflicting data2d");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let palette = RenderPalette::default();
        let expected = palette.biome_color(12).to_array();
        let renderer = MapRenderer::new(world, palette);
        let job = RenderJob {
            tile_size: 16,
            ..RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::Biome { y: 64 },
            )
        };
        let base_options = RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            region_layout: RegionLayout {
                chunks_per_region: 1,
            },
            memory_budget: RenderMemoryBudget::Disabled,
            surface: SurfaceRenderOptions {
                height_shading: false,
                block_boundaries: BlockBoundaryRenderOptions::off(),
                ..SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        };

        let direct = renderer
            .render_tile_with_options_blocking(
                job.clone(),
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct legacy biome render");
        assert_eq!(pixel_rgba(&direct.rgba, direct.width, 0, 0), expected);

        let shared = renderer
            .render_tiles_blocking(
                vec![job],
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options
                },
            )
            .expect("shared legacy biome render")
            .pop()
            .expect("shared tile");
        assert_eq!(direct.rgba, shared.rgba);
    }

    #[test]
    fn legacy_terrain_water_does_not_use_grass_biome_rgb() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_terrain_bytes(9, 65),
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    surface: SurfaceRenderOptions {
                        transparent_water: false,
                        biome_tint: true,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy water");
        let pixel = pixel_rgba(&tile.rgba, tile.width, 8, 8);
        assert!(
            pixel[2] > pixel[1],
            "legacy water should stay water-tinted rather than grass-green, got {pixel:?}"
        );
    }

    #[test]
    fn legacy_terrain_raw_biome_mode_uses_biome_id_when_known() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut terrain = test_legacy_terrain_bytes(2, 65);
        write_legacy_biome_sample(&mut terrain, 0, 0, 4, 0x0011_2233);
        write_legacy_biome_sample(&mut terrain, 1, 0, 250, 0x0044_5566);
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &terrain,
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let palette = RenderPalette::default();
        let known_raw = palette.raw_biome_color(4).to_array();
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::RawBiomeLayer { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy raw biome");

        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 0, 0), known_raw);
        assert_eq!(
            pixel_rgba(&tile.rgba, tile.width, 1, 0),
            [0x44, 0x55, 0x66, 255]
        );
    }

    #[test]
    fn mixed_legacy_terrain_and_subchunk_prefers_subchunk_surface() {
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
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0),
                    ("minecraft:stone", 1),
                    ("minecraft:grass_block", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(90, 90, 90, 255))
            .with_block_color("minecraft:grass_block", RgbaColor::new(20, 200, 20, 255));
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render mixed chunk");
        let pixel = pixel_rgba(&tile.rgba, tile.width, 8, 8);
        assert!(
            pixel[1] > pixel[0] && pixel[1] > pixel[2],
            "mixed chunk should prefer subchunk grass over legacy stone, got {pixel:?}"
        );
    }

    #[test]
    fn legacy_terrain_uses_yzx_payload_order_without_stripes() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_asymmetric_legacy_terrain_bytes(),
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(90, 90, 90, 255))
            .with_block_color("minecraft:sand", RgbaColor::new(220, 210, 120, 255))
            .with_block_color("minecraft:sandstone", RgbaColor::new(180, 155, 85, 255))
            .with_block_color("minecraft:bricks", RgbaColor::new(150, 45, 35, 255));
        let stone = palette.block_color("minecraft:stone").to_array();
        let sand = palette.block_color("minecraft:sand").to_array();
        let sandstone = palette.block_color("minecraft:sandstone").to_array();
        let bricks = palette.block_color("minecraft:bricks").to_array();
        let height_low = palette.height_color(20, 0, 127).to_array();
        let height_high = palette.height_color(50, 0, 127).to_array();
        let renderer = MapRenderer::new(world, palette);

        let layer = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 10 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy layer");
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 0, 0), stone);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 15, 0), sand);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 0, 15), sandstone);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 15, 15), bricks);

        let heightmap = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::HeightMap,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy heightmap");
        assert_eq!(
            pixel_rgba(&heightmap.rgba, heightmap.width, 0, 0),
            height_low
        );
        assert_eq!(
            pixel_rgba(&heightmap.rgba, heightmap.width, 15, 15),
            height_high
        );
        assert_ne!(
            pixel_rgba(&heightmap.rgba, heightmap.width, 0, 0),
            pixel_rgba(&heightmap.rgba, heightmap.width, 15, 15)
        );

        let surface = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    surface: SurfaceRenderOptions {
                        transparent_water: false,
                        biome_tint: false,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy surface");
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 0, 0), stone);
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 15, 0), sand);
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 0, 15), sandstone);
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 15, 15), bricks);
    }

    #[test]
    fn legacy_subchunk_uses_xzy_payload_order_without_transposed_columns() {
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
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(90, 90, 90, 255))
            .with_block_color("minecraft:sand", RgbaColor::new(220, 210, 120, 255))
            .with_block_color("minecraft:sandstone", RgbaColor::new(180, 155, 85, 255))
            .with_block_color("minecraft:bricks", RgbaColor::new(150, 45, 35, 255));
        let stone = palette.block_color("minecraft:stone").to_array();
        let sand = palette.block_color("minecraft:sand").to_array();
        let sandstone = palette.block_color("minecraft:sandstone").to_array();
        let bricks = palette.block_color("minecraft:bricks").to_array();
        let renderer = MapRenderer::new(world, palette);

        let layer = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 10 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy subchunk layer");

        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 0, 0), stone);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 15, 0), sand);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 0, 15), sandstone);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 15, 15), bricks);
    }

    #[test]
    fn legacy_terrain_sand_data_one_renders_red_sand_without_column_bleed() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_sand_data_terrain_bytes(),
            )
            .expect("put legacy sand terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:sand", RgbaColor::new(220, 210, 120, 255))
            .with_block_color("minecraft:red_sand", RgbaColor::new(190, 80, 20, 255));
        let sand = palette.block_color("minecraft:sand").to_array();
        let red_sand = palette.block_color("minecraft:red_sand").to_array();
        let renderer = MapRenderer::new(world, palette);

        let layer = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 10 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy sand layer");
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 7, 8), sand);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 8, 8), red_sand);

        let surface = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    surface: SurfaceRenderOptions {
                        transparent_water: false,
                        biome_tint: false,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy sand surface");
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 7, 8), sand);
        assert_eq!(pixel_rgba(&surface.rgba, surface.width, 8, 8), red_sand);
    }

    #[test]
    fn legacy_subchunk_sand_data_one_renders_red_sand_without_column_bleed() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_legacy_sand_data_subchunk_bytes(),
            )
            .expect("put legacy sand subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:sand", RgbaColor::new(220, 210, 120, 255))
            .with_block_color("minecraft:red_sand", RgbaColor::new(190, 80, 20, 255));
        let sand = palette.block_color("minecraft:sand").to_array();
        let red_sand = palette.block_color("minecraft:red_sand").to_array();
        let renderer = MapRenderer::new(world, palette);

        let layer = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 10 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    backend: RenderBackend::Cpu,
                    ..RenderOptions::default()
                },
            )
            .expect("render legacy sand subchunk layer");

        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 7, 8), sand);
        assert_eq!(pixel_rgba(&layer.rgba, layer.width, 8, 8), red_sand);
    }

    #[test]
    fn cancelled_batch_returns_error() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let cancel = RenderCancelFlag::new();
        cancel.cancel();
        let result = renderer.render_tiles_blocking(
            vec![RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::Biome { y: 64 },
            )],
            RenderOptions {
                format: ImageFormat::Rgba,
                cancel: Some(cancel),
                ..RenderOptions::default()
            },
        );
        assert!(matches!(result, Err(BedrockRenderError::Cancelled)));
    }

    #[test]
    fn chunk_region_plans_web_map_tiles() {
        let layout = ChunkTileLayout::default();
        let region = ChunkRegion::new(Dimension::Overworld, -1, -1, 15, 15);
        let tiles = MapRenderer::<Arc<dyn WorldStorage>>::plan_region_tiles(
            region,
            RenderMode::HeightMap,
            layout,
        )
        .expect("plan region");

        assert_eq!(tiles.len(), 4);
        assert_eq!(
            tiles[0].relative_path(TilePathScheme::WebMap, "webp"),
            std::path::PathBuf::from("overworld")
                .join("heightmap")
                .join("16c-1bpp-1ppb")
                .join("-1")
                .join("-1.webp")
        );
        assert_eq!(tiles[0].job.tile_size, 256);
        assert_eq!(tiles[0].job.scale, 1);
    }

    #[test]
    fn two_pixels_per_block_repeats_block_color() {
        let job = RenderJob {
            coord: TileCoord {
                x: -1,
                z: -1,
                dimension: Dimension::Overworld,
            },
            tile_size: 512,
            scale: 1,
            pixels_per_block: 2,
            mode: RenderMode::HeightMap,
        };
        assert_eq!(
            tile_pixel_to_block(&job, 0, 0).expect("origin"),
            (-256, -256)
        );
        assert_eq!(
            tile_pixel_to_block(&job, 1, 1).expect("duplicated first pixel"),
            (-256, -256)
        );
        assert_eq!(
            tile_pixel_to_block(&job, 2, 2).expect("next block"),
            (-255, -255)
        );
    }

    #[test]
    fn soft_lighting_keeps_flat_terrain_stable() {
        let base = RgbaColor::new(100, 120, 140, 255);
        let shaded = terrain_lit_color(
            base,
            cardinal_heights(64, 64, 64, 64),
            0,
            TerrainLightingOptions::soft(),
        );
        assert_eq!(shaded, base);
    }

    #[test]
    fn strong_lighting_changes_slope_contrast() {
        let base = RgbaColor::new(100, 100, 100, 255);
        let soft = terrain_lit_color(
            base,
            cardinal_heights(64, 88, 64, 64),
            0,
            TerrainLightingOptions::soft(),
        );
        let strong = terrain_lit_color(
            base,
            cardinal_heights(64, 88, 64, 64),
            0,
            TerrainLightingOptions::strong(),
        );
        let soft_delta = i16::from(soft.red).abs_diff(i16::from(base.red));
        let strong_delta = i16::from(strong.red).abs_diff(i16::from(base.red));
        assert!(soft_delta > 0);
        assert!(strong_delta > 0);
        assert_ne!(strong, soft);
        assert_eq!(strong.alpha, 255);
    }

    #[test]
    fn land_max_shadow_limits_darkest_terrain_shadow() {
        let base = RgbaColor::new(100, 100, 100, 255);
        let mut uncapped = TerrainLightingOptions::strong();
        uncapped.max_shadow = 70.0;
        let mut capped = uncapped;
        capped.max_shadow = 12.0;

        let dark = terrain_lit_color(base, cardinal_heights(72, 64, 64, 64), 0, uncapped);
        let limited = terrain_lit_color(base, cardinal_heights(72, 64, 64, 64), 0, capped);

        assert!(limited.red > dark.red);
        assert!(limited.red >= 88);
    }

    #[test]
    fn land_slope_softness_reduces_extreme_shadow_overlap() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let mut harsh = TerrainLightingOptions::strong();
        harsh.land_slope_softness = 1_000.0;
        harsh.max_shadow = 70.0;
        harsh.edge_relief_strength = 0.0;
        let mut softened = harsh;
        softened.land_slope_softness = 4.0;

        let steep = cardinal_heights(112, 32, 64, 64);
        let harsh_color = terrain_lit_color(base, steep, 0, harsh);
        let softened_color = terrain_lit_color(base, steep, 0, softened);

        assert!(softened_color.red > harsh_color.red);
    }

    #[test]
    fn land_slope_softness_keeps_small_slopes_visible() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let shaded = terrain_lit_color(
            base,
            cardinal_heights(64, 68, 64, 64),
            0,
            TerrainLightingOptions::soft(),
        );

        assert_ne!(shaded, base);
    }

    #[test]
    fn land_slope_softness_does_not_reduce_underwater_relief() {
        let base = RgbaColor::new(100, 100, 100, 255);
        let mut flat_land = TerrainLightingOptions::strong();
        flat_land.land_slope_softness = 0.0;
        let mut normal_land = flat_land;
        normal_land.land_slope_softness = 8.0;
        let slope = cardinal_heights(96, 32, 64, 64);

        let flat_land_water = terrain_lit_color(base, slope, 1, flat_land);
        let normal_land_water = terrain_lit_color(base, slope, 1, normal_land);

        assert_eq!(flat_land_water, normal_land_water);
    }

    #[test]
    fn edge_relief_does_not_change_flat_terrain() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let shaded = terrain_lit_color(
            base,
            uniform_neighbor_heights(64, 64),
            0,
            TerrainLightingOptions::strong(),
        );

        assert_eq!(shaded, base);
    }

    #[test]
    fn edge_relief_adds_contact_shadow_to_deep_pit() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let mut without_edge = TerrainLightingOptions::strong();
        without_edge.edge_relief_strength = 0.0;
        let mut with_edge = without_edge;
        with_edge.edge_relief_strength = 1.0;
        with_edge.edge_relief_max_shadow = 18.0;

        let pit = uniform_neighbor_heights(48, 72);
        let flat_pit = terrain_lit_color(base, pit, 0, without_edge);
        let edged_pit = terrain_lit_color(base, pit, 0, with_edge);

        assert!(edged_pit.red < flat_pit.red);
    }

    #[test]
    fn edge_relief_threshold_ignores_small_height_noise() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let mut lighting = TerrainLightingOptions::strong();
        lighting.edge_relief_strength = 1.0;
        lighting.edge_relief_threshold = 3.0;

        let shaded = terrain_lit_color(base, uniform_neighbor_heights(64, 66), 0, lighting);

        assert_eq!(shaded, base);
    }

    #[test]
    fn edge_relief_max_shadow_limits_pit_edge_darkness() {
        let base = RgbaColor::new(120, 120, 120, 255);
        let mut limited = TerrainLightingOptions::strong();
        limited.edge_relief_strength = 1.0;
        limited.edge_relief_max_shadow = 4.0;
        let mut strong = limited;
        strong.edge_relief_max_shadow = 18.0;

        let pit = uniform_neighbor_heights(48, 72);
        let limited_color = terrain_lit_color(base, pit, 0, limited);
        let strong_color = terrain_lit_color(base, pit, 0, strong);

        assert!(limited_color.red > strong_color.red);
        assert!(limited_color.red >= 115);
    }

    #[test]
    fn block_boundaries_off_preserves_surface_lighting() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(120, 120, 120, 255);
        let heights = uniform_neighbor_heights(64, 64);
        let mut surface = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions::off(),
            ..SurfaceRenderOptions::default()
        };
        surface.lighting.edge_relief_strength = 0.0;
        let without_boundary = surface_lit_color(&palette, base, heights, 0, surface, None, None);
        let with_disabled_boundary = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            surface,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 4,
                blocks_per_pixel: 1,
            }),
        );

        assert_eq!(without_boundary, with_disabled_boundary);
    }

    #[test]
    fn flat_block_boundary_adds_subtle_grid_shadow() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let surface = SurfaceRenderOptions {
            lighting: TerrainLightingOptions::off(),
            block_boundaries: BlockBoundaryRenderOptions {
                flat_strength: 0.35,
                max_shadow: 16.0,
                ..BlockBoundaryRenderOptions::default()
            },
            ..SurfaceRenderOptions::default()
        };
        let shaded = surface_lit_color(
            &palette,
            base,
            uniform_neighbor_heights(64, 64),
            0,
            surface,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 4,
                blocks_per_pixel: 1,
            }),
        );

        assert!(shaded.red < base.red);
        assert!(base.red - shaded.red <= 12);
    }

    #[test]
    fn block_boundary_contact_shadow_emphasizes_height_step() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let mut no_boundary = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions::off(),
            ..SurfaceRenderOptions::default()
        };
        no_boundary.lighting.edge_relief_strength = 0.0;
        let with_boundary = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions {
                strength: 1.0,
                flat_strength: 0.0,
                max_shadow: 18.0,
                height_threshold: 1.0,
                ..BlockBoundaryRenderOptions::default()
            },
            ..no_boundary
        };
        let heights = uniform_neighbor_heights(48, 72);
        let baseline = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            no_boundary,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 1,
                blocks_per_pixel: 1,
            }),
        );
        let shaded = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            with_boundary,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 1,
                blocks_per_pixel: 1,
            }),
        );

        assert!(shaded.red < baseline.red);
    }

    #[test]
    fn block_boundary_threshold_ignores_small_height_noise() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let surface = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions {
                strength: 1.0,
                flat_strength: 0.0,
                height_threshold: 3.0,
                ..BlockBoundaryRenderOptions::default()
            },
            ..SurfaceRenderOptions::default()
        };
        let without_boundary = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions::off(),
            ..surface
        };
        let heights = uniform_neighbor_heights(64, 66);
        let baseline = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            without_boundary,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 1,
                blocks_per_pixel: 1,
            }),
        );
        let shaded = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            surface,
            None,
            Some(BlockBoundaryContext {
                pixel_x: 0,
                pixel_z: 0,
                pixels_per_block: 1,
                blocks_per_pixel: 1,
            }),
        );

        assert_eq!(baseline, shaded);
    }

    fn test_block_volume_options() -> BlockVolumeRenderOptions {
        BlockVolumeRenderOptions {
            enabled: true,
            face_width_pixels: 1.5,
            face_shadow_strength: 0.8,
            contact_shadow_strength: 0.8,
            cast_shadow_strength: 0.8,
            cast_shadow_max_blocks: 2,
            cast_shadow_height_scale: 0.5,
            highlight_strength: 0.2,
            max_shadow: 30.0,
            max_highlight: 14.0,
            height_threshold: 1.0,
            softness: 4.0,
        }
    }

    fn test_volume_context<'a>(
        pixel_x: u32,
        pixel_z: u32,
        block_heights: &'a [i32],
        padding: u32,
    ) -> BlockVolumeContext<'a> {
        BlockVolumeContext {
            pixel_x,
            pixel_z,
            pixels_per_block: 4,
            blocks_per_pixel: 1,
            block_heights,
            block_aux: &[],
            grid_width: 5,
            grid_height: 5,
            grid_padding: padding,
        }
    }

    #[test]
    fn block_volume_default_preserves_surface_lighting() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let heights = uniform_neighbor_heights(64, 72);
        let mut surface = SurfaceRenderOptions {
            block_boundaries: BlockBoundaryRenderOptions::off(),
            ..SurfaceRenderOptions::default()
        };
        surface.lighting.edge_relief_strength = 0.0;
        let block_heights = vec![64_i32; 25];
        let baseline = surface_lit_color(&palette, base, heights, 0, surface, None, None);
        let with_volume_context = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            surface,
            Some(test_volume_context(3, 0, &block_heights, 2)),
            None,
        );

        assert_eq!(baseline, with_volume_context);
    }

    #[test]
    fn block_volume_flat_terrain_stays_stable() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let surface = SurfaceRenderOptions {
            lighting: TerrainLightingOptions::off(),
            block_boundaries: BlockBoundaryRenderOptions::off(),
            block_volume: test_block_volume_options(),
            ..SurfaceRenderOptions::default()
        };
        let block_heights = vec![64_i32; 25];
        let shaded = surface_lit_color(
            &palette,
            base,
            uniform_neighbor_heights(64, 64),
            0,
            surface,
            Some(test_volume_context(3, 0, &block_heights, 2)),
            None,
        );

        assert_eq!(base, shaded);
    }

    #[test]
    fn block_volume_contact_shadow_darkens_near_higher_neighbor() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let surface = SurfaceRenderOptions {
            lighting: TerrainLightingOptions::off(),
            block_boundaries: BlockBoundaryRenderOptions::off(),
            block_volume: test_block_volume_options(),
            ..SurfaceRenderOptions::default()
        };
        let heights = TerrainHeightNeighborhood {
            center: 64,
            north_west: 64,
            north: 64,
            north_east: 80,
            west: 64,
            east: 80,
            south_west: 64,
            south: 64,
            south_east: 80,
        };
        let mut block_heights = vec![64_i32; 25];
        block_heights[12] = 64;
        block_heights[13] = 80;
        let shaded = surface_lit_color(
            &palette,
            base,
            heights,
            0,
            surface,
            Some(test_volume_context(3, 0, &block_heights, 2)),
            None,
        );

        assert!(shaded.red < base.red);
    }

    #[test]
    fn block_volume_cast_shadow_uses_nearby_higher_block() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(140, 140, 140, 255);
        let surface = SurfaceRenderOptions {
            lighting: TerrainLightingOptions {
                enabled: false,
                light_azimuth_degrees: 315.0,
                ..TerrainLightingOptions::off()
            },
            block_boundaries: BlockBoundaryRenderOptions::off(),
            block_volume: test_block_volume_options(),
            ..SurfaceRenderOptions::default()
        };
        let mut block_heights = vec![64_i32; 25];
        block_heights[12] = 64;
        block_heights[6] = 92;
        let shaded = surface_lit_color(
            &palette,
            base,
            uniform_neighbor_heights(64, 64),
            0,
            surface,
            Some(test_volume_context(1, 1, &block_heights, 2)),
            None,
        );

        assert!(shaded.red < base.red);
    }

    #[test]
    fn atlas_renderer_classifies_common_materials() {
        assert_eq!(
            classify_surface_material("minecraft:oak_leaves"),
            SurfaceMaterialId::Foliage
        );
        assert_eq!(
            classify_surface_material("minecraft:snow"),
            SurfaceMaterialId::Snow
        );
        assert_eq!(
            classify_surface_material("minecraft:stone"),
            SurfaceMaterialId::Stone
        );
        assert_eq!(
            classify_surface_material("minecraft:grass_block"),
            SurfaceMaterialId::Grass
        );
    }

    fn map_atlas_test_surface() -> SurfaceRenderOptions {
        SurfaceRenderOptions {
            lighting: TerrainLightingOptions::soft(),
            block_boundaries: BlockBoundaryRenderOptions::off(),
            block_volume: BlockVolumeRenderOptions::off(),
            atlas: AtlasRenderOptions {
                enabled: true,
                texture_detail_strength: 0.34,
                height_contour_interval: 4,
                height_contour_strength: 0.92,
                slope_hatching_strength: 0.44,
                forest_canopy_strength: 0.78,
                snow_ridge_strength: 0.86,
                water_grid_strength: 0.14,
                shoreline_shadow_strength: 0.58,
                chunk_grid_strength: 0.10,
                material_edge_strength: 0.30,
                cast_shadow_strength: 0.50,
                ambient_occlusion_strength: 0.58,
            },
            ..SurfaceRenderOptions::default()
        }
    }

    fn map_atlas_edge_test_surface() -> SurfaceRenderOptions {
        let mut surface = map_atlas_test_surface();
        surface.lighting.shadow_strength = 0.0;
        surface.lighting.highlight_strength = 0.0;
        surface.atlas.texture_detail_strength = 0.0;
        surface.atlas.height_contour_strength = 0.0;
        surface.atlas.slope_hatching_strength = 0.0;
        surface.atlas.forest_canopy_strength = 0.0;
        surface.atlas.snow_ridge_strength = 0.0;
        surface.atlas.water_grid_strength = 0.0;
        surface.atlas.shoreline_shadow_strength = 0.0;
        surface.atlas.cast_shadow_strength = 0.0;
        surface.atlas.ambient_occlusion_strength = 0.0;
        surface
    }

    fn test_map_atlas_context<'a>(
        pixel_x: u32,
        pixel_z: u32,
        block_heights: &'a [i32],
        block_aux: &'a [u32],
        grid_width: u32,
        padding: u32,
    ) -> BlockVolumeContext<'a> {
        BlockVolumeContext {
            pixel_x,
            pixel_z,
            pixels_per_block: 4,
            blocks_per_pixel: 1,
            block_heights,
            block_aux,
            grid_width,
            grid_height: grid_width,
            grid_padding: padding,
        }
    }

    #[test]
    fn atlas_same_local_inputs_do_not_depend_on_world_position() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_test_surface();
        let heights = uniform_neighbor_heights(70, 70);
        let block_heights = vec![70_i32; 64];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 64];
        let first = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                1,
                1,
                &block_heights,
                &block_aux,
                8,
                2,
            )),
        );
        let second = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                5,
                1,
                &block_heights,
                &block_aux,
                8,
                2,
            )),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn atlas_flat_grass_detail_stays_clean() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_test_surface();
        let heights = uniform_neighbor_heights(70, 70);
        let block_heights = vec![70_i32; 25];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 25];
        let low = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                0,
                0,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        let high = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                2,
                2,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        assert!(low.red.abs_diff(high.red) <= 16);
        assert!(low.green.abs_diff(high.green) <= 16);
    }

    #[test]
    fn atlas_flat_grass_has_no_pixel_atlas_grain() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_test_surface();
        let heights = uniform_neighbor_heights(70, 70);
        let block_heights = vec![70_i32; 25];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 25];
        let aux = pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0);
        let mut samples = Vec::new();
        for pixel_z in 0..4 {
            for pixel_x in 0..4 {
                samples.push(apply_atlas_shading(
                    base,
                    heights,
                    0,
                    surface,
                    aux,
                    Some(test_map_atlas_context(
                        pixel_x,
                        pixel_z,
                        &block_heights,
                        &block_aux,
                        5,
                        2,
                    )),
                ));
            }
        }
        let min_green = samples.iter().map(|color| color.green).min().unwrap_or(0);
        let max_green = samples.iter().map(|color| color.green).max().unwrap_or(0);
        assert!(max_green.abs_diff(min_green) <= 4);
    }

    #[test]
    fn atlas_same_height_connected_blocks_do_not_draw_inner_shadow() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let mut surface = map_atlas_edge_test_surface();
        surface.atlas.material_edge_strength = 1.0;
        surface.atlas.chunk_grid_strength = 1.0;
        let heights = uniform_neighbor_heights(70, 70);
        let block_heights = vec![70_i32; 25];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 25];
        let edge = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                0,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        let center = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                1,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        assert_eq!(edge, center);
    }

    #[test]
    fn atlas_height_break_shadow_stays_on_actual_edge() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_edge_test_surface();
        let heights = TerrainHeightNeighborhood {
            center: 70,
            north_west: 70,
            north: 70,
            north_east: 60,
            west: 70,
            east: 60,
            south_west: 70,
            south: 70,
            south_east: 60,
        };
        let block_heights = vec![70_i32; 25];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 25];
        let edge = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                3,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        let center = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                1,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        assert!(edge.green < center.green);
    }

    #[test]
    fn atlas_lower_block_gets_contact_shadow_only_on_high_edge() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_edge_test_surface();
        let heights = TerrainHeightNeighborhood {
            center: 60,
            north_west: 60,
            north: 60,
            north_east: 70,
            west: 60,
            east: 70,
            south_west: 60,
            south: 60,
            south_east: 70,
        };
        let block_heights = vec![60_i32; 25];
        let block_aux = vec![pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0); 25];
        let high_edge = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                3,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        let center = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            Some(test_map_atlas_context(
                1,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        assert!(high_edge.green < center.green);
    }

    #[test]
    fn atlas_dense_foliage_interior_has_no_pixel_canopy_grain() {
        let base = RgbaColor::new(58, 132, 52, 255);
        let surface = map_atlas_test_surface();
        let heights = uniform_neighbor_heights(82, 82);
        let block_heights = vec![82_i32; 25];
        let block_aux =
            vec![pack_atlas_aux(0, SurfaceMaterialId::Foliage, ATLAS_SHAPE_FOLIAGE, 0); 25];
        let aux = pack_atlas_aux(0, SurfaceMaterialId::Foliage, ATLAS_SHAPE_FOLIAGE, 0);
        let mut samples = Vec::new();
        for pixel_z in 0..4 {
            for pixel_x in 0..4 {
                samples.push(apply_atlas_shading(
                    base,
                    heights,
                    0,
                    surface,
                    aux,
                    Some(test_map_atlas_context(
                        pixel_x,
                        pixel_z,
                        &block_heights,
                        &block_aux,
                        5,
                        2,
                    )),
                ));
            }
        }
        let min_green = samples.iter().map(|color| color.green).min().unwrap_or(0);
        let max_green = samples.iter().map(|color| color.green).max().unwrap_or(0);
        assert!(max_green.abs_diff(min_green) <= 3);
    }

    #[test]
    fn atlas_foliage_canopy_edge_keeps_cluster_shape() {
        let base = RgbaColor::new(58, 132, 52, 255);
        let surface = map_atlas_test_surface();
        let heights = uniform_neighbor_heights(82, 82);
        let block_heights = vec![82_i32; 25];
        let foliage = pack_atlas_aux(0, SurfaceMaterialId::Foliage, ATLAS_SHAPE_FOLIAGE, 0);
        let grass = pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0);
        let mut block_aux = vec![foliage; 25];
        block_aux[2 * 5 + 1] = grass;
        let edge = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            foliage,
            Some(test_map_atlas_context(
                0,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        let interior = apply_atlas_shading(
            base,
            heights,
            0,
            surface,
            foliage,
            Some(test_map_atlas_context(
                1,
                1,
                &block_heights,
                &block_aux,
                5,
                2,
            )),
        );
        assert!(rgba_color_distance(edge, interior) >= 4);
    }

    #[test]
    fn atlas_snow_slope_creates_ridge_detail() {
        let base = RgbaColor::new(235, 238, 238, 255);
        let surface = map_atlas_test_surface();
        let flat = apply_atlas_shading(
            base,
            uniform_neighbor_heights(92, 92),
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Snow, ATLAS_SHAPE_SOLID, 0),
            None,
        );
        let sloped = apply_atlas_shading(
            base,
            uniform_neighbor_heights(92, 84),
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Snow, ATLAS_SHAPE_SOLID, 0),
            None,
        );
        assert!(sloped.red < flat.red);
    }

    #[test]
    fn atlas_stone_slope_contour_is_stronger_than_flat() {
        let base = RgbaColor::new(124, 128, 124, 255);
        let surface = map_atlas_test_surface();
        let flat = apply_atlas_shading(
            base,
            uniform_neighbor_heights(80, 80),
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Stone, ATLAS_SHAPE_SOLID, 0),
            None,
        );
        let sloped = apply_atlas_shading(
            base,
            TerrainHeightNeighborhood {
                center: 80,
                north_west: 86,
                north: 84,
                north_east: 82,
                west: 82,
                east: 76,
                south_west: 78,
                south: 76,
                south_east: 74,
            },
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Stone, ATLAS_SHAPE_SOLID, 0),
            None,
        );
        assert!(flat.green > sloped.green);
        assert!(flat.green.abs_diff(sloped.green) >= 6);
    }

    #[test]
    fn atlas_grass_slope_contour_is_visible_but_soft() {
        let base = RgbaColor::new(94, 142, 86, 255);
        let surface = map_atlas_test_surface();
        let flat = apply_atlas_shading(
            base,
            uniform_neighbor_heights(80, 80),
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            None,
        );
        let sloped = apply_atlas_shading(
            base,
            TerrainHeightNeighborhood {
                center: 80,
                north_west: 86,
                north: 84,
                north_east: 82,
                west: 82,
                east: 76,
                south_west: 78,
                south: 76,
                south_east: 74,
            },
            0,
            surface,
            pack_atlas_aux(0, SurfaceMaterialId::Grass, 0, 0),
            None,
        );
        let delta = flat.green.abs_diff(sloped.green);
        assert!(sloped.green < flat.green);
        assert!((4..=22).contains(&delta), "grass slope delta was {delta}");
    }

    #[test]
    fn atlas_surface_hash_changes_with_options() {
        let base = SurfaceRenderOptions::default();
        let mut changed = base;
        changed.atlas.texture_detail_strength += 0.1;
        assert_ne!(surface_options_hash(base), surface_options_hash(changed));
    }

    #[test]
    fn shadow_limit_caps_underwater_relief() {
        let base = RgbaColor::new(100, 100, 100, 255);
        let mut capped = TerrainLightingOptions::strong();
        capped.max_shadow = 1.0;
        let normal = TerrainLightingOptions::strong();

        let capped_water = terrain_lit_color(base, cardinal_heights(72, 64, 64, 64), 1, capped);
        let normal_water = terrain_lit_color(base, cardinal_heights(72, 64, 64, 64), 1, normal);

        assert!(capped_water.red > normal_water.red);
    }

    #[test]
    fn terrain_lighting_preserves_transparency() {
        let base = RgbaColor::new(100, 100, 100, 0);
        let shaded = terrain_lit_color(
            base,
            cardinal_heights(64, 72, 64, 64),
            0,
            TerrainLightingOptions::strong(),
        );
        assert_eq!(shaded, base);
    }

    #[test]
    fn terrain_lighting_scope_matches_surface_and_heightmap() {
        let surface = SurfaceRenderOptions::default();
        assert!(lighting_enabled_for(RenderMode::SurfaceBlocks, surface));
        assert!(lighting_enabled_for(RenderMode::HeightMap, surface));
        assert!(!lighting_enabled_for(RenderMode::Biome { y: 64 }, surface));
        assert!(!lighting_enabled_for(
            RenderMode::LayerBlocks { y: 64 },
            surface
        ));
        assert!(!lighting_enabled_for(
            RenderMode::CaveSlice { y: 32 },
            surface
        ));
    }

    #[test]
    fn fixed_y_layer_uses_xz_plane_not_vertical_slice() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(&ChunkKey::subchunk(pos, 4).encode(), &test_subchunk_bytes())
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:x_axis", RgbaColor::new(10, 0, 0, 255))
            .with_block_color("minecraft:z_axis", RgbaColor::new(0, 10, 0, 255));
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 2,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render layer");

        assert_eq!(&tile.rgba[0..4], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[4..8], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[8..12], &[0, 10, 0, 255]);
    }

    #[test]
    fn render_tiles_shared_bake_pipeline_preserves_output_order() {
        let storage = Arc::new(MemoryStorage::new());
        for (chunk_x, block_name) in [
            (0, "minecraft:first_batch_test"),
            (1, "minecraft:second_batch_test"),
        ] {
            let pos = ChunkPos {
                x: chunk_x,
                z: 0,
                dimension: Dimension::Overworld,
            };
            storage
                .put(
                    &ChunkKey::subchunk(pos, 4).encode(),
                    &test_surface_subchunk_bytes_with_values(
                        [("minecraft:air", 0_u16), (block_name, 1)],
                        |_, _, local_y| u16::from(local_y == 0),
                    ),
                )
                .expect("put subchunk");
        }
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(
            world,
            RenderPalette::default()
                .with_block_color(
                    "minecraft:first_batch_test",
                    RgbaColor::new(180, 20, 30, 255),
                )
                .with_block_color(
                    "minecraft:second_batch_test",
                    RgbaColor::new(20, 40, 190, 255),
                ),
        );
        let jobs = [1, 0, 1].map(|tile_x| RenderJob {
            coord: TileCoord {
                x: tile_x,
                z: 0,
                dimension: Dimension::Overworld,
            },
            tile_size: 16,
            ..RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::LayerBlocks { y: 64 },
            )
        });

        let tiles = renderer
            .render_tiles_blocking(
                jobs,
                RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Fixed(2),
                    ..RenderOptions::default()
                },
            )
            .expect("render batch");

        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].coord.x, 1);
        assert_eq!(tiles[1].coord.x, 0);
        assert_eq!(tiles[2].coord.x, 1);
        assert_eq!(
            pixel_rgba(&tiles[0].rgba, tiles[0].width, 0, 0),
            [20, 40, 190, 255]
        );
        assert_eq!(
            pixel_rgba(&tiles[1].rgba, tiles[1].width, 0, 0),
            [180, 20, 30, 255]
        );
        assert_eq!(tiles[0].rgba, tiles[2].rgba);
    }

    #[test]
    fn clipped_region_bake_keeps_full_region_origin_for_chunk_offsets() {
        let storage = Arc::new(MemoryStorage::new());
        let pos = ChunkPos {
            x: -1,
            z: -1,
            dimension: Dimension::Overworld,
        };
        let block_name = "minecraft:signature_clipped_region";
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_uniform_layer_subchunk_bytes(block_name),
            )
            .expect("put signature subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette =
            RenderPalette::default().with_block_color(block_name, RgbaColor::new(11, 77, 199, 255));
        let renderer = MapRenderer::new(world, palette);
        let region_layout = RegionLayout {
            chunks_per_region: 4,
        };
        let coord = RegionCoord::from_chunk(pos, region_layout);
        let clipped = ChunkRegion::new(Dimension::Overworld, -1, -1, -1, -1);

        let bake = renderer
            .bake_region_chunk_region_blocking(
                coord,
                clipped,
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    backend: RenderBackend::Cpu,
                    region_layout,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
                RenderMode::LayerBlocks { y: 64 },
            )
            .expect("bake clipped region");

        assert_eq!(bake.chunk_region, coord.chunk_region(region_layout));
        assert_eq!(bake.chunks_copied, 1);
        assert_eq!(bake.chunks_out_of_bounds, 0);
        assert_eq!(
            bake.color_at_chunk_local(pos, 0, 0)
                .expect("signature pixel")
                .to_array(),
            [11, 77, 199, 255]
        );
    }

    #[test]
    fn coordinate_signature_matches_direct_shared_region_and_session_paths() {
        let storage = Arc::new(MemoryStorage::new());
        let mut palette = RenderPalette::default();
        let mut expected = Vec::new();

        for (index, (chunk_x, chunk_z)) in [(-2, -2), (-1, -2), (-2, -1), (-1, -1)]
            .into_iter()
            .enumerate()
        {
            let pos = ChunkPos {
                x: chunk_x,
                z: chunk_z,
                dimension: Dimension::Overworld,
            };
            let base = signature_block_name(chunk_x, chunk_z, "base");
            let tl = signature_block_name(chunk_x, chunk_z, "tl");
            let tr = signature_block_name(chunk_x, chunk_z, "tr");
            let bl = signature_block_name(chunk_x, chunk_z, "bl");
            let br = signature_block_name(chunk_x, chunk_z, "br");
            let base_color = signature_color(index, 0);
            let top_left_color = signature_color(index, 1);
            let top_right_color = signature_color(index, 2);
            let bottom_left_color = signature_color(index, 3);
            let bottom_right_color = signature_color(index, 4);
            for (name, color) in [
                (&base, base_color),
                (&tl, top_left_color),
                (&tr, top_right_color),
                (&bl, bottom_left_color),
                (&br, bottom_right_color),
            ] {
                palette = palette.with_block_color(name, color);
            }
            storage
                .put(
                    &ChunkKey::subchunk(pos, 4).encode(),
                    &test_signature_layer_subchunk_bytes(&base, &tl, &tr, &bl, &br),
                )
                .expect("put signature subchunk");

            let tile_x = u32::try_from((chunk_x - -2) * 16).expect("tile x");
            let tile_z = u32::try_from((chunk_z - -2) * 16).expect("tile z");
            expected.extend_from_slice(&[
                (tile_x, tile_z, top_left_color.to_array()),
                (tile_x + 15, tile_z, top_right_color.to_array()),
                (tile_x, tile_z + 15, bottom_left_color.to_array()),
                (tile_x + 15, tile_z + 15, bottom_right_color.to_array()),
                (tile_x + 8, tile_z + 8, base_color.to_array()),
            ]);
        }

        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, palette);
        let layout = RenderLayout {
            chunks_per_tile: 2,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
        };
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: -1,
                z: -1,
                dimension: Dimension::Overworld,
            },
            RenderMode::LayerBlocks { y: 64 },
            layout,
        )
        .expect("signature job");
        let base_options = RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            region_layout: RegionLayout {
                chunks_per_region: 1,
            },
            memory_budget: RenderMemoryBudget::Disabled,
            ..RenderOptions::default()
        };

        let direct = renderer
            .render_tile_with_options_blocking(
                job.clone(),
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct render");
        assert_signature_pixels("direct", &direct, &expected);

        let shared = renderer
            .render_tiles_blocking(
                vec![job.clone()],
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options.clone()
                },
            )
            .expect("shared render")
            .pop()
            .expect("shared tile");
        assert_signature_pixels("shared", &shared, &expected);
        assert_eq!(direct.rgba, shared.rgba);

        let planned = PlannedTile {
            job: job.clone(),
            region: ChunkRegion::new(Dimension::Overworld, -2, -2, -1, -1),
            layout,
            chunk_positions: None,
        };
        let region_tiles = Arc::new(Mutex::new(Vec::new()));
        let result = renderer
            .render_tiles_from_regions_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options.clone()
                },
                {
                    let region_tiles = Arc::clone(&region_tiles);
                    move |_planned, tile| {
                        region_tiles.lock().expect("region tile lock").push(tile);
                        Ok(())
                    }
                },
            )
            .expect("region render");
        assert_eq!(result.stats.region_chunks_copied, 4);
        assert_eq!(result.stats.region_chunks_out_of_bounds, 0);
        assert_eq!(result.stats.tile_missing_region_samples, 0);
        let region = region_tiles
            .lock()
            .expect("region tile lock")
            .pop()
            .expect("region tile");
        assert_signature_pixels("region", &region, &expected);
        assert_eq!(direct.rgba, region.rgba);

        let session_tiles = Arc::new(Mutex::new(Vec::new()));
        let session = MapRenderSession::new(
            renderer,
            MapRenderSessionConfig {
                cull_missing_chunks: false,
                ..MapRenderSessionConfig::default()
            },
        );
        let session_result = session
            .render_web_tiles_streaming_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    cache_policy: RenderCachePolicy::Bypass,
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options
                },
                {
                    let session_tiles = Arc::clone(&session_tiles);
                    move |event| {
                        if let TileStreamEvent::Ready {
                            tile,
                            source: TileReadySource::Render,
                            ..
                        } = event
                        {
                            session_tiles.lock().expect("session tile lock").push(tile);
                        }
                        Ok(())
                    }
                },
            )
            .expect("session render");
        assert_eq!(session_result.stats.region_chunks_copied, 4);
        assert_eq!(session_result.stats.tile_missing_region_samples, 0);
        let session_tile = session_tiles
            .lock()
            .expect("session tile lock")
            .pop()
            .expect("session tile");
        assert_signature_pixels("session", &session_tile, &expected);
        assert_eq!(direct.rgba, session_tile.rgba);
    }

    #[test]
    fn pixels_per_block_renders_repeated_source_blocks() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(&ChunkKey::subchunk(pos, 4).encode(), &test_subchunk_bytes())
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:x_axis", RgbaColor::new(10, 0, 0, 255))
            .with_block_color("minecraft:z_axis", RgbaColor::new(0, 10, 0, 255));
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 4,
                    pixels_per_block: 2,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render layer");

        assert_eq!(&tile.rgba[0..4], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[4..8], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[16..20], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[20..24], &[10, 0, 0, 255]);
        assert_eq!(&tile.rgba[32..36], &[0, 10, 0, 255]);
    }

    #[test]
    fn surface_blocks_blend_thin_overlay_with_support_block() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:grass_block", 2),
                        ("minecraft:stone_button", 3),
                    ],
                    |_, _, local_y| match local_y {
                        0 => 1,
                        1 => 2,
                        2 => 3,
                        _ => 0,
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:stone_button", RgbaColor::new(90, 90, 90, 255))
            .with_block_color("minecraft:grass_block", RgbaColor::new(20, 200, 20, 255));
        let support = palette.surface_block_color("minecraft:grass_block", None, false);
        let overlay = palette.surface_block_color("minecraft:stone_button", None, false);
        let expected = alpha_blend_surface(
            overlay,
            support,
            surface_overlay_alpha("minecraft:stone_button").expect("button overlay alpha"),
        )
        .to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render surface overlay");

        assert_eq!(&tile.rgba[0..4], &expected);
        assert_ne!(&tile.rgba[0..4], &support.to_array());
        assert_ne!(&tile.rgba[0..4], &overlay.to_array());
    }

    #[test]
    fn surface_blocks_keep_bedrock_grass_block_id() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:dirt", 1),
                        ("minecraft:grass", 2),
                    ],
                    |_, _, local_y| match local_y {
                        0 => 1,
                        1 => 2,
                        _ => 0,
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:dirt", RgbaColor::new(130, 90, 55, 255))
            .with_block_color("minecraft:grass", RgbaColor::new(20, 200, 20, 255));
        let expected = palette
            .surface_block_color("minecraft:grass", None, false)
            .to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render bedrock grass");

        assert_eq!(&tile.rgba[0..4], &expected);
        assert!(tile.rgba[1] > tile.rgba[0], "grass should stay green");
    }

    #[test]
    fn surface_blocks_show_common_thin_overlays() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:grass_block", 1),
                        ("minecraft:short_grass", 2),
                        ("minecraft:poppy", 3),
                        ("minecraft:rail", 4),
                        ("minecraft:oak_pressure_plate", 5),
                    ],
                    |local_x, _, local_y| match local_y {
                        0 => 1,
                        1 => match local_x {
                            0 => 2,
                            1 => 3,
                            2 => 4,
                            _ => 5,
                        },
                        _ => 0,
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:grass_block", RgbaColor::new(20, 180, 20, 255))
            .with_block_color("minecraft:short_grass", RgbaColor::new(40, 240, 40, 255))
            .with_block_color("minecraft:poppy", RgbaColor::new(230, 40, 30, 255))
            .with_block_color("minecraft:rail", RgbaColor::new(150, 130, 80, 255))
            .with_block_color(
                "minecraft:oak_pressure_plate",
                RgbaColor::new(160, 110, 55, 255),
            );
        let support = palette.surface_block_color("minecraft:grass_block", None, false);
        let expected = [
            ("minecraft:short_grass", 0_u32),
            ("minecraft:poppy", 1),
            ("minecraft:rail", 2),
            ("minecraft:oak_pressure_plate", 3),
        ]
        .map(|(name, x)| {
            let overlay = palette.surface_block_color(name, None, false);
            let color = alpha_blend_surface(
                overlay,
                support,
                surface_overlay_alpha(name).expect("overlay alpha"),
            )
            .to_array();
            (x, color)
        });
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 4,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render thin overlays");

        for (x, color) in expected {
            let actual = pixel_rgba(&tile.rgba, tile.width, x, 0);
            assert_eq!(actual, color);
            assert_ne!(actual, support.to_array());
        }
    }

    #[test]
    fn surface_blocks_keep_carpet_building_details() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:white_concrete", 2),
                        ("minecraft:gray_carpet", 3),
                    ],
                    |_, _, local_y| match local_y {
                        0 => 1,
                        1 => 2,
                        2 => 3,
                        _ => 0,
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color(
                "minecraft:white_concrete",
                RgbaColor::new(220, 220, 220, 255),
            )
            .with_block_color("minecraft:gray_carpet", RgbaColor::new(80, 80, 80, 255));
        let expected = palette.block_color("minecraft:gray_carpet").to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render carpet detail");

        assert_eq!(&tile.rgba[0..4], &expected);
    }

    #[test]
    fn surface_blocks_per_pixel_averages_covered_blocks() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_top_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:green_test", 2),
                    ],
                    |local_x, local_z| {
                        if local_x == 0 && local_z == 0 { 1 } else { 2 }
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:green_test", RgbaColor::new(20, 200, 20, 255));
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    scale: 16,
                    pixels_per_block: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render downsampled surface");

        assert!(tile.rgba[1] > 180, "covered grass should dominate");
        assert_ne!(&tile.rgba[0..4], &[10, 10, 10, 255]);
    }

    #[test]
    fn region_surface_blocks_per_pixel_preserves_partial_cross() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes_with_top_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:light_platform", 1),
                        ("minecraft:dark_platform", 2),
                    ],
                    |local_x, local_z| {
                        if (6..=9).contains(&local_x) || (6..=9).contains(&local_z) {
                            2
                        } else {
                            1
                        }
                    },
                ),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color(
                "minecraft:light_platform",
                RgbaColor::new(200, 200, 200, 255),
            )
            .with_block_color("minecraft:dark_platform", RgbaColor::new(80, 80, 80, 255));
        let renderer = MapRenderer::new(world, palette);
        let tiles = renderer
            .render_region_tiles_blocking(
                ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
                RenderMode::SurfaceBlocks,
                ChunkTileLayout {
                    chunks_per_tile: 1,
                    blocks_per_pixel: 4,
                    pixels_per_block: 1,
                },
                RenderOptions {
                    format: ImageFormat::Rgba,
                    backend: RenderBackend::Cpu,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render region surface tiles");
        let tile = tiles.tiles.first().expect("one tile should render");

        let corner = pixel_rgba(&tile.rgba, tile.width, 0, 0);
        let partial_arm = pixel_rgba(&tile.rgba, tile.width, 1, 0);
        let center = pixel_rgba(&tile.rgba, tile.width, 1, 1);
        assert_eq!(corner, [200, 200, 200, 255]);
        assert!(
            (110..=170).contains(&partial_arm[0]),
            "partial cross arm should be averaged, got {partial_arm:?}"
        );
        assert!(
            center[0] < partial_arm[0],
            "cross center should stay darker than an edge arm"
        );
    }

    #[test]
    fn heightmap_and_cave_slice_render_rgba() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        for mode in [RenderMode::HeightMap, RenderMode::CaveSlice { y: 32 }] {
            let tile = renderer
                .render_tile_with_options_blocking(
                    RenderJob {
                        tile_size: 2,
                        ..RenderJob::new(
                            TileCoord {
                                x: 0,
                                z: 0,
                                dimension: Dimension::Overworld,
                            },
                            mode,
                        )
                    },
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render diagnostic mode");
            assert_eq!(tile.rgba.len(), 16);
        }
    }

    #[test]
    fn heightmap_uses_computed_surface_and_raw_heightmap_keeps_diagnostic_data() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(0, 4),
            )
            .expect("put misleading raw height");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:grass_block", 2),
                    ],
                    |_, _, local_y| if local_y == 0 { 1 } else { 2 },
                ),
            )
            .expect("put exact surface subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let options = RenderOptions {
            format: ImageFormat::Rgba,
            threading: RenderThreadingOptions::Single,
            surface: SurfaceRenderOptions {
                height_shading: false,
                ..SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        };
        let computed = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::HeightMap,
                    )
                },
                &options,
            )
            .expect("render computed heightmap");
        let raw = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::RawHeightMap,
                    )
                },
                &options,
            )
            .expect("render raw heightmap");

        assert_ne!(
            pixel_rgba(&computed.rgba, computed.width, 0, 0),
            pixel_rgba(&raw.rgba, raw.width, 0, 0)
        );
        assert_eq!(
            pixel_rgba(&computed.rgba, computed.width, 0, 0),
            RenderPalette::default()
                .height_color(15, -64, 320)
                .to_array()
        );
        assert_eq!(
            pixel_rgba(&raw.rgba, raw.width, 0, 0),
            RenderPalette::default()
                .height_color(0, -64, 320)
                .to_array()
        );
    }

    #[test]
    fn heightmap_uses_surface_y_even_when_surface_block_color_is_suppressed() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(0, 4),
            )
            .expect("put misleading raw height");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:unmapped_roof_block", 1),
                    ],
                    |_, _, local_y| if local_y == 15 { 1 } else { 0 },
                ),
            )
            .expect("put unknown surface subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default();
        assert!(!palette.has_block_color("minecraft:unmapped_roof_block"));
        let expected_height = palette.height_color(15, -64, 320).to_array();
        let renderer = MapRenderer::new(world, palette);
        let options = RenderOptions {
            format: ImageFormat::Rgba,
            threading: RenderThreadingOptions::Single,
            surface: SurfaceRenderOptions {
                render_unknown_blocks: false,
                height_shading: false,
                ..SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        };

        let heightmap = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 16,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::HeightMap,
                    )
                },
                &options,
            )
            .expect("render computed heightmap");

        assert_eq!(
            pixel_rgba(&heightmap.rgba, heightmap.width, 0, 0),
            expected_height
        );
    }

    #[test]
    fn surface_payload_keeps_height_when_unknown_block_color_is_suppressed() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:unmapped_roof_block", 1),
                    ],
                    |_, _, local_y| u16::from(local_y == 15),
                ),
            )
            .expect("put unknown surface subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let bake = renderer
            .bake_chunk_blocking(
                pos,
                BakeOptions {
                    mode: RenderMode::SurfaceBlocks,
                    surface: SurfaceRenderOptions {
                        render_unknown_blocks: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                },
            )
            .expect("bake surface chunk");
        let ChunkBakePayload::SurfaceAtlas(surface) = bake.payload else {
            panic!("expected surface payload");
        };

        assert_eq!(
            surface.colors.color_at(0, 0),
            Some(RenderPalette::default().void_color())
        );
        assert_eq!(surface.heights.height_at(0, 0), Some(15));
        assert_eq!(surface.relief_heights.height_at(0, 0), Some(15));
    }

    #[test]
    fn missing_fixed_y_layer_renders_transparent_pixels() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
        let diagnostics_sink = RenderDiagnosticsSink::new({
            let diagnostics = Arc::clone(&diagnostics);
            move |value| {
                diagnostics.lock().expect("diagnostics lock").add(value);
            }
        });
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::LayerBlocks { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    diagnostics: Some(diagnostics_sink),
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render missing layer");

        assert_eq!(&tile.rgba[0..4], &[0, 0, 0, 0]);
        let diagnostics = diagnostics.lock().expect("diagnostics lock");
        assert_eq!(diagnostics.transparent_pixels, 256);
        assert_eq!(diagnostics.purple_error_pixels, 0);
    }

    #[test]
    fn missing_biome_chunk_renders_transparent_not_unknown_biome() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::Biome { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render missing biome chunk");

        assert_eq!(&tile.rgba[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn biome_layer_falls_back_to_top_non_empty_biome() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
                &test_data3d_single_biome_bytes(4),
            )
            .expect("put Data3D");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let expected = RenderPalette::default().biome_color(4).to_array();
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::Biome { y: 64 },
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render biome fallback");

        assert_eq!(&tile.rgba[0..4], &expected);
    }

    #[test]
    fn surface_blocks_uses_highest_non_air_block() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:grass_block", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:grass_block", RgbaColor::new(20, 200, 20, 255));
        let expected = palette
            .surface_block_color("minecraft:grass_block", None, false)
            .to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render surface");

        assert_eq!(&tile.rgba[0..4], &expected);
    }

    #[test]
    fn surface_blocks_without_column_samples_does_not_rescan_subchunks() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:grass_block", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let data = world
            .query_chunk_data_blocking(
                pos,
                ChunkLoadOptions {
                    data_request: ChunkDataRequest::new().height_map(),
                    ..ChunkLoadOptions::default()
                },
            )
            .expect("load render chunk without exact samples");
        assert!(data.column_samples.is_none());
        let renderer = MapRenderer::new(world, RenderPalette::default());

        let bake = renderer
            .bake_chunk_data(
                data,
                BakeOptions {
                    mode: RenderMode::SurfaceBlocks,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                },
            )
            .expect("bake surface without samples");

        let ChunkBakePayload::SurfaceAtlas(surface) = bake.payload else {
            panic!("expected surface payload");
        };
        assert_eq!(
            surface.colors.color_at(0, 0),
            Some(RenderPalette::default().missing_chunk_color())
        );
        assert_eq!(surface.heights.height_at(0, 0), None);
        assert!(bake.diagnostics.transparent_pixels > 0);
    }

    #[test]
    fn surface_blocks_applies_distinct_biome_tint_to_grass() {
        let storage = Arc::new(MemoryStorage::new());
        for (chunk_x, biome) in [(0, 2), (1, 21)] {
            let pos = ChunkPos {
                x: chunk_x,
                z: 0,
                dimension: Dimension::Overworld,
            };
            storage
                .put(
                    &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                    &test_data2d_bytes(65, biome),
                )
                .expect("put data2d");
            storage
                .put(
                    &ChunkKey::subchunk(pos, 4).encode(),
                    &test_surface_subchunk_bytes([
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:grass_block", 2),
                    ]),
                )
                .expect("put subchunk");
        }
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default();
        let expected_desert = palette
            .surface_block_color("minecraft:grass_block", Some(2), true)
            .to_array();
        let expected_jungle = palette
            .surface_block_color("minecraft:grass_block", Some(21), true)
            .to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 32,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render surface biome tint");

        let desert = pixel_rgba(&tile.rgba, tile.width, 0, 0);
        let jungle = pixel_rgba(&tile.rgba, tile.width, 16, 0);
        assert_eq!(desert, expected_desert);
        assert_eq!(jungle, expected_jungle);
        assert!(desert[0] > desert[1] && desert[1] > desert[2]);
        assert!(jungle[1] > jungle[0] && jungle[1] > jungle[2]);
        assert!(rgba_distance(desert, jungle) >= 80);
    }

    #[test]
    fn surface_blocks_scan_above_heightmap_for_tree_canopy() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:stone", 2),
                ]),
            )
            .expect("put raw-height subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 5).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:oak_leaves", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:oak_leaves", RgbaColor::new(20, 120, 20, 255));
        let expected = palette
            .surface_block_color("minecraft:oak_leaves", None, false)
            .to_array();
        let renderer = MapRenderer::new(world, palette);
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render surface");

        assert_eq!(&tile.rgba[0..4], &expected);
    }

    #[test]
    fn surface_top_layer_matches_direct_shared_region_and_session_paths() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 4),
            )
            .expect("put low data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:oak_leaves", 2),
                ]),
            )
            .expect("put top-layer subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:oak_leaves", RgbaColor::new(20, 120, 20, 255));
        let expected = palette
            .surface_block_color("minecraft:oak_leaves", None, false)
            .to_array();
        let renderer = MapRenderer::new(world, palette);
        let layout = RenderLayout {
            chunks_per_tile: 1,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
        };
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("surface top layer job");
        let base_options = RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            region_layout: RegionLayout {
                chunks_per_region: 1,
            },
            memory_budget: RenderMemoryBudget::Disabled,
            surface: SurfaceRenderOptions {
                biome_tint: false,
                height_shading: false,
                block_boundaries: BlockBoundaryRenderOptions::off(),
                ..SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        };

        let direct = renderer
            .render_tile_with_options_blocking(
                job.clone(),
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct surface render");
        assert_eq!(pixel_rgba(&direct.rgba, direct.width, 0, 0), expected);

        let shared = renderer
            .render_tiles_blocking(
                vec![job.clone()],
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options.clone()
                },
            )
            .expect("shared surface render")
            .pop()
            .expect("shared tile");
        assert_eq!(direct.rgba, shared.rgba);

        let planned = PlannedTile {
            job: job.clone(),
            region: ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            layout,
            chunk_positions: None,
        };
        let region_tiles = Arc::new(Mutex::new(Vec::new()));
        let result = renderer
            .render_tiles_from_regions_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options.clone()
                },
                {
                    let region_tiles = Arc::clone(&region_tiles);
                    move |_planned, tile| {
                        region_tiles.lock().expect("region tile lock").push(tile);
                        Ok(())
                    }
                },
            )
            .expect("region surface render");
        assert_eq!(result.stats.tile_missing_region_samples, 0);
        let region = region_tiles
            .lock()
            .expect("region tile lock")
            .pop()
            .expect("region tile");
        assert_eq!(direct.rgba, region.rgba);

        let session_tiles = Arc::new(Mutex::new(Vec::new()));
        let session = MapRenderSession::new(
            renderer,
            MapRenderSessionConfig {
                cull_missing_chunks: false,
                ..MapRenderSessionConfig::default()
            },
        );
        let session_result = session
            .render_web_tiles_streaming_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    cache_policy: RenderCachePolicy::Bypass,
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options
                },
                {
                    let session_tiles = Arc::clone(&session_tiles);
                    move |event| {
                        if let TileStreamEvent::Ready {
                            tile,
                            source: TileReadySource::Render,
                            ..
                        } = event
                        {
                            session_tiles.lock().expect("session tile lock").push(tile);
                        }
                        Ok(())
                    }
                },
            )
            .expect("session surface render");
        assert_eq!(session_result.stats.tile_missing_region_samples, 0);
        let session_tile = session_tiles
            .lock()
            .expect("session tile lock")
            .pop()
            .expect("session tile");
        assert_eq!(direct.rgba, session_tile.rgba);
    }

    #[test]
    fn flat_world_surface_and_heightmap_use_highest_roof_across_paths() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(0, 4),
            )
            .expect("put stale low heightmap");
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_subchunk_bytes_with_values(
                    [
                        ("minecraft:air", 0_u16),
                        ("minecraft:stone", 1),
                        ("minecraft:stone", 2),
                    ],
                    |_, _, local_y| if local_y == 0 { 1 } else { 0 },
                ),
            )
            .expect("put flat ground subchunk");
        storage
            .put(
                &ChunkKey::subchunk(pos, 10).encode(),
                &test_surface_layered_subchunk_bytes(
                    [("minecraft:air", 0_u16)],
                    [("minecraft:air", 0_u16), ("minecraft:copper_block", 1)],
                    |_, _, _| 0,
                    |_, _, local_y| u16::from(local_y == 15),
                ),
            )
            .expect("put high roof subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:stone", RgbaColor::new(10, 10, 10, 255))
            .with_block_color("minecraft:copper_block", RgbaColor::new(190, 110, 60, 255));
        let expected_surface = palette
            .surface_block_color("minecraft:copper_block", None, false)
            .to_array();
        let expected_height = palette.height_color(175, -64, 320).to_array();
        let expected_raw_height = palette.height_color(0, -64, 320).to_array();
        let renderer = MapRenderer::new(world, palette);
        let layout = RenderLayout {
            chunks_per_tile: 1,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
        };
        let surface_job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("surface job");
        let height_job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::HeightMap,
            layout,
        )
        .expect("height job");
        let raw_height_job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::RawHeightMap,
            layout,
        )
        .expect("raw height job");
        let base_options = RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            region_layout: RegionLayout {
                chunks_per_region: 1,
            },
            memory_budget: RenderMemoryBudget::Disabled,
            surface: SurfaceRenderOptions {
                biome_tint: false,
                height_shading: false,
                block_boundaries: BlockBoundaryRenderOptions::off(),
                ..SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        };

        let surface = renderer
            .render_tile_with_options_blocking(
                surface_job.clone(),
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct surface render");
        assert_eq!(
            pixel_rgba(&surface.rgba, surface.width, 0, 0),
            expected_surface
        );

        let height = renderer
            .render_tile_with_options_blocking(
                height_job.clone(),
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct computed height render");
        assert_eq!(
            pixel_rgba(&height.rgba, height.width, 0, 0),
            expected_height
        );

        let raw_height = renderer
            .render_tile_with_options_blocking(
                raw_height_job,
                &RenderOptions {
                    threading: RenderThreadingOptions::Single,
                    ..base_options.clone()
                },
            )
            .expect("direct raw height render");
        assert_eq!(
            pixel_rgba(&raw_height.rgba, raw_height.width, 0, 0),
            expected_raw_height
        );

        let planned = PlannedTile {
            job: height_job.clone(),
            region: ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            layout,
            chunk_positions: None,
        };
        let region_tiles = Arc::new(Mutex::new(Vec::new()));
        let region_result = renderer
            .render_tiles_from_regions_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options.clone()
                },
                {
                    let region_tiles = Arc::clone(&region_tiles);
                    move |_planned, tile| {
                        region_tiles.lock().expect("region tile lock").push(tile);
                        Ok(())
                    }
                },
            )
            .expect("region height render");
        assert_eq!(region_result.stats.tile_missing_region_samples, 0);
        let region = region_tiles
            .lock()
            .expect("region tile lock")
            .pop()
            .expect("region tile");
        assert_eq!(height.rgba, region.rgba);

        let session_tiles = Arc::new(Mutex::new(Vec::new()));
        let session = MapRenderSession::new(
            renderer,
            MapRenderSessionConfig {
                cull_missing_chunks: false,
                ..MapRenderSessionConfig::default()
            },
        );
        let session_result = session
            .render_web_tiles_streaming_blocking(
                std::slice::from_ref(&planned),
                RenderOptions {
                    cache_policy: RenderCachePolicy::Bypass,
                    threading: RenderThreadingOptions::Fixed(2),
                    ..base_options
                },
                {
                    let session_tiles = Arc::clone(&session_tiles);
                    move |event| {
                        if let TileStreamEvent::Ready {
                            tile,
                            source: TileReadySource::Render,
                            ..
                        } = event
                        {
                            session_tiles.lock().expect("session tile lock").push(tile);
                        }
                        Ok(())
                    }
                },
            )
            .expect("session height render");
        assert_eq!(session_result.stats.tile_missing_region_samples, 0);
        let session_tile = session_tiles
            .lock()
            .expect("session tile lock")
            .pop()
            .expect("session tile");
        assert_eq!(height.rgba, session_tile.rgba);
    }

    #[test]
    fn surface_blocks_render_old_and_modern_cobblestone_slab_aliases_consistently() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(1, 4),
            )
            .expect("put heightmap");
        let old_cobblestone_slab = test_block_state(
            "minecraft:stone_slab",
            [
                ("stone_slab_type", NbtTag::String("cobblestone".to_string())),
                ("top_slot_bit", NbtTag::Byte(0)),
            ]
            .into_iter(),
        );
        let modern_cobblestone_slab = test_block_state(
            "minecraft:cobblestone_slab",
            [(
                "minecraft:vertical_half",
                NbtTag::String("bottom".to_string()),
            )]
            .into_iter(),
        );
        storage
            .put(
                &ChunkKey::subchunk(pos, 0).encode(),
                &test_surface_state_subchunk_bytes(
                    &[
                        test_block_state("minecraft:air", std::iter::empty()),
                        old_cobblestone_slab,
                        modern_cobblestone_slab,
                    ],
                    |local_x, _, local_y| {
                        if local_y == 1 {
                            if local_x < 8 { 1 } else { 2 }
                        } else {
                            0
                        }
                    },
                ),
            )
            .expect("put slab aliases");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob::chunk_tile(
                    TileCoord {
                        x: 0,
                        z: 0,
                        dimension: Dimension::Overworld,
                    },
                    RenderMode::SurfaceBlocks,
                    RenderLayout {
                        chunks_per_tile: 1,
                        blocks_per_pixel: 1,
                        pixels_per_block: 1,
                    },
                )
                .expect("surface job"),
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    backend: RenderBackend::Cpu,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        biome_tint: false,
                        height_shading: false,
                        block_boundaries: BlockBoundaryRenderOptions::off(),
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render surface");
        let expected = RenderPalette::default()
            .block_color("minecraft:cobblestone")
            .to_array();

        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 0, 0), expected);
        assert_eq!(pixel_rgba(&tile.rgba, tile.width, 8, 0), expected);
        assert_eq!(
            pixel_rgba(&tile.rgba, tile.width, 0, 0),
            pixel_rgba(&tile.rgba, tile.width, 8, 0)
        );
    }

    #[test]
    fn surface_water_blends_underlying_block() {
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
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:sand", 1),
                    ("minecraft:water", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render water surface");

        assert_eq!(tile.rgba[3], 255);
        assert_ne!(&tile.rgba[0..3], &[43, 92, 210]);
        assert_ne!(&tile.rgba[0..3], &[218, 210, 158]);
    }

    #[test]
    fn surface_blocks_use_banner_block_entity_base_color() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 1),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:standing_banner", 2),
                ]),
            )
            .expect("put subchunk");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
                &test_block_entity_bytes(NbtTag::Compound(IndexMap::from([
                    ("id".to_string(), NbtTag::String("Banner".to_string())),
                    ("x".to_string(), NbtTag::Int(0)),
                    ("y".to_string(), NbtTag::Int(65)),
                    ("z".to_string(), NbtTag::Int(0)),
                    ("Base".to_string(), NbtTag::Int(14)),
                ]))),
            )
            .expect("put banner block entity");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let mut palette = RenderPalette::default();
        palette
            .merge_json_str(
                r#"{
                    "blocks": {
                        "minecraft:standing_banner": {
                            "default": [255, 255, 255, 255],
                            "variant_colors": {
                                "banner_base_red": [200, 20, 10, 255],
                                "banner_base_white": [255, 255, 255, 255]
                            }
                        }
                    }
                }"#,
            )
            .expect("merge banner variants");
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render banner surface");

        assert_eq!(&tile.rgba[0..4], &[200, 20, 10, 255]);
    }

    #[test]
    fn surface_blocks_use_bed_block_entity_color() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 1),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:bed", 2),
                ]),
            )
            .expect("put subchunk");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
                &test_block_entity_bytes(NbtTag::Compound(IndexMap::from([
                    ("id".to_string(), NbtTag::String("Bed".to_string())),
                    ("x".to_string(), NbtTag::Int(0)),
                    ("y".to_string(), NbtTag::Int(65)),
                    ("z".to_string(), NbtTag::Int(0)),
                    ("Color".to_string(), NbtTag::String("blue".to_string())),
                ]))),
            )
            .expect("put bed block entity");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let mut palette = RenderPalette::default();
        palette
            .merge_json_str(
                r#"{
                    "blocks": {
                        "minecraft:bed": {
                            "default": [200, 20, 10, 255],
                            "variant_colors": {
                                "bed_blue": [20, 30, 180, 255],
                                "bed_red": [200, 20, 10, 255]
                            }
                        }
                    }
                }"#,
            )
            .expect("merge bed variants");
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render bed surface");

        assert_eq!(&tile.rgba[0..4], &[20, 30, 180, 255]);
    }

    #[test]
    fn surface_blocks_use_moving_block_entity_state() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(65, 1),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:stone", 1),
                    ("minecraft:movingBlock", 2),
                ]),
            )
            .expect("put subchunk");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
                &test_block_entity_bytes(NbtTag::Compound(IndexMap::from([
                    ("id".to_string(), NbtTag::String("MovingBlock".to_string())),
                    ("x".to_string(), NbtTag::Int(0)),
                    ("y".to_string(), NbtTag::Int(65)),
                    ("z".to_string(), NbtTag::Int(0)),
                    (
                        "movingBlock".to_string(),
                        NbtTag::Compound(IndexMap::from([
                            (
                                "name".to_string(),
                                NbtTag::String("minecraft:gold_block".to_string()),
                            ),
                            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                            ("version".to_string(), NbtTag::Int(1)),
                        ])),
                    ),
                ]))),
            )
            .expect("put moving block entity");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let palette = RenderPalette::default()
            .with_block_color("minecraft:movingBlock", RgbaColor::new(1, 1, 1, 255))
            .with_block_color("minecraft:gold_block", RgbaColor::new(240, 200, 30, 255));
        let renderer = MapRenderer::new(world, palette);

        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
            )
            .expect("render moving block surface");

        assert_eq!(&tile.rgba[0..4], &[240, 200, 30, 255]);
    }

    #[test]
    fn transparent_water_records_seabed_relief_height_and_depth() {
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
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:sand", 1),
                    ("minecraft:water", 2),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let bake = renderer
            .bake_chunk_blocking(pos, BakeOptions::default())
            .expect("bake surface chunk");
        let ChunkBakePayload::SurfaceAtlas(surface) = bake.payload else {
            panic!("expected surface payload");
        };

        assert_eq!(surface.heights.height_at(0, 0), Some(65));
        assert_eq!(surface.relief_heights.height_at(0, 0), Some(64));
        assert_eq!(surface.water_depths.depth_at(0, 0), 1);
    }

    #[test]
    fn underwater_depth_fade_reduces_deep_water_relief() {
        let base = RgbaColor::new(100, 100, 100, 255);
        let shallow = terrain_lit_color(
            base,
            cardinal_heights(64, 72, 64, 64),
            1,
            TerrainLightingOptions::strong(),
        );
        let deep = terrain_lit_color(
            base,
            cardinal_heights(64, 72, 64, 64),
            16,
            TerrainLightingOptions::strong(),
        );
        let shallow_delta = i16::from(shallow.red).abs_diff(i16::from(base.red));
        let deep_delta = i16::from(deep.red).abs_diff(i16::from(base.red));
        assert!(shallow_delta > deep_delta);
    }

    #[test]
    fn surface_diagnostics_count_unknown_blocks() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 4),
            )
            .expect("put data2d");
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_surface_subchunk_bytes([
                    ("minecraft:air", 0_u16),
                    ("minecraft:custom_unknown", 1),
                    ("minecraft:custom_unknown", 1),
                ]),
            )
            .expect("put subchunk");
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            storage,
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
        let diagnostics_sink = RenderDiagnosticsSink::new({
            let diagnostics = Arc::clone(&diagnostics);
            move |value| {
                diagnostics.lock().expect("diagnostics lock").add(value);
            }
        });
        renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    threading: RenderThreadingOptions::Single,
                    diagnostics: Some(diagnostics_sink),
                    ..RenderOptions::default()
                },
            )
            .expect("render unknown surface");

        let diagnostics = diagnostics.lock().expect("diagnostics lock");
        assert_eq!(diagnostics.unknown_blocks, 256);
        assert_eq!(
            diagnostics
                .unknown_blocks_by_name
                .get("minecraft:custom_unknown")
                .copied(),
            Some(256)
        );
        assert_eq!(diagnostics.purple_error_pixels, 256);
    }

    #[test]
    fn missing_heightmap_renders_transparent_pixels() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
        let diagnostics_sink = RenderDiagnosticsSink::new({
            let diagnostics = Arc::clone(&diagnostics);
            move |value| {
                diagnostics.lock().expect("diagnostics lock").add(value);
            }
        });
        let tile = renderer
            .render_tile_with_options_blocking(
                RenderJob {
                    tile_size: 1,
                    ..RenderJob::new(
                        TileCoord {
                            x: 0,
                            z: 0,
                            dimension: Dimension::Overworld,
                        },
                        RenderMode::SurfaceBlocks,
                    )
                },
                &RenderOptions {
                    format: ImageFormat::Rgba,
                    diagnostics: Some(diagnostics_sink),
                    threading: RenderThreadingOptions::Single,
                    ..RenderOptions::default()
                },
            )
            .expect("render missing heightmap");

        assert_eq!(&tile.rgba[0..4], &[0, 0, 0, 0]);
        let diagnostics = diagnostics.lock().expect("diagnostics lock");
        assert_eq!(diagnostics.transparent_pixels, 256);
        assert_eq!(diagnostics.missing_chunks, 256);
        assert_eq!(diagnostics.missing_heightmaps, 0);
    }

    #[test]
    fn tile_cache_authority_key_changes_with_world_signature_and_layout() {
        let base = TileCacheKey {
            world_id: "world".to_string(),
            world_signature: "a".to_string(),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            dimension: Dimension::Overworld,
            mode: "surface".to_string(),
            chunks_per_tile: 16,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
            tile_x: 0,
            tile_z: 0,
            extension: "webp".to_string(),
        };
        let mut changed_signature = base.clone();
        changed_signature.world_signature = "b".to_string();
        let mut changed_layout = base.clone();
        changed_layout.blocks_per_pixel = 4;
        let mut changed_pixel_scale = base.clone();
        changed_pixel_scale.pixels_per_block = 2;

        let base = tile_authority_cache_key_from_tile_key(&base);
        assert_ne!(
            base.validation_value(),
            tile_authority_cache_key_from_tile_key(&changed_signature).validation_value()
        );
        assert_ne!(
            base.validation_value(),
            tile_authority_cache_key_from_tile_key(&changed_layout).validation_value()
        );
        assert_ne!(
            base.validation_value(),
            tile_authority_cache_key_from_tile_key(&changed_pixel_scale).validation_value()
        );
    }

    #[test]
    fn render_options_default_bypasses_tile_cache() {
        assert_eq!(
            RenderOptions::default().cache_policy,
            RenderCachePolicy::Bypass
        );
    }

    #[test]
    fn fast_rgba_zstd_round_trips_tile_bytes() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
        ];
        let encoded = encode_fast_rgba_zstd(&rgba, 2, 2).expect("encode fast tile");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode fast tile header");
        let decoded = decode_fast_rgba_zstd(&encoded).expect("decode fast tile");

        assert_eq!(header.version, FAST_RGBA_ZSTD_VERSION);
        assert_eq!(header.validation_value, None);
        assert!(header.is_non_empty());
        assert!(!header.is_empty_negative());
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.validation_value, None);
        assert_eq!(decoded.rgba, rgba);

        let encoded =
            encode_fast_rgba_zstd_with_validation(&rgba, 2, 2, 0xabc).expect("encode validated");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode validated header");
        let decoded = decode_fast_rgba_zstd(&encoded).expect("decode validated tile");
        assert_eq!(header.validation_value, Some(0xabc));
        assert_eq!(decoded.validation_value, Some(0xabc));
        assert_eq!(decoded.rgba, rgba);
    }

    #[test]
    fn fast_rgba_zstd_content_flags_round_trip() {
        let empty = vec![0; 16];
        let encoded = encode_fast_rgba_zstd_with_validation(&empty, 2, 2, 0x55)
            .expect("encode empty negative");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode empty header");
        assert!(header.is_empty_negative());
        assert!(!header.is_non_empty());
        assert_eq!(header.validation_value, Some(0x55));

        let mut invalid_flags = encoded.clone();
        invalid_flags[28..32].copy_from_slice(
            &(FAST_RGBA_ZSTD_FLAG_NON_EMPTY | FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE).to_le_bytes(),
        );
        assert!(decode_fast_rgba_zstd_header(&invalid_flags).is_err());
    }

    #[test]
    fn fast_rgba_zstd_empty_negative_can_be_header_only() {
        let encoded =
            encode_fast_rgba_zstd_empty_negative(2, 2, 0x66).expect("encode header-only negative");
        let header =
            decode_fast_rgba_zstd_header(&encoded).expect("decode header-only negative header");

        assert_eq!(encoded.len(), FAST_RGBA_ZSTD_HEADER_LEN);
        assert!(header.is_empty_negative());
        assert!(!header.is_non_empty());
        assert_eq!(header.rgba_len, 16);
        assert_eq!(header.validation_value, Some(0x66));
        assert!(decode_fast_rgba_zstd(&encoded).is_err());
    }

    #[test]
    fn fast_rgba_zstd_v1_decodes_without_validation() {
        let rgba = vec![0, 1, 2, 255, 3, 4, 5, 255, 6, 7, 8, 255, 9, 10, 11, 255];
        let encoded = encode_fast_rgba_zstd_v1_for_test(&rgba, 2, 2).expect("encode v1");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode v1 header");
        let decoded = decode_fast_rgba_zstd(&encoded).expect("decode v1");

        assert_eq!(header.version, FAST_RGBA_ZSTD_V1_VERSION);
        assert_eq!(header.validation_value, None);
        assert_eq!(decoded.validation_value, None);
        assert_eq!(decoded.rgba, rgba);
    }

    #[test]
    fn fast_rgba_zstd_rejects_invalid_entries() {
        let rgba = vec![0; 16];
        let mut encoded = encode_fast_rgba_zstd(&rgba, 2, 2).expect("encode fast tile");
        encoded[0] = b'X';
        assert!(decode_fast_rgba_zstd(&encoded).is_err());

        let encoded = encode_fast_rgba_zstd(&rgba, 2, 2).expect("encode fast tile");
        assert!(decode_fast_rgba_zstd(&encoded[..8]).is_err());
        assert!(encode_fast_rgba_zstd(&rgba[..15], 2, 2).is_err());
    }

    #[test]
    fn fast_rgba_cache_rejects_transparent_legacy_v2_for_non_empty_chunks() {
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(vec![ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            7,
        );
        let rgba = vec![
            0;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        let mut encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode transparent tile");
        encoded[28..32].copy_from_slice(&0_u32.to_le_bytes());
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode legacy v2 header");
        assert!(header.has_legacy_content_flags());

        assert_eq!(
            tile_cache_entry_decision(&encoded, &key, &planned, ImageFormat::FastRgbaZstd, 7)
                .expect("cache decision"),
            TileCacheEntryDecision::Miss
        );
    }

    #[test]
    fn fast_rgba_cache_accepts_empty_negative_only_for_empty_chunk_set() {
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(Vec::new()));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            11,
        );
        let rgba = vec![
            0;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode negative tile");

        assert_eq!(
            tile_cache_entry_decision(&encoded, &key, &planned, ImageFormat::FastRgbaZstd, 11)
                .expect("cache decision"),
            TileCacheEntryDecision::EmptyNegative
        );
    }

    #[test]
    fn fast_rgba_cache_accepts_header_only_empty_negative() {
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(Vec::new()));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            19,
        );
        let encoded = encode_fast_rgba_zstd_empty_negative(
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode header-only negative");

        assert_eq!(
            tile_cache_entry_decision(&encoded, &key, &planned, ImageFormat::FastRgbaZstd, 19)
                .expect("cache decision"),
            TileCacheEntryDecision::EmptyNegative
        );
    }

    #[test]
    fn empty_negative_cache_write_payload_is_header_only() {
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(Vec::new()));
        let key = tile_cache_key_for_test(&planned);
        let empty_payload = TileCacheWritePayload::EmptyNegative {
            tile_size: planned.job.tile_size,
            region: planned.region,
            validation_seed: 23,
        };
        let encoded = encode_tile_cache_write_payload(&key, &empty_payload)
            .expect("encode empty negative payload");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode header-only negative");

        assert_eq!(encoded.len(), FAST_RGBA_ZSTD_HEADER_LEN);
        assert!(header.is_empty_negative());
        assert_eq!(
            header.validation_value,
            Some(tile_cache_validation_value(&key, &planned.region, &[], 23))
        );

        let rgba = Arc::<[u8]>::from(vec![
            0;
            fast_rgba_byte_len(
                planned.job.tile_size,
                planned.job.tile_size
            )
            .expect("tile byte length")
        ]);
        let fast_payload = TileCacheWritePayload::FastRgbaZstd {
            rgba,
            width: planned.job.tile_size,
            height: planned.job.tile_size,
            region: planned.region,
            chunk_positions: Arc::<[ChunkPos]>::from(Vec::new()),
            validation_seed: 23,
            flags: FAST_RGBA_ZSTD_FLAG_EMPTY_NEGATIVE,
        };
        let encoded = encode_tile_cache_write_payload(&key, &fast_payload)
            .expect("encode flagged empty negative payload");
        let header = decode_fast_rgba_zstd_header(&encoded).expect("decode flagged empty negative");

        assert_eq!(encoded.len(), FAST_RGBA_ZSTD_HEADER_LEN);
        assert!(header.is_empty_negative());
        assert_eq!(
            header.validation_value,
            Some(tile_cache_validation_value(&key, &planned.region, &[], 23))
        );
    }

    #[test]
    fn fast_rgba_cache_rejects_validated_entries_without_chunk_positions() {
        let planned = planned_tile_at(0, 0);
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(&key, &planned.region, &[], 13);
        let mut rgba = vec![
            0;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        rgba[3] = 255;
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode dense fallback tile");

        assert_eq!(
            tile_cache_entry_decision(&encoded, &key, &planned, ImageFormat::FastRgbaZstd, 13)
                .expect("cache decision"),
            TileCacheEntryDecision::Miss
        );
    }

    #[test]
    fn validated_disk_cache_is_ready_from_disk() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-optimistic-cache-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let cache = Arc::new(Mutex::new(TileCache::new(cache_root.clone(), 4)));
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(vec![ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            17,
        );
        let mut rgba = vec![
            0;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        rgba[3] = 255;
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode cache tile");
        cache
            .lock()
            .expect("cache lock")
            .write_encoded(&key, &encoded)
            .expect("write cache");

        let mut scratch = TileCacheDecodeScratch::new();
        let authority_snapshots = authority_snapshots_for_test(&cache, &key);
        let blob_readers = blob_readers_for_test(&cache, &key);
        let session_memory = session_tile_memory_for_test(4);
        let probe = resolve_tile_cache_entry(
            &cache,
            &session_memory,
            &authority_snapshots,
            &blob_readers,
            &key,
            &key,
            &planned,
            ImageFormat::FastRgbaZstd,
            17,
            &mut scratch,
        )
        .expect("probe cache");
        match probe.decision {
            TileCacheProbeDecision::Ready { source, .. } => {
                assert_eq!(source, TileReadySource::DiskCacheFresh);
            }
            _ => panic!("expected ready tile"),
        }
        let probe = resolve_tile_cache_entry(
            &cache,
            &session_memory,
            &authority_snapshots,
            &blob_readers,
            &key,
            &key,
            &planned,
            ImageFormat::FastRgbaZstd,
            17,
            &mut scratch,
        )
        .expect("probe session memory");
        match probe.decision {
            TileCacheProbeDecision::Ready { source, .. } => {
                assert_eq!(source, TileReadySource::MemoryCache);
                assert_eq!(probe.read_ms, 0);
                assert_eq!(probe.decode_ms, 0);
            }
            _ => panic!("expected memory ready tile"),
        }
        fs::remove_dir_all(cache_root).ok();
    }

    #[test]
    fn authority_empty_negative_cache_is_ready_from_disk() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-empty-authority-cache-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let cache = Arc::new(Mutex::new(TileCache::new(cache_root.clone(), 4)));
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(Vec::new()));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(&key, &planned.region, &[], 23);
        let encoded = encode_fast_rgba_zstd_empty_negative(
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode empty cache tile");
        cache
            .lock()
            .expect("cache lock")
            .write_encoded(&key, &encoded)
            .expect("write empty cache");

        let mut scratch = TileCacheDecodeScratch::new();
        let authority_snapshots = authority_snapshots_for_test(&cache, &key);
        let blob_readers = blob_readers_for_test(&cache, &key);
        let session_memory = session_tile_memory_for_test(4);
        let probe = resolve_tile_cache_entry(
            &cache,
            &session_memory,
            &authority_snapshots,
            &blob_readers,
            &key,
            &key,
            &planned,
            ImageFormat::FastRgbaZstd,
            23,
            &mut scratch,
        )
        .expect("probe cache");
        assert!(matches!(
            probe.decision,
            TileCacheProbeDecision::EmptyNegative
        ));
        fs::remove_dir_all(cache_root).ok();
    }

    #[test]
    fn authority_cache_write_records_tile_dependencies() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-authority-deps-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let cache = TileCache::new(cache_root.clone(), 4);
        let mut planned = planned_tile_at(0, 0);
        let chunk_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(vec![chunk_pos]));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            29,
        );
        let mut rgba = vec![
            0;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        rgba[3] = 255;
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode tile");
        cache
            .write_fast_rgba_zstd_entry(
                &key,
                &encoded,
                Some(planned.region),
                planned.chunk_positions.as_deref(),
                29,
            )
            .expect("write authority cache");

        let authority_key = tile_authority_cache_key_from_tile_key(&key);
        let snapshot = cache
            .authority
            .load_index(&authority_key)
            .expect("load authority index")
            .expect("authority index exists");
        assert_eq!(snapshot.tiles.len(), 1);
        assert_eq!(snapshot.dependencies.len(), 1);
        assert_eq!(snapshot.dependencies[0].position, chunk_pos);
        assert_eq!(snapshot.chunk_tile_refs.len(), 1);
        assert_eq!(snapshot.chunk_tile_refs[0].position, chunk_pos);
        fs::remove_dir_all(cache_root).ok();
    }

    #[test]
    fn authority_cache_write_rebuilds_after_corrupt_index() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-authority-rebuild-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let cache = TileCache::new(cache_root.clone(), 4);
        let mut planned = planned_tile_at(0, 0);
        planned.chunk_positions = Some(Arc::<[ChunkPos]>::from(vec![ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]));
        let key = tile_cache_key_for_test(&planned);
        let validation_value = tile_cache_validation_value(
            &key,
            &planned.region,
            planned.chunk_positions.as_deref().expect("chunk positions"),
            31,
        );
        let rgba = vec![
            255;
            fast_rgba_byte_len(planned.job.tile_size, planned.job.tile_size)
                .expect("tile byte length")
        ];
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            planned.job.tile_size,
            planned.job.tile_size,
            validation_value,
        )
        .expect("encode first tile");
        cache
            .write_fast_rgba_zstd_entry(
                &key,
                &encoded,
                Some(planned.region),
                planned.chunk_positions.as_deref(),
                31,
            )
            .expect("write first authority cache");

        let authority_key = tile_authority_cache_key_from_tile_key(&key);
        let tiles_path = cache
            .authority
            .root_for_key(&authority_key)
            .join("tiles.0000000000000001.bin");
        fs::write(&tiles_path, b"corrupt tiles").expect("corrupt authority tiles index");
        assert!(cache.authority.load_index(&authority_key).is_err());

        let mut rebuilt = planned_tile_at(1, 0);
        rebuilt.chunk_positions = planned.chunk_positions.clone();
        let rebuilt_key = tile_cache_key_for_test(&rebuilt);
        let rebuilt_validation_value = tile_cache_validation_value(
            &rebuilt_key,
            &rebuilt.region,
            rebuilt.chunk_positions.as_deref().expect("chunk positions"),
            31,
        );
        let encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            rebuilt.job.tile_size,
            rebuilt.job.tile_size,
            rebuilt_validation_value,
        )
        .expect("encode rebuilt tile");
        cache
            .write_fast_rgba_zstd_entry(
                &rebuilt_key,
                &encoded,
                Some(rebuilt.region),
                rebuilt.chunk_positions.as_deref(),
                31,
            )
            .expect("rebuild authority cache after corrupt index");

        let snapshot = cache
            .authority
            .load_index(&authority_key)
            .expect("load rebuilt authority index")
            .expect("rebuilt authority index exists");
        assert_eq!(snapshot.tiles.len(), 1);
        assert_eq!((snapshot.tiles[0].tile_x, snapshot.tiles[0].tile_z), (1, 0));
        fs::remove_dir_all(cache_root).ok();
    }

    #[test]
    fn prepared_authority_commits_are_batched_into_one_generation() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-authority-prepared-batch-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let cache = TileCache::new(cache_root.clone(), 4);
        let mut first = planned_tile_at(0, 0);
        let mut second = planned_tile_at(1, 0);
        let chunk_positions = Arc::<[ChunkPos]>::from(vec![ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]);
        first.chunk_positions = Some(Arc::clone(&chunk_positions));
        second.chunk_positions = Some(chunk_positions);
        let first_key = tile_cache_key_for_test(&first);
        let second_key = tile_cache_key_for_test(&second);
        let rgba = vec![
            64;
            fast_rgba_byte_len(first.job.tile_size, first.job.tile_size)
                .expect("tile byte length")
        ];
        let first_validation = tile_cache_validation_value(
            &first_key,
            &first.region,
            first.chunk_positions.as_deref().expect("first chunks"),
            37,
        );
        let second_validation = tile_cache_validation_value(
            &second_key,
            &second.region,
            second.chunk_positions.as_deref().expect("second chunks"),
            37,
        );
        let first_encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            first.job.tile_size,
            first.job.tile_size,
            first_validation,
        )
        .expect("encode first tile");
        let second_encoded = encode_fast_rgba_zstd_with_validation(
            &rgba,
            second.job.tile_size,
            second.job.tile_size,
            second_validation,
        )
        .expect("encode second tile");
        let commits = vec![
            prepare_fast_rgba_zstd_commit(
                &first_key,
                &first_encoded,
                Some(first.region),
                first.chunk_positions.as_deref(),
                37,
            )
            .expect("prepare first commit"),
            prepare_fast_rgba_zstd_commit(
                &second_key,
                &second_encoded,
                Some(second.region),
                second.chunk_positions.as_deref(),
                37,
            )
            .expect("prepare second commit"),
        ];

        let committed = cache
            .commit_prepared_tile_cache_commits(commits)
            .expect("commit prepared batch");
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].1.generation, 1);
        assert_eq!(committed[0].1.tiles.len(), 2);
        let authority_key = tile_authority_cache_key_from_tile_key(&first_key);
        let loaded = cache
            .authority
            .load_index(&authority_key)
            .expect("load authority index")
            .expect("authority index exists");
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.tiles.len(), 2);
        fs::remove_dir_all(cache_root).ok();
    }

    #[test]
    fn shared_builtin_palette_reuses_single_allocation() {
        let first = RenderPalette::builtin_shared();
        let second = RenderPalette::builtin_shared();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn refresh_cache_policy_renders_even_when_cache_exists() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-refresh-policy-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let storage = Arc::new(MemoryStorage::new());
        let chunk_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(chunk_pos, ChunkRecordTag::LegacyTerrain).encode(),
                &test_legacy_terrain_bytes(2, 65),
            )
            .expect("put legacy terrain");
        let world = Arc::new(BedrockWorld::from_storage_with_format(
            "memory",
            storage,
            OpenOptions::default(),
            bedrock_world::WorldFormat::LevelDbLegacyTerrain,
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let config = MapRenderSessionConfig {
            cache_root: cache_root.clone(),
            world_id: "refresh-test".to_string(),
            world_signature: "v1".to_string(),
            cull_missing_chunks: false,
            ..MapRenderSessionConfig::default()
        };
        let session = MapRenderSession::new(renderer, config.clone());
        let layout = RenderLayout {
            chunks_per_tile: 1,
            blocks_per_pixel: 16,
            pixels_per_block: 1,
        };
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("build planned tile job");
        let planned = PlannedTile {
            job,
            region: ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            layout,
            chunk_positions: Some(Arc::<[ChunkPos]>::from(vec![chunk_pos])),
        };
        let cache_key = TileCacheKey {
            world_id: config.world_id,
            world_signature: config.world_signature,
            renderer_version: config.renderer_version,
            palette_version: config.palette_version,
            dimension: Dimension::Overworld,
            mode: mode_slug(RenderMode::SurfaceBlocks),
            chunks_per_tile: layout.chunks_per_tile,
            blocks_per_pixel: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
            tile_x: 0,
            tile_z: 0,
            extension: image_format_extension(ImageFormat::FastRgbaZstd).to_string(),
        };
        let cached = encode_fast_rgba_zstd(&[0, 0, 0, 255], 1, 1).expect("encode cached tile");
        TileCache::new(&cache_root, 1)
            .write_encoded(&cache_key, &cached)
            .expect("write cached tile");

        let use_events = collect_stream_event_kinds(&session, &planned, RenderCachePolicy::Use);
        assert!(use_events.iter().any(|event| *event == "stale"));
        assert!(!use_events.iter().any(|event| *event == "rendered"));

        let refresh_events = collect_stream_event_kinds_with_seed(
            &session,
            &planned,
            RenderCachePolicy::Refresh,
            99,
        );
        assert!(!refresh_events.iter().any(|event| *event == "cached"));
        assert!(refresh_events.iter().any(|event| *event == "rendered"));

        fs::remove_dir_all(&cache_root).expect("remove test cache");
    }

    #[test]
    fn use_cache_rejects_fast_rgba_validation_mismatch() {
        let cache_root = std::env::temp_dir().join(format!(
            "bedrock-render-validation-mismatch-{}",
            TILE_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).expect("remove stale test cache");
        }
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let config = MapRenderSessionConfig {
            cache_root: cache_root.clone(),
            world_id: "validation-test".to_string(),
            world_signature: "v1".to_string(),
            cull_missing_chunks: false,
            ..MapRenderSessionConfig::default()
        };
        let session = MapRenderSession::new(renderer, config.clone());
        let layout = RenderLayout {
            chunks_per_tile: 1,
            blocks_per_pixel: 16,
            pixels_per_block: 1,
        };
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            layout,
        )
        .expect("build planned tile job");
        let planned = PlannedTile {
            job,
            region: ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            layout,
            chunk_positions: None,
        };
        let cache_key = TileCacheKey {
            world_id: config.world_id,
            world_signature: config.world_signature,
            renderer_version: config.renderer_version,
            palette_version: config.palette_version,
            dimension: Dimension::Overworld,
            mode: mode_slug(RenderMode::SurfaceBlocks),
            chunks_per_tile: layout.chunks_per_tile,
            blocks_per_pixel: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
            tile_x: 0,
            tile_z: 0,
            extension: image_format_extension(ImageFormat::FastRgbaZstd).to_string(),
        };
        let cached =
            encode_fast_rgba_zstd_with_validation(&[0, 0, 0, 0], 1, 1, 123).expect("encode cached");
        TileCache::new(&cache_root, 1)
            .write_encoded(&cache_key, &cached)
            .expect("write cached tile");

        let events =
            collect_stream_event_kinds_with_seed(&session, &planned, RenderCachePolicy::Use, 456);
        assert!(!events.iter().any(|event| *event == "cached"));
        assert!(events.iter().any(|event| *event == "rendered"));

        fs::remove_dir_all(&cache_root).expect("remove test cache");
    }

    fn collect_stream_event_kinds<S>(
        session: &MapRenderSession<S>,
        planned: &PlannedTile,
        cache_policy: RenderCachePolicy,
    ) -> Vec<&'static str>
    where
        S: WorldStorageHandle,
    {
        collect_stream_event_kinds_with_seed(session, planned, cache_policy, 0)
    }

    fn collect_stream_event_kinds_with_seed<S>(
        session: &MapRenderSession<S>,
        planned: &PlannedTile,
        cache_policy: RenderCachePolicy,
        tile_cache_validation_seed: u64,
    ) -> Vec<&'static str>
    where
        S: WorldStorageHandle,
    {
        let events = Arc::new(Mutex::new(Vec::new()));
        session
            .render_web_tiles_streaming_blocking(
                std::slice::from_ref(planned),
                RenderOptions {
                    format: ImageFormat::FastRgbaZstd,
                    backend: RenderBackend::Cpu,
                    cache_policy,
                    tile_cache_validation_seed,
                    threading: RenderThreadingOptions::Single,
                    surface: SurfaceRenderOptions {
                        height_shading: false,
                        ..SurfaceRenderOptions::default()
                    },
                    ..RenderOptions::default()
                },
                {
                    let events = Arc::clone(&events);
                    move |event| {
                        let kind = match event {
                            TileStreamEvent::Ready { source, .. } => match source {
                                TileReadySource::MemoryCache => "memory",
                                TileReadySource::DiskCacheFresh => "cached",
                                TileReadySource::DiskCacheStale => "stale",
                                TileReadySource::Render => "rendered",
                                TileReadySource::Preview => "preview",
                            },
                            TileStreamEvent::Empty { .. } => "empty",
                            TileStreamEvent::Failed { .. } => "failed",
                            TileStreamEvent::Complete { .. } => "complete",
                            TileStreamEvent::Progress(_) => "progress",
                        };
                        events.lock().expect("events lock").push(kind);
                        Ok(())
                    }
                },
            )
            .expect("render stream");
        Arc::try_unwrap(events)
            .expect("events still shared")
            .into_inner()
            .expect("events lock")
    }

    #[test]
    fn render_session_lifts_stale_renderer_cache_version() {
        let world = Arc::new(BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions::default(),
        ));
        let renderer = MapRenderer::new(world, RenderPalette::default());
        let stale = RENDERER_CACHE_VERSION.saturating_sub(1);
        let session = MapRenderSession::new(
            renderer,
            MapRenderSessionConfig {
                renderer_version: stale,
                ..MapRenderSessionConfig::default()
            },
        );

        assert_eq!(session.config.renderer_version, RENDERER_CACHE_VERSION);
    }

    #[test]
    fn gpu_backend_cache_slugs_are_stable_and_distinct() {
        assert_eq!(RenderBackend::Cpu.cache_slug(), "cpu");
        assert_eq!(RenderBackend::Auto.cache_slug(), "auto");
        assert_eq!(RenderBackend::Wgpu.cache_slug(), "wgpu");

        let slugs = [
            RenderGpuBackend::Auto.cache_slug(),
            RenderGpuBackend::Dx11.cache_slug(),
            RenderGpuBackend::Dx12.cache_slug(),
            RenderGpuBackend::Vulkan.cache_slug(),
        ];
        assert_eq!(slugs, ["auto", "dx11", "dx12", "vulkan"]);

        let mut unique = BTreeSet::new();
        for slug in slugs {
            assert!(unique.insert(slug), "duplicate GPU cache slug: {slug}");
        }
    }

    #[test]
    fn gpu_backend_diagnostics_labels_are_stable() {
        assert_eq!(ResolvedRenderBackend::Cpu.label(), "cpu");
        assert_eq!(ResolvedRenderBackend::Dx11.label(), "dx11");
        assert_eq!(ResolvedRenderBackend::WgpuDx12.label(), "wgpu-dx12");
        assert_eq!(ResolvedRenderBackend::WgpuVulkan.label(), "wgpu-vulkan");
        assert_eq!(ResolvedRenderBackend::Mixed.label(), "mixed");

        assert_eq!(
            resolved_backend_from_gpu(RenderGpuBackend::Dx11),
            ResolvedRenderBackend::Dx11
        );
        assert_eq!(
            resolved_backend_from_gpu(RenderGpuBackend::Vulkan),
            ResolvedRenderBackend::WgpuVulkan
        );
        assert_eq!(
            resolved_backend_from_gpu(RenderGpuBackend::Dx12),
            ResolvedRenderBackend::WgpuDx12
        );
        assert_eq!(
            resolved_backend_from_gpu(RenderGpuBackend::Auto),
            ResolvedRenderBackend::Mixed
        );
    }

    #[test]
    fn gpu_options_default_to_compose_only_with_cpu_fallback() {
        let options = RenderGpuOptions::default();
        assert_eq!(options.backend, RenderGpuBackend::Auto);
        assert_eq!(options.fallback_policy, RenderGpuFallbackPolicy::AllowCpu);
        assert_eq!(options.pipeline_level, RenderGpuPipelineLevel::ComposeOnly);
        assert_eq!(options.max_in_flight, 0);
        assert_eq!(options.batch_pixels, 0);
        assert_eq!(options.staging_pool_bytes, 0);
        assert!(!options.diagnostics);
    }

    #[test]
    fn gpu_compose_requires_batch_large_enough() {
        let mut options = RenderOptions::max_speed_interactive();
        options.backend = RenderBackend::Wgpu;
        options.gpu.batch_pixels = 0;

        assert!(!should_process_tile_on_gpu(&options, 256, 1));
        assert!(!should_process_tile_on_gpu(&options, 256, 3));
        assert!(should_process_tile_on_gpu(&options, 256, 4));

        options.gpu.batch_pixels = 256 * 256;
        assert!(should_process_tile_on_gpu(&options, 256, 1));

        options.backend = RenderBackend::Cpu;
        assert!(!should_process_tile_on_gpu(&options, 256, 8));
    }

    fn test_subchunk_bytes() -> Vec<u8> {
        let palette = ["minecraft:x_axis", "minecraft:z_axis"];
        let mut bytes = vec![8, 1, 1 << 1];
        let mut words = vec![0_u32; 128];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let value = u32::from(local_z > 0);
                let block_index = block_storage_index(local_x, 0, local_z);
                let word_index = block_index / 32;
                let bit_offset = block_index % 32;
                words[word_index] |= value << bit_offset;
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let palette_len = i32::try_from(palette.len()).expect("test palette length fits i32");
        bytes.extend_from_slice(&palette_len.to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&bedrock_world::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_uniform_layer_subchunk_bytes(block_name: &str) -> Vec<u8> {
        test_signature_layer_subchunk_bytes(
            block_name, block_name, block_name, block_name, block_name,
        )
    }

    fn test_signature_layer_subchunk_bytes(
        base: &str,
        top_left: &str,
        top_right: &str,
        bottom_left: &str,
        bottom_right: &str,
    ) -> Vec<u8> {
        let palette = [
            "minecraft:air",
            base,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        ];
        let bits_per_value = 4_u8;
        let values_per_word = usize::from(32 / bits_per_value);
        let word_count = 4096_usize.div_ceil(values_per_word);
        let mut bytes = vec![8, 1, bits_per_value << 1];
        let mut words = vec![0_u32; word_count];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let value = match (local_x, local_z) {
                    (0, 0) => 2_u16,
                    (15, 0) => 3,
                    (0, 15) => 4,
                    (15, 15) => 5,
                    _ => 1,
                };
                let block_index = block_storage_index(local_x, 0, local_z);
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * usize::from(bits_per_value);
                words[word_index] |= u32::from(value) << bit_offset;
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let palette_len = i32::try_from(palette.len()).expect("test palette length fits i32");
        bytes.extend_from_slice(&palette_len.to_le_bytes());
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&bedrock_world::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn signature_block_name(chunk_x: i32, chunk_z: i32, suffix: &str) -> String {
        let x = if chunk_x < 0 {
            format!("m{}", chunk_x.saturating_abs())
        } else {
            chunk_x.to_string()
        };
        let z = if chunk_z < 0 {
            format!("m{}", chunk_z.saturating_abs())
        } else {
            chunk_z.to_string()
        };
        format!("minecraft:signature_{x}_{z}_{suffix}")
    }

    fn signature_color(chunk_index: usize, marker_index: usize) -> RgbaColor {
        let seed = u8::try_from(chunk_index * 37 + marker_index * 19).expect("small seed");
        RgbaColor::new(
            20_u8.saturating_add(seed),
            40_u8.saturating_add(seed.wrapping_mul(3)),
            80_u8.saturating_add(seed.wrapping_mul(5)),
            255,
        )
    }

    fn assert_signature_pixels(label: &str, tile: &TileImage, expected: &[(u32, u32, [u8; 4])]) {
        for (x, z, color) in expected {
            assert_eq!(
                pixel_rgba(&tile.rgba, tile.width, *x, *z),
                *color,
                "{label} signature pixel ({x}, {z})"
            );
        }
    }

    fn test_surface_subchunk_bytes<const N: usize>(palette_entries: [(&str, u16); N]) -> Vec<u8> {
        test_surface_subchunk_bytes_with_top_values(palette_entries, |_, _| 2)
    }

    fn test_surface_subchunk_bytes_with_top_values<const N: usize>(
        palette_entries: [(&str, u16); N],
        top_value: impl Fn(u8, u8) -> u16,
    ) -> Vec<u8> {
        test_surface_subchunk_bytes_with_values(palette_entries, |local_x, local_z, local_y| {
            match local_y {
                0 => 1,
                1 => top_value(local_x, local_z),
                _ => 0,
            }
        })
    }

    fn test_surface_subchunk_bytes_with_values<const N: usize>(
        palette_entries: [(&str, u16); N],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) -> Vec<u8> {
        let bits_per_value = match palette_entries.len() {
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
        let palette_len =
            i32::try_from(palette_entries.len()).expect("test palette length fits i32");
        bytes.extend_from_slice(&palette_len.to_le_bytes());
        for (name, _) in palette_entries {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&bedrock_world::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_surface_state_subchunk_bytes(
        palette_entries: &[BlockState],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) -> Vec<u8> {
        let bits_per_value = match palette_entries.len() {
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
        let palette_len =
            i32::try_from(palette_entries.len()).expect("test palette length fits i32");
        bytes.extend_from_slice(&palette_len.to_le_bytes());
        for state in palette_entries {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(state.name.clone())),
                (
                    "states".to_string(),
                    NbtTag::Compound(
                        state
                            .states
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    ),
                ),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&bedrock_world::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
        bytes
    }

    fn test_surface_layered_subchunk_bytes<const L: usize, const U: usize>(
        lower_palette: [(&str, u16); L],
        upper_palette: [(&str, u16); U],
        lower_value_at: impl Fn(u8, u8, u8) -> u16,
        upper_value_at: impl Fn(u8, u8, u8) -> u16,
    ) -> Vec<u8> {
        let mut bytes = vec![8, 2];
        append_surface_palette_storage(&mut bytes, lower_palette, lower_value_at);
        append_surface_palette_storage(&mut bytes, upper_palette, upper_value_at);
        bytes
    }

    fn append_surface_palette_storage<const N: usize>(
        bytes: &mut Vec<u8>,
        palette_entries: [(&str, u16); N],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) {
        let bits_per_value = match palette_entries.len() {
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
        let palette_len =
            i32::try_from(palette_entries.len()).expect("test palette length fits i32");
        bytes.extend_from_slice(&palette_len.to_le_bytes());
        for (name, _) in palette_entries {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String(name.to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&bedrock_world::nbt::serialize_root_nbt(&tag).expect("nbt"));
        }
    }

    fn test_data2d_bytes(height: i16, biome: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(768);
        for _ in 0..256 {
            bytes.extend_from_slice(&height.to_le_bytes());
        }
        bytes.extend(std::iter::repeat_n(biome, 256));
        bytes
    }

    fn test_legacy_terrain_bytes(block_id: u8, height: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_TERRAIN_VALUE_LEN];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..=height.min(127) {
                    bytes[legacy_yzx_block_index(local_x, local_y, local_z)] = block_id;
                }
                let height_offset = bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT
                    + (bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
                    + legacy_column_index(local_x, local_z);
                bytes[height_offset] = height;
                write_legacy_biome_sample(&mut bytes, local_x, local_z, 1, 0x007f_b238);
            }
        }
        bytes
    }

    fn test_legacy_grass_over_stone_bytes(height: u8, biome_color: u32) -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_TERRAIN_VALUE_LEN];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..height.min(127) {
                    bytes[legacy_yzx_block_index(local_x, local_y, local_z)] = 1;
                }
                bytes[legacy_yzx_block_index(local_x, height.min(127), local_z)] = 2;
                let height_offset = bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT
                    + (bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
                    + legacy_column_index(local_x, local_z);
                bytes[height_offset] = height;
                write_legacy_biome_sample(&mut bytes, local_x, local_z, 1, biome_color);
            }
        }
        bytes
    }

    fn test_asymmetric_legacy_terrain_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_TERRAIN_VALUE_LEN];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let (block_id, height) = match (local_x >= 8, local_z >= 8) {
                    (false, false) => (1, 20),
                    (true, false) => (12, 30),
                    (false, true) => (24, 40),
                    (true, true) => (45, 50),
                };
                for local_y in 0..=height {
                    bytes[legacy_yzx_block_index(local_x, local_y, local_z)] = block_id;
                }
                let height_offset = bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT
                    + (bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
                    + legacy_column_index(local_x, local_z);
                bytes[height_offset] = height;
            }
        }
        bytes
    }

    fn test_asymmetric_legacy_subchunk_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
        bytes[0] = 2;
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let block_id = match (local_x >= 8, local_z >= 8) {
                    (false, false) => 1,
                    (true, false) => 12,
                    (false, true) => 24,
                    (true, true) => 45,
                };
                bytes[1 + legacy_xzy_block_index(local_x, 10, local_z)] = block_id;
            }
        }
        bytes
    }

    fn test_legacy_sand_data_terrain_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_TERRAIN_VALUE_LEN];
        let height = 20_u8;
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let data = u8::from(local_x >= 8);
                for local_y in 0..=height {
                    let index = legacy_yzx_block_index(local_x, local_y, local_z);
                    bytes[index] = 12;
                    write_nibble(
                        &mut bytes,
                        bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT,
                        index,
                        data,
                    );
                }
                let height_offset = bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT
                    + (bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
                    + legacy_column_index(local_x, local_z);
                bytes[height_offset] = height;
                write_legacy_biome_sample(&mut bytes, local_x, local_z, 1, 0x007f_b238);
            }
        }
        bytes
    }

    fn test_legacy_sand_data_subchunk_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; bedrock_world::LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
        bytes[0] = 2;
        let data_offset = 1 + 16 * 16 * 16;
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let index = legacy_xzy_block_index(local_x, 10, local_z);
                bytes[1 + index] = 12;
                write_nibble(&mut bytes, data_offset, index, u8::from(local_x >= 8));
            }
        }
        bytes
    }

    fn write_nibble(bytes: &mut [u8], offset: usize, index: usize, value: u8) {
        let target = offset + index / 2;
        let value = value & 0x0f;
        if index % 2 == 0 {
            bytes[target] = (bytes[target] & 0xf0) | value;
        } else {
            bytes[target] = (bytes[target] & 0x0f) | (value << 4);
        }
    }

    fn legacy_yzx_block_index(local_x: u8, local_y: u8, local_z: u8) -> usize {
        (usize::from(local_x) << 11) | (usize::from(local_z) << 7) | usize::from(local_y)
    }

    fn legacy_xzy_block_index(local_x: u8, local_y: u8, local_z: u8) -> usize {
        usize::from(local_x) * 256 + usize::from(local_z) * 16 + usize::from(local_y)
    }

    fn legacy_column_index(local_x: u8, local_z: u8) -> usize {
        usize::from(local_z) * 16 + usize::from(local_x)
    }

    fn write_legacy_biome_sample(
        bytes: &mut [u8],
        local_x: u8,
        local_z: u8,
        biome_id: u8,
        color: u32,
    ) {
        let offset = bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT
            + (bedrock_world::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
            + 16 * 16
            + legacy_column_index(local_x, local_z) * 4;
        bytes[offset] = biome_id;
        bytes[offset + 1] = ((color >> 16) & 0xff) as u8;
        bytes[offset + 2] = ((color >> 8) & 0xff) as u8;
        bytes[offset + 3] = (color & 0xff) as u8;
    }

    fn test_block_entity_bytes(tag: NbtTag) -> Vec<u8> {
        bedrock_world::nbt::serialize_root_nbt(&tag).expect("block entity nbt")
    }

    fn test_data3d_single_biome_bytes(biome: i32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(517);
        for _ in 0..256 {
            bytes.extend_from_slice(&64_i16.to_le_bytes());
        }
        bytes.push(0);
        bytes.extend_from_slice(&biome.to_le_bytes());
        bytes
    }

    fn pixel_rgba(rgba: &[u8], width: u32, x: u32, z: u32) -> [u8; 4] {
        let index = usize::try_from((z * width + x) * 4).expect("pixel index fits usize");
        [
            rgba[index],
            rgba[index + 1],
            rgba[index + 2],
            rgba[index + 3],
        ]
    }

    fn rgba_distance(left: [u8; 4], right: [u8; 4]) -> u16 {
        u16::from(left[0].abs_diff(right[0]))
            + u16::from(left[1].abs_diff(right[1]))
            + u16::from(left[2].abs_diff(right[2]))
    }

    fn rgba_color_distance(left: RgbaColor, right: RgbaColor) -> u16 {
        u16::from(left.red.abs_diff(right.red))
            + u16::from(left.green.abs_diff(right.green))
            + u16::from(left.blue.abs_diff(right.blue))
    }

    fn test_block_state<'a>(
        name: &str,
        states: impl Iterator<Item = (&'a str, NbtTag)>,
    ) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: states
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
            version: Some(1),
        }
    }
}
