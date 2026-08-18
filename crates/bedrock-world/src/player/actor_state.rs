//! Historical Minecraft Bedrock player Actor state fields.
//!
//! These accessors operate on the literal persisted player-root field names. Existing integer NBT
//! widths are retained, missing fields use the ordinary Bedrock integer/byte shape, and unrelated or
//! unknown fields are never rewritten.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};

impl PlayerData {
    /// Returns the persisted `PortalCooldown` value when present.
    pub fn portal_cooldown(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("PortalCooldown"), "PortalCooldown")
    }

    /// Sets `PortalCooldown`, preserving the existing integer NBT width.
    pub fn set_portal_cooldown(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "PortalCooldown", value)
    }

    /// Returns the persisted `SleepTimer` value when present.
    pub fn sleep_timer(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("SleepTimer"), "SleepTimer")
    }

    /// Sets `SleepTimer`, preserving the existing integer NBT width.
    pub fn set_sleep_timer(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "SleepTimer", value)
    }

    /// Returns the persisted `TimeSinceRest` value when present.
    pub fn time_since_rest(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("TimeSinceRest"), "TimeSinceRest")
    }

    /// Sets `TimeSinceRest`, preserving the existing integer NBT width.
    pub fn set_time_since_rest(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "TimeSinceRest", value)
    }

    /// Returns the persisted `Sleeping` flag when present.
    pub fn sleeping(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("Sleeping"), "Sleeping")
    }

    /// Sets `Sleeping`, preserving its existing integer NBT width.
    pub fn set_sleeping(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "Sleeping", value)
    }

    /// Returns the persisted `Sneaking` flag when present.
    pub fn sneaking(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("Sneaking"), "Sneaking")
    }

    /// Sets `Sneaking`, preserving its existing integer NBT width.
    pub fn set_sneaking(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "Sneaking", value)
    }

    /// Returns the persisted `IsGliding` flag when present.
    pub fn is_gliding(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("IsGliding"), "IsGliding")
    }

    /// Sets `IsGliding`, preserving its existing integer NBT width.
    pub fn set_is_gliding(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "IsGliding", value)
    }

    /// Returns the persisted `IsSwimming` flag when present.
    pub fn is_swimming(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("IsSwimming"), "IsSwimming")
    }

    /// Sets `IsSwimming`, preserving its existing integer NBT width.
    pub fn set_is_swimming(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "IsSwimming", value)
    }

    /// Returns the persisted `Persistent` flag when present.
    pub fn persistent(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("Persistent"), "Persistent")
    }

    /// Sets `Persistent`, preserving its existing integer NBT width.
    pub fn set_persistent(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "Persistent", value)
    }
}

fn set_actor_integer(player: &mut PlayerData, field: &str, value: i32) -> Result<()> {
    set_integer_preserving_type(player.root_mut()?, field, value)?;
    player.finish_edit();
    Ok(())
}

fn actor_bool(value: Option<&NbtTag>, field: &str) -> Result<Option<bool>> {
    Ok(integer_tag(value, field)?.map(|value| value != 0))
}

fn set_actor_bool(player: &mut PlayerData, field: &str, value: bool) -> Result<()> {
    let numeric = if value { 1_i8 } else { 0_i8 };
    let tag = match player.root()?.get(field) {
        Some(NbtTag::Short(_)) => NbtTag::Short(i16::from(numeric)),
        Some(NbtTag::Int(_)) => NbtTag::Int(i32::from(numeric)),
        Some(NbtTag::Long(_)) => NbtTag::Long(i64::from(numeric)),
        Some(NbtTag::Byte(_)) | None => NbtTag::Byte(numeric),
        Some(other) => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "{field} has unexpected NBT type: {other:?}"
            )));
        }
    };
    player.root_mut()?.insert(field.to_string(), tag);
    player.finish_edit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    #[test]
    fn historical_actor_state_keeps_exact_integer_widths() {
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([
                ("PortalCooldown".to_string(), NbtTag::Int(12)),
                ("SleepTimer".to_string(), NbtTag::Short(4)),
                ("Sleeping".to_string(), NbtTag::Byte(0)),
                ("Sneaking".to_string(), NbtTag::Byte(1)),
                ("FutureActorState".to_string(), NbtTag::Long(77)),
            ])),
        )
        .unwrap();

        player.set_portal_cooldown(20).unwrap();
        player.set_sleep_timer(5).unwrap();
        player.set_sleeping(true).unwrap();
        player.set_sneaking(false).unwrap();

        let root = player.root().unwrap();
        assert_eq!(root.get("PortalCooldown"), Some(&NbtTag::Int(20)));
        assert_eq!(root.get("SleepTimer"), Some(&NbtTag::Short(5)));
        assert_eq!(root.get("Sleeping"), Some(&NbtTag::Byte(1)));
        assert_eq!(root.get("Sneaking"), Some(&NbtTag::Byte(0)));
        assert_eq!(root.get("FutureActorState"), Some(&NbtTag::Long(77)));
    }
}
