//! Minecraft Bedrock player data from `level.dat.Player`, `~local_player` and `player_<id>`.
//!
//! Reads retain the persisted player record and version evidence. Normal writes only write back to the
//! same Bedrock record family; no game-version or saved-item rewrite is performed implicitly.

mod abilities;
mod actor;
mod actor_state;
mod attributes;
mod data;
mod effects;
mod equipment;
mod experience;
mod game_mode;
mod inventory;
mod legacy_saved_items;
mod level_dat;
mod local_player;
mod mcpe_0_6_1;
mod modern_saved_items;
mod position;
mod saved_item_format;
mod server_player;
mod spawn;
mod storage;

pub use crate::parsed::{ItemStack, ParsedPlayer};
pub use abilities::PlayerAbilities;
pub use attributes::PlayerAttribute;
pub use data::{PlayerData, PlayerId, SavedItemKind};
pub use effects::PlayerActiveEffect;
pub use equipment::{PlayerArmor, PlayerEquipmentEntry, PlayerOffhand};
pub use inventory::{PlayerInventoryEntry, PlayerInventorySlot};
pub use level_dat::{read_level_dat_player, remove_level_dat_player, write_level_dat_player};
pub use local_player::{
    LocalPlayerStorageMoveReport, delete_local_player, move_level_dat_player_to_local_player,
    move_local_player_to_level_dat, read_local_player, read_local_player_with_level,
    write_local_player,
};
pub use mcpe_0_6_1::{Mcpe061PlayerCheckReport, write_mcpe_0_6_1_level_dat_player};
pub use server_player::{
    PlayerKeyRecord, delete_player_key, delete_server_player, read_player_key, read_server_player,
    read_server_player_with_level, write_player_key, write_server_player,
};
pub use storage::{
    LocalPlayerRecords, LocalPlayerStorage, PlayerStorageOverview, inspect_player_storage,
};
