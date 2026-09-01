//! Minecraft Bedrock blocks, BlockStates, palettes and BlockEntity records.

pub(crate) mod block_entity;
pub(crate) mod state;
pub(crate) mod version;
mod query;

pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::chunk::position::BlockPos;
pub use crate::scan::{BlockEntityRecord, BlockEntity};
pub use block_entity::{
    BlockEntityChunkRewriteReport, BlockEntityRewriteContext, BlockEntityRewriteOutcome,
    BlockEntityRewriteStatus, BlockEntityRewriter, rewrite_block_entity_chunk,
};
pub use state::{
    BlockFace, DoorBlockStates, HorizontalDirection, RedstoneBlockStates, SlabBlockStates,
    StairBlockStates, StairCorner, TrapdoorBlockStates, VerticalHalf, read_block_state_nbt,
};
pub use query::{
    BlockStateBatchStats, BlockStateQueryControl, BlockStateQueryResult, BlockStateView,
};
pub use version::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    BlockUpgradeData, LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats, LegacyNumericBlockUpgradeTable,
    LegacyNumericBlockUpgradeTableStats, PINNED_BLOCK_STATE_SCHEMA_FILES,
    PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT, PINNED_BLOCK_UPGRADE_SCHEMA_VERSION,
    VanillaBlockStatePalette, load_pinned_block_state_catalog,
    load_pinned_block_state_catalog_for_target, load_pinned_block_upgrade_data_for_palette,
};

// BlockState historical rule executors remain crate-private while public writes are rebuilt around
// concrete target-version data. Third-party BlockEntity tooling can instead implement
// `BlockEntityRewriter`, whose contract requires explicit caller evidence and preservation of fields it
// does not own.
pub(crate) use block_entity::{
    VanillaBlockEntityRewriter, rewrite_block_entity_sign_text,
};
pub(crate) use state::{
    BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateMigrator, BlockStateUpgradeResult,
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, BlockStateValueRewrite,
};
pub(crate) use version::{
    PINNED_BLOCK_MIGRATION_CORPUS_FILES, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
    PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir, verify_pinned_block_migration_corpus,
};
