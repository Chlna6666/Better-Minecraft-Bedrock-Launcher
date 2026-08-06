//! Tile rendering utilities for Minecraft Bedrock worlds.
//!
//! `bedrock-render` provides palette management, tile planning, top-down render
//! modes, image encoding, cancellation, diagnostics, and Criterion-backed
//! benchmark support for map tooling.

pub mod editor;
mod error;
mod palette;
mod renderer;

pub use editor::{MapEditInvalidation, MapWorldEditor};
pub use error::{BedrockRenderError, BedrockRenderErrorKind, Result};
pub use palette::{PaletteImportReport, RenderPalette, RgbaColor};
#[cfg(feature = "async")]
pub use renderer::validate_chunk_fingerprints_async;
pub use renderer::{
    AtlasRenderOptions, BakeDiagnostics, BakeOptions, BlockBoundaryRenderOptions,
    BlockVolumeRenderOptions, ChunkBounds, ChunkPos, ChunkRegion, ChunkTileLayout,
    DEFAULT_PALETTE_VERSION, DecodedTileImage, DepthPlane, Dimension, FastRgbaZstdHeader,
    FastRgbaZstdTile, HeightPlane, ImageFormat, LevelDbRenderSource, MAX_RENDER_THREADS,
    MAX_TILE_SIZE_PIXELS, MapRenderSession, MapRenderSessionConfig, MapRenderer, NbtTag,
    PlannedTile, RENDERER_CACHE_VERSION, RegionBake, RegionBakePayload, RegionCoord, RegionLayout,
    RenderBackend, RenderCachePolicy, RenderCancelFlag, RenderChunkSource,
    RenderCpuPipelineOptions, RenderDiagnostics, RenderDiagnosticsSink, RenderExecutionProfile,
    RenderGpuBackend, RenderGpuDiagnostics, RenderGpuFallbackPolicy, RenderGpuOptions,
    RenderGpuPipelineLevel, RenderJob, RenderLayout, RenderMemoryBudget, RenderMode, RenderOptions,
    RenderPerformanceOptions, RenderPerformanceProfile, RenderPipelineStats, RenderProgress,
    RenderProgressSink, RenderSurfaceLoadPolicy, RenderTaskControl, RenderThreadingOptions,
    RenderTileOutputOptions, RenderTilePriority, RenderWebTilesResult, ResolvedRenderBackend,
    RgbaPlane, SurfacePlane, SurfacePlaneAtlas, SurfaceRenderOptions, TerrainLightingOptions,
    TerrainLightingPreset, TileCache, TileCacheKey, TileCoord, TileImage, TileManifestProbeRequest,
    TileManifestProbeResult, TilePathScheme, TilePixelFormat, TileReadySource, TileSet,
    TileStreamEvent, TileStreamEventV2, decode_fast_rgba_zstd, decode_fast_rgba_zstd_header,
    encode_fast_rgba_zstd, encode_fast_rgba_zstd_with_validation, tile_cache_validation_value,
};
pub use renderer::{
    ChunkFingerprintInput, TILE_AUTHORITY_FLAG_EMPTY, TILE_AUTHORITY_FLAG_NON_EMPTY,
    TileAuthorityBlobReader, TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityChunkState,
    TileAuthorityChunkTileRef, TileAuthorityCommit, TileAuthorityDependency, TileAuthorityEntry,
    TileAuthorityFreeExtent, TileAuthorityIndexSnapshot, TileManifestCache, TileManifestCacheKey,
    TileManifestCacheSnapshot, WorldCacheIdentity, render_backend_cache_slug,
    render_cache_validation_seed_from_signature, render_gpu_backend_cache_slug,
    render_mode_cache_slug, render_preset_cache_signature, render_preset_cache_validation_seed,
    tile_manifest_cache_path, tile_payload_fingerprint, validate_chunk_fingerprints_parallel,
    world_cache_id, world_cache_identity, world_cache_signature,
};
