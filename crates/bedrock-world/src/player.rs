//! Minecraft Bedrock player data from `level.dat.Player`, `~local_player` and `player_<id>`.
//!
//! Reads retain the persisted player record and version evidence. Normal writes only write back to the
//! same Bedrock record family; no game-version or saved-item rewrite is performed implicitly.

mod data;
mod experience;
mod game_mode;
mod inventory;
mod level_dat;
mod local_player;
mod position;
mod server_player;
mod spawn;

pub use crate::parsed::{ItemStack, ParsedPlayer};
pub use data::{PlayerData, PlayerId, SavedItemKind};
pub use inventory::{PlayerInventoryEntry, PlayerInventorySlot};
pub use level_dat::{read_level_dat_player, remove_level_dat_player, write_level_dat_player};
pub use local_player::{
    delete_local_player, read_local_player, read_local_player_with_level, write_local_player,
};
pub use server_player::{
    delete_server_player, read_server_player, read_server_player_with_level, write_server_player,
};
