//! Tools for inspecting, migrating and editing Minecraft Bedrock worlds.
//!
//! `bedrock-world` owns Minecraft world semantics above the raw storage engine: `level.dat`, Bedrock
//! NBT, chunk/subchunk formats, BlockState, actors, block entities, players, maps, villages, historical
//! codecs, migrations, compatibility auditing and typed edits. Raw Mojang LevelDB mechanics remain in
//! `bedrock-leveldb`.
//!
//! # Public API layers
//!
//! New consumers should prefer the grouped facades:
//!
//! - [`model`] — semantic world/chunk/block/entity/player data types.
//! - [`codec`] — Bedrock NBT, chunk/subchunk and structure codecs.
//! - [`migration`] — historical formats, block-state migration graphs and importers.
//! - [`edit`] — typed guarded world modifications.
//! - [`audit`] — compatibility and integrity inspection.
//! - [`storage`] — raw world-storage abstraction and the `bedrock-leveldb` adapter.
//!
//! The historical root-level exports remain during the 0.6 transition. Internally, code should move
//! toward these layers so model types do not depend on editors/auditors and storage never depends on
//! Minecraft gameplay policy.
//!
//! The default APIs are deliberately lazy. Opening a [`BedrockWorld`] does not parse the full database;
//! callers choose targeted operations. Async wrappers use `tokio::task::spawn_blocking` so LevelDB/NBT
//! work does not execute on foreground async tasks.

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

/// Typed modern paletted-chunk block editing helpers.
pub mod block_edit;
/// Canonical semantic identity helpers for Bedrock block states.
pub mod block_state;
/// Directed version graph for historical BlockState schema migrations.
pub mod block_state_graph;
/// Data-driven historical block-state migration helpers.
pub mod block_state_upgrade;
/// Chunk keys, subchunk formats, palette data, and legacy terrain helpers.
pub mod chunk;
/// Historical world/chunk/subchunk compatibility and explicit write-policy helpers.
pub mod compatibility;
/// Whole-world compatibility scanning for mixed historical/modern record sets.
pub mod compatibility_scan;
/// Filesystem discovery for Bedrock world folders.
pub mod discover;
/// Crate-wide error and result types.
pub mod error;
/// Canonical decoding of legacy numeric terrain through caller-supplied authoritative mappings.
pub mod historical_chunk;
/// Read-only whole-world consistency and corruption auditing.
pub mod integrity;
/// Explicit pre-LevelDB Pocket Edition container import helpers.
pub mod legacy_import;
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

/// Semantic Minecraft world model types with no editor or migration policy attached.
pub mod model {
    pub use crate::chunk::key::{
        ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, ChunkKey, ChunkRecordTag,
        EncodedChunkKey, GlobalRecordKind, MapRecordId, ParsedVillageKey, VillageRecordKind,
    };
    pub use crate::chunk::legacy::{LegacyBiomeSample, LegacySubChunk, LegacyTerrain};
    pub use crate::chunk::model::{
        BlockPos, Chunk, ChunkPos, ChunkRecord, ChunkVersion, Dimension, EntityData,
    };
    pub use crate::chunk::palette::{BlockPalette, BlockState};
    pub use crate::chunk::subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
    pub use crate::parsed::{
        ActorRecord, ActorResolution, ActorSource, Biome2d, Biome3d, BlockEntityRecord,
        HardcodedSpawnAreaKind, HeightMap2d, ItemStack, MapKnownFields, MapPixels,
        ParsedActorDigest, ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData,
        ParsedChunkRecord, ParsedChunkRecordValue, ParsedDbEntry, ParsedDbValue, ParsedEntity,
        ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData, ParsedPlayer, ParsedVillageData,
        ParsedWorld,
    };
    pub use crate::player::{PlayerData, PlayerId};
}

/// Bedrock binary/NBT/chunk/structure codecs and decode policies.
pub mod codec {
    pub use crate::chunk::legacy::{
        LEGACY_SUBCHUNK_BLOCK_COUNT, LEGACY_SUBCHUNK_MIN_VALUE_LEN,
        LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN, LEGACY_TERRAIN_BLOCK_COUNT,
        LEGACY_TERRAIN_VALUE_LEN,
    };
    pub use crate::chunk::palette::block_storage_index;
    pub use crate::chunk::subchunk::{parse_subchunk, parse_subchunk_with_mode};
    pub use crate::mcstructure::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
        McStructureRotation, McStructureSize, read_mcstructure_file, write_mcstructure_file,
    };
    pub use crate::nbt::{
        NbtEvent, NbtReader, NbtRef, NbtTag, NbtValue, NbtView, NbtWriter, visit_nbt_events,
    };
    pub use crate::parsed::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
}

/// Historical format conversion, BlockState schema migration and legacy world import.
pub mod migration {
    pub use crate::block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
    pub use crate::block_state_upgrade::{
        BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
        BlockStateValueRewrite,
    };
    pub use crate::historical_chunk::{
        LegacyBlockMapping, LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource,
        ResolvedHistoricalSubChunk, ResolvedLegacyTerrain, resolve_legacy_subchunk,
        resolve_legacy_terrain,
    };
    pub use crate::legacy_import::{
        PocketChunksDatImportOptions, PocketChunksDatImportReport,
        import_pocket_chunks_dat_records_blocking,
    };
}

/// Typed, policy-guarded world editing and structure placement APIs.
pub mod edit {
    pub use crate::block_edit::{
        BlockEdit, BlockEditOptions, BlockEditResult, BlockEntityEdit, BlockStorageLayer,
        apply_block_edits_blocking, set_block_state_blocking,
    };
    pub use crate::mcstructure::{
        McStructurePlacement, McStructureRotation, McStructureWritePhase, McStructureWriteProgress,
        McStructureWriteResult,
    };
    pub use crate::query::{
        WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
        write_chunk_record_nbt_blocking,
    };
}

/// Compatibility classification, whole-world capability scans and integrity auditing.
pub mod audit {
    pub use crate::compatibility::{
        ActorStorageModel, ChunkCapabilities, CompatibilityLevel, SubChunkCodecKind,
        WorldCapabilities, WritePolicy,
    };
    pub use crate::compatibility_scan::{
        ChunkCompatibilitySummary, WorldCompatibilityReport, scan_world_compatibility_blocking,
    };
    pub use crate::integrity::{
        WorldIntegrityIssue, WorldIntegrityIssueKind, WorldIntegrityOptions, WorldIntegrityReport,
        WorldIntegritySeverity, WorldIntegrityStatus, audit_world_integrity_blocking,
    };
}

// Transitional root facade for existing BMCBL/Calcite consumers. New code should prefer the grouped
// modules above; root exports can be reduced after internal callers have migrated.
pub use block_edit::{
    BlockEdit, BlockEditOptions, BlockEditResult, BlockEntityEdit, BlockStorageLayer,
    apply_block_edits_blocking, set_block_state_blocking,
};
pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};
pub use chunk::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, BlockPalette, BlockPos, BlockState,
    Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, Dimension,
    EncodedChunkKey, EntityData, GlobalRecordKind, LEGACY_SUBCHUNK_BLOCK_COUNT,
    LEGACY_SUBCHUNK_MIN_VALUE_LEN, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN,
    LEGACY_TERRAIN_BLOCK_COUNT, LEGACY_TERRAIN_VALUE_LEN, LegacyBiomeSample, LegacySubChunk,
    LegacyTerrain, MapRecordId, ParsedVillageKey, SubChunk, SubChunkDecodeMode, SubChunkFormat,
    VillageRecordKind, block_storage_index,
};
pub use compatibility::{
    ActorStorageModel, ChunkCapabilities, CompatibilityLevel, SubChunkCodecKind, WorldCapabilities,
    WritePolicy,
};
pub use compatibility_scan::{
    ChunkCompatibilitySummary, WorldCompatibilityReport, scan_world_compatibility_blocking,
};
pub use discover::{WorldDiscovery, WorldSummary, discover_worlds};
pub use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub use historical_chunk::{
    LegacyBlockMapping, LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource,
    ResolvedHistoricalSubChunk, ResolvedLegacyTerrain, resolve_legacy_subchunk,
    resolve_legacy_terrain,
};
pub use integrity::{
    WorldIntegrityIssue, WorldIntegrityIssueKind, WorldIntegrityOptions, WorldIntegrityReport,
    WorldIntegritySeverity, WorldIntegrityStatus, audit_world_integrity_blocking,
};
pub use legacy_import::{
    PocketChunksDatImportOptions, PocketChunksDatImportReport,
    import_pocket_chunks_dat_records_blocking,
};
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
