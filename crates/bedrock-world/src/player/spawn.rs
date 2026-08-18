//! Minecraft Bedrock player spawn position and spawn dimension fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use crate::player::inventory::{integer_tag, set_integer_preserving_type};
use indexmap::IndexMap;

impl PlayerData {
    /// Returns `SpawnX`, `SpawnY` and `SpawnZ` when all three are present.
    pub fn spawn_position(&self) -> Result<Option<[i32; 3]>> {
        read_integer_triplet(self.root()?, ["SpawnX", "SpawnY", "SpawnZ"])
    }

    /// Sets `SpawnX`, `SpawnY` and `SpawnZ`.
    pub fn set_spawn_position(&mut self, position: [i32; 3]) -> Result<()> {
        set_integer_triplet(self, ["SpawnX", "SpawnY", "SpawnZ"], position)
    }

    /// Returns `SpawnBlockPositionX/Y/Z` when all three are present.
    pub fn spawn_block_position(&self) -> Result<Option<[i32; 3]>> {
        read_integer_triplet(
            self.root()?,
            [
                "SpawnBlockPositionX",
                "SpawnBlockPositionY",
                "SpawnBlockPositionZ",
            ],
        )
    }

    /// Sets `SpawnBlockPositionX/Y/Z`.
    pub fn set_spawn_block_position(&mut self, position: [i32; 3]) -> Result<()> {
        set_integer_triplet(
            self,
            [
                "SpawnBlockPositionX",
                "SpawnBlockPositionY",
                "SpawnBlockPositionZ",
            ],
            position,
        )
    }

    /// Returns the raw `SpawnDimension` integer when present.
    pub fn spawn_dimension(&self) -> Result<Option<i32>> {
        integer_tag(self.root()?.get("SpawnDimension"), "SpawnDimension")
    }

    /// Sets `SpawnDimension`, preserving the existing integer NBT width where possible.
    pub fn set_spawn_dimension(&mut self, dimension: i32) -> Result<()> {
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "SpawnDimension", dimension)?;
        self.finish_edit();
        Ok(())
    }
}

fn read_integer_triplet(
    root: &IndexMap<String, NbtTag>,
    fields: [&str; 3],
) -> Result<Option<[i32; 3]>> {
    let values = [
        integer_tag(root.get(fields[0]), fields[0])?,
        integer_tag(root.get(fields[1]), fields[1])?,
        integer_tag(root.get(fields[2]), fields[2])?,
    ];
    let present = values.iter().filter(|value| value.is_some()).count();
    match present {
        0 => Ok(None),
        3 => Ok(Some([
            values[0].unwrap(),
            values[1].unwrap(),
            values[2].unwrap(),
        ])),
        _ => Err(BedrockWorldError::CorruptWorld(format!(
            "player contains a partial {} / {} / {} spawn coordinate triplet",
            fields[0], fields[1], fields[2]
        ))),
    }
}

fn set_integer_triplet(
    player: &mut PlayerData,
    fields: [&str; 3],
    values: [i32; 3],
) -> Result<()> {
    {
        let root = player.root_mut()?;
        for (field, value) in fields.into_iter().zip(values) {
            set_integer_preserving_type(root, field, value)?;
        }
    }
    player.finish_edit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    #[test]
    fn partial_spawn_coordinates_are_rejected() {
        let nbt = NbtTag::Compound(IndexMap::from([
            ("SpawnX".to_string(), NbtTag::Int(1)),
            ("SpawnY".to_string(), NbtTag::Int(64)),
        ]));
        let player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        assert!(player.spawn_position().is_err());
    }

    #[test]
    fn spawn_fields_are_written_without_touching_other_data() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "FutureField".to_string(),
            NbtTag::String("keep".to_string()),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        player.set_spawn_position([1, 65, -2]).unwrap();
        player.set_spawn_dimension(1).unwrap();
        assert_eq!(player.spawn_position().unwrap(), Some([1, 65, -2]));
        assert_eq!(player.spawn_dimension().unwrap(), Some(1));
        assert_eq!(
            player.root().unwrap().get("FutureField"),
            Some(&NbtTag::String("keep".to_string()))
        );
    }
}
