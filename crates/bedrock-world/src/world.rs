//! High-level Minecraft Bedrock world lifecycle and data access.

mod bedrock_world;
mod biome_downgrade;
mod biome_upgrade;
/// Filesystem discovery for Minecraft Bedrock world folders.
pub mod discover;
mod downgrade;
mod legacy_terrain;
mod level_dat;
mod player_storage;
mod pocket_chunks_dat;
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
pub use level_dat::{SubChunkVersionCount, WorldVersions};
pub use crate::parsed::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
pub use pocket_chunks_dat::{
    PocketChunksDatImportOptions, PocketChunksDatImportReport,
    import_pocket_chunks_dat_records_blocking,
};
pub use subchunk_upgrade::WorldSubChunkUpgradeReport;
pub use upgrade::{UpgradeAction, UpgradeIssue, UpgradeLoss, UpgradePlan};
