//! Tile rendering utilities for Minecraft Bedrock worlds.
//!
//! `bedrock-render` provides palette management, tile planning, top-down render
//! modes, image encoding, cancellation, diagnostics, and Criterion-backed
//! benchmark support for map tooling.

pub mod cache;
mod error;
/// Exact non-rectangular selection planning with separate compact 2D-surface and full 3D-chunk loads.
pub mod exact_selection;
mod palette;
mod renderer;

pub use bedrock_world::{
    World,
    query::SlimeChunkBounds,
    surface::{
        ChunkData, ChunkLoadOptions, SurfaceMapBatchStats, SurfaceMapChunk, SurfaceMapColumn,
        SurfaceMapMaterial, SurfaceMapQueryOptions,
    },
};
pub use cache::{
    ChunkFingerprintInput, TILE_AUTHORITY_FLAG_EMPTY, TILE_AUTHORITY_FLAG_NON_EMPTY,
    TileAuthorityBlobReader, TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityChunkState,
    TileAuthorityChunkTileRef, TileAuthorityCommit, TileAuthorityDependency, TileAuthorityEntry,
    TileAuthorityFreeExtent, TileAuthorityIndexSnapshot, WorldCacheIdentity,
};
pub use error::{BedrockRenderError, BedrockRenderErrorKind, Result};
pub use palette::{
    MapColor, MapColorEntry, MapTintMethod, PaletteImportReport, RenderPalette, RgbaColor,
    SurfaceTintKind,
};
pub use renderer::{
    AtlasRenderOptions, BakeDiagnostics, BakeOptions, BlockBoundaryRenderOptions,
    BlockVolumeRenderOptions, ChunkBounds, ChunkPos, ChunkRegion, ChunkTileLayout,
    DEFAULT_PALETTE_VERSION, DecodedTileEvent, DecodedTileImage, DepthPlane, Dimension,
    FastBgraZstdTile, FastRgbaZstdHeader, FastRgbaZstdTile, HeightPlane, ImageFormat,
    MAX_RENDER_THREADS, MAX_TILE_SIZE_PIXELS, MapRenderSession, MapRenderSessionConfig,
    MapRenderer, NbtTag, PlannedTile, RENDERER_CACHE_VERSION, RegionBake, RegionBakePayload,
    RegionCoord, RegionLayout, RenderBackend, RenderCachePolicy, RenderCancelFlag,
    RenderChunkSource, RenderCpuPipelineOptions, RenderDiagnostics, RenderDiagnosticsSink,
    RenderExecutionProfile, RenderGpuBackend, RenderGpuDiagnostics, RenderGpuFallbackPolicy,
    RenderGpuOptions, RenderGpuPipelineLevel, RenderJob, RenderLayout, RenderMemoryBudget,
    RenderMode, RenderOptions, RenderPerformanceOptions, RenderPerformanceProfile,
    RenderPipelineStats, RenderProgress, RenderProgressSink, RenderSimdPolicy,
    RenderSurfaceLoadPolicy, RenderTaskControl, RenderThreadingOptions, RenderTileOutputOptions,
    RenderTilePriority, RenderWebTilesResult, ResolvedRenderBackend, RgbaPlane, SurfacePlane,
    SurfacePlaneAtlas, SurfaceRenderOptions, TerrainGradientAlgorithm, TerrainLightingOptions,
    TerrainLightingPreset, TerrainShadingMode, TileCache, TileCacheKey, TileCoord, TileImage,
    TilePathScheme, TilePixelFormat, TileReadySource, TileSet, TileStreamEvent, WorldRenderSource,
};
