//! Historical Minecraft Bedrock format migration.
//!
//! Migration is explicit and version-aware. Unknown future data is never rewritten implicitly.

pub mod actor;
pub mod block_state_graph;
pub mod block_state_upgrade;
pub mod historical_chunk;
pub mod legacy_import;

pub use actor::{ActorMigrationAction, actor_storage_compatibility, classify_actor_migration};
pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};
pub use historical_chunk::{
    LegacyBlockMapping, LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource,
    ResolvedHistoricalSubChunk, ResolvedLegacyTerrain, resolve_legacy_subchunk,
    resolve_legacy_terrain,
};
pub use legacy_import::{
    PocketChunksDatImportOptions, PocketChunksDatImportReport,
    import_pocket_chunks_dat_records_blocking,
};
