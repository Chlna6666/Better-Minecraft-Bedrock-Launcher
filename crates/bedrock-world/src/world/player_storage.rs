//! World-level access to Minecraft Bedrock player storage evidence and explicit local-player moves.

use crate::error::Result;
use crate::player::{
    LocalPlayerStorage, LocalPlayerStorageMoveReport, PlayerStorageOverview, inspect_player_storage,
    move_level_dat_player_to_local_player, move_local_player_to_level_dat,
};
use crate::world::{BedrockWorld, WorldStorageHandle};

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Inspects `level.dat.Player`, `~local_player` and raw `player_*` keys without modifying them.
    pub fn player_storage_overview_blocking(&self) -> Result<PlayerStorageOverview> {
        let level = self.read_level_dat_blocking()?;
        inspect_player_storage(self.storage(), &level)
    }

    /// Moves the local player to one explicitly selected Bedrock storage form.
    ///
    /// This only changes physical storage location. Player NBT, saved-item representation and game
    /// version data are not upgraded or downgraded. Matching duplicate source/destination records are
    /// treated as a recoverable interrupted move; conflicting duplicates are rejected before writes.
    pub fn move_local_player_storage_blocking(
        &self,
        target: LocalPlayerStorage,
    ) -> Result<LocalPlayerStorageMoveReport> {
        match target {
            LocalPlayerStorage::LevelDatPlayer => {
                move_local_player_to_level_dat(self.path(), self.storage())
            }
            LocalPlayerStorage::LocalPlayer => {
                move_level_dat_player_to_local_player(self.path(), self.storage())
            }
        }
    }
}
