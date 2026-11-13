//! Minecraft Bedrock BlockState storage versions and authoritative historical version data.

mod authoritative;
pub mod corpus;
pub mod corpus_bundle;
mod legacy_numeric;
mod legacy_numeric_upgrade;
mod target;
pub mod upgrade_data;
mod vanilla_palette;

pub use authoritative::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
};
pub use corpus::{
    PINNED_BLOCK_STATE_SCHEMA_FILES, PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT,
    PINNED_BLOCK_UPGRADE_SCHEMA_VERSION, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
};
pub use corpus_bundle::{
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
};
pub use legacy_numeric::{
    LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats,
};
pub use legacy_numeric_upgrade::{
    LegacyNumericBlockUpgradeTable, LegacyNumericBlockUpgradeTableStats,
};
pub use target::{BlockStateVersionMatch, BlockStateVersionTarget};
pub use upgrade_data::BlockUpgradeData;
pub use vanilla_palette::VanillaBlockStatePalette;
