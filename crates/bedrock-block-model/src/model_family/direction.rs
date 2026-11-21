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
        .and_then(from_string)
        .or_else(|| state_string(state, "cardinal_direction").and_then(from_string))
        .or_else(|| state_string(state, "facing").and_then(from_string))
        .or_else(|| state_string(state, "facing_direction").and_then(from_string))
        .or_else(|| block_face(state).and_then(from_string))
        .or_else(|| state_i64(state, "facing_direction").and_then(facing_direction_from_int))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(cardinal_direction_from_int))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn stair_direction(state: &BlockStateQuery) -> Option<CardinalDirection> {
    state_string(state, "minecraft:cardinal_direction")
        .and_then(from_string)
        .or_else(|| state_string(state, "cardinal_direction").and_then(from_string))
        .or_else(|| state_string(state, "facing").and_then(from_string))
        .or_else(|| state_string(state, "direction").and_then(from_string))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(stair_direction_from_int))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn block_face(state: &BlockStateQuery) -> Option<&str> {
    state_string(state, "minecraft:block_face")
        .or_else(|| state_string(state, "block_face"))
        .or_else(|| state_string(state, "torch_facing_direction"))
}

/// One of the six block-space faces named by a six-way `facing_direction` state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BlockFacing {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl BlockFacing {
    pub(super) const fn cardinal(self) -> Option<CardinalDirection> {
        match self {
            Self::North => Some(CardinalDirection::North),
            Self::South => Some(CardinalDirection::South),
            Self::West => Some(CardinalDirection::West),
            Self::East => Some(CardinalDirection::East),
            Self::Down | Self::Up => None,
        }
    }
}

/// Maps a model-space face onto the world face it lands on when a block's front turns to
/// `facing`. A model authored with its front on the north face is the usual case.
/// Where a block's model paints its `front` face.
///
/// `blocks.json` is not consistent: the observer, command blocks, jigsaw, lectern, loom and vault
/// author their front on **north**, while furnaces, smokers, dispensers, droppers, beehives,
/// chests and dried ghasts author it on **south**. Dispensers and droppers additionally use the
/// `east` slot for their vertical front when they point up or down. Rotating every block as if
/// its front were north flipped the whole south-authored group 180 degrees.
pub(super) fn model_front(name: &str) -> crate::material::BlockFace {
    use crate::material::BlockFace;
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    // North-authored fronts.
    const NORTH_FRONT: [&str; 9] = [
        "observer",
        "command_block",
        "chain_command_block",
        "repeating_command_block",
        "jigsaw",
        "lectern",
        "loom",
        "vault",
        "chiseled_bookshelf",
    ];
    if NORTH_FRONT.contains(&name) {
        return BlockFace::North;
    }
    // Everything else with a `front` slot (furnaces, chests, beehives, dispensers, ...) is
    // authored facing south.
    BlockFace::South
}

/// How a block's six faces map when its authored front is turned onto the state's facing.
///
/// `blocks.json` authors some fronts on the north face (observer, command blocks, jigsaw,
/// lectern, loom, vault) and others on the south face (furnaces, chests, beehives, dispensers,
/// droppers). Dispensers, droppers and observers can also aim straight up or down, which needs a
/// real pitch of the cube rather than a yaw. Returning the whole mapping keeps the side faces
/// consistent with the front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FaceRotation {
    /// Yaw applied first, then the pitch.
    yaw: u8,
    /// 0 = none, 1 = +90 about X, 2 = -90 about X.
    pitch: u8,
}

impl FaceRotation {
    /// Maps one model face through this rotation.
    #[must_use]
    pub(super) fn apply(self, face: crate::material::BlockFace) -> crate::material::BlockFace {
        let yawed = match self.yaw.rem_euclid(4) {
            1 => turn_model_face(face, BlockFacing::East),
            2 => turn_model_face(face, BlockFacing::South),
            3 => turn_model_face(face, BlockFacing::West),
            _ => face,
        };
        match self.pitch {
            // +90 about X: north→up, up→south, south→down, down→north.
            1 => pitch_face(yawed, true),
            // -90 about X: north→down, down→south, south→up, up→north.
            2 => pitch_face(yawed, false),
            _ => yawed,
        }
    }

    /// The identity rotation, used for blocks that never turn.
    pub(super) const NONE: Self = Self { yaw: 0, pitch: 0 };

    /// The yaw as quarter turns about Y (0..3), for rigid-cap UV rotation.
    #[must_use]
    pub(super) fn quarter_turns(self) -> i32 {
        i32::from(self.yaw.rem_euclid(4))
    }

    /// The authored face whose art lands on `world` under this rotation.
    ///
    /// `apply` sends an authored face to its world face; this inverts it, so a world face's
    /// slot is the slot authored on the returned face. Blocks with distinct front/back/side
    /// art (jigsaw, observer) need the whole map — keeping the authored roles in place leaves
    /// the front art on the wrong side faces once the block turns.
    #[must_use]
    pub(super) fn authored_face_for(
        self,
        world: crate::material::BlockFace,
    ) -> crate::material::BlockFace {
        use crate::material::BlockFace;
        for authored in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            if self.apply(authored) == world {
                return authored;
            }
        }
        world
    }
}

/// One ±90 degree pitch about the X axis.
fn pitch_face(
    face: crate::material::BlockFace,
    forward: bool,
) -> crate::material::BlockFace {
    use crate::material::BlockFace;
    match (face, forward) {
        // +90 about X.
        (BlockFace::North, true) => BlockFace::Up,
        (BlockFace::Up, true) => BlockFace::South,
        (BlockFace::South, true) => BlockFace::Down,
        (BlockFace::Down, true) => BlockFace::North,
        // -90 about X.
        (BlockFace::North, false) => BlockFace::Down,
        (BlockFace::Down, false) => BlockFace::South,
        (BlockFace::South, false) => BlockFace::Up,
        (BlockFace::Up, false) => BlockFace::North,
        (other, _) => other,
    }
}

/// Resolves the rotation that turns a block's authored front onto its state facing.
pub(super) fn face_rotation(name: &str, state: &BlockStateQuery) -> FaceRotation {
    use crate::material::BlockFace;
    let authored = model_front(name);
    let Some(target) = block_facing(state).map(target_face) else {
        return FaceRotation::NONE;
    };
    if authored == target {
        return FaceRotation::NONE;
    }
    // Horizontal target: a pure yaw from the authored face.
    if !matches!(target, BlockFace::Up | BlockFace::Down) {
        for yaw in 1..4 {
            if (FaceRotation { yaw, pitch: 0 }).apply(authored) == target {
                return FaceRotation { yaw, pitch: 0 };
            }
        }
        return FaceRotation::NONE;
    }
    // Vertical target: yaw the authored face onto north (or south) first, then pitch.
    for yaw in 0..4 {
        for pitch in 1..3 {
            if (FaceRotation { yaw, pitch }).apply(authored) == target {
                return FaceRotation { yaw, pitch };
            }
        }
    }
    FaceRotation::NONE
}

pub(super) fn target_face(facing: BlockFacing) -> crate::material::BlockFace {
    use crate::material::BlockFace;
    match facing {
        BlockFacing::North => BlockFace::North,
        BlockFacing::South => BlockFace::South,
        BlockFacing::East => BlockFace::East,
        BlockFacing::West => BlockFace::West,
        BlockFacing::Up => BlockFace::Up,
        BlockFacing::Down => BlockFace::Down,
    }
}

pub(super) fn turn_model_face(
    face: crate::material::BlockFace,
    facing: BlockFacing,
) -> crate::material::BlockFace {
    use crate::material::BlockFace;
    match facing {
        BlockFacing::North => face,
        BlockFacing::East => match face {
            BlockFace::North => BlockFace::East,
            BlockFace::East => BlockFace::South,
            BlockFace::South => BlockFace::West,
            BlockFace::West => BlockFace::North,
            other => other,
        },
        BlockFacing::South => match face {
            BlockFace::North => BlockFace::South,
            BlockFace::South => BlockFace::North,
            BlockFace::East => BlockFace::West,
            BlockFace::West => BlockFace::East,
            other => other,
        },
        BlockFacing::West => match face {
            BlockFace::North => BlockFace::West,
            BlockFace::West => BlockFace::South,
            BlockFace::South => BlockFace::East,
            BlockFace::East => BlockFace::North,
            other => other,
        },
        BlockFacing::Up => match face {
            BlockFace::North => BlockFace::Up,
            BlockFace::Up => BlockFace::South,
            BlockFace::South => BlockFace::Down,
            BlockFace::Down => BlockFace::North,
            other => other,
        },
        BlockFacing::Down => match face {
            BlockFace::North => BlockFace::Down,
            BlockFace::Down => BlockFace::South,
            BlockFace::South => BlockFace::Up,
            BlockFace::Up => BlockFace::North,
            other => other,
        },
    }
}

/// Reads a six-way facing from `facing_direction`, `minecraft:facing_direction` or the
/// block-face spellings. `facing_direction` integers order as 0=down, 1=up, 2=north,
/// 3=south, 4=west, 5=east; the string spellings use the same names.
pub(super) fn block_facing(state: &BlockStateQuery) -> Option<BlockFacing> {
    for key in [
        "minecraft:facing_direction",
        "facing_direction",
        "minecraft:block_face",
        "block_face",
        "torch_facing_direction",
    ] {
        if let Some(value) = state_string(state, key) {
            match normalize_state_literal(value) {
                "down" | "bottom" => return Some(BlockFacing::Down),
                "up" | "top" => return Some(BlockFacing::Up),
                "north" => return Some(BlockFacing::North),
                "south" => return Some(BlockFacing::South),
                "west" => return Some(BlockFacing::West),
                "east" => return Some(BlockFacing::East),
                _ => {}
            }
        }
    }
    if let Some(value) = state_i64(state, "facing_direction") {
        return match value {
            0 => Some(BlockFacing::Down),
            1 => Some(BlockFacing::Up),
            2 => Some(BlockFacing::North),
            3 => Some(BlockFacing::South),
            4 => Some(BlockFacing::West),
            5 => Some(BlockFacing::East),
            _ => None,
        };
    }
    cardinal_direction(state).and_then(|direction| match direction {
        CardinalDirection::North => Some(BlockFacing::North),
        CardinalDirection::South => Some(BlockFacing::South),
        CardinalDirection::West => Some(BlockFacing::West),
        CardinalDirection::East => Some(BlockFacing::East),
    })
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

pub(super) fn connected(state: &BlockStateQuery, direction: CardinalDirection) -> bool {
    connection(state, direction).is_some_and(DirectionConnection::is_connected)
}

pub(super) fn connection(
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

pub(super) fn from_string(value: &str) -> Option<CardinalDirection> {
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

fn stair_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::East),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::South),
        3 => Some(CardinalDirection::North),
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
