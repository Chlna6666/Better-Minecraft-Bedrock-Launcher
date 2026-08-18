//! Bedrock player records, inventory item data and historical player migration.

mod data;
/// Historical player-record migration.
pub mod migration;

pub use crate::parsed::{ItemStack, ParsedPlayer};
pub use data::{PlayerData, PlayerId};
pub use migration::{
    PlayerMigrationReport, embedded_player_bytes, migrate_embedded_player_blocking,
};
