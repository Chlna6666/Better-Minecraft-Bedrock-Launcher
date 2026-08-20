use crate::state::{BlockStateQuery, BlockStateValue};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CardinalDirection {
    North,
    South,
    East,
    West,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectionConnection {
    Disconnected,
    Short,
    Tall,
}

impl DirectionConnection {
    pub(super) const fn is_connected(self) -> bool {
        !matches!(self, Self::Disconnected)
    }
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

    pub(super) const fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
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

    pub(super) const fn normal(self) -> [i32; 3] {
        match self {
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::East => [1, 0, 0],
            Self::West => [-1, 0, 0],
        }
    }
}

pub(super) fn cardinal_direction(state: &BlockStateQuery) -> Option<CardinalDirection> {
    state_string(state, "minecraft:cardinal_direction")
        .and_then(cardinal_direction_from_string)
        .or_else(|| {
            state_string(state, "cardinal_direction").and_then(cardinal_direction_from_string)
        })
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_from_string))
        .or_else(|| {
            state_string(state, "facing_direction").and_then(cardinal_direction_from_string)
        })
        .or_else(|| block_face(state).and_then(cardinal_direction_from_string))
        .or_else(|| state_i64(state, "facing_direction").and_then(facing_direction_from_int))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(cardinal_direction_from_int))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn block_face(state: &BlockStateQuery) -> Option<&str> {
    state_string(state, "minecraft:block_face")
        .or_else(|| state_string(state, "block_face"))
        .or_else(|| state_string(state, "torch_facing_direction"))
}

pub(super) fn state_string<'a>(state: &'a BlockStateQuery, key: &str) -> Option<&'a str> {
    state.state(key)?.as_string()
}

pub(super) fn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {
    state.state(key)?.as_i64()
}

pub(super) fn state_bool(state: &BlockStateQuery, key: &str) -> Option<bool> {
    state.state(key)?.as_bool_like()
}

pub(super) fn direction_connected(state: &BlockStateQuery, direction: CardinalDirection) -> bool {
    direction_connection(state, direction).is_some_and(DirectionConnection::is_connected)
}

pub(super) fn direction_connection(
    state: &BlockStateQuery,
    direction: CardinalDirection,
) -> Option<DirectionConnection> {
    let key = direction.state_key();
    [
        format!("{key}_connection_type"),
        format!("wall_connection_type_{key}"),
        format!("connection_type_{key}"),
        format!("{key}_connection"),
        format!("connection_{key}"),
        format!("connected_{key}"),
        key.to_owned(),
        format!("{key}_bit"),
        format!("{key}_connected"),
        format!("connects_{key}"),
        format!("connected_to_{key}"),
        format!("{key}_connection_bit"),
        format!("{key}_wall_bit"),
    ]
    .into_iter()
    .find_map(|state_key| {
        state
            .state(&state_key)
            .and_then(connection_from_state_value)
    })
}

pub(super) fn cardinal_direction_from_string(value: &str) -> Option<CardinalDirection> {
    match normalize_state_literal(value) {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

fn cardinal_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::South),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::North),
        3 => Some(CardinalDirection::East),
        _ => None,
    }
}

fn facing_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value {
        2 => Some(CardinalDirection::North),
        3 => Some(CardinalDirection::South),
        4 => Some(CardinalDirection::West),
        5 => Some(CardinalDirection::East),
        _ => None,
    }
}

trait BlockStateValueExt {
    fn as_string(&self) -> Option<&str>;
    fn as_i64(&self) -> Option<i64>;
    fn as_bool_like(&self) -> Option<bool>;
}

impl BlockStateValueExt for crate::state::BlockStateValue {
    fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Bool(_) | Self::Int(_) => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Bool(value) => Some(i64::from(*value)),
            Self::String(value) => normalize_state_literal(value).parse::<i64>().ok(),
        }
    }

    fn as_bool_like(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Int(value) => Some(*value != 0),
            Self::String(value) => {
                match normalize_state_literal(value) {
                    "true" | "1" | "yes" | "top" | "upper" | "up" | "short" | "tall"
                    | "connected" => Some(true),
                    "false" | "0" | "no" | "bottom" | "lower" | "down" | "none"
                    | "disconnected" => Some(false),
                    _ => None,
                }
            }
        }
    }
}

fn connection_from_state_value(value: &BlockStateValue) -> Option<DirectionConnection> {
    match value {
        BlockStateValue::Bool(value) => Some(if *value {
            DirectionConnection::Short
        } else {
            DirectionConnection::Disconnected
        }),
        BlockStateValue::Int(value) => Some(connection_from_int(*value)),
        BlockStateValue::String(value) => connection_from_string(value),
    }
}

fn connection_from_int(value: i64) -> DirectionConnection {
    match value {
        0 => DirectionConnection::Disconnected,
        2 => DirectionConnection::Tall,
        _ => DirectionConnection::Short,
    }
}

fn connection_from_string(value: &str) -> Option<DirectionConnection> {
    let normalized = normalize_state_literal(value);
    if normalized.is_empty() {
        return None;
    }
    if let Ok(value) = normalized.parse::<i64>() {
        return Some(connection_from_int(value));
    }
    Some(match normalized {
        "none" | "false" | "no" | "disconnected" => DirectionConnection::Disconnected,
        "tall" | "high" => DirectionConnection::Tall,
        _ => DirectionConnection::Short,
    })
}

fn normalize_state_literal(value: &str) -> &str {
    value
        .trim()
        .strip_prefix("minecraft:")
        .unwrap_or(value.trim())
}
