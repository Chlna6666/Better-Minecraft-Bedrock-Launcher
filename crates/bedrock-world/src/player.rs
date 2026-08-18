//! Minecraft Bedrock player data from `level.dat.Player`, `~local_player` and `player_<xuid>`.
//!
//! Reads detect the source and saved-item generation automatically. Normal writes keep the caller's
//! selected Bedrock record; no game-version rewrite is performed implicitly.

mod data;
mod level_dat;
mod local_player;
mod server_player;

pub use crate::parsed::{ItemStack, ParsedPlayer};
pub use data::{PlayerData, PlayerId, SavedItemKind};
pub use level_dat::{read_level_dat_player, remove_level_dat_player, write_level_dat_player};
pub use local_player::{
    delete_local_player, read_local_player, read_local_player_with_level, write_local_player,
};
pub use server_player::{
    delete_server_player, read_server_player, read_server_player_with_level, write_server_player,
};
