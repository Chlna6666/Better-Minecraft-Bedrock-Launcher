// Internal renderer import surface assembled exclusively from the current bedrock-world domain API.
// It is private to bedrock-render and does not preserve any removed codec/model/storage namespaces.
mod bedrock_api {
    use std::{path::Path, sync::Arc};

    pub use ::bedrock_world::biome::ParsedBiomeStorage;
    pub use ::bedrock_world::block::{BlockPos, BlockState, block_storage_index};
    pub use ::bedrock_world::chunk::{
        ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN,
        LEGACY_TERRAIN_BLOCK_COUNT, LEGACY_TERRAIN_VALUE_LEN, LegacyBiomeSample, SubChunk,
        SubChunkDecodeMode,
    };
    pub use ::bedrock_world::database::{
        BedrockDbKey, MemoryStorage, PartitionedWorldStorage, StorageCachePolicy,
        StoragePipelineOptions, StorageReadOptions, StorageScanMode, StorageThreadingOptions,
        StorageVisitorControl, WorldStorage,
    };
    pub use ::bedrock_world::error::BedrockWorldErrorKind;
    pub use ::bedrock_world::nbt::NbtTag;
    pub use ::bedrock_world::{
        BedrockWorld, BiomeDataRequirement, CancelFlag, ChunkBlockEntity, ChunkBounds, ChunkData,
        ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority, ChunkLoadStats,
        ExactSurfaceSubchunkPolicy, OpenOptions, SubchunkDataRequirement, TerrainColumnBiome,
        TerrainColumnOverlay, TerrainColumnSample, TerrainColumnSamples, WorldChunkQueryRegion,
        WorldChunkQueryRegionData, WorldChunkQueryRegionLoadOptions, WorldFormat,
        WorldPipelineOptions, WorldScanOptions, WorldStorageHandle, WorldThreadingOptions,
        terrain_surface_overlay_alpha,
    };

    /// Renderer storage handle backed by `bedrock-world` automatic world opening.
    ///
    /// The included renderer pipeline still uses a storage-handle type parameter for historical
    /// reasons. This handle no longer opens or drives `bedrock-leveldb` directly: it asks
    /// `bedrock-world` to detect the world folder and delegates raw access through the public world
    /// storage handle. Renderer reads therefore follow the same version-aware path as servers, tools,
    /// and BMCBL.
    #[derive(Clone)]
    pub struct BedrockLevelDbStorage {
        inner: Arc<dyn ::bedrock_world::WorldStorage>,
    }

    impl BedrockLevelDbStorage {
        /// Opens the world containing `db_path` through `bedrock-world` automatic detection.
        ///
        /// # Errors
        ///
        /// Returns errors from `bedrock-world` world detection/opening.
        pub fn open_read_only_best_effort(db_path: impl AsRef<Path>) -> ::bedrock_world::Result<Self> {
            let db_path = db_path.as_ref();
            let world_path = db_path
                .parent()
                .filter(|_| db_path.file_name().is_some_and(|name| name == "db"))
                .unwrap_or(db_path);
            let world: ::bedrock_world::BedrockWorld<Arc<dyn ::bedrock_world::WorldStorage>> =
                ::bedrock_world::BedrockWorld::open_auto_blocking(world_path)?;
            Ok(Self {
                inner: Arc::clone(world.storage_backend()),
            })
        }
    }

    impl ::bedrock_world::WorldStorage for BedrockLevelDbStorage {
        fn get(&self, key: &[u8]) -> ::bedrock_world::Result<Option<bytes::Bytes>> {
            self.inner.get(key)
        }

        fn get_many(&self, keys: &[bytes::Bytes]) -> ::bedrock_world::Result<Vec<Option<bytes::Bytes>>> {
            self.inner.get_many(keys)
        }

        fn get_many_ordered_with_control(
            &self,
            keys: &[bytes::Bytes],
            options: ::bedrock_world::StorageReadOptions,
        ) -> ::bedrock_world::Result<Vec<Option<bytes::Bytes>>> {
            self.inner.get_many_ordered_with_control(keys, options)
        }

        fn put(&self, key: &[u8], value: &[u8]) -> ::bedrock_world::Result<()> {
            self.inner.put(key, value)
        }

        fn delete(&self, key: &[u8]) -> ::bedrock_world::Result<()> {
            self.inner.delete(key)
        }

        fn for_each_key(
            &self,
            options: ::bedrock_world::StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> ::bedrock_world::Result<::bedrock_world::StorageVisitorControl> + Send),
        ) -> ::bedrock_world::Result<::bedrock_world::StorageScanOutcome> {
            self.inner.for_each_key(options, visitor)
        }

        fn for_each_prefix(
            &self,
            prefix: &[u8],
            options: ::bedrock_world::StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8], &bytes::Bytes) -> ::bedrock_world::Result<::bedrock_world::StorageVisitorControl> + Send),
        ) -> ::bedrock_world::Result<::bedrock_world::StorageScanOutcome> {
            self.inner.for_each_prefix(prefix, options, visitor)
        }

        fn for_each_prefix_ref(
            &self,
            prefix: &[u8],
            options: ::bedrock_world::StorageReadOptions,
            visitor: &mut (dyn FnMut(::bedrock_world::StorageEntryRef<'_>) -> ::bedrock_world::Result<::bedrock_world::StorageVisitorControl> + Send),
        ) -> ::bedrock_world::Result<::bedrock_world::StorageScanOutcome> {
            self.inner.for_each_prefix_ref(prefix, options, visitor)
        }

        fn for_each_prefix_key(
            &self,
            prefix: &[u8],
            options: ::bedrock_world::StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> ::bedrock_world::Result<::bedrock_world::StorageVisitorControl> + Send),
        ) -> ::bedrock_world::Result<::bedrock_world::StorageScanOutcome> {
            self.inner.for_each_prefix_key(prefix, options, visitor)
        }

        fn write_batch(&self, batch: &::bedrock_world::StorageBatch) -> ::bedrock_world::Result<()> {
            self.inner.write_batch(batch)
        }

        fn flush(&self) -> ::bedrock_world::Result<()> {
            self.inner.flush()
        }

        fn compact(&self) -> ::bedrock_world::Result<()> {
            self.inner.compact()
        }
    }

    impl ::bedrock_world::PartitionedWorldStorage for BedrockLevelDbStorage {
        fn scan_keys_partitioned<T, I, F>(
            &self,
            options: ::bedrock_world::StorageReadOptions,
            init: I,
            visitor: F,
        ) -> ::bedrock_world::Result<(::bedrock_world::StorageScanOutcome, Vec<T>)>
        where
            T: Send,
            I: Fn() -> T + Send + Sync,
            F: Fn(&mut T, &[u8]) -> ::bedrock_world::Result<::bedrock_world::StorageVisitorControl> + Send + Sync,
        {
            let mut state = init();
            let outcome = self.inner.for_each_key(options, &mut |key| visitor(&mut state, key))?;
            Ok((outcome, vec![state]))
        }
    }

    pub mod nbt {
        pub use ::bedrock_world::nbt::*;
    }
}

mod cache {
    include!("renderer/cache.rs");
    use super::bedrock_api as bedrock_world;
}
#[path = "renderer/gpu.rs"]
mod gpu;
mod occupancy {
    use super::bedrock_api as bedrock_world;
    include!("renderer/occupancy.rs");
}
mod pipeline {
    use super::bedrock_api as bedrock_world;
    include!("renderer/pipeline.rs");
}

/// Render source backed by `bedrock-world` automatic world opening.
pub type WorldRenderSource = pipeline::LevelDbRenderSource;

pub use pipeline::{
    AtlasRenderOptions, BakeDiagnostics, BakeOptions, BlockBoundaryRenderOptions,
    BlockVolumeRenderOptions, ChunkRegion, ChunkTileLayout, DEFAULT_PALETTE_VERSION,
    DecodedTileImage, DepthPlane, FastRgbaZstdHeader, FastRgbaZstdTile, HeightPlane, ImageFormat,
    MAX_RENDER_THREADS, MAX_TILE_SIZE_PIXELS, MapRenderSession, MapRenderSessionConfig,
    MapRenderer, PlannedTile, RENDERER_CACHE_VERSION, RegionBake, RegionBakePayload, RegionCoord,
    RegionLayout, RenderBackend, RenderCachePolicy, RenderCancelFlag, RenderChunkSource,
    RenderCpuPipelineOptions, RenderDiagnostics, RenderDiagnosticsSink, RenderExecutionProfile,
    RenderGpuBackend, RenderGpuDiagnostics, RenderGpuFallbackPolicy, RenderGpuOptions,
    RenderGpuPipelineLevel, RenderJob, RenderLayout, RenderMemoryBudget, RenderMode,
    RenderOptions, RenderPerformanceOptions, RenderPerformanceProfile, RenderPipelineStats,
    RenderProgress, RenderProgressSink, RenderSurfaceLoadPolicy, RenderTaskControl,
    RenderThreadingOptions, RenderTileOutputOptions, RenderTilePriority, RenderWebTilesResult,
    ResolvedRenderBackend, RgbaPlane, SurfacePlane, SurfacePlaneAtlas, SurfaceRenderOptions,
    TerrainLightingOptions, TerrainLightingPreset, TileCache, TileCacheKey, TileCoord, TileImage,
    TilePathScheme, TilePixelFormat, TileReadySource, TileSet, TileStreamEvent, TileStreamEventV2,
    decode_fast_rgba_zstd, decode_fast_rgba_zstd_header, encode_fast_rgba_zstd,
    encode_fast_rgba_zstd_with_validation, tile_cache_validation_value,
};

pub use occupancy::{
    TileOccupancyEntry, TileOccupancyIndex, TileOccupancyIndexRequest, TileOccupancyIndexResult,
    TileOccupancyIndexSource, load_or_build_tile_occupancy_index_blocking,
    tile_occupancy_cache_path,
};

#[cfg(feature = "async")]
pub use cache::validate_chunk_fingerprints_async;
pub use cache::{
    ChunkFingerprintInput, TILE_AUTHORITY_FLAG_EMPTY, TILE_AUTHORITY_FLAG_NON_EMPTY,
    TileAuthorityBlobReader, TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityChunkState,
    TileAuthorityChunkTileRef, TileAuthorityCommit, TileAuthorityDependency, TileAuthorityEntry,
    TileAuthorityFreeExtent, TileAuthorityIndexSnapshot, WorldCacheIdentity,
    render_backend_cache_slug, render_cache_validation_seed_from_signature,
    render_gpu_backend_cache_slug, render_mode_cache_slug, render_preset_cache_signature,
    render_preset_cache_validation_seed, tile_payload_fingerprint,
    validate_chunk_fingerprints_parallel, world_cache_id, world_cache_identity,
    world_cache_signature,
};

pub use ::bedrock_world::{ChunkBounds, ChunkPos, Dimension, NbtTag};