//! Minecraft Bedrock Actor fields persisted directly on player NBT.
//!
//! Bedrock players inherit Actor/Mob state in the same root compound. These accessors operate on the
//! literal persisted field names and preserve the numeric NBT width already present in historical
//! worlds. Unknown fields remain untouched.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::integer_tag;
use indexmap::IndexMap;

impl PlayerData {
    /// Returns the persisted Actor `Air` value when present.
    pub fn air(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("Air"), "Air")
    }

    /// Sets Actor `Air`, preserving the existing integer NBT width.
    pub fn set_air(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "Air", value)
    }

    /// Returns the persisted Actor `AttackTime` value when present.
    pub fn attack_time(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("AttackTime"), "AttackTime")
    }

    /// Sets Actor `AttackTime`, preserving the existing integer NBT width.
    pub fn set_attack_time(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "AttackTime", value)
    }

    /// Returns the persisted Actor `DeathTime` value when present.
    pub fn death_time(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("DeathTime"), "DeathTime")
    }

    /// Sets Actor `DeathTime`, preserving the existing integer NBT width.
    pub fn set_death_time(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "DeathTime", value)
    }

    /// Returns the persisted Actor `Fire` tick value when present.
    pub fn fire(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("Fire"), "Fire")
    }

    /// Sets Actor `Fire`, preserving the existing integer NBT width.
    pub fn set_fire(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "Fire", value)
    }

    /// Returns the persisted Actor `HurtTime` value when present.
    pub fn hurt_time(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("HurtTime"), "HurtTime")
    }

    /// Sets Actor `HurtTime`, preserving the existing integer NBT width.
    pub fn set_hurt_time(&mut self, value: i32) -> Result<()> {
        set_actor_integer(self, "HurtTime", value)
    }

    /// Returns the persisted Actor `FallDistance` value when present.
    pub fn fall_distance(&self) -> Result<Option<f64>> {
        actor_number(self.root()?.get("FallDistance"), "FallDistance")
    }

    /// Sets Actor `FallDistance`, preserving Float/Double width when present.
    pub fn set_fall_distance(&mut self, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(BedrockWorldError::Validation(
                "FallDistance must be finite".to_string(),
            ));
        }
        let tag = match self.root()?.get("FallDistance") {
            Some(NbtTag::Double(_)) => NbtTag::Double(value),
            Some(NbtTag::Float(_)) | None => {
                if value < -(f32::MAX as f64) || value > f32::MAX as f64 {
                    return Err(BedrockWorldError::Validation(format!(
                        "FallDistance value {value} does not fit float"
                    )));
                }
                NbtTag::Float(value as f32)
            }
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "FallDistance has unexpected NBT type: {other:?}"
                )));
            }
        };
        self.root_mut()?.insert("FallDistance".to_string(), tag);
        self.finish_edit();
        Ok(())
    }

    /// Returns the persisted Actor `Dead` flag when present.
    pub fn dead(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("Dead"), "Dead")
    }

    /// Sets Actor `Dead`, preserving its existing integer NBT width.
    pub fn set_dead(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "Dead", value)
    }

    /// Returns the persisted root Actor `Invulnerable` flag when present.
    ///
    /// This is distinct from `abilities.invulnerable`.
    pub fn actor_invulnerable(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("Invulnerable"), "Invulnerable")
    }

    /// Sets the root Actor `Invulnerable` flag without changing `abilities.invulnerable`.
    pub fn set_actor_invulnerable(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "Invulnerable", value)
    }

    /// Returns the persisted Actor `OnGround` flag when present.
    pub fn on_ground(&self) -> Result<Option<bool>> {
        actor_bool(self.root()?.get("OnGround"), "OnGround")
    }

    /// Sets Actor `OnGround`, preserving its existing integer NBT width.
    pub fn set_on_ground(&mut self, value: bool) -> Result<()> {
        set_actor_bool(self, "OnGround", value)
    }

    /// Returns the persisted Actor `UniqueID` when present.
    pub fn unique_id(&self) -> Result<Option<i64>> {
        actor_i64(self.root()?.get("UniqueID"), "UniqueID")
    }

    /// Sets Actor `UniqueID`, preserving the existing integer NBT width when representable.
    pub fn set_unique_id(&mut self, value: i64) -> Result<()> {
        let tag = match self.root()?.get("UniqueID") {
            Some(NbtTag::Byte(_)) => NbtTag::Byte(i8::try_from(value).map_err(|_| {
                BedrockWorldError::Validation(format!("UniqueID value {value} does not fit byte"))
            })?),
            Some(NbtTag::Short(_)) => NbtTag::Short(i16::try_from(value).map_err(|_| {
                BedrockWorldError::Validation(format!("UniqueID value {value} does not fit short"))
            })?),
            Some(NbtTag::Int(_)) => NbtTag::Int(i32::try_from(value).map_err(|_| {
                BedrockWorldError::Validation(format!("UniqueID value {value} does not fit int"))
            })?),
            Some(NbtTag::Long(_)) | None => NbtTag::Long(value),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "UniqueID has unexpected NBT type: {other:?}"
                )));
            }
        };
        self.root_mut()?.insert("UniqueID".to_string(), tag);
        self.finish_edit();
        Ok(())
    }
}

fn set_actor_integer(player: &mut PlayerData, field: &str, value: i32) -> Result<()> {
    let tag = match player.root()?.get(field) {
        Some(NbtTag::Byte(_)) => NbtTag::Byte(i8::try_from(value).map_err(|_| {
            BedrockWorldError::Validation(format!("{field} value {value} does not fit byte"))
        })?),
        Some(NbtTag::Short(_)) | None => NbtTag::Short(i16::try_from(value).map_err(|_| {
            BedrockWorldError::Validation(format!("{field} value {value} does not fit short"))
        })?),
        Some(NbtTag::Int(_)) => NbtTag::Int(value),
        Some(NbtTag::Long(_)) => NbtTag::Long(i64::from(value)),
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

fn actor_bool(value: Option<&NbtTag>, field: &str) -> Result<Option<bool>> {
    Ok(integer_tag(value, field)?.map(|value| value != 0))
}

fn actor_i64(value: Option<&NbtTag>, field: &str) -> Result<Option<i64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        NbtTag::Byte(value) => Ok(Some(i64::from(*value))),
        NbtTag::Short(value) => Ok(Some(i64::from(*value))),
        NbtTag::Int(value) => Ok(Some(i64::from(*value))),
        NbtTag::Long(value) => Ok(Some(*value)),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "{field} has unexpected NBT type: {other:?}"
        ))),
    }
}

fn actor_number(value: Option<&NbtTag>, field: &str) -> Result<Option<f64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let number = match value {
        NbtTag::Byte(value) => f64::from(*value),
        NbtTag::Short(value) => f64::from(*value),
        NbtTag::Int(value) => f64::from(*value),
        NbtTag::Long(value) => *value as f64,
        NbtTag::Float(value) => f64::from(*value),
        NbtTag::Double(value) => *value,
        other => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "{field} has unexpected NBT type: {other:?}"
            )));
        }
    };
    if !number.is_finite() {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "{field} is not finite"
        )));
    }
    Ok(Some(number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;

    #[test]
    fn historical_actor_fields_keep_persisted_nbt_widths() {
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([
                ("Air".to_string(), NbtTag::Short(300)),
                ("Fire".to_string(), NbtTag::Short(0)),
                ("OnGround".to_string(), NbtTag::Byte(1)),
                ("FallDistance".to_string(), NbtTag::Float(0.0)),
                ("UniqueID".to_string(), NbtTag::Long(123)),
                ("FutureActorField".to_string(), NbtTag::Long(9)),
            ])),
        )
        .unwrap();

        player.set_air(280).unwrap();
        player.set_fire(20).unwrap();
        player.set_on_ground(false).unwrap();
        player.set_fall_distance(3.5).unwrap();
        player.set_unique_id(456).unwrap();

        let root = player.root().unwrap();
        assert_eq!(root.get("Air"), Some(&NbtTag::Short(280)));
        assert_eq!(root.get("Fire"), Some(&NbtTag::Short(20)));
        assert_eq!(root.get("OnGround"), Some(&NbtTag::Byte(0)));
        assert_eq!(root.get("FallDistance"), Some(&NbtTag::Float(3.5)));
        assert_eq!(root.get("UniqueID"), Some(&NbtTag::Long(456)));
        assert_eq!(root.get("FutureActorField"), Some(&NbtTag::Long(9)));
    }

    #[test]
    fn root_invulnerable_is_independent_from_abilities() {
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([
                ("Invulnerable".to_string(), NbtTag::Byte(0)),
                (
                    "abilities".to_string(),
                    NbtTag::Compound(IndexMap::from([(
                        "invulnerable".to_string(),
                        NbtTag::Byte(1),
                    )])),
                ),
            ])),
        )
        .unwrap();

        player.set_actor_invulnerable(true).unwrap();
        assert_eq!(player.actor_invulnerable().unwrap(), Some(true));
        assert_eq!(
            player.abilities().unwrap().unwrap().invulnerable().unwrap(),
            Some(true)
        );
    }
}
