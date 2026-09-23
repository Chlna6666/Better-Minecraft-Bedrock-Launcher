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
    BlockEntityRewriteStatus, BlockEntityRewriter, VanillaBlockEntityRewriter,
    rewrite_block_entity_chunk, rewrite_block_entity_sign_text,
};
pub use state::{
    BlockFace, BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateMigrator,
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite, DoorBlockStates, HorizontalDirection, RedstoneBlockStates,
    SlabBlockStates, StairBlockStates, StairCorner, TrapdoorBlockStates, VerticalHalf,
    read_block_state_nbt,
};
pub use query::{
    BlockStateBatchStats, BlockStateQueryControl, BlockStateQueryResult, BlockStateView,
};
pub use version::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    BlockUpgradeData, LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats, LegacyNumericBlockUpgradeTable,
    LegacyNumericBlockUpgradeTableStats, PINNED_BLOCK_MIGRATION_CORPUS_FILES,
    PINNED_BLOCK_STATE_SCHEMA_FILES,
    PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT, PINNED_BLOCK_UPGRADE_SCHEMA_VERSION,
    PINNED_LEGACY_BLOCK_ID_MAP_FILE, PINNED_LEGACY_ID_META_1_9_TABLE_FILE,
    PINNED_LEGACY_ID_META_1_12_TABLE_FILE, PinnedBlockMigrationBundle, PinnedCorpusFileSpec,
    VanillaBlockStatePalette, load_pinned_block_state_catalog,
    load_pinned_block_migration_bundle_for_target_from_dir,
    load_pinned_block_migration_bundle_from_dir,
    load_pinned_block_state_catalog_for_target, load_pinned_block_upgrade_data_for_palette,
    verify_pinned_block_migration_corpus,
};
