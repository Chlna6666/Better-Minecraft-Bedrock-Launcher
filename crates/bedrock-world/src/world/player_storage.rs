//! World-level access to Minecraft Bedrock player storage evidence.

use crate::error::Result;
use crate::player::{PlayerStorageOverview, inspect_player_storage};
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
}
