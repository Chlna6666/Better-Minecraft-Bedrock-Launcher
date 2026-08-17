//! High-level lazy world access built on top of the storage layer.
//!
//! The implementation is split behind responsibility-oriented facades. Existing
//! `bedrock_world::world::*` consumers keep the same API during the 0.6 transition.

#[path = "world/impl.rs"]
mod implementation;

pub use implementation::*;

/// World opening, format detection and executor configuration.
pub mod open {
    pub use super::{
        BedrockWorld, OpenOptions, WorldExecutor, WorldFormat, WorldFormatHint, WorldStorageHandle,
        WorldThreadingOptions,
    };
}

/// Whole-world scans, cancellation and progress reporting.
pub mod scan {
    pub use super::{
        CancelFlag, ProgressSink, WorldPipelineOptions, WorldScanOptions, WorldScanProgress,
    };
}

/// Chunk loading, region queries and load-policy types.
pub mod chunk_io {
    pub use super::{
        BiomeDataRequirement, ChunkBlockEntity, ChunkBounds, ChunkData, ChunkDataRequest,
        ChunkLoadOptions, ChunkLoadPriority, ChunkLoadStats, ExactSurfaceBiomeLoad,
        ExactSurfaceSubchunkPolicy, SubchunkDataRequirement, WorldChunkQueryRegion,
        WorldChunkQueryRegionData, WorldChunkQueryRegionLoadOptions,
    };
}

/// Terrain, biome, water and surface sampling models.
pub mod terrain {
    pub use super::{
        SurfaceColumn, SurfaceColumnOptions, TerrainColumnBiome, TerrainColumnOverlay,
        TerrainColumnSample, TerrainColumnSamples, TerrainColumnWater, TerrainSampleSource,
        TerrainSurfaceRole, terrain_surface_overlay_alpha, terrain_surface_role,
    };
}

/// Transactional world mutation entry point.
pub mod transaction {
    pub use super::WorldTransaction;
}
