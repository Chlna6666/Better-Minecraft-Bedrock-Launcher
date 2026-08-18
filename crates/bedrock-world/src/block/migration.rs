//! Version-aware Bedrock block-state migration.
//!
//! Lightweight caller-defined rules and authoritative external corpora intentionally coexist. The
//! generic graph is useful for applications with a narrow target schema; the authoritative catalog
//! executes complete ordered Bedrock upgrade-schema documents without forcing those data files into
//! core model types.

mod authoritative;
mod block_state_graph;
mod block_state_upgrade;
mod corpus;
mod legacy_numeric;

use crate::chunk::palette::BlockState;
use crate::error::{BedrockWorldError, Result};

pub use authoritative::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
};
pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};
pub use corpus::{
    PINNED_BLOCK_STATE_SCHEMA_FILES, PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT,
    PINNED_BLOCK_UPGRADE_SCHEMA_VERSION, PINNED_LEGACY_BLOCK_ID_MAP_FILE,
    PINNED_LEGACY_ID_META_1_9_TABLE_FILE, PINNED_LEGACY_ID_META_1_12_TABLE_FILE,
    load_pinned_block_state_catalog, load_pinned_block_state_catalog_for_target,
};
pub use legacy_numeric::{LegacyNumericBlockStateTable, LegacyNumericBlockStateTableStats};

/// Common semantic BlockState migration backend used by chunk/world migration.
pub trait BlockStateMigrator: Send + Sync {
    /// Migrates one BlockState to the requested storage version.
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
                "authoritative BlockState catalog outputs version {}, but world migration targets {target_version}; build a target-bound catalog with load_pinned_block_state_catalog_for_target",
                self.output_version().raw()
            )));
        }
        self.upgrade(state)
    }
}
