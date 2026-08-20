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

    /// Sets the player `Pos` list while retaining the existing homogeneous numeric NBT type.
    ///
    /// A missing field uses Bedrock Float elements. Existing Byte/Short/Int/Long/Float/Double lists
    /// keep that exact element type; values that cannot be represented in the persisted type are
    /// rejected instead of silently changing the player's historical representation.
    pub fn set_position(&mut self, position: [f64; 3]) -> Result<()> {
        set_numeric_list3(self, "Pos", position)
    }

    /// Returns the player `Motion` list when present.
    pub fn motion(&self) -> Result<Option<[f64; 3]>> {
        read_numeric_list3(self.root()?, "Motion")
    }

    /// Sets the player `Motion` list while retaining its existing numeric NBT type.
    pub fn set_motion(&mut self, motion: [f64; 3]) -> Result<()> {
        set_numeric_list3(self, "Motion", motion)
    }

    /// Returns the player `Rotation` list as yaw/pitch values when present.
    pub fn rotation(&self) -> Result<Option<[f64; 2]>> {
        read_numeric_list2(self.root()?, "Rotation")
    }

    /// Sets the player `Rotation` list while retaining its existing numeric NBT type.
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

fn read_numeric_list3(root: &IndexMap<String, NbtTag>, field: &str) -> Result<Option<[f64; 3]>> {
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

fn read_numeric_list2(root: &IndexMap<String, NbtTag>, field: &str) -> Result<Option<[f64; 2]>> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericListType {
    Byte,
    Short,
    Int,
    Long,
    Float,
    Double,
}

impl NumericListType {
    fn of(tag: &NbtTag) -> Option<Self> {
        match tag {
            NbtTag::Byte(_) => Some(Self::Byte),
            NbtTag::Short(_) => Some(Self::Short),
            NbtTag::Int(_) => Some(Self::Int),
            NbtTag::Long(_) => Some(Self::Long),
            NbtTag::Float(_) => Some(Self::Float),
            NbtTag::Double(_) => Some(Self::Double),
            _ => None,
        }
    }
}

fn set_numeric_list3(player: &mut PlayerData, field: &str, values: [f64; 3]) -> Result<()> {
    validate_finite(field, &values)?;
    let persisted_type = existing_numeric_list_type(player.root()?.get(field), field, 3)?
        .unwrap_or(NumericListType::Float);
    let encoded = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| numeric_tag(field, index, value, persisted_type))
        .collect::<Result<Vec<_>>>()?;
    player
        .root_mut()?
        .insert(field.to_string(), NbtTag::List(encoded));
    player.finish_edit();
    Ok(())
}

fn set_numeric_list2(player: &mut PlayerData, field: &str, values: [f64; 2]) -> Result<()> {
    validate_finite(field, &values)?;
    let persisted_type = existing_numeric_list_type(player.root()?.get(field), field, 2)?
        .unwrap_or(NumericListType::Float);
    let encoded = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| numeric_tag(field, index, value, persisted_type))
        .collect::<Result<Vec<_>>>()?;
    player
        .root_mut()?
        .insert(field.to_string(), NbtTag::List(encoded));
    player.finish_edit();
    Ok(())
}

fn existing_numeric_list_type(
    value: Option<&NbtTag>,
    field: &str,
    expected_len: usize,
) -> Result<Option<NumericListType>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {value:?}"
        )));
    };
    if values.len() != expected_len {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} contains {} values instead of {expected_len}",
            values.len()
        )));
    }
    let first = NumericListType::of(&values[0]).ok_or_else(|| {
        BedrockWorldError::CorruptWorld(format!("player {field}[0] is not a numeric NBT value"))
    })?;
    for (index, tag) in values.iter().enumerate().skip(1) {
        if NumericListType::of(tag) != Some(first) {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player {field} is not a homogeneous numeric NBT list at index {index}"
            )));
        }
    }
    Ok(Some(first))
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

fn numeric_tag(
    field: &str,
    index: usize,
    value: f64,
    persisted_type: NumericListType,
) -> Result<NbtTag> {
    let error = || {
        BedrockWorldError::Validation(format!(
            "player {field}[{index}] value {value} cannot be represented as persisted {persisted_type:?}"
        ))
    };
    match persisted_type {
        NumericListType::Byte => exact_integer(value)
            .and_then(|value| i8::try_from(value).ok())
            .map(NbtTag::Byte)
            .ok_or_else(error),
        NumericListType::Short => exact_integer(value)
            .and_then(|value| i16::try_from(value).ok())
            .map(NbtTag::Short)
            .ok_or_else(error),
        NumericListType::Int => exact_integer(value)
            .and_then(|value| i32::try_from(value).ok())
            .map(NbtTag::Int)
            .ok_or_else(error),
        NumericListType::Long => exact_integer(value).map(NbtTag::Long).ok_or_else(error),
        NumericListType::Float => {
            if value < -(f32::MAX as f64) || value > f32::MAX as f64 {
                Err(error())
            } else {
                Ok(NbtTag::Float(value as f32))
            }
        }
        NumericListType::Double => Ok(NbtTag::Double(value)),
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
    fn historical_integer_position_keeps_its_nbt_type() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Pos".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(64), NbtTag::Int(-2)]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        player.set_position([2.0, 70.0, 3.0]).unwrap();
        assert_eq!(
            player.root().unwrap().get("Pos"),
            Some(&NbtTag::List(vec![
                NbtTag::Int(2),
                NbtTag::Int(70),
                NbtTag::Int(3),
            ]))
        );
        assert!(player.set_position([2.5, 70.0, 3.0]).is_err());
    }

    #[test]
    fn malformed_mixed_numeric_position_is_not_rewritten() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Pos".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(64), NbtTag::Int(-2)]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        player.edit_nbt(|nbt| {
            let NbtTag::Compound(root) = nbt else {
                panic!("player root must remain a compound")
            };
            root.insert(
                "Pos".to_string(),
                NbtTag::List(vec![NbtTag::Int(1), NbtTag::Double(64.0), NbtTag::Int(-2)]),
            );
        });
        assert!(player.set_position([2.0, 70.0, 3.0]).is_err());
    }

    #[test]
    fn long_position_rejects_positive_i64_exclusive_bound() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Pos".to_string(),
            NbtTag::List(vec![NbtTag::Long(0), NbtTag::Long(0), NbtTag::Long(0)]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        assert!(
            player
                .set_position([9_223_372_036_854_775_808.0, 0.0, 0.0])
                .is_err()
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
