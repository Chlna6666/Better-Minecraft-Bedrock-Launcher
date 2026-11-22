use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CardinalDirection {
    North,
    South,
    East,
    West,
}

impl CardinalDirection {
    pub(super) const ALL: [Self; 4] = [Self::North, Self::South, Self::East, Self::West];

    pub(super) const fn state_key(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    pub(super) const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    pub(super) const fn counter_clockwise(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
            Self::East => Self::North,
        }
    }

    pub(super) const fn is_perpendicular(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::North | Self::South, Self::East | Self::West)
                | (Self::East | Self::West, Self::North | Self::South)
        )
    }

    pub(super) const fn as_str(self) -> &'static str {
        self.state_key()
    }

    pub(super) const fn normal(self) -> [i32; 3] {
        match self {
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::East => [1, 0, 0],
            Self::West => [-1, 0, 0],
        }
    }
}

pub(super) fn cardinal_direction(state: &BlockState) -> Option<CardinalDirection> {
    state_string(state, "minecraft:cardinal_direction")
        .and_then(cardinal_direction_from_string)
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_from_string))
        .or_else(|| {
            state_string(state, "facing_direction").and_then(cardinal_direction_from_string)
        })
        .or_else(|| block_face_from_state(state).and_then(cardinal_direction_from_string))
        .or_else(|| state_i32(state, "facing_direction").and_then(facing_direction_from_int))
        .or_else(|| state_i32(state, "weirdo_direction").and_then(cardinal_direction_from_int))
        .or_else(|| state_i32(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn cardinal_direction_from_string(value: &str) -> Option<CardinalDirection> {
    match value {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

pub(super) fn block_face_from_state<'a>(state: &'a BlockState) -> Option<&'a str> {
    state_string(state, "minecraft:block_face")
        .or_else(|| state_string(state, "block_face"))
        .or_else(|| state_string(state, "torch_facing_direction"))
}

/// A piston base's own facing, read from the world state.
///
/// Bedrock's piston `facing_direction` int is **not** the shared table: pistons point
/// `0=down, 1=up, 2=south, 3=north, 4=east, 5=west` (minecraft.wiki/w/Piston block
/// states). Modern worlds write the string-valued `minecraft:facing_direction`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PistonFacing {
    /// Neighbour offset in the facing direction.
    pub(super) normal: [i32; 3],
    /// The facing as a `facing_direction` state value.
    pub(super) direction: &'static str,
}

pub(super) fn piston_facing(state: &BlockState) -> Option<PistonFacing> {
    let normalized = normalized_block_name(&state.name);
    if !matches!(normalized.as_str(), "piston" | "sticky_piston") {
        return None;
    }
    let direction = piston_facing_direction(state)?;
    let normal = match direction {
        "north" => [0, 0, -1],
        "south" => [0, 0, 1],
        "east" => [1, 0, 0],
        "west" => [-1, 0, 0],
        "up" => [0, 1, 0],
        "down" => [0, -1, 0],
        _ => return None,
    };
    Some(PistonFacing { normal, direction })
}

/// Reads the piston's facing the way the model layer does: string spellings first, then
/// the piston's own non-uniform int table.
fn piston_facing_direction(state: &BlockState) -> Option<&'static str> {
    for key in [
        "minecraft:facing_direction",
        "facing_direction",
        "minecraft:block_face",
        "block_face",
    ] {
        if let Some(value) = state_string(state, key) {
            match value.trim().strip_prefix("minecraft:").unwrap_or(value.trim()) {
                "down" | "bottom" => return Some("down"),
                "up" | "top" => return Some("up"),
                "north" => return Some("north"),
                "south" => return Some("south"),
                "west" => return Some("west"),
                "east" => return Some("east"),
                _ => {}
            }
        }
    }
    let value = state_i32(state, "facing_direction")?;
    match value {
        0 => Some("down"),
        1 => Some("up"),
        2 => Some("south"),
        3 => Some("north"),
        4 => Some("east"),
        5 => Some("west"),
        _ => None,
    }
}

pub(super) fn neighbor_model(state: &BlockState) -> NeighborModel {
    let family = model_family::classify(&state.name);
    match family {
        ModelFamily::Pane if !has_direction_connection_state(state) => NeighborModel::Pane,
        ModelFamily::Fence if !has_direction_connection_state(state) => NeighborModel::Fence,
        ModelFamily::Wall if !has_direction_connection_state(state) => NeighborModel::Wall,
        ModelFamily::RedstoneWire => NeighborModel::RedstoneWire {
            power: state_i32(state, "redstone_signal")
                .or_else(|| state_i32(state, "power"))
                .unwrap_or(0)
                .clamp(0, 15) as u8,
        },
        ModelFamily::Stairs => NeighborModel::Stairs,
        // A trip wire runs only along its connected axes; Bedrock stores no direction
        // states, so the connections come from the neighbouring cells.
        ModelFamily::Tripwire => NeighborModel::Tripwire,
        // Banners render a fixed official assembly (the standing form reaches 1.83 blocks
        // upward, the wall form dips below the block line) and need no neighbour context.
        _ => NeighborModel::None,
    }
}

pub(super) fn connection_kind(state: &BlockState) -> Option<ConnectionKind> {
    let family = model_family::classify(&state.name);
    match family {
        ModelFamily::Pane => Some(ConnectionKind::Pane),
        ModelFamily::Fence => Some(ConnectionKind::Fence),
        ModelFamily::FenceGate => Some(ConnectionKind::FenceGate),
        ModelFamily::Wall => Some(ConnectionKind::Wall),
        ModelFamily::RedstoneWire => Some(ConnectionKind::RedstoneWire),
        ModelFamily::Tripwire => Some(ConnectionKind::Tripwire),
        ModelFamily::Button | ModelFamily::PressurePlate => Some(ConnectionKind::RedstoneAny),
        ModelFamily::RedstoneDevice => redstone_device_connection_kind(state),
        // The door lower half is the functional half: it stores the hinge and open states
        // the upper half cannot encode reliably, so they reach the upper block through
        // this per-position kind.
        ModelFamily::Door
            if !door_is_upper(state) =>
        {
            Some(ConnectionKind::DoorLower {
                hinge: door_hinge_right(state),
                open: door_open(state),
            })
        }
        _ => {
            let normalized = normalized_block_name(&state.name);
            matches!(
                normalized.as_str(),
                "redstone_block"
                    | "target"
                    | "lever"
                    | "redstone_torch"
                    | "unlit_redstone_torch"
                    | "daylight_detector"
                    | "daylight_detector_inverted"
            )
            .then_some(ConnectionKind::RedstoneAny)
        }
    }
}

/// Whether the door block is the upper half (`upper_block_bit`, with legacy spellings).
pub(super) fn door_is_upper(state: &BlockState) -> bool {
    state_bool(state, "upper_block_bit").unwrap_or(false)
}

/// The door's hinge side from its own states (`door_hinge_bit`, legacy `hinge_bit`/`hinge`).
pub(super) fn door_hinge_right(state: &BlockState) -> bool {
    state_bool(state, "door_hinge_bit")
        .or_else(|| state_bool(state, "hinge_bit"))
        .or_else(|| state_string(state, "hinge").map(|value| value == "right"))
        .unwrap_or(false)
}

/// The door's open state from its own states (`open_bit`, legacy `open`).
pub(super) fn door_open(state: &BlockState) -> bool {
    state_bool(state, "open_bit").or_else(|| state_bool(state, "open")).unwrap_or(false)
}

/// A door upper half resolves its placement from the lower half below; only the facing
/// direction is its own.
pub(super) fn door_upper(state: &BlockState) -> Option<CardinalDirection> {
    let family = model_family::classify(&state.name);
    if family != ModelFamily::Door || !door_is_upper(state) {
        return None;
    }
    cardinal_direction(state)
}

pub(super) fn redstone_device_connection_kind(state: &BlockState) -> Option<ConnectionKind> {
    let normalized = normalized_block_name(&state.name);
    if normalized.contains("repeater") || normalized.contains("comparator") {
        return cardinal_direction(state).map(|direction| match direction {
            CardinalDirection::East | CardinalDirection::West => ConnectionKind::RedstoneAxisX,
            CardinalDirection::North | CardinalDirection::South => ConnectionKind::RedstoneAxisZ,
        });
    }
    // The piston head block gets its own kind: piston bases derive their extended
    // pushed-face art from it, which must not fire for every other redstone device.
    // `blocks.json` spells the id `pistonArmCollision` while world palettes write the
    // flattened snake_case form; match both.
    if normalized.contains("arm_collision") || normalized.contains("ArmCollision") {
        return Some(ConnectionKind::PistonArm);
    }
    // A tripwire hook anchors a wire running into it.
    if normalized == "tripwire_hook" {
        return Some(ConnectionKind::TripwireHook);
    }
    Some(ConnectionKind::RedstoneAny)
}

pub(super) fn stair_state(state: &BlockState) -> Option<StairState> {
    if model_family::classify(&state.name) != ModelFamily::Stairs {
        return None;
    }
    let facing = stair_direction(state)?;
    let top = state_string(state, "vertical_half")
        .or_else(|| state_string(state, "half"))
        .and_then(|value| {
            match value
                .trim()
                .strip_prefix("minecraft:")
                .unwrap_or(value.trim())
            {
                "top" | "upper" => Some(true),
                "bottom" | "lower" => Some(false),
                _ => None,
            }
        })
        .or_else(|| state_bool(state, "upside_down_bit"))
        .unwrap_or(false);
    Some(StairState { facing, top })
}

pub(super) fn stair_direction(state: &BlockState) -> Option<CardinalDirection> {
    state_string(state, "minecraft:cardinal_direction")
        .and_then(cardinal_direction_from_string)
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_from_string))
        .or_else(|| state_string(state, "direction").and_then(cardinal_direction_from_string))
        .or_else(|| state_i32(state, "weirdo_direction").and_then(stair_direction_from_int))
        .or_else(|| state_i32(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn has_direction_connection_state(state: &BlockState) -> bool {
    CardinalDirection::ALL
        .into_iter()
        .any(|direction| has_direction_connection_state_for(state, direction))
}

pub(super) fn has_direction_connection_state_for(
    state: &BlockState,
    direction: CardinalDirection,
) -> bool {
    let key = direction.state_key();
    state_tag(state, &format!("{key}_connection_type")).is_some()
        || state_tag(state, &format!("wall_connection_type_{key}")).is_some()
        || state_tag(state, key).is_some()
        || state_tag(state, &format!("{key}_bit")).is_some()
        || state_tag(state, &format!("connected_{key}")).is_some()
        || state_tag(state, &format!("{key}_connection_bit")).is_some()
        || state_tag(state, &format!("{key}_wall_bit")).is_some()
}

pub(super) fn normalized_block_name(name: &str) -> String {
    name.strip_prefix("minecraft:").unwrap_or(name).to_string()
}

pub(super) fn cardinal_direction_from_int(value: i32) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::South),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::North),
        3 => Some(CardinalDirection::East),
        _ => None,
    }
}

pub(super) fn stair_direction_from_int(value: i32) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::East),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::South),
        3 => Some(CardinalDirection::North),
        _ => None,
    }
}

pub(super) fn facing_direction_from_int(value: i32) -> Option<CardinalDirection> {
    match value {
        2 => Some(CardinalDirection::North),
        3 => Some(CardinalDirection::South),
        4 => Some(CardinalDirection::West),
        5 => Some(CardinalDirection::East),
        _ => None,
    }
}

pub(super) fn state_tag<'a>(state: &'a BlockState, key: &str) -> Option<&'a NbtTag> {
    state
        .states
        .get(key)
        .or_else(|| state.states.get(&format!("minecraft:{key}")))
}

pub(super) fn state_string<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    match state_tag(state, key)? {
        NbtTag::String(value) => Some(value),
        _ => None,
    }
}

pub(super) fn state_i32(state: &BlockState, key: &str) -> Option<i32> {
    state_tag(state, key).and_then(NbtTag::as_i32)
}

pub(super) fn state_bool(state: &BlockState, key: &str) -> Option<bool> {
    match state_tag(state, key)? {
        NbtTag::Byte(value) => Some(*value != 0),
        NbtTag::Short(value) => Some(*value != 0),
        NbtTag::Int(value) => Some(*value != 0),
        NbtTag::Long(value) => Some(*value != 0),
        NbtTag::String(value) => match value.as_str() {
            "true" | "top" | "upper" | "up" => Some(true),
            "false" | "bottom" | "lower" | "down" => Some(false),
            _ => None,
        },
        _ => None,
    }
}
