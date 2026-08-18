//! Minecraft Bedrock player level, level progress and enchantment seed fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};

impl PlayerData {
    /// Returns the raw `PlayerLevel` integer when present.
    pub fn player_level(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("PlayerLevel"), "PlayerLevel")
    }

    /// Sets `PlayerLevel`.
    pub fn set_player_level(&mut self, level: i32) -> Result<()> {
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "PlayerLevel", level)?;
        self.finish_edit();
        Ok(())
    }

    /// Returns `PlayerLevelProgress` when present.
    pub fn player_level_progress(&self) -> Result<Option<f64>> {
        let Some(tag) = self.root()?.get("PlayerLevelProgress") else {
            return Ok(None);
        };
        let value = match tag {
            NbtTag::Float(value) => f64::from(*value),
            NbtTag::Double(value) => *value,
            other => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "PlayerLevelProgress has unexpected NBT type: {other:?}"
                )));
            }
        };
        if !value.is_finite() {
            return Err(BedrockWorldError::CorruptWorld(
                "PlayerLevelProgress is not finite".to_string(),
            ));
        }
        Ok(Some(value))
    }

    /// Sets `PlayerLevelProgress`, preserving an existing double representation.
    pub fn set_player_level_progress(&mut self, progress: f64) -> Result<()> {
        if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
            return Err(BedrockWorldError::Validation(format!(
                "PlayerLevelProgress must be finite and within 0..=1, got {progress}"
            )));
        }
        let tag = match self.root()?.get("PlayerLevelProgress") {
            Some(NbtTag::Double(_)) => NbtTag::Double(progress),
            Some(NbtTag::Float(_)) | None => NbtTag::Float(progress as f32),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "PlayerLevelProgress has unexpected NBT type: {other:?}"
                )));
            }
        };
        self.root_mut()?
            .insert("PlayerLevelProgress".to_string(), tag);
        self.finish_edit();
        Ok(())
    }

    /// Returns the raw `EnchantmentSeed` integer when present.
    pub fn enchantment_seed(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("EnchantmentSeed"), "EnchantmentSeed")
    }

    /// Sets `EnchantmentSeed`.
    pub fn set_enchantment_seed(&mut self, seed: i32) -> Result<()> {
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "EnchantmentSeed", seed)?;
        self.finish_edit();
        Ok(())
    }
}
