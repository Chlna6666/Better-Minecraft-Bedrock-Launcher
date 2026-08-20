//! Typed access to common persisted Minecraft Bedrock block states.

use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;

/// Horizontal Minecraft direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HorizontalDirection {
    /// Negative Z.
    North,
    /// Positive Z.
    South,
    /// Positive X.
    East,
    /// Negative X.
    West,
}

/// Six-way Minecraft block face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockFace {
    /// Negative Y.
    Down,
    /// Positive Y.
    Up,
    /// Negative Z.
    North,
    /// Positive Z.
    South,
    /// Negative X.
    West,
    /// Positive X.
    East,
}

/// Persisted state of a Minecraft door block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoorBlockStates {
    /// Horizontal direction of the door.
    pub direction: HorizontalDirection,
    /// Whether the door is open.
    pub open: bool,
    /// Whether this is the upper half.
    pub upper: bool,
    /// Whether the hinge is on the alternate side.
    pub hinge: bool,
}

/// Persisted state of a Minecraft trapdoor block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrapdoorBlockStates {
    /// Horizontal direction of the trapdoor.
    pub direction: HorizontalDirection,
    /// Whether the trapdoor is open.
    pub open: bool,
    /// Whether the trapdoor occupies the upper half of its block space.
    pub upside_down: bool,
}

/// Persisted redstone-related states carried by one block permutation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedstoneBlockStates {
    /// Signal strength in the inclusive Minecraft range `0..=15`, when stored.
    pub signal: Option<u8>,
    /// Boolean powered state, when stored by this block family.
    pub powered: Option<bool>,
}

/// Vertical half occupied by a slab or modern placement trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerticalHalf {
    /// Lower half of the block space.
    Bottom,
    /// Upper half of the block space.
    Top,
}

/// Horizontal corner shape stored by modern Minecraft stairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StairCorner {
    /// No turn.
    Straight,
    /// Inner corner on the left.
    InnerLeft,
    /// Inner corner on the right.
    InnerRight,
    /// Outer corner on the left.
    OuterLeft,
    /// Outer corner on the right.
    OuterRight,
}

/// Persisted state of a Minecraft stair block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StairBlockStates {
    /// Horizontal direction encoded by Bedrock's `weirdo_direction` state.
    pub direction: HorizontalDirection,
    /// Whether the stair is placed upside down.
    pub upside_down: bool,
    /// Optional modern corner state.
    pub corner: Option<StairCorner>,
}

/// Persisted state of a Minecraft slab block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlabBlockStates {
    /// Vertical half stored by `minecraft:vertical_half`.
    pub vertical_half: VerticalHalf,
}

impl BlockState {
    /// Returns one raw persisted state, accepting an exact key only.
    #[must_use]
    pub fn state(&self, key: &str) -> Option<&NbtTag> {
        self.states.get(key)
    }

    /// Iterates every persisted state without filtering unknown or future keys.
    pub fn state_entries(&self) -> impl ExactSizeIterator<Item = (&str, &NbtTag)> {
        self.states.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// Reads any persisted boolean state by its exact Bedrock key.
    pub fn state_boolean(&self, key: &str) -> Result<Option<bool>> {
        optional_boolean_key(self, key)
    }

    /// Reads any persisted integer state by its exact Bedrock key.
    pub fn state_integer(&self, key: &str) -> Result<Option<i64>> {
        optional_integer_key(self, key)
    }

    /// Reads any persisted string state by its exact Bedrock key.
    pub fn state_string(&self, key: &str) -> Result<Option<&str>> {
        self.states
            .get(key)
            .map(|value| match value {
                NbtTag::String(value) => Ok(value.as_str()),
                _ => Err(invalid_state(key, "expected string state")),
            })
            .transpose()
    }

    /// Reads the standard horizontal direction states used by modern Bedrock blocks.
    pub fn horizontal_direction(&self) -> Result<Option<HorizontalDirection>> {
        let Some((key, value)) = first_state(
            self,
            &["minecraft:cardinal_direction", "cardinal_direction"],
        ) else {
            return Ok(None);
        };
        parse_horizontal_string(key, value).map(Some)
    }

    /// Reads the standard six-way facing state used by observers, pistons and similar blocks.
    pub fn facing_direction(&self) -> Result<Option<BlockFace>> {
        let Some((key, value)) =
            first_state(self, &["minecraft:facing_direction", "facing_direction"])
        else {
            return Ok(None);
        };
        parse_block_face(key, value).map(Some)
    }

    /// Reads the face on which a block was placed.
    pub fn block_face(&self) -> Result<Option<BlockFace>> {
        let Some((key, value)) = first_state(self, &["minecraft:block_face", "block_face"]) else {
            return Ok(None);
        };
        parse_block_face(key, value).map(Some)
    }

    /// Reads the standard top/bottom placement state independently of a block family.
    pub fn vertical_half(&self) -> Result<Option<VerticalHalf>> {
        let Some((key, value)) = first_state(self, &["minecraft:vertical_half", "vertical_half"])
        else {
            return Ok(None);
        };
        parse_vertical_half(key, value).map(Some)
    }

    /// Reads the standard corner placement state independently of a block family.
    pub fn corner(&self) -> Result<Option<StairCorner>> {
        let Some((key, value)) = first_state(self, &["minecraft:corner", "corner"]) else {
            return Ok(None);
        };
        parse_stair_corner(key, value).map(Some)
    }

    /// Reads the complete persisted state required to interpret a door permutation.
    pub fn door_states(&self) -> Result<Option<DoorBlockStates>> {
        if !is_door(&self.name) {
            return Ok(None);
        }
        let direction = if let Some(direction) = self.horizontal_direction()? {
            direction
        } else {
            let value = required_state(self, "direction")?;
            parse_cardinal_integer("direction", value)?
        };
        Ok(Some(DoorBlockStates {
            direction,
            open: required_boolean(self, "open_bit")?,
            upper: required_boolean(self, "upper_block_bit")?,
            hinge: required_boolean(self, "door_hinge_bit")?,
        }))
    }

    /// Reads the complete persisted state required to interpret a trapdoor permutation.
    pub fn trapdoor_states(&self) -> Result<Option<TrapdoorBlockStates>> {
        if !is_trapdoor(&self.name) {
            return Ok(None);
        }
        Ok(Some(TrapdoorBlockStates {
            direction: parse_trapdoor_integer("direction", required_state(self, "direction")?)?,
            open: required_boolean(self, "open_bit")?,
            upside_down: required_boolean(self, "upside_down_bit")?,
        }))
    }

    /// Reads redstone signal and powered states without inventing values for absent properties.
    pub fn redstone_states(&self) -> Result<RedstoneBlockStates> {
        let signal = optional_integer(self, "redstone_signal")?
            .map(|value| {
                u8::try_from(value)
                    .ok()
                    .filter(|value| *value <= 15)
                    .ok_or_else(|| invalid_state("redstone_signal", "expected integer 0..=15"))
            })
            .transpose()?;
        let powered = optional_boolean(self, "powered_bit")?;
        Ok(RedstoneBlockStates { signal, powered })
    }

    /// Reads the direction, vertical placement and optional corner of a stair block.
    pub fn stair_states(&self) -> Result<Option<StairBlockStates>> {
        if !block_name_is(&self.name, "stairs") {
            return Ok(None);
        }
        Ok(Some(StairBlockStates {
            direction: parse_cardinal_integer(
                "weirdo_direction",
                required_state(self, "weirdo_direction")?,
            )?,
            upside_down: required_boolean(self, "upside_down_bit")?,
            corner: self.corner()?,
        }))
    }

    /// Reads the vertical placement of a slab or double-slab block.
    pub fn slab_states(&self) -> Result<Option<SlabBlockStates>> {
        if !block_name_is(&self.name, "slab") {
            return Ok(None);
        }
        let vertical_half = self
            .vertical_half()?
            .ok_or_else(|| invalid_state("minecraft:vertical_half", "missing required state"))?;
        Ok(Some(SlabBlockStates { vertical_half }))
    }
}

fn is_door(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    !is_trapdoor(name) && (name == "door" || name.ends_with("_door"))
}

fn is_trapdoor(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "trapdoor" || name.ends_with("_trapdoor")
}

fn block_name_is(name: &str, suffix: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == suffix || name.ends_with(&format!("_{suffix}"))
}

fn first_state<'a>(
    state: &'a BlockState,
    keys: &[&'static str],
) -> Option<(&'static str, &'a NbtTag)> {
    keys.iter()
        .find_map(|key| state.states.get(*key).map(|value| (*key, value)))
}

fn required_state<'a>(state: &'a BlockState, key: &'static str) -> Result<&'a NbtTag> {
    state
        .states
        .get(key)
        .ok_or_else(|| invalid_state(key, "missing required state"))
}

fn required_boolean(state: &BlockState, key: &'static str) -> Result<bool> {
    optional_boolean(state, key)?.ok_or_else(|| invalid_state(key, "missing required state"))
}

fn optional_boolean(state: &BlockState, key: &'static str) -> Result<Option<bool>> {
    optional_boolean_key(state, key)
}

fn optional_boolean_key(state: &BlockState, key: &str) -> Result<Option<bool>> {
    state
        .states
        .get(key)
        .map(|value| match integer(value) {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(invalid_state(key, "expected boolean byte 0 or 1")),
        })
        .transpose()
}

fn optional_integer(state: &BlockState, key: &'static str) -> Result<Option<i64>> {
    optional_integer_key(state, key)
}

fn optional_integer_key(state: &BlockState, key: &str) -> Result<Option<i64>> {
    state
        .states
        .get(key)
        .map(|value| integer(value).ok_or_else(|| invalid_state(key, "expected integer state")))
        .transpose()
}

fn integer(value: &NbtTag) -> Option<i64> {
    match value {
        NbtTag::Byte(value) => Some(i64::from(*value)),
        NbtTag::Short(value) => Some(i64::from(*value)),
        NbtTag::Int(value) => Some(i64::from(*value)),
        NbtTag::Long(value) => Some(*value),
        _ => None,
    }
}

fn parse_horizontal_string(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {
    let NbtTag::String(value) = value else {
        return Err(invalid_state(key, "expected north/south/east/west string"));
    };
    match value.as_str() {
        "north" => Ok(HorizontalDirection::North),
        "south" => Ok(HorizontalDirection::South),
        "east" => Ok(HorizontalDirection::East),
        "west" => Ok(HorizontalDirection::West),
        _ => Err(invalid_state(key, "unknown horizontal direction")),
    }
}

fn parse_block_face(key: &'static str, value: &NbtTag) -> Result<BlockFace> {
    if let NbtTag::String(value) = value {
        return match value.as_str() {
            "down" => Ok(BlockFace::Down),
            "up" => Ok(BlockFace::Up),
            "north" => Ok(BlockFace::North),
            "south" => Ok(BlockFace::South),
            "west" => Ok(BlockFace::West),
            "east" => Ok(BlockFace::East),
            _ => Err(invalid_state(key, "unknown block face")),
        };
    }
    match integer(value) {
        Some(0) => Ok(BlockFace::Down),
        Some(1) => Ok(BlockFace::Up),
        Some(2) => Ok(BlockFace::North),
        Some(3) => Ok(BlockFace::South),
        Some(4) => Ok(BlockFace::West),
        Some(5) => Ok(BlockFace::East),
        _ => Err(invalid_state(key, "expected facing value 0..=5")),
    }
}

fn parse_cardinal_integer(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {
    match integer(value) {
        Some(0) => Ok(HorizontalDirection::South),
        Some(1) => Ok(HorizontalDirection::West),
        Some(2) => Ok(HorizontalDirection::North),
        Some(3) => Ok(HorizontalDirection::East),
        _ => Err(invalid_state(key, "expected door direction 0..=3")),
    }
}

fn parse_trapdoor_integer(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {
    match integer(value) {
        Some(0) => Ok(HorizontalDirection::West),
        Some(1) => Ok(HorizontalDirection::East),
        Some(2) => Ok(HorizontalDirection::North),
        Some(3) => Ok(HorizontalDirection::South),
        _ => Err(invalid_state(key, "expected trapdoor direction 0..=3")),
    }
}

fn parse_vertical_half(key: &'static str, value: &NbtTag) -> Result<VerticalHalf> {
    match value {
        NbtTag::String(value) if value == "bottom" => Ok(VerticalHalf::Bottom),
        NbtTag::String(value) if value == "top" => Ok(VerticalHalf::Top),
        _ => Err(invalid_state(key, "expected bottom or top")),
    }
}

fn parse_stair_corner(key: &'static str, value: &NbtTag) -> Result<StairCorner> {
    let NbtTag::String(value) = value else {
        return Err(invalid_state(key, "expected stair corner string"));
    };
    match value.as_str() {
        "straight" => Ok(StairCorner::Straight),
        "inner_left" => Ok(StairCorner::InnerLeft),
        "inner_right" => Ok(StairCorner::InnerRight),
        "outer_left" => Ok(StairCorner::OuterLeft),
        "outer_right" => Ok(StairCorner::OuterRight),
        _ => Err(invalid_state(key, "unknown stair corner")),
    }
}

fn invalid_state(key: &str, reason: &str) -> BedrockWorldError {
    BedrockWorldError::Validation(format!("Minecraft block state {key}: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn block(name: &str, states: impl IntoIterator<Item = (&'static str, NbtTag)>) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: states
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<BTreeMap<_, _>>(),
            version: None,
        }
    }

    #[test]
    fn reads_modern_door_states() {
        let state = block(
            "minecraft:mangrove_door",
            [
                (
                    "minecraft:cardinal_direction",
                    NbtTag::String("east".to_string()),
                ),
                ("open_bit", NbtTag::Byte(1)),
                ("upper_block_bit", NbtTag::Byte(0)),
                ("door_hinge_bit", NbtTag::Byte(1)),
            ],
        );
        assert_eq!(
            state.door_states().unwrap(),
            Some(DoorBlockStates {
                direction: HorizontalDirection::East,
                open: true,
                upper: false,
                hinge: true,
            })
        );
    }

    #[test]
    fn door_and_trapdoor_direction_encodings_are_not_conflated() {
        let door = block(
            "minecraft:crimson_door",
            [
                ("direction", NbtTag::Int(0)),
                ("open_bit", NbtTag::Byte(0)),
                ("upper_block_bit", NbtTag::Byte(0)),
                ("door_hinge_bit", NbtTag::Byte(0)),
            ],
        );
        let trapdoor = block(
            "minecraft:crimson_trapdoor",
            [
                ("direction", NbtTag::Int(0)),
                ("open_bit", NbtTag::Byte(1)),
                ("upside_down_bit", NbtTag::Byte(1)),
            ],
        );
        assert_eq!(
            door.door_states().unwrap().unwrap().direction,
            HorizontalDirection::South
        );
        assert_eq!(
            trapdoor.trapdoor_states().unwrap().unwrap().direction,
            HorizontalDirection::West
        );
    }

    #[test]
    fn reads_observer_facing_and_redstone_states() {
        let observer = block(
            "minecraft:observer",
            [
                ("minecraft:facing_direction", NbtTag::Int(5)),
                ("powered_bit", NbtTag::Byte(1)),
            ],
        );
        assert_eq!(observer.facing_direction().unwrap(), Some(BlockFace::East));
        assert_eq!(
            observer.redstone_states().unwrap(),
            RedstoneBlockStates {
                signal: None,
                powered: Some(true),
            }
        );
    }

    #[test]
    fn reads_family_independent_placement_states() {
        let state = block(
            "example:placed_shape",
            [
                ("minecraft:block_face", NbtTag::String("up".to_string())),
                (
                    "minecraft:vertical_half",
                    NbtTag::String("bottom".to_string()),
                ),
                (
                    "minecraft:corner",
                    NbtTag::String("outer_right".to_string()),
                ),
            ],
        );
        assert_eq!(state.block_face().unwrap(), Some(BlockFace::Up));
        assert_eq!(state.vertical_half().unwrap(), Some(VerticalHalf::Bottom));
        assert_eq!(state.corner().unwrap(), Some(StairCorner::OuterRight));
    }

    #[test]
    fn validates_redstone_signal_range() {
        let wire = block(
            "minecraft:redstone_wire",
            [("redstone_signal", NbtTag::Int(16))],
        );
        assert!(wire.redstone_states().is_err());
    }

    #[test]
    fn reads_stair_slab_and_arbitrary_future_states() {
        let stairs = block(
            "minecraft:deepslate_tile_stairs",
            [
                ("weirdo_direction", NbtTag::Int(2)),
                ("upside_down_bit", NbtTag::Byte(1)),
                ("minecraft:corner", NbtTag::String("inner_left".to_string())),
                ("future_rotation_mode", NbtTag::String("custom".to_string())),
            ],
        );
        assert_eq!(
            stairs.stair_states().unwrap(),
            Some(StairBlockStates {
                direction: HorizontalDirection::North,
                upside_down: true,
                corner: Some(StairCorner::InnerLeft),
            })
        );
        assert_eq!(
            stairs.state_string("future_rotation_mode").unwrap(),
            Some("custom")
        );
        assert_eq!(stairs.state_entries().len(), 4);

        let slab = block(
            "minecraft:blackstone_slab",
            [("minecraft:vertical_half", NbtTag::String("top".to_string()))],
        );
        assert_eq!(
            slab.slab_states().unwrap(),
            Some(SlabBlockStates {
                vertical_half: VerticalHalf::Top,
            })
        );
    }
}
