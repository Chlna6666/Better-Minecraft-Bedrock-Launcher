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

use super::bedrock_api::BedrockLevelDbStorage;
use super::cache::{
    TILE_AUTHORITY_FLAG_EMPTY, TILE_AUTHORITY_FLAG_NON_EMPTY, TileAuthorityBlobReader,
    TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityChunkState, TileAuthorityChunkTileRef,
    TileAuthorityCommit, TileAuthorityDependency, TileAuthorityEntry, TileAuthorityIndexSnapshot,
};
use super::gpu::{GpuProcessResult, GpuRenderContext};
use crate::error::{BedrockRenderError, Result};
use crate::palette::{RenderPalette, RgbaColor};
use bedrock_world::{
    BedrockWorld, BiomeDataRequirement, BlockPos, BlockState, CancelFlag as WorldCancelFlag,
    ChunkBlockEntity, ChunkData, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    ChunkLoadStats, ChunkPos, Dimension, ExactSurfaceSubchunkPolicy, LegacyBiomeSample, NbtTag,
    OpenOptions as WorldOpenOptions, PartitionedWorldStorage, StorageCachePolicy,
    StoragePipelineOptions, StorageReadOptions, StorageScanMode, StorageThreadingOptions,
    StorageVisitorControl, SubChunk, SubChunkDecodeMode, TerrainColumnBiome, TerrainColumnOverlay,
    TerrainColumnSample, TerrainColumnSamples, WorldChunkQueryRegion, WorldChunkQueryRegionData,
    WorldChunkQueryRegionLoadOptions, WorldPipelineOptions, WorldScanOptions, WorldStorage,
    WorldStorageHandle, WorldThreadingOptions, terrain_surface_overlay_alpha,
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