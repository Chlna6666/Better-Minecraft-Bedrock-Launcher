//! Minecraft Bedrock BlockState storage versions and authoritative historical version data.

mod authoritative;
mod corpus;
mod corpus_bundle;
mod legacy_numeric;
mod upgrade_data;
mod vanilla_palette;

pub use authoritative::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
};
pub use corpus::{
    PINNED_BLOCK_STATE_SCHEMA_FILES, PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT,
    PINNED_BLOCK_UPGRADE_SCHEMA_VERSION, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
    load_pinned_block_state_catalog, load_pinned_block_state_catalog_for_target,
};
pub use corpus_bundle::{
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir, verify_pinned_block_migration_corpus,
};
pub use legacy_numeric::{
    LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats,
};
pub use upgrade_data::{BlockUpgradeData, load_pinned_block_upgrade_data_for_palette};
pub use vanilla_palette::VanillaBlockStatePalette;
