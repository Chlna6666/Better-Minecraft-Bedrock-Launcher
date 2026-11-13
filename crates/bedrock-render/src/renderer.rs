pub mod gpu;
pub mod occupancy;
pub mod pipeline;
mod pixels;
mod shading;

/// Render source backed by `bedrock-world` automatic world opening.
pub type WorldRenderSource = pipeline::LevelDbRenderSource;

pub use pipeline::{
    AtlasRenderOptions, BakeDiagnostics, BakeOptions, BlockBoundaryRenderOptions,
    BlockVolumeRenderOptions, ChunkRegion, ChunkTileLayout, DEFAULT_PALETTE_VERSION,
    DecodedTileEvent, DecodedTileImage, DepthPlane, FastBgraZstdTile, FastRgbaZstdHeader,
    FastRgbaZstdTile, HeightPlane, ImageFormat, MAX_RENDER_THREADS, MAX_TILE_SIZE_PIXELS,
    MapRenderSession, MapRenderSessionConfig, MapRenderer, PlannedTile, RENDERER_CACHE_VERSION,
    RegionBake, RegionBakePayload, RegionCoord, RegionLayout, RenderBackend, RenderCachePolicy,
    RenderCancelFlag, RenderChunkSource, RenderCpuPipelineOptions, RenderDiagnostics,
    RenderDiagnosticsSink, RenderExecutionProfile, RenderGpuBackend, RenderGpuDiagnostics,
    RenderGpuFallbackPolicy, RenderGpuOptions, RenderGpuPipelineLevel, RenderJob, RenderLayout,
    RenderMemoryBudget, RenderMode, RenderOptions, RenderPerformanceOptions,
    RenderPerformanceProfile, RenderPipelineStats, RenderProgress, RenderProgressSink,
    RenderSimdPolicy, RenderSurfaceLoadPolicy, RenderTaskControl, RenderThreadingOptions,
    RenderTileOutputOptions, RenderTilePriority, RenderWebTilesResult, ResolvedRenderBackend,
    RgbaPlane, SurfacePlane, SurfacePlaneAtlas, SurfaceRenderOptions, TerrainLightingOptions,
    TerrainLightingPreset, TileCache, TileCacheKey, TileCoord, TileImage, TilePathScheme,
    TilePixelFormat, TileReadySource, TileSet, TileStreamEvent,
};
pub use shading::{TerrainGradientAlgorithm, TerrainShadingMode};

pub use ::bedrock_world::surface::ChunkBounds;
pub use ::bedrock_world::{ChunkPos, Dimension, NbtTag};
