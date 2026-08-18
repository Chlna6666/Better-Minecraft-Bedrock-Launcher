//! Minecraft Bedrock blocks, BlockStates, palettes and BlockEntity records.

/// Minecraft Bedrock BlockState identity and persisted `version` history.
pub mod block_state;
/// Minecraft Bedrock BlockEntity NBT.
pub mod block_entity;
/// Authoritative BlockState storage-version data.
pub mod version;

pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::chunk::position::BlockPos;
pub use crate::parsed::{BlockEntityRecord, ParsedBlockEntity};
pub use block_entity::{
    BlockEntityChunkMigrationReport, BlockEntityMigrationContext, BlockEntityMigrationOutcome,
    BlockEntityMigrationStatus, BlockEntityMigrator, VanillaBlockEntityMigrator,
    migrate_block_entity_chunk_blocking, migrate_block_entity_chunk_to_modern_blocking,
};
pub use block_state::{
    BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateMigrator, BlockStateUpgradeResult,
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, BlockStateValueRewrite,
};
pub use version::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    LegacyNumericBlockStateTable, LegacyNumericBlockStateTableStats,
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PINNED_BLOCK_STATE_SCHEMA_FILES,
    PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT, PINNED_BLOCK_UPGRADE_SCHEMA_VERSION,
    PINNED_LEGACY_BLOCK_ID_MAP_FILE, PINNED_LEGACY_ID_META_1_9_TABLE_FILE,
    PINNED_LEGACY_ID_META_1_12_TABLE_FILE, PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir, load_pinned_block_state_catalog,
    load_pinned_block_state_catalog_for_target, verify_pinned_block_migration_corpus,
};
