//! Multi-version Minecraft Bedrock player records and persisted player-data formats.
//!
//! Reads automatically detect the source storage/data representation and never convert it. Callers
//! may write the same representation directly or request an explicit conversion through
//! [`conversion`].

mod data;
/// Explicit cross-format player-data conversion.
pub mod conversion;
/// Persisted player-data format detection.
pub mod format;
/// Read/write access to `level.dat.Player`, `~local_player` and `player_<xuid>`.
pub mod storage;

pub use crate::parsed::{ItemStack, ParsedPlayer};
pub use conversion::{
    PlayerDataTarget, convert_player_data, player_conversion_compatibility,
};
pub use data::{PlayerData, PlayerId};
pub use format::{PlayerDataFormat, PlayerStorage, SavedItemFormat};
pub use storage::{
    delete_player_record, read_level_dat_player, read_player_record,
    read_player_record_with_level, remove_level_dat_player, write_level_dat_player,
    write_player_record,
};
