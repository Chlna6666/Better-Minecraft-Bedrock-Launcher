//! Minecraft Bedrock player `abilities` compound.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};
use indexmap::IndexMap;

/// Borrowed view of the exact Bedrock `abilities` compound.
#[derive(Debug, Clone, Copy)]
pub struct PlayerAbilities<'a> {
    nbt: &'a IndexMap<String, NbtTag>,
}

impl<'a> PlayerAbilities<'a> {
    /// Returns the complete persisted `abilities` compound.
    #[must_use]
    pub const fn nbt(&self) -> &'a IndexMap<String, NbtTag> {
        self.nbt
    }

    /// Returns `flying` when present.
    pub fn flying(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "flying")
    }

    /// Returns `mayfly` when present.
    pub fn mayfly(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "mayfly")
    }

    /// Returns `invulnerable` when present.
    pub fn invulnerable(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "invulnerable")
    }

    /// Returns `instabuild` when present.
    pub fn instabuild(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "instabuild")
    }

    /// Returns `worldbuilder` when present.
    pub fn worldbuilder(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "worldbuilder")
    }

    /// Returns historical `attackmobs` when present.
    pub fn attack_mobs(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "attackmobs")
    }

    /// Returns historical `attackplayers` when present.
    pub fn attack_players(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "attackplayers")
    }

    /// Returns historical `build` when present.
    pub fn build(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "build")
    }

    /// Returns historical `doorsandswitches` when present.
    pub fn doors_and_switches(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "doorsandswitches")
    }

    /// Returns historical `lightning` when present.
    pub fn lightning(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "lightning")
    }

    /// Returns historical `mine` when present.
    pub fn mine(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "mine")
    }

    /// Returns historical `op` when present.
    pub fn op(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "op")
    }

    /// Returns historical `opencontainers` when present.
    pub fn open_containers(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "opencontainers")
    }

    /// Returns historical `teleport` when present.
    pub fn teleport(&self) -> Result<Option<bool>> {
        ability_bool(self.nbt, "teleport")
    }

    /// Returns `permissionsLevel` when present.
    pub fn permissions_level(&self) -> Result<Option<i32>> {
        integer_tag(
            self.nbt.get("permissionsLevel"),
            "abilities.permissionsLevel",
        )
    }

    /// Returns `playerPermissionsLevel` when present.
    pub fn player_permissions_level(&self) -> Result<Option<i32>> {
        integer_tag(
            self.nbt.get("playerPermissionsLevel"),
            "abilities.playerPermissionsLevel",
        )
    }

    /// Returns `flySpeed` when present.
    pub fn fly_speed(&self) -> Result<Option<f32>> {
        ability_float(self.nbt, "flySpeed")
    }

    /// Returns `walkSpeed` when present.
    pub fn walk_speed(&self) -> Result<Option<f32>> {
        ability_float(self.nbt, "walkSpeed")
    }
}

impl PlayerData {
    /// Returns the player's exact `abilities` compound when present.
    pub fn abilities(&self) -> Result<Option<PlayerAbilities<'_>>> {
        match self.root()?.get("abilities") {
            Some(NbtTag::Compound(nbt)) => Ok(Some(PlayerAbilities { nbt })),
            Some(other) => Err(BedrockWorldError::CorruptWorld(format!(
                "player abilities has unexpected NBT type: {other:?}"
            ))),
            None => Ok(None),
        }
    }

    /// Sets the persisted `flying` ability without changing other ability fields.
    pub fn set_flying(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "flying", value)
    }

    /// Sets the persisted `mayfly` ability without changing other ability fields.
    pub fn set_mayfly(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "mayfly", value)
    }

    /// Sets the persisted `invulnerable` ability without changing other ability fields.
    pub fn set_invulnerable(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "invulnerable", value)
    }

    /// Sets the persisted `instabuild` ability without changing other ability fields.
    pub fn set_instabuild(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "instabuild", value)
    }

    /// Sets the persisted `worldbuilder` ability without changing other ability fields.
    pub fn set_worldbuilder(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "worldbuilder", value)
    }

    /// Sets historical `attackmobs` without rewriting other ability fields.
    pub fn set_attack_mobs(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "attackmobs", value)
    }

    /// Sets historical `attackplayers` without rewriting other ability fields.
    pub fn set_attack_players(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "attackplayers", value)
    }

    /// Sets historical `build` without rewriting other ability fields.
    pub fn set_build(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "build", value)
    }

    /// Sets historical `doorsandswitches` without rewriting other ability fields.
    pub fn set_doors_and_switches(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "doorsandswitches", value)
    }

    /// Sets historical `lightning` without rewriting other ability fields.
    pub fn set_lightning(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "lightning", value)
    }

    /// Sets historical `mine` without rewriting other ability fields.
    pub fn set_mine(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "mine", value)
    }

    /// Sets historical `op` without rewriting other ability fields.
    pub fn set_op(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "op", value)
    }

    /// Sets historical `opencontainers` without rewriting other ability fields.
    pub fn set_open_containers(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "opencontainers", value)
    }

    /// Sets historical `teleport` without rewriting other ability fields.
    pub fn set_teleport(&mut self, value: bool) -> Result<()> {
        set_ability_bool(self, "teleport", value)
    }

    /// Sets `permissionsLevel`, preserving the persisted integer NBT width when present.
    pub fn set_permissions_level(&mut self, value: i32) -> Result<()> {
        set_ability_integer(self, "permissionsLevel", value)
    }

    /// Sets `playerPermissionsLevel`, preserving the persisted integer NBT width when present.
    pub fn set_player_permissions_level(&mut self, value: i32) -> Result<()> {
        set_ability_integer(self, "playerPermissionsLevel", value)
    }

    /// Sets `flySpeed`, preserving Float/Double width when the field already exists.
    pub fn set_fly_speed(&mut self, value: f32) -> Result<()> {
        set_ability_float(self, "flySpeed", value)
    }

    /// Sets `walkSpeed`, preserving Float/Double width when the field already exists.
    pub fn set_walk_speed(&mut self, value: f32) -> Result<()> {
        set_ability_float(self, "walkSpeed", value)
    }
}

fn abilities_mut(player: &mut PlayerData) -> Result<&mut IndexMap<String, NbtTag>> {
    let root = player.root_mut()?;
    let value = root
        .entry("abilities".to_string())
        .or_insert_with(|| NbtTag::Compound(IndexMap::new()));
    match value {
        NbtTag::Compound(abilities) => Ok(abilities),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "player abilities has unexpected NBT type: {other:?}"
        ))),
    }
}

fn ability_bool(root: &IndexMap<String, NbtTag>, field: &str) -> Result<Option<bool>> {
    let Some(value) = root.get(field) else {
        return Ok(None);
    };
    match value {
        NbtTag::Byte(value) => Ok(Some(*value != 0)),
        NbtTag::Short(value) => Ok(Some(*value != 0)),
        NbtTag::Int(value) => Ok(Some(*value != 0)),
        NbtTag::Long(value) => Ok(Some(*value != 0)),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "player abilities.{field} has unexpected NBT type: {other:?}"
        ))),
    }
}

fn ability_float(root: &IndexMap<String, NbtTag>, field: &str) -> Result<Option<f32>> {
    let Some(value) = root.get(field) else {
        return Ok(None);
    };
    match value {
        NbtTag::Float(value) => Ok(Some(*value)),
        NbtTag::Double(value) => Ok(Some(*value as f32)),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "player abilities.{field} has unexpected NBT type: {other:?}"
        ))),
    }
}

fn set_ability_bool(player: &mut PlayerData, field: &str, value: bool) -> Result<()> {
    {
        let root = abilities_mut(player)?;
        let numeric = if value { 1_i8 } else { 0_i8 };
        let tag = match root.get(field) {
            Some(NbtTag::Short(_)) => NbtTag::Short(i16::from(numeric)),
            Some(NbtTag::Int(_)) => NbtTag::Int(i32::from(numeric)),
            Some(NbtTag::Long(_)) => NbtTag::Long(i64::from(numeric)),
            Some(NbtTag::Byte(_)) | None => NbtTag::Byte(numeric),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player abilities.{field} has unexpected NBT type: {other:?}"
                )));
            }
        };
        root.insert(field.to_string(), tag);
    }
    player.finish_edit();
    Ok(())
}

fn set_ability_integer(player: &mut PlayerData, field: &str, value: i32) -> Result<()> {
    {
        let root = abilities_mut(player)?;
        set_integer_preserving_type(root, field, value)?;
    }
    player.finish_edit();
    Ok(())
}

fn set_ability_float(player: &mut PlayerData, field: &str, value: f32) -> Result<()> {
    if !value.is_finite() {
        return Err(BedrockWorldError::Validation(format!(
            "player abilities.{field} must be finite"
        )));
    }
    {
        let root = abilities_mut(player)?;
        let tag = match root.get(field) {
            Some(NbtTag::Double(_)) => NbtTag::Double(f64::from(value)),
            Some(NbtTag::Float(_)) | None => NbtTag::Float(value),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player abilities.{field} has unexpected NBT type: {other:?}"
                )));
            }
        };
        root.insert(field.to_string(), tag);
    }
    player.finish_edit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;

    #[test]
    fn abilities_preserve_unknown_fields() {
        let mut abilities = IndexMap::new();
        abilities.insert("flying".to_string(), NbtTag::Byte(0));
        abilities.insert("attackmobs".to_string(), NbtTag::Byte(1));
        abilities.insert("permissionsLevel".to_string(), NbtTag::Int(1));
        abilities.insert("FutureAbility".to_string(), NbtTag::Int(42));
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([(
                "abilities".to_string(),
                NbtTag::Compound(abilities),
            )])),
        )
        .unwrap();

        player.set_flying(true).unwrap();
        player.set_attack_mobs(false).unwrap();
        player.set_permissions_level(2).unwrap();

        let abilities = player.abilities().unwrap().unwrap();
        assert_eq!(abilities.flying().unwrap(), Some(true));
        assert_eq!(abilities.attack_mobs().unwrap(), Some(false));
        assert_eq!(abilities.permissions_level().unwrap(), Some(2));
        assert_eq!(
            abilities.nbt().get("permissionsLevel"),
            Some(&NbtTag::Int(2))
        );
        assert_eq!(abilities.nbt().get("FutureAbility"), Some(&NbtTag::Int(42)));
    }
}
