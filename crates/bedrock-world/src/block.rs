//! Blocks, block states, palettes, block-entity data and versioned block-state migration.

mod state;
/// Preservation-first block-entity NBT migration.
pub mod entity_migration;
/// Version-aware block-state migration rules, authoritative schema execution and legacy numeric maps.
pub mod migration;

pub use crate::chunk::position::BlockPos;
pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::parsed::{BlockEntityRecord, ParsedBlockEntity};
pub use entity_migration::{
    BlockEntityChunkMigrationReport, BlockEntityMigrationContext, BlockEntityMigrationOutcome,
    BlockEntityMigrationStatus, BlockEntityMigrator, VanillaBlockEntityMigrator,
    migrate_block_entity_chunk_blocking, migrate_block_entity_chunk_to_modern_blocking,
};
pub use migration::{
    AuthoritativeBlockStateCatalog, BlockStateMigrationGraph, BlockStateMigrationStep,
    BlockStateMigrator, BlockStateSchemaSource, BlockStateStorageVersion, BlockStateUpgradeResult,
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, BlockStateValueRewrite,
    LegacyNumericBlockStateTable, LegacyNumericBlockStateTableStats,
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PINNED_BLOCK_STATE_SCHEMA_FILES,
    PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT, PINNED_BLOCK_UPGRADE_SCHEMA_VERSION,
    PINNED_LEGACY_BLOCK_ID_MAP_FILE, PINNED_LEGACY_ID_META_1_9_TABLE_FILE,
    PINNED_LEGACY_ID_META_1_12_TABLE_FILE, PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir, load_pinned_block_state_catalog,
    load_pinned_block_state_catalog_for_target, verify_pinned_block_migration_corpus,
};
