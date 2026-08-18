//! Explicit BlockState conversion between persisted Minecraft Bedrock storage versions.

mod block_state_graph;
mod block_state_upgrade;

use crate::block::version::AuthoritativeBlockStateCatalog;
use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};

pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};

/// Common semantic BlockState conversion backend used only by explicit cross-version operations.
pub trait BlockStateMigrator: Send + Sync {
    /// Converts one BlockState to the requested persisted storage version.
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
                "authoritative BlockState data outputs version {}, but conversion targets {target_version}; load target-bound version data first",
                self.output_version().raw()
            )));
        }
        self.upgrade(state)
    }
}
