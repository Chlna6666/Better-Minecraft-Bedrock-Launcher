//! Minecraft Bedrock BlockState identity and explicit storage-version rewriting.

mod migration;
mod upgrade;
mod identity;
mod nbt;
mod properties;

use crate::block::BlockState;
use crate::block::version::AuthoritativeBlockStateCatalog;
use crate::error::{BedrockWorldError, Result};

pub use migration::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use nbt::read_block_state_nbt;
pub use properties::{
    BlockFace, DoorBlockStates, HorizontalDirection, RedstoneBlockStates, SlabBlockStates,
    StairBlockStates, StairCorner, TrapdoorBlockStates, VerticalHalf,
};
pub use upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};

/// Converts one in-memory `BlockState` between explicitly selected Bedrock storage versions.
///
/// This operation does not read or write world storage. Implementations must use explicit migration
/// evidence and reject missing paths or states they cannot safely represent; they must not relabel an
/// old state with a target version without applying a known rule.
pub trait BlockStateMigrator: Send + Sync {
    /// Migrates one semantic block state to the requested persisted schema version.
    ///
    /// # Errors
    ///
    /// Returns an error when source/target version evidence cannot resolve the migration or a rewrite
    /// is ambiguous. Implementations may report additional authoritative target-data validation errors.
    fn migrate_to(&self, state: &BlockState, target_version: i32) -> Result<BlockState>;
}

impl BlockStateMigrator for BlockStateMigrationGraph {
    fn migrate_to(&self, state: &BlockState, target_version: i32) -> Result<BlockState> {
        BlockStateMigrationGraph::migrate_to(self, state, target_version)
    }
}

impl BlockStateMigrator for AuthoritativeBlockStateCatalog {
    fn migrate_to(&self, state: &BlockState, target_version: i32) -> Result<BlockState> {
        if self.output_version().raw() != target_version {
            return Err(BedrockWorldError::Validation(format!(
                "BlockState history data outputs version {}, requested {target_version}",
                self.output_version().raw()
            )));
        }
        self.upgrade(state)
    }
}
