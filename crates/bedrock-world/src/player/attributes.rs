//! Minecraft Bedrock player `Attributes` list.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use indexmap::IndexMap;

/// Borrowed entry from the player's persisted `Attributes` list.
#[derive(Debug, Clone, Copy)]
pub struct PlayerAttribute<'a> {
    nbt: &'a IndexMap<String, NbtTag>,
}

impl<'a> PlayerAttribute<'a> {
    /// Returns the complete persisted attribute compound.
    #[must_use]
    pub const fn nbt(&self) -> &'a IndexMap<String, NbtTag> {
        self.nbt
    }

    /// Returns `Name`/`name` when present.
    #[must_use]
    pub fn name(&self) -> Option<&'a str> {
        ["Name", "name"].into_iter().find_map(|field| match self.nbt.get(field) {
            Some(NbtTag::String(value)) => Some(value.as_str()),
            _ => None,
        })
    }

    /// Returns the persisted `Base` value when present.
    pub fn base(&self) -> Result<Option<f64>> {
        number(self.nbt.get("Base"), "Attributes[].Base")
    }

    /// Returns the persisted `Current` value when present.
    pub fn current(&self) -> Result<Option<f64>> {
        number(self.nbt.get("Current"), "Attributes[].Current")
    }

    /// Returns the persisted `Min` value when present.
    pub fn min(&self) -> Result<Option<f64>> {
        number(self.nbt.get("Min"), "Attributes[].Min")
    }

    /// Returns the persisted `Max` value when present.
    pub fn max(&self) -> Result<Option<f64>> {
        number(self.nbt.get("Max"), "Attributes[].Max")
    }
}

impl PlayerData {
    /// Returns every compound from the player's `Attributes` list.
    pub fn attributes(&self) -> Result<Vec<PlayerAttribute<'_>>> {
        let Some(value) = self.root()?.get("Attributes") else {
            return Ok(Vec::new());
        };
        let NbtTag::List(values) = value else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player Attributes has unexpected NBT type: {value:?}"
            )));
        };
        let mut attributes = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let NbtTag::Compound(nbt) = value else {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player Attributes[{index}] is not an NBT compound"
                )));
            };
            attributes.push(PlayerAttribute { nbt });
        }
        Ok(attributes)
    }

    /// Replaces the exact `Attributes` list without translating attribute names or values.
    pub fn set_attributes(&mut self, attributes: Vec<NbtTag>) -> Result<()> {
        for (index, value) in attributes.iter().enumerate() {
            if !matches!(value, NbtTag::Compound(_)) {
                return Err(BedrockWorldError::Validation(format!(
                    "Attributes[{index}] must be an NBT compound"
                )));
            }
        }
        self.root_mut()?
            .insert("Attributes".to_string(), NbtTag::List(attributes));
        self.finish_edit();
        Ok(())
    }
}

fn number(value: Option<&NbtTag>, field: &str) -> Result<Option<f64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        NbtTag::Byte(value) => Ok(Some(f64::from(*value))),
        NbtTag::Short(value) => Ok(Some(f64::from(*value))),
        NbtTag::Int(value) => Ok(Some(f64::from(*value))),
        NbtTag::Long(value) => Ok(Some(*value as f64)),
        NbtTag::Float(value) => Ok(Some(f64::from(*value))),
        NbtTag::Double(value) => Ok(Some(*value)),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;

    #[test]
    fn attributes_keep_complete_compounds() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Attributes".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                ("Name".to_string(), NbtTag::String("minecraft:health".to_string())),
                ("Base".to_string(), NbtTag::Float(20.0)),
                ("Unknown".to_string(), NbtTag::Long(9)),
            ]))]),
        )]));
        let player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        let attributes = player.attributes().unwrap();
        assert_eq!(attributes[0].name(), Some("minecraft:health"));
        assert_eq!(attributes[0].base().unwrap(), Some(20.0));
        assert_eq!(attributes[0].nbt().get("Unknown"), Some(&NbtTag::Long(9)));
    }
}
