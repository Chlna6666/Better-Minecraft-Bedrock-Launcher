//! Minecraft Bedrock blocks, BlockStates, palettes and BlockEntity records.

pub(crate) mod entity;
mod query;
pub(crate) mod state;
pub mod version;

pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::chunk::position::BlockPos;
pub use crate::scan::{BlockEntity, BlockEntityRecord};
pub use entity::{
    BlockEntityChunkRewriteReport, BlockEntityRewriteContext, BlockEntityRewriteOutcome,
    BlockEntityRewriteStatus, BlockEntityRewriter, VanillaBlockEntityRewriter,
    rewrite_block_entity_chunk, rewrite_sign_text,
};
pub use query::{
    BlockStateBatchStats, BlockStateQueryControl, BlockStateQueryResult, BlockStateView,
};
pub use state::{
    BlockFace, BlockSchema, BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateMigrator,
    BlockStateSchema, BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus,
    BlockStateUpgrader, BlockStateValueRewrite, DoorBlockStates, HorizontalDirection,
    RedstoneBlockStates, SlabBlockStates, StairBlockStates, StairCorner, StatePropertySchema,
    TrapdoorBlockStates, VerticalHalf, catalog, fluid_level, for_block, is_fluid,
    read_block_state_nbt, validate_state,
};
pub use version::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    BlockUpgradeData, LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockStateTableStats, LegacyNumericBlockUpgradeTable,
    LegacyNumericBlockUpgradeTableStats, PINNED_BLOCK_MIGRATION_CORPUS_FILES,
    PINNED_BLOCK_STATE_SCHEMA_FILES, PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT,
    PINNED_BLOCK_UPGRADE_SCHEMA_VERSION, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
    PinnedBlockMigrationBundle, PinnedCorpusFileSpec, VanillaBlockStatePalette,
};
