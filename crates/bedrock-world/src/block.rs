//! Minecraft Bedrock blocks, BlockStates, palettes and BlockEntity records.

pub(crate) mod block_state;
pub(crate) mod block_entity;
pub(crate) mod version;

pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::chunk::position::BlockPos;
pub use crate::parsed::{BlockEntityRecord, ParsedBlockEntity};
pub use version::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    BlockUpgradeData, LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats, PINNED_BLOCK_STATE_SCHEMA_FILES,
    PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT, PINNED_BLOCK_UPGRADE_SCHEMA_VERSION,
    VanillaBlockStatePalette, load_pinned_block_state_catalog,
    load_pinned_block_state_catalog_for_target, load_pinned_block_upgrade_data_for_palette,
};

// Historical rule executors remain crate-private while the dev-stage API is rebuilt around concrete
// Bedrock data objects and version-specific writes. They are intentionally not compatibility exports.
pub(crate) use block_state::{
    BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateMigrator, BlockStateUpgradeResult,
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, BlockStateValueRewrite,
};
pub(crate) use block_entity::{
    BlockEntityChunkMigrationReport, BlockEntityMigrationContext, BlockEntityMigrationOutcome,
    BlockEntityMigrationStatus, BlockEntityMigrator, VanillaBlockEntityMigrator,
    migrate_block_entity_chunk_blocking, migrate_block_entity_chunk_to_modern_blocking,
};
pub(crate) use version::{
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
    PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir, verify_pinned_block_migration_corpus,
};
