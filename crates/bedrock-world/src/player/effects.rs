//! Minecraft Bedrock player `ActiveEffects` list.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use indexmap::IndexMap;

/// Borrowed entry from the player's persisted `ActiveEffects` list.
#[derive(Debug, Clone, Copy)]
pub struct PlayerActiveEffect<'a> {
    nbt: &'a IndexMap<String, NbtTag>,
}

impl<'a> PlayerActiveEffect<'a> {
    /// Returns the complete persisted effect compound.
    #[must_use]
    pub const fn nbt(&self) -> &'a IndexMap<String, NbtTag> {
        self.nbt
    }

    /// Returns `Id` when present.
    pub fn id(&self) -> Result<Option<i32>> {
        integer(self.nbt.get("Id"), "ActiveEffects[].Id")
    }

    /// Returns `Amplifier` when present.
    pub fn amplifier(&self) -> Result<Option<i32>> {
        integer(self.nbt.get("Amplifier"), "ActiveEffects[].Amplifier")
    }

    /// Returns `Duration` when present.
    pub fn duration(&self) -> Result<Option<i32>> {
        integer(self.nbt.get("Duration"), "ActiveEffects[].Duration")
    }
}

impl PlayerData {
    /// Returns every compound from the player's `ActiveEffects` list.
    pub fn active_effects(&self) -> Result<Vec<PlayerActiveEffect<'_>>> {
        let Some(value) = self.root()?.get("ActiveEffects") else {
            return Ok(Vec::new());
        };
        let NbtTag::List(values) = value else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player ActiveEffects has unexpected NBT type: {value:?}"
            )));
        };
        let mut effects = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let NbtTag::Compound(nbt) = value else {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player ActiveEffects[{index}] is not an NBT compound"
                )));
            };
            effects.push(PlayerActiveEffect { nbt });
        }
        Ok(effects)
    }

    /// Replaces the exact `ActiveEffects` list without translating effect ids or fields.
    pub fn set_active_effects(&mut self, effects: Vec<NbtTag>) -> Result<()> {
        for (index, value) in effects.iter().enumerate() {
            if !matches!(value, NbtTag::Compound(_)) {
                return Err(BedrockWorldError::Validation(format!(
                    "ActiveEffects[{index}] must be an NBT compound"
                )));
            }
        }
        self.root_mut()?
            .insert("ActiveEffects".to_string(), NbtTag::List(effects));
        self.finish_edit();
        Ok(())
    }
}

fn integer(value: Option<&NbtTag>, field: &str) -> Result<Option<i32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        NbtTag::Byte(value) => Ok(Some(i32::from(*value))),
        NbtTag::Short(value) => Ok(Some(i32::from(*value))),
        NbtTag::Int(value) => Ok(Some(*value)),
        NbtTag::Long(value) => i32::try_from(*value).map(Some).map_err(|_| {
            BedrockWorldError::CorruptWorld(format!("player {field} value {value} does not fit i32"))
        }),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {other:?}"
        ))),
    }
}
