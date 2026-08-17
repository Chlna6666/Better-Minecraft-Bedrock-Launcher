//! Historical Minecraft Bedrock format migration.
//!
//! Migration is explicit and version-aware. Unknown future data is never rewritten implicitly.

pub mod actor;
pub mod biome;
pub mod block_state_graph;
pub mod block_state_upgrade;
pub mod chunk;
pub mod historical_chunk;
pub mod legacy_import;
pub mod level_dat;
pub mod plan;
pub mod player;

pub use actor::{
    ActorMigrationAction, ActorMigrationReport, actor_storage_compatibility,
    classify_actor_migration, migrate_inline_actor_chunk_blocking,
};
pub use biome::promote_data2d_to_data3d;
pub use block_state_graph::{BlockStateMigrationGraph, BlockStateMigrationStep};
pub use block_state_upgrade::{
    BlockStateUpgradeResult, BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
    BlockStateValueRewrite,
};
pub use chunk::{
    HistoricalChunkMigrationOptions, HistoricalChunkMigrationReport,
    migrate_historical_chunk_blocking,
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
pub use level_dat::{LevelDatMigrationOptions, migrate_level_dat_document};
pub use plan::{ChunkMigrationTarget, MigrationBlocker, WorldMigrationPlan};
pub use player::{
    PlayerMigrationReport, embedded_player_bytes, migrate_embedded_player_blocking,
};
