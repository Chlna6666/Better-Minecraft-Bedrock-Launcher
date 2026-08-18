//! Minecraft Bedrock player `Pos`, `Motion`, `Rotation` and `DimensionId` fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};
use indexmap::IndexMap;

impl PlayerData {
    /// Returns the exact numeric values stored in the player `Pos` list.
    pub fn position(&self) -> Result<Option<[f64; 3]>> {
        read_numeric_list3(self.root()?, "Pos")
    }

    /// Sets the player `Pos` list.
    ///
    /// Existing all-double lists remain doubles; otherwise Bedrock float elements are written.
    pub fn set_position(&mut self, position: [f64; 3]) -> Result<()> {
        set_numeric_list3(self, "Pos", position)
    }

    /// Returns the player `Motion` list when present.
    pub fn motion(&self) -> Result<Option<[f64; 3]>> {
        read_numeric_list3(self.root()?, "Motion")
    }

    /// Sets the player `Motion` list.
    pub fn set_motion(&mut self, motion: [f64; 3]) -> Result<()> {
        set_numeric_list3(self, "Motion", motion)
    }

    /// Returns the player `Rotation` list as yaw/pitch values when present.
    pub fn rotation(&self) -> Result<Option<[f64; 2]>> {
        read_numeric_list2(self.root()?, "Rotation")
    }

    /// Sets the player `Rotation` list.
    pub fn set_rotation(&mut self, rotation: [f64; 2]) -> Result<()> {
        set_numeric_list2(self, "Rotation", rotation)
    }

    /// Returns the raw `DimensionId` integer when present.
    pub fn dimension_id(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("DimensionId"), "DimensionId")
    }

    /// Sets the raw `DimensionId`, preserving the existing integer NBT width where possible.
    pub fn set_dimension_id(&mut self, dimension_id: i32) -> Result<()> {
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "DimensionId", dimension_id)?;
        self.finish_edit();
        Ok(())
    }
}

fn read_numeric_list3(
    root: &IndexMap<String, NbtTag>,
    field: &str,
) -> Result<Option<[f64; 3]>> {
    let Some(value) = root.get(field) else {
        return Ok(None);
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {value:?}"
        )));
    };
    if values.len() != 3 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} contains {} values instead of 3",
            values.len()
        )));
    }
    Ok(Some([
        numeric_component(&values[0], field, 0)?,
        numeric_component(&values[1], field, 1)?,
        numeric_component(&values[2], field, 2)?,
    ]))
}

fn read_numeric_list2(
    root: &IndexMap<String, NbtTag>,
    field: &str,
) -> Result<Option<[f64; 2]>> {
    let Some(value) = root.get(field) else {
        return Ok(None);
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {value:?}"
        )));
    };
    if values.len() != 2 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} contains {} values instead of 2",
            values.len()
        )));
    }
    Ok(Some([
        numeric_component(&values[0], field, 0)?,
        numeric_component(&values[1], field, 1)?,
    ]))
}

fn numeric_component(tag: &NbtTag, field: &str, index: usize) -> Result<f64> {
    let value = match tag {
        NbtTag::Byte(value) => f64::from(*value),
        NbtTag::Short(value) => f64::from(*value),
        NbtTag::Int(value) => f64::from(*value),
        NbtTag::Long(value) => *value as f64,
        NbtTag::Float(value) => f64::from(*value),
        NbtTag::Double(value) => *value,
        other => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player {field}[{index}] has unexpected NBT type: {other:?}"
            )));
        }
    };
    if !value.is_finite() {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field}[{index}] is not finite"
        )));
    }
    Ok(value)
}

fn set_numeric_list3(player: &mut PlayerData, field: &str, values: [f64; 3]) -> Result<()> {
    validate_finite(field, &values)?;
    let use_double = existing_list_is_all_double(player.root()?.get(field), field)?;
    let encoded = if use_double {
        values.into_iter().map(NbtTag::Double).collect()
    } else {
        values
            .into_iter()
            .map(|value| f32_tag(field, value))
            .collect::<Result<Vec<_>>>()?
    };
    player
        .root_mut()?
        .insert(field.to_string(), NbtTag::List(encoded));
    player.finish_edit();
    Ok(())
}

fn set_numeric_list2(player: &mut PlayerData, field: &str, values: [f64; 2]) -> Result<()> {
    validate_finite(field, &values)?;
    let use_double = existing_list_is_all_double(player.root()?.get(field), field)?;
    let encoded = if use_double {
        values.into_iter().map(NbtTag::Double).collect()
    } else {
        values
            .into_iter()
            .map(|value| f32_tag(field, value))
            .collect::<Result<Vec<_>>>()?
    };
    player
        .root_mut()?
        .insert(field.to_string(), NbtTag::List(encoded));
    player.finish_edit();
    Ok(())
}

fn existing_list_is_all_double(value: Option<&NbtTag>, field: &str) -> Result<bool> {
    match value {
        None => Ok(false),
        Some(NbtTag::List(values)) => Ok(!values.is_empty()
            && values.iter().all(|value| matches!(value, NbtTag::Double(_)))),
        Some(other) => Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {other:?}"
        ))),
    }
}

fn validate_finite<const N: usize>(field: &str, values: &[f64; N]) -> Result<()> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(BedrockWorldError::Validation(format!(
            "player {field} cannot contain NaN or infinity"
        )))
    }
}

fn f32_tag(field: &str, value: f64) -> Result<NbtTag> {
    if value < -(f32::MAX as f64) || value > f32::MAX as f64 {
        return Err(BedrockWorldError::Validation(format!(
            "player {field} value {value} does not fit Bedrock float"
        )));
    }
    Ok(NbtTag::Float(value as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    #[test]
    fn position_roundtrips_and_unknown_fields_survive() {
        let nbt = NbtTag::Compound(IndexMap::from([
            (
                "Pos".to_string(),
                NbtTag::List(vec![
                    NbtTag::Float(1.5),
                    NbtTag::Float(64.0),
                    NbtTag::Float(-2.25),
                ]),
            ),
            ("FutureField".to_string(), NbtTag::Long(123)),
        ]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        assert_eq!(player.position().unwrap(), Some([1.5, 64.0, -2.25]));
        player.set_position([2.0, 70.0, 3.0]).unwrap();
        assert_eq!(player.position().unwrap(), Some([2.0, 70.0, 3.0]));
        assert_eq!(
            player.root().unwrap().get("FutureField"),
            Some(&NbtTag::Long(123))
        );
    }

    #[test]
    fn dimension_id_preserves_integer_width() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "DimensionId".to_string(),
            NbtTag::Short(0),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        player.set_dimension_id(2).unwrap();
        assert_eq!(
            player.root().unwrap().get("DimensionId"),
            Some(&NbtTag::Short(2))
        );
    }
}
