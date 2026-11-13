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
mod item_representation;
pub mod level_dat;
pub mod local;
pub mod mcpe_0_6_1;
mod overview;
mod position;
pub mod server;
mod spawn;
mod storage;

pub use crate::scan::{ItemStack, PlayerEntry};
pub use abilities::PlayerAbilities;
pub use attributes::PlayerAttribute;
pub use data::{PlayerData, PlayerId, SavedItemKind};
pub use effects::PlayerActiveEffect;
pub use equipment::{PlayerArmor, PlayerEquipmentEntry, PlayerOffhand};
pub use inventory::{PlayerInventoryEntry, PlayerInventorySlot};
pub use storage::{
    LocalPlayerRecords, LocalPlayerStorage, PlayerStorageOverview, inspect_player_storage,
};
