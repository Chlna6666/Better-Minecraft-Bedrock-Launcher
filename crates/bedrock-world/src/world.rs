//! High-level Minecraft Bedrock world lifecycle and data access.

mod bedrock_world;
mod biome_downgrade;
mod biome_upgrade;
/// Filesystem discovery for Minecraft Bedrock world folders.
pub mod discover;
mod downgrade;
mod legacy_terrain;
mod level_dat;
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
pub(crate) mod surface;
mod upgrade;

pub use bedrock_world::*;
pub use biome_downgrade::BiomeData2dDowngradeReport;
pub use biome_upgrade::BiomeData3dUpgradeReport;
pub use downgrade::{
    DowngradeAction, DowngradeIssue, DowngradeLoss, DowngradePlan, DowngradeRequirement,
};
pub use level_dat::{LevelChunkVersionCount, SubChunkVersionCount, WorldVersions};
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
pub use crate::parsed::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
pub use pocket_chunks_dat::{
    PocketChunksDatImportOptions, PocketChunksDatImportReport,
    import_pocket_chunks_dat_records_blocking,
};
pub use pocket_entities_dat::{
    PocketEntitiesDatDocument, PocketEntitiesDatImportOptions, PocketEntitiesDatImportReport,
    import_pocket_entities_dat_records_blocking, read_pocket_entities_dat,
    write_pocket_entities_dat_atomic,
};
pub use subchunk_upgrade::WorldSubChunkUpgradeReport;
pub use upgrade::{UpgradeAction, UpgradeIssue, UpgradeLoss, UpgradePlan};
