//! Minecraft Bedrock player `PlayerGameMode` field.

use crate::error::Result;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};

impl PlayerData {
    /// Returns the raw persisted `PlayerGameMode` integer when present.
    ///
    /// The library deliberately does not reinterpret this value as a synthetic cross-version enum.
    pub fn player_game_mode(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("PlayerGameMode"), "PlayerGameMode")
    }

    /// Sets the raw persisted `PlayerGameMode` value.
    pub fn set_player_game_mode(&mut self, game_mode: i32) -> Result<()> {
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "PlayerGameMode", game_mode)?;
        self.finish_edit();
        Ok(())
    }
}
