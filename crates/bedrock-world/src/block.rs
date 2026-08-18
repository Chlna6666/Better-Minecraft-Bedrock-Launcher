//! Blocks, block states, palettes, block-entity data and versioned block-state migration.

mod state;
/// Version-aware block-state migration rules and migration graphs.
pub mod migration;

pub use crate::chunk::position::BlockPos;
pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::parsed::{BlockEntityRecord, ParsedBlockEntity};
pub use migration::{
    BlockStateMigrationGraph, BlockStateMigrationStep, BlockStateUpgradeResult,
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, BlockStateValueRewrite,
};
