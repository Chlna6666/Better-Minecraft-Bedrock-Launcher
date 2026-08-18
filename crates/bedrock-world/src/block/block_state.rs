//! Minecraft Bedrock BlockState identity and explicit storage-version rewriting.

mod block_state_graph;
mod block_state_upgrade;
mod identity;

use crate::block::version::AuthoritativeBlockStateCatalog;
use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};

pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};

/// BlockState version writer used only when a caller explicitly selects another persisted version.
pub trait BlockStateMigrator: Send + Sync {
    /// Writes one BlockState for the requested persisted `version` value.
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
