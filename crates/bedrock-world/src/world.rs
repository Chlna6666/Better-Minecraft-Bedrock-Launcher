//! High-level Minecraft Bedrock world lifecycle and data access.

mod bedrock_world;
mod biome;
mod biome_downgrade;
mod biome_upgrade;
mod block_query;
mod chunk_data;
mod chunk_presence;
mod create;
/// Filesystem discovery for Minecraft Bedrock world folders.
pub mod discover;
mod height_map;
mod legacy_terrain;
mod level_dat;
mod migration;
mod player_classic_saved_items;
mod player_medieval_saved_items;
mod player_modern_saved_items;
mod player_saved_items;
mod player_storage;
mod pocket_chunks_dat;
mod pocket_entities_dat;
mod pocket_world_storage;
mod subchunk_numeric;
mod subchunk_upgrade;
mod world_clock;
pub(crate) mod surface;

pub use crate::parsed::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
pub use bedrock_world::*;
pub use block_query::BlockStateQueryResult;
pub use chunk_presence::{ChunkPresence, ChunkPresenceMode};
pub use height_map::{ChunkHeightMap, ChunkHeightMapStatus};
/// Minecraft Bedrock world-open options.
///
/// This explicit name is the canonical public API and avoids collision with
/// storage/backend-specific `OpenOptions` types.
pub use bedrock_world::OpenOptions as BedrockWorldOpenOptions;
/// Compatibility name retained for existing internal and downstream callers.
///
/// New code should use [`BedrockWorldOpenOptions`]. This alias is hidden from
/// generated API documentation so the explicit world-domain name remains the
/// authoritative public surface.
#[doc(hidden)]
pub type OpenOptions = BedrockWorldOpenOptions;
pub use biome_downgrade::BiomeData2dDowngradeReport;
pub use biome_upgrade::BiomeData3dUpgradeReport;
pub use create::{
    BedrockDifficulty, BedrockGameMode, BedrockWorldCreateOptions, BedrockWorldSpawn,
};
pub use level_dat::{LevelChunkVersionCount, SubChunkVersionCount, WorldVersions};
pub use migration::{
    BedrockWorldDowngradeOptions, BedrockWorldDowngradeReport, BedrockWorldMigrationGap,
    BedrockWorldSubChunkUpgradeOptions, BedrockWorldUpgradeOptions, BedrockWorldUpgradeReport,
};
pub use player_classic_saved_items::{
    PlayerClassicSavedItemCheckEntry, WorldPlayerClassicSavedItemCheckReport,
};
pub use player_medieval_saved_items::{
    PlayerMedievalSavedItemCheckEntry, WorldPlayerMedievalSavedItemCheckReport,
};
pub use player_modern_saved_items::{
    PlayerModernSavedItemCheckEntry, WorldPlayerModernSavedItemCheckReport,
};
pub use player_saved_items::{
    PlayerSavedItemCheckEntry, PlayerSavedItemStorage, WorldPlayerSavedItemCheckReport,
};
pub use pocket_chunks_dat::{
    PocketChunksDatImportCheck, PocketChunksDatImportOptions, PocketChunksDatImportReport,
    check_pocket_chunks_dat_leveldb_import_blocking, import_pocket_chunks_dat_records_blocking,
};
pub use pocket_entities_dat::{
    PocketEntitiesDatDocument, PocketEntitiesDatImportOptions, PocketEntitiesDatImportReport,
    import_pocket_entities_dat_records_blocking, read_pocket_entities_dat,
    write_pocket_entities_dat_atomic,
};
pub use subchunk_upgrade::BedrockWorldSubChunkUpgradeReport;
