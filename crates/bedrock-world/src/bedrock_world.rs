//! Tools for inspecting and editing Minecraft Bedrock `LevelDB` worlds.
//!
//! `bedrock-world` focuses on world-level concepts layered above
//! `bedrock-leveldb`: `level.dat`, little-endian Bedrock NBT, chunk and
//! subchunk decoding, player records, entity summaries, biome data, map/village
//! records, and scan-oriented APIs for launchers or offline tools.
//!
//! The default APIs are deliberately lazy. Opening a [`BedrockWorld`] does not
//! parse the full database; callers choose targeted operations such as
//! [`read_level_dat`], [`BedrockWorld::list_players_blocking`],
//! [`BedrockWorld::parse_chunk_blocking`], or [`BedrockWorld::scan_items_blocking`].
//! Async wrappers use `tokio::task::spawn_blocking` so `LevelDB` and NBT work does
//! not run on foreground async tasks.
//!
//! # Features
//!
//! docs.rs builds this crate with all features enabled. Default builds enable
//! `async` and the `bedrock-leveldb` backend. Disable default features for
//! pure parsing, in-memory storage, `level.dat`, and NBT workflows that should
//! not depend on a database backend.

#![deny(missing_docs)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::items_after_test_module,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::type_complexity,
    clippy::wildcard_imports
)]

/// Canonical semantic identity helpers for Bedrock block states.
pub mod block_state;
/// Chunk keys, subchunk formats, palette data, and legacy terrain helpers.
pub mod chunk;
/// Filesystem discovery for Bedrock world folders.
pub mod discover;
/// Crate-wide error and result types.
pub mod error;
/// `level.dat` header parsing, validation, and atomic write helpers.
pub mod level_dat;
/// Minecraft Bedrock `.mcstructure` files and world placement helpers.
pub mod mcstructure;
/// Little-endian Bedrock NBT reader and writer.
pub mod nbt;
mod nbt_ref;
/// Structured parsers for world, chunk, entity, biome, map, and village data.
pub mod parsed;
/// Player identifiers and raw player record helpers.
pub mod player;
/// Professional map query helpers and guarded world edits.
pub mod query;
/// Exact non-rectangular chunk selection primitives and queries.
pub mod selection_query;
/// Storage abstraction and LevelDB backend adapters.
pub mod storage;
/// Terrain surface role helpers shared by chunk decoding and render sampling.
pub mod surface;
/// High-level lazy world handle and scan/render helpers.
pub mod world;

pub use chunk::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, BlockPalette, BlockPos, BlockState,
    Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, Dimension,
    EncodedChunkKey, EntityData, GlobalRecordKind, LEGACY_SUBCHUNK_BLOCK_COUNT,
    LEGACY_SUBCHUNK_MIN_VALUE_LEN, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN,
    LEGACY_TERRAIN_BLOCK_COUNT, LEGACY_TERRAIN_VALUE_LEN, LegacyBiomeSample, LegacySubChunk,
    LegacyTerrain, MapRecordId, ParsedVillageKey, SubChunk, SubChunkDecodeMode, SubChunkFormat,
    VillageRecordKind, block_storage_index,
};
pub use discover::{WorldDiscovery, WorldSummary, discover_worlds};
pub use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub use level_dat::{
    LevelDatDocument, LevelDatHeader, LevelDatReadWarning,
    initialize_level_dat_random_seed_if_missing, parse_level_dat_document, read_level_dat,
    read_level_dat_document, read_level_dat_random_seed, write_level_dat_atomic,
    write_level_dat_document,
};
#[cfg(feature = "async")]
pub use level_dat::{read_level_dat_async, write_level_dat_atomic_async};
pub use mcstructure::{
    McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
    McStructureRotation, McStructureSize, McStructureWritePhase, McStructureWriteProgress,
    McStructureWriteResult, read_mcstructure_file, write_mcstructure_file,
};
pub use nbt::{
    NbtEvent, NbtReader, NbtRef, NbtTag, NbtValue, NbtView, NbtWriter, visit_nbt_events,
};
pub use parsed::{
    ActorRecord, ActorResolution, ActorSource, Biome2d, Biome3d, BlockEntityRecord,
    HardcodedSpawnAreaKind, HeightMap2d, ItemStack, MapKnownFields, MapPixels, ParsedActorDigest,
    ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData, ParsedChunkRecord,
    ParsedChunkRecordValue, ParsedDbEntry, ParsedDbValue, ParsedEntity, ParsedGlobalData,
    ParsedHardcodedSpawnArea, ParsedMapData, ParsedPlayer, ParsedVillageData, ParsedWorld,
    RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport,
};
pub use player::{PlayerData, PlayerId};
pub use query::{
    BlockEntityOverlay, BlockTip, ChunkDetail, ChunkRecordDetail, ChunkRecordFingerprint,
    ChunkRecordQuery, ChunkRecordQueryResult, EntityOverlay, HardcodedSpawnAreaOverlay,
    PendingTickOverlay, RegionOverlayQuery, RegionOverlayQueryOptions, SelectionStats,
    SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize, VillageOverlay, VillageOverlayIndex,
    WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
    fingerprint_chunk_records_many_blocking, fingerprint_chunk_records_many_blocking_with_control,
    is_bedrock_slime_chunk, is_slime_chunk, query_block_tip_blocking, query_chunk_detail_blocking,
    query_chunk_records_many_blocking, query_chunk_records_many_blocking_with_control,
    query_region_overlays_blocking, query_region_overlays_blocking_with_control,
    query_selection_stats_blocking, query_slime_chunk_windows, write_chunk_record_nbt_blocking,
};
pub use selection_query::{
    ExactChunkSelection, query_selection_stats_chunks_blocking,
    query_selection_stats_exact_blocking, rasterize_chunk_line,
};
pub use storage::{
    MemoryStorage, POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN, PartitionedWorldStorage,
    PocketChunksDatStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag, StorageEntry,
    StorageEntryRef, StorageOp, StoragePipelineOptions, StorageProgressSink, StorageReadOptions,
    StorageScanMode, StorageScanOutcome, StorageScanProgress, StorageThreadingOptions,
    StorageVisitorControl, WorldStorage, backend::BedrockLevelDbStorage,
};
pub use world::{
    BedrockWorld, BiomeDataRequirement, CancelFlag, ChunkBlockEntity, ChunkBounds, ChunkData,
    ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority, ChunkLoadStats, ExactSurfaceBiomeLoad,
    ExactSurfaceSubchunkPolicy, OpenOptions, ProgressSink, SubchunkDataRequirement, SurfaceColumn,
    SurfaceColumnOptions, TerrainColumnBiome, TerrainColumnOverlay, TerrainColumnSample,
    TerrainColumnSamples, TerrainColumnWater, TerrainSampleSource, TerrainSurfaceRole,
    WorldChunkQueryRegion, WorldChunkQueryRegionData, WorldChunkQueryRegionLoadOptions,
    WorldExecutor, WorldFormat, WorldFormatHint, WorldPipelineOptions, WorldScanOptions,
    WorldScanProgress, WorldStorageHandle, WorldThreadingOptions, WorldTransaction,
    terrain_surface_overlay_alpha, terrain_surface_role,
};
