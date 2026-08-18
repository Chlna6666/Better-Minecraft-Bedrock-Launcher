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
        ["Name", "name"]
            .into_iter()
            .find_map(|field| match self.nbt.get(field) {
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

    /// Returns the exact persisted attribute whose `Name`/`name` matches `name`.
    ///
    /// Duplicate names are rejected instead of choosing one implicitly. This is suitable for
    /// historical player states such as `minecraft:health`, `minecraft:player.hunger`,
    /// `minecraft:player.saturation`, `minecraft:player.exhaustion` and player XP attributes.
    pub fn attribute(&self, name: &str) -> Result<Option<PlayerAttribute<'_>>> {
        let mut found = None;
        for attribute in self.attributes()? {
            if attribute.name() == Some(name) {
                if found.is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "player Attributes contains duplicate attribute {name}"
                    )));
                }
                found = Some(attribute);
            }
        }
        Ok(found)
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

    /// Sets `Current` on an existing named Bedrock attribute while preserving its numeric NBT type.
    ///
    /// The function never creates a missing attribute because the required fields and numeric widths
    /// differ between historical game versions.
    pub fn set_attribute_current(&mut self, name: &str, value: f64) -> Result<()> {
        set_attribute_number(self, name, "Current", value)
    }

    /// Sets `Base` on an existing named Bedrock attribute while preserving its numeric NBT type.
    ///
    /// The function never creates a missing attribute because the required fields and numeric widths
    /// differ between historical game versions.
    pub fn set_attribute_base(&mut self, name: &str, value: f64) -> Result<()> {
        set_attribute_number(self, name, "Base", value)
    }
}

fn set_attribute_number(
    player: &mut PlayerData,
    attribute_name: &str,
    field: &str,
    value: f64,
) -> Result<()> {
    if !value.is_finite() {
        return Err(BedrockWorldError::Validation(format!(
            "attribute {attribute_name} {field} must be finite"
        )));
    }

    let root = player.root_mut()?;
    let attributes = match root.get_mut("Attributes") {
        Some(NbtTag::List(attributes)) => attributes,
        Some(other) => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player Attributes has unexpected NBT type: {other:?}"
            )));
        }
        None => {
            return Err(BedrockWorldError::Validation(format!(
                "player has no Attributes list containing {attribute_name}"
            )));
        }
    };

    let mut found_index = None;
    for (index, attribute) in attributes.iter().enumerate() {
        let NbtTag::Compound(nbt) = attribute else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player Attributes[{index}] is not an NBT compound"
            )));
        };
        if attribute_name_of(nbt) == Some(attribute_name) {
            if found_index.replace(index).is_some() {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player Attributes contains duplicate attribute {attribute_name}"
                )));
            }
        }
    }

    let Some(index) = found_index else {
        return Err(BedrockWorldError::Validation(format!(
            "player has no attribute {attribute_name}"
        )));
    };
    let NbtTag::Compound(attribute) = &mut attributes[index] else {
        unreachable!("attribute type validated above")
    };
    let old = attribute.get(field).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "attribute {attribute_name} has no persisted {field} field"
        ))
    })?;
    let replacement = number_tag_like(old, value, attribute_name, field)?;
    attribute.insert(field.to_string(), replacement);
    player.finish_edit();
    Ok(())
}

fn attribute_name_of(root: &IndexMap<String, NbtTag>) -> Option<&str> {
    ["Name", "name"]
        .into_iter()
        .find_map(|field| match root.get(field) {
            Some(NbtTag::String(value)) => Some(value.as_str()),
            _ => None,
        })
}

fn number_tag_like(source: &NbtTag, value: f64, name: &str, field: &str) -> Result<NbtTag> {
    let validation = || {
        BedrockWorldError::Validation(format!(
            "attribute {name} {field} value {value} does not fit persisted NBT type"
        ))
    };
    match source {
        NbtTag::Byte(_) => exact_integer(value)
            .and_then(|value| i8::try_from(value).ok())
            .map(NbtTag::Byte)
            .ok_or_else(validation),
        NbtTag::Short(_) => exact_integer(value)
            .and_then(|value| i16::try_from(value).ok())
            .map(NbtTag::Short)
            .ok_or_else(validation),
        NbtTag::Int(_) => exact_integer(value)
            .and_then(|value| i32::try_from(value).ok())
            .map(NbtTag::Int)
            .ok_or_else(validation),
        NbtTag::Long(_) => exact_integer(value)
            .map(NbtTag::Long)
            .ok_or_else(validation),
        NbtTag::Float(_) => {
            if value < -(f32::MAX as f64) || value > f32::MAX as f64 {
                Err(validation())
            } else {
                Ok(NbtTag::Float(value as f32))
            }
        }
        NbtTag::Double(_) => Ok(NbtTag::Double(value)),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "attribute {name} {field} has unexpected NBT type: {other:?}"
        ))),
    }
}

fn exact_integer(value: f64) -> Option<i64> {
    const I64_EXCLUSIVE_MAX: f64 = 9_223_372_036_854_775_808.0;
    if !value.is_finite()
        || value.fract() != 0.0
        || value < i64::MIN as f64
        || value >= I64_EXCLUSIVE_MAX
    {
        return None;
    }
    let integer = value as i64;
    ((integer as f64) == value).then_some(integer)
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
                (
                    "Name".to_string(),
                    NbtTag::String("minecraft:health".to_string()),
                ),
                ("Base".to_string(), NbtTag::Float(20.0)),
                ("Unknown".to_string(), NbtTag::Long(9)),
            ]))]),
        )]));
        let player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        let attributes = player.attributes().unwrap();
        assert_eq!(attributes[0].name(), Some("minecraft:health"));
        assert_eq!(attributes[0].base().unwrap(), Some(20.0));
        assert_eq!(
            attributes[0].nbt().get("Unknown"),
            Some(&NbtTag::Long(9))
        );
    }

    #[test]
    fn named_status_attribute_preserves_float_and_unknown_fields() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Attributes".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                (
                    "Name".to_string(),
                    NbtTag::String("minecraft:player.hunger".to_string()),
                ),
                ("Base".to_string(), NbtTag::Float(20.0)),
                ("Current".to_string(), NbtTag::Float(14.0)),
                ("Max".to_string(), NbtTag::Float(20.0)),
                ("FutureAttributeField".to_string(), NbtTag::Long(7)),
            ]))]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();

        assert_eq!(
            player
                .attribute("minecraft:player.hunger")
                .unwrap()
                .unwrap()
                .current()
                .unwrap(),
            Some(14.0)
        );
        player
            .set_attribute_current("minecraft:player.hunger", 16.0)
            .unwrap();

        let attribute = player
            .attribute("minecraft:player.hunger")
            .unwrap()
            .unwrap();
        assert_eq!(attribute.current().unwrap(), Some(16.0));
        assert_eq!(
            attribute.nbt().get("Current"),
            Some(&NbtTag::Float(16.0))
        );
        assert_eq!(
            attribute.nbt().get("FutureAttributeField"),
            Some(&NbtTag::Long(7))
        );
    }

    #[test]
    fn missing_attribute_is_not_created_without_version_shape() {
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([(
                "Attributes".to_string(),
                NbtTag::List(Vec::new()),
            )])),
        )
        .unwrap();
        assert!(
            player
                .set_attribute_current("minecraft:health", 20.0)
                .is_err()
        );
    }
}
