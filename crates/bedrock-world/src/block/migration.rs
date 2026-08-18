//! Version-aware Bedrock block-state migration.
//!
//! This is a block-domain capability rather than a crate-wide software layer. Migration is explicit,
//! data-driven and refuses unresolved or future states unless a caller supplies authoritative rules.

mod block_state_graph;
mod block_state_upgrade;

pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};
