//! World-level access to persisted Minecraft Bedrock player-storage evidence.
//!
//! Physical player record families are historical storage generations, not interchangeable aliases.
//! Moving NBT between `level.dat.Player` and `~local_player` requires a concrete target-version writer
//! that proves the destination game accepts the record shape; the generic world API therefore exposes
//! inspection only.

use crate::error::Result;
use crate::player::{PlayerStorageOverview, inspect_player_storage};
use crate::world::{World, StorageBackend};

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Inspects `level.dat.Player`, `~local_player` and raw `player_*` keys without modifying them.
    pub fn player_storage_overview(&self) -> Result<PlayerStorageOverview> {
        let level = self.read_level_dat()?;
        inspect_player_storage(self.storage(), &level)
    }
}
