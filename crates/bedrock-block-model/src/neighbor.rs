use crate::{BlockStateQuery, BlockStateValue, ModelFamily, is_full_opaque_block, model_family_for_block_name};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum HorizontalDirection {
    North = 0,
    East = 1,
    South = 2,
    West = 3,
}

impl HorizontalDirection {
    pub const ALL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    #[must_use]
    pub const fn index(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn offset(self) -> (i8, i8) {
        match self {
            Self::North => (0, -1),
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
        }
    }

    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::East => Self::West,
            Self::South => Self::North,
            Self::West => Self::East,
        }
    }

    #[must_use]
    pub const fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
        }
    }

    #[must_use]
    pub const fn counter_clockwise(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::East => Self::North,
            Self::South => Self::East,
            Self::West => Self::South,
        }
    }

    #[must_use]
    pub const fn same_axis(self, other: Self) -> bool {
        matches!((self, other), (Self::North | Self::South, Self::North | Self::South) | (Self::East | Self::West, Self::East | Self::West))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct HorizontalMask(u8);

impl HorizontalMask {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0b1111);
    pub const NORTH_SOUTH: Self = Self((1 << HorizontalDirection::North.index()) | (1 << HorizontalDirection::South.index()));
    pub const EAST_WEST: Self = Self((1 << HorizontalDirection::East.index()) | (1 << HorizontalDirection::West.index()));

    #[must_use]
    pub const fn from_direction(direction: HorizontalDirection) -> Self {
        Self(1 << direction.index())
    }

    #[must_use]
    pub const fn contains(self, direction: HorizontalDirection) -> bool {
        self.0 & (1 << direction.index()) != 0
    }

    pub const fn insert(&mut self, direction: HorizontalDirection) {
        self.0 |= 1 << direction.index();
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum NeighborModelKind {
    #[default]
    None = 0,
    Pane = 1,
    Fence = 2,
    Wall = 3,
    RedstoneWire = 4,
    Stairs = 5,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum WallConnection {
    #[default]
    None = 0,
    Low = 1,
    Tall = 2,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum RedstoneConnection {
    #[default]
    None = 0,
    Side = 1,
    Up = 2,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum StairShape {
    #[default]
    Straight = 0,
    InnerLeft = 1,
    InnerRight = 2,
    OuterLeft = 3,
    OuterRight = 4,
}

/// Palette/static information needed by neighbor-dependent model derivation.
///
/// This type is intentionally `Copy` and contains no owned strings. A renderer should build it
/// once per palette entry, then only pass these descriptors through the per-block hot path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NeighborBlockDescriptor {
    pub kind: NeighborModelKind,
    pub sturdy_faces: HorizontalMask,
    /// Directions from a redstone wire toward this block that may visually connect.
    pub redstone_ports: HorizontalMask,
    pub redstone_conductor: bool,
    pub facing: Option<HorizontalDirection>,
    pub top_half: bool,
    pub power: u8,
    pub known_connections: HorizontalMask,
    connection_bits: u8,
    pub wall_up: Option<bool>,
    pub stair_shape: Option<StairShape>,
    pub fence_gate: bool,
}

impl Default for NeighborBlockDescriptor {
    fn default() -> Self {
        Self {
            kind: NeighborModelKind::None,
            sturdy_faces: HorizontalMask::NONE,
            redstone_ports: HorizontalMask::NONE,
            redstone_conductor: false,
            facing: None,
            top_half: false,
            power: 0,
            known_connections: HorizontalMask::NONE,
            connection_bits: 0,
            wall_up: None,
            stair_shape: None,
            fence_gate: false,
        }
    }
}

impl NeighborBlockDescriptor {
    #[must_use]
    pub fn from_state(state: &BlockStateQuery) -> Self {
        let family = model_family_for_block_name(&state.name);
        let name = normalized_name(&state.name);
        let facing = horizontal_facing(state, family);
        let sturdy = is_full_opaque_block(&state.name);
        let mut descriptor = Self {
            kind: match family {
                ModelFamily::Pane => NeighborModelKind::Pane,
                ModelFamily::Fence => NeighborModelKind::Fence,
                ModelFamily::Wall => NeighborModelKind::Wall,
                ModelFamily::RedstoneWire => NeighborModelKind::RedstoneWire,
                ModelFamily::Stairs => NeighborModelKind::Stairs,
                _ => NeighborModelKind::None,
            },
            sturdy_faces: if sturdy { HorizontalMask::ALL } else { HorizontalMask::NONE },
            redstone_ports: redstone_ports(name, family, facing),
            redstone_conductor: sturdy,
            facing,
            top_half: state_top_half(state).unwrap_or(false),
            power: state_i64(state, "redstone_signal")
                .or_else(|| state_i64(state, "power"))
                .unwrap_or(0)
                .clamp(0, 15) as u8,
            known_connections: HorizontalMask::NONE,
            connection_bits: 0,
            wall_up: state_bool_like(state, "wall_post_bit").or_else(|| state_bool_like(state, "up")),
            stair_shape: state_string(state, "shape")
                .or_else(|| state_string(state, "minecraft:corner"))
                .or_else(|| state_string(state, "corner"))
                .and_then(stair_shape_from_string),
            fence_gate: matches!(family, ModelFamily::FenceGate),
        };

        for direction in HorizontalDirection::ALL {
            let value = match descriptor.kind {
                NeighborModelKind::Pane | NeighborModelKind::Fence => persisted_boolean_connection(state, direction).map(u8::from),
                NeighborModelKind::Wall => persisted_wall_connection(state, direction).map(|value| value as u8),
                NeighborModelKind::RedstoneWire => persisted_redstone_connection(state, direction).map(|value| value as u8),
                NeighborModelKind::None | NeighborModelKind::Stairs => None,
            };
            if let Some(value) = value {
                descriptor.known_connections.insert(direction);
                descriptor.set_raw_connection(direction, value);
            }
        }
        descriptor
    }

    #[must_use]
    pub const fn raw_connection(self, direction: HorizontalDirection) -> u8 {
        (self.connection_bits >> (direction.index() * 2)) & 0b11
    }

    const fn set_raw_connection(&mut self, direction: HorizontalDirection, value: u8) {
        let shift = direction.index() * 2;
        self.connection_bits = (self.connection_bits & !(0b11 << shift)) | ((value & 0b11) << shift);
    }
}

/// Fully derived, position-specific model variant. The representation is stable inside the crate
/// and is suitable as part of a model-cache key.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct DerivedModelVariant(u32);

impl DerivedModelVariant {
    const CONNECTION_MASK: u32 = 0xff;
    const WALL_UP_BIT: u32 = 1 << 8;
    const TOP_HALF_BIT: u32 = 1 << 9;
    const STAIR_SHIFT: u32 = 10;
    const FACING_SHIFT: u32 = 13;
    const POWER_SHIFT: u32 = 15;
    const KIND_SHIFT: u32 = 19;

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn kind(self) -> NeighborModelKind {
        match (self.0 >> Self::KIND_SHIFT) & 0x7 {
            1 => NeighborModelKind::Pane,
            2 => NeighborModelKind::Fence,
            3 => NeighborModelKind::Wall,
            4 => NeighborModelKind::RedstoneWire,
            5 => NeighborModelKind::Stairs,
            _ => NeighborModelKind::None,
        }
    }

    #[must_use]
    pub const fn raw_connection(self, direction: HorizontalDirection) -> u8 {
        ((self.0 >> (direction.index() * 2)) & 0b11) as u8
    }

    #[must_use]
    pub const fn connected(self, direction: HorizontalDirection) -> bool {
        self.raw_connection(direction) != 0
    }

    #[must_use]
    pub const fn wall_connection(self, direction: HorizontalDirection) -> WallConnection {
        match self.raw_connection(direction) {
            1 => WallConnection::Low,
            2 => WallConnection::Tall,
            _ => WallConnection::None,
        }
    }

    #[must_use]
    pub const fn redstone_connection(self, direction: HorizontalDirection) -> RedstoneConnection {
        match self.raw_connection(direction) {
            1 => RedstoneConnection::Side,
            2 => RedstoneConnection::Up,
            _ => RedstoneConnection::None,
        }
    }

    #[must_use]
    pub const fn wall_up(self) -> bool {
        self.0 & Self::WALL_UP_BIT != 0
    }

    #[must_use]
    pub const fn top_half(self) -> bool {
        self.0 & Self::TOP_HALF_BIT != 0
    }

    #[must_use]
    pub const fn stair_shape(self) -> StairShape {
        match (self.0 >> Self::STAIR_SHIFT) & 0x7 {
            1 => StairShape::InnerLeft,
            2 => StairShape::InnerRight,
            3 => StairShape::OuterLeft,
            4 => StairShape::OuterRight,
            _ => StairShape::Straight,
        }
    }

    #[must_use]
    pub const fn facing(self) -> Option<HorizontalDirection> {
        match (self.0 >> Self::FACING_SHIFT) & 0x7 {
            0 => Some(HorizontalDirection::North),
            1 => Some(HorizontalDirection::East),
            2 => Some(HorizontalDirection::South),
            3 => Some(HorizontalDirection::West),
            _ => None,
        }
    }

    #[must_use]
    pub const fn power(self) -> u8 {
        ((self.0 >> Self::POWER_SHIFT) & 0xf) as u8
    }

    const fn from_descriptor(descriptor: NeighborBlockDescriptor) -> Self {
        let mut bits = u32::from(descriptor.connection_bits) & Self::CONNECTION_MASK;
        if descriptor.wall_up.unwrap_or(false) {
            bits |= Self::WALL_UP_BIT;
        }
        if descriptor.top_half {
            bits |= Self::TOP_HALF_BIT;
        }
        if let Some(shape) = descriptor.stair_shape {
            bits |= (shape as u32) << Self::STAIR_SHIFT;
        }
        let facing = descriptor.facing.map_or(4, HorizontalDirection::index);
        bits |= u32::from(facing) << Self::FACING_SHIFT;
        bits |= u32::from(descriptor.power & 0xf) << Self::POWER_SHIFT;
        bits |= (descriptor.kind as u32) << Self::KIND_SHIFT;
        Self(bits)
    }

    fn set_raw_connection(&mut self, direction: HorizontalDirection, value: u8) {
        let shift = u32::from(direction.index() * 2);
        self.0 = (self.0 & !(0b11 << shift)) | (u32::from(value & 0b11) << shift);
    }

    fn set_stair_shape(&mut self, shape: StairShape) {
        self.0 = (self.0 & !(0x7 << Self::STAIR_SHIFT)) | ((shape as u32) << Self::STAIR_SHIFT);
    }

    fn set_wall_up(&mut self, up: bool) {
        if up {
            self.0 |= Self::WALL_UP_BIT;
        } else {
            self.0 &= !Self::WALL_UP_BIT;
        }
    }
}

/// Derives the position-specific model state from a static palette descriptor and nearby static
/// descriptors. Offsets are world-relative; renderers can therefore route the callback across
/// chunk boundaries without the model crate knowing anything about chunk storage.
#[must_use]
pub fn derive_model_variant(
    center: NeighborBlockDescriptor,
    mut neighbor: impl FnMut(i8, i8, i8) -> Option<NeighborBlockDescriptor>,
) -> DerivedModelVariant {
    let mut variant = DerivedModelVariant::from_descriptor(center);
    match center.kind {
        NeighborModelKind::Pane | NeighborModelKind::Fence => {
            for direction in HorizontalDirection::ALL {
                if center.known_connections.contains(direction) {
                    continue;
                }
                let (dx, dz) = direction.offset();
                let connected = neighbor(dx, 0, dz)
                    .is_some_and(|other| surface_connects(center, other, direction));
                variant.set_raw_connection(direction, u8::from(connected));
            }
        }
        NeighborModelKind::Wall => {
            for direction in HorizontalDirection::ALL {
                if center.known_connections.contains(direction) {
                    continue;
                }
                let (dx, dz) = direction.offset();
                let connected = neighbor(dx, 0, dz)
                    .is_some_and(|other| wall_connects(other, direction));
                variant.set_raw_connection(direction, if connected { WallConnection::Low as u8 } else { 0 });
            }
            if center.wall_up.is_none() {
                let north = variant.connected(HorizontalDirection::North);
                let east = variant.connected(HorizontalDirection::East);
                let south = variant.connected(HorizontalDirection::South);
                let west = variant.connected(HorizontalDirection::West);
                let straight = (north && south && !east && !west) || (east && west && !north && !south);
                variant.set_wall_up(!straight);
            }
        }
        NeighborModelKind::Stairs => {
            if center.stair_shape.is_none() {
                variant.set_stair_shape(derive_stair_shape(center, &mut neighbor));
            }
        }
        NeighborModelKind::RedstoneWire => {
            for direction in HorizontalDirection::ALL {
                if center.known_connections.contains(direction) {
                    continue;
                }
                variant.set_raw_connection(
                    direction,
                    derive_redstone_connection(direction, &mut neighbor) as u8,
                );
            }
        }
        NeighborModelKind::None => {}
    }
    variant
}

fn surface_connects(
    center: NeighborBlockDescriptor,
    other: NeighborBlockDescriptor,
    direction: HorizontalDirection,
) -> bool {
    match center.kind {
        NeighborModelKind::Pane => {
            matches!(other.kind, NeighborModelKind::Pane)
                || other.sturdy_faces.contains(direction.opposite())
        }
        NeighborModelKind::Fence => {
            matches!(other.kind, NeighborModelKind::Fence)
                || (other.fence_gate && gate_accepts_from(other.facing, direction))
                || other.sturdy_faces.contains(direction.opposite())
        }
        _ => false,
    }
}

fn wall_connects(other: NeighborBlockDescriptor, direction: HorizontalDirection) -> bool {
    matches!(other.kind, NeighborModelKind::Wall)
        || (other.fence_gate && gate_accepts_from(other.facing, direction))
        || other.sturdy_faces.contains(direction.opposite())
}

fn gate_accepts_from(
    facing: Option<HorizontalDirection>,
    direction_from_center: HorizontalDirection,
) -> bool {
    facing.is_some_and(|facing| !facing.same_axis(direction_from_center))
}

fn derive_stair_shape(
    center: NeighborBlockDescriptor,
    neighbor: &mut impl FnMut(i8, i8, i8) -> Option<NeighborBlockDescriptor>,
) -> StairShape {
    let Some(facing) = center.facing else {
        return StairShape::Straight;
    };
    let (front_x, front_z) = facing.offset();
    if let Some(front) = neighbor(front_x, 0, front_z)
        && stair_compatible(center, front)
        && front.facing.is_some_and(|other| !other.same_axis(facing))
    {
        let other_facing = front.facing.expect("checked above");
        if different_stair(center, other_facing.opposite(), neighbor) {
            return if other_facing == facing.counter_clockwise() {
                StairShape::OuterLeft
            } else {
                StairShape::OuterRight
            };
        }
    }

    let back = facing.opposite();
    let (back_x, back_z) = back.offset();
    if let Some(rear) = neighbor(back_x, 0, back_z)
        && stair_compatible(center, rear)
        && rear.facing.is_some_and(|other| !other.same_axis(facing))
    {
        let other_facing = rear.facing.expect("checked above");
        if different_stair(center, other_facing, neighbor) {
            return if other_facing == facing.counter_clockwise() {
                StairShape::InnerLeft
            } else {
                StairShape::InnerRight
            };
        }
    }
    StairShape::Straight
}

fn stair_compatible(center: NeighborBlockDescriptor, other: NeighborBlockDescriptor) -> bool {
    other.kind == NeighborModelKind::Stairs && other.top_half == center.top_half && other.facing.is_some()
}

fn different_stair(
    center: NeighborBlockDescriptor,
    direction: HorizontalDirection,
    neighbor: &mut impl FnMut(i8, i8, i8) -> Option<NeighborBlockDescriptor>,
) -> bool {
    let (dx, dz) = direction.offset();
    !neighbor(dx, 0, dz).is_some_and(|side| {
        side.kind == NeighborModelKind::Stairs
            && side.top_half == center.top_half
            && side.facing == center.facing
    })
}

fn derive_redstone_connection(
    direction: HorizontalDirection,
    neighbor: &mut impl FnMut(i8, i8, i8) -> Option<NeighborBlockDescriptor>,
) -> RedstoneConnection {
    let (dx, dz) = direction.offset();
    let same_level = neighbor(dx, 0, dz);
    if same_level.is_some_and(|other| redstone_directly_connects(other, direction)) {
        return RedstoneConnection::Side;
    }

    if same_level.is_some_and(|other| other.redstone_conductor) {
        let above_center_open = !neighbor(0, 1, 0).is_some_and(|above| above.redstone_conductor);
        if above_center_open
            && neighbor(dx, 1, dz).is_some_and(|other| redstone_directly_connects(other, direction))
        {
            return RedstoneConnection::Up;
        }
    } else if neighbor(dx, -1, dz).is_some_and(|other| redstone_directly_connects(other, direction)) {
        return RedstoneConnection::Side;
    }
    RedstoneConnection::None
}

fn redstone_directly_connects(
    other: NeighborBlockDescriptor,
    direction_from_wire: HorizontalDirection,
) -> bool {
    other.kind == NeighborModelKind::RedstoneWire || other.redstone_ports.contains(direction_from_wire)
}

fn redstone_ports(
    name: &str,
    family: ModelFamily,
    facing: Option<HorizontalDirection>,
) -> HorizontalMask {
    if matches!(family, ModelFamily::RedstoneWire) {
        return HorizontalMask::ALL;
    }
    if name.contains("repeater") || name.contains("comparator") {
        return match facing {
            Some(HorizontalDirection::North | HorizontalDirection::South) => HorizontalMask::NORTH_SOUTH,
            Some(HorizontalDirection::East | HorizontalDirection::West) => HorizontalMask::EAST_WEST,
            None => HorizontalMask::NONE,
        };
    }
    if name == "observer" {
        return facing.map_or(HorizontalMask::NONE, HorizontalMask::from_direction);
    }
    if is_omnidirectional_redstone_source(name, family) {
        return HorizontalMask::ALL;
    }
    HorizontalMask::NONE
}

fn is_omnidirectional_redstone_source(name: &str, family: ModelFamily) -> bool {
    matches!(family, ModelFamily::Button | ModelFamily::PressurePlate)
        || matches!(
            name,
            "lever"
                | "redstone_torch"
                | "unlit_redstone_torch"
                | "redstone_block"
                | "daylight_detector"
                | "daylight_detector_inverted"
                | "tripwire_hook"
                | "target"
                | "sculk_sensor"
                | "calibrated_sculk_sensor"
                | "trapped_chest"
                | "detector_rail"
    )
}

fn persisted_boolean_connection(
    state: &BlockStateQuery,
    direction: HorizontalDirection,
) -> Option<bool> {
    state_value_by_keys(state, boolean_connection_keys(direction)).and_then(value_as_bool_like)
}

fn persisted_wall_connection(
    state: &BlockStateQuery,
    direction: HorizontalDirection,
) -> Option<WallConnection> {
    state_value_by_keys(state, wall_connection_keys(direction)).and_then(value_as_wall_connection)
}

fn persisted_redstone_connection(
    state: &BlockStateQuery,
    direction: HorizontalDirection,
) -> Option<RedstoneConnection> {
    state_value_by_keys(state, redstone_connection_keys(direction)).and_then(value_as_redstone_connection)
}

fn boolean_connection_keys(direction: HorizontalDirection) -> &'static [&'static str] {
    match direction {
        HorizontalDirection::North => &["minecraft:connection_north", "connection_north", "north_connection", "north_connected", "north_bit"],
        HorizontalDirection::East => &["minecraft:connection_east", "connection_east", "east_connection", "east_connected", "east_bit"],
        HorizontalDirection::South => &["minecraft:connection_south", "connection_south", "south_connection", "south_connected", "south_bit"],
        HorizontalDirection::West => &["minecraft:connection_west", "connection_west", "west_connection", "west_connected", "west_bit"],
    }
}

fn wall_connection_keys(direction: HorizontalDirection) -> &'static [&'static str] {
    match direction {
        HorizontalDirection::North => &["wall_connection_type_north", "north_connection_type", "north_connection", "north"],
        HorizontalDirection::East => &["wall_connection_type_east", "east_connection_type", "east_connection", "east"],
        HorizontalDirection::South => &["wall_connection_type_south", "south_connection_type", "south_connection", "south"],
        HorizontalDirection::West => &["wall_connection_type_west", "west_connection_type", "west_connection", "west"],
    }
}

fn redstone_connection_keys(direction: HorizontalDirection) -> &'static [&'static str] {
    match direction {
        HorizontalDirection::North => &["north", "redstone_north"],
        HorizontalDirection::East => &["east", "redstone_east"],
        HorizontalDirection::South => &["south", "redstone_south"],
        HorizontalDirection::West => &["west", "redstone_west"],
    }
}

fn state_value_by_keys<'a>(
    state: &'a BlockStateQuery,
    keys: &'static [&'static str],
) -> Option<&'a BlockStateValue> {
    keys.iter().find_map(|key| state.state(key))
}

fn value_as_bool_like(value: &BlockStateValue) -> Option<bool> {
    match value {
        BlockStateValue::Bool(value) => Some(*value),
        BlockStateValue::Int(value) => Some(*value != 0),
        BlockStateValue::String(value) => match normalize_literal(value) {
            "true" | "1" | "yes" | "connected" | "low" | "short" | "tall" => Some(true),
            "false" | "0" | "no" | "none" | "disconnected" => Some(false),
            _ => None,
        },
    }
}

fn value_as_wall_connection(value: &BlockStateValue) -> Option<WallConnection> {
    match value {
        BlockStateValue::Bool(value) => Some(if *value { WallConnection::Low } else { WallConnection::None }),
        BlockStateValue::Int(value) => Some(match *value {
            0 => WallConnection::None,
            2 => WallConnection::Tall,
            _ => WallConnection::Low,
        }),
        BlockStateValue::String(value) => Some(match normalize_literal(value) {
            "none" | "false" | "0" | "disconnected" => WallConnection::None,
            "tall" | "high" | "2" => WallConnection::Tall,
            "low" | "short" | "true" | "1" | "connected" => WallConnection::Low,
            _ => return None,
        }),
    }
}

fn value_as_redstone_connection(value: &BlockStateValue) -> Option<RedstoneConnection> {
    match value {
        BlockStateValue::Bool(value) => Some(if *value { RedstoneConnection::Side } else { RedstoneConnection::None }),
        BlockStateValue::Int(value) => Some(match *value {
            0 => RedstoneConnection::None,
            2 => RedstoneConnection::Up,
            _ => RedstoneConnection::Side,
        }),
        BlockStateValue::String(value) => Some(match normalize_literal(value) {
            "none" | "false" | "0" => RedstoneConnection::None,
            "up" | "2" => RedstoneConnection::Up,
            "side" | "low" | "true" | "1" => RedstoneConnection::Side,
            _ => return None,
        }),
    }
}

fn state_top_half(state: &BlockStateQuery) -> Option<bool> {
    state_string(state, "vertical_half")
        .or_else(|| state_string(state, "half"))
        .and_then(|value| match normalize_literal(value) {
            "top" | "upper" => Some(true),
            "bottom" | "lower" => Some(false),
            _ => None,
        })
        .or_else(|| state_bool_like(state, "upside_down_bit"))
}

fn horizontal_facing(
    state: &BlockStateQuery,
    family: ModelFamily,
) -> Option<HorizontalDirection> {
    for key in ["minecraft:cardinal_direction", "cardinal_direction", "facing", "direction"] {
        if let Some(value) = state_string(state, key).and_then(horizontal_direction_from_string) {
            return Some(value);
        }
    }
    if matches!(family, ModelFamily::Stairs) {
        return state_i64(state, "weirdo_direction").and_then(stair_direction_from_int);
    }
    state_i64(state, "facing_direction")
        .and_then(facing_direction_from_int)
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_from_int))
}

fn horizontal_direction_from_string(value: &str) -> Option<HorizontalDirection> {
    match normalize_literal(value) {
        "north" => Some(HorizontalDirection::North),
        "east" => Some(HorizontalDirection::East),
        "south" => Some(HorizontalDirection::South),
        "west" => Some(HorizontalDirection::West),
        _ => None,
    }
}

fn stair_direction_from_int(value: i64) -> Option<HorizontalDirection> {
    match value.rem_euclid(4) {
        0 => Some(HorizontalDirection::East),
        1 => Some(HorizontalDirection::West),
        2 => Some(HorizontalDirection::South),
        3 => Some(HorizontalDirection::North),
        _ => None,
    }
}

fn cardinal_direction_from_int(value: i64) -> Option<HorizontalDirection> {
    match value.rem_euclid(4) {
        0 => Some(HorizontalDirection::South),
        1 => Some(HorizontalDirection::West),
        2 => Some(HorizontalDirection::North),
        3 => Some(HorizontalDirection::East),
        _ => None,
    }
}

fn facing_direction_from_int(value: i64) -> Option<HorizontalDirection> {
    match value {
        2 => Some(HorizontalDirection::North),
        3 => Some(HorizontalDirection::South),
        4 => Some(HorizontalDirection::West),
        5 => Some(HorizontalDirection::East),
        _ => None,
    }
}

fn stair_shape_from_string(value: &str) -> Option<StairShape> {
    Some(match normalize_literal(value) {
        "straight" | "none" => StairShape::Straight,
        "inner_left" | "innerleft" => StairShape::InnerLeft,
        "inner_right" | "innerright" => StairShape::InnerRight,
        "outer_left" | "outerleft" => StairShape::OuterLeft,
        "outer_right" | "outerright" => StairShape::OuterRight,
        _ => return None,
    })
}

fn state_string<'a>(state: &'a BlockStateQuery, key: &str) -> Option<&'a str> {
    match state.state(key) {
        Some(BlockStateValue::String(value)) => Some(value),
        _ => None,
    }
}

fn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {
    match state.state(key) {
        Some(BlockStateValue::Int(value)) => Some(*value),
        Some(BlockStateValue::Bool(value)) => Some(i64::from(*value)),
        Some(BlockStateValue::String(value)) => normalize_literal(value).parse().ok(),
        None => None,
    }
}

fn state_bool_like(state: &BlockStateQuery, key: &str) -> Option<bool> {
    state.state(key).and_then(value_as_bool_like)
}

fn normalized_name(name: &str) -> &str {
    name.trim().strip_prefix("minecraft:").unwrap_or(name.trim())
}

fn normalize_literal(value: &str) -> &str {
    value.trim().strip_prefix("minecraft:").unwrap_or(value.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(name: &str) -> NeighborBlockDescriptor {
        NeighborBlockDescriptor::from_state(&BlockStateQuery::new(name))
    }

    #[test]
    fn persisted_wall_state_keeps_tall_and_post() {
        let state = BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("wall_connection_type_north", "tall")
            .with_state("wall_connection_type_east", "short")
            .with_state("wall_connection_type_south", "none")
            .with_state("wall_connection_type_west", "none")
            .with_state("wall_post_bit", false);
        let descriptor = NeighborBlockDescriptor::from_state(&state);
        let variant = derive_model_variant(descriptor, |_, _, _| None);
        assert_eq!(variant.wall_connection(HorizontalDirection::North), WallConnection::Tall);
        assert_eq!(variant.wall_connection(HorizontalDirection::East), WallConnection::Low);
        assert!(!variant.wall_up());
    }

    #[test]
    fn fence_derives_missing_neighbor_connection() {
        let center = desc("minecraft:oak_fence");
        let stone = desc("minecraft:stone");
        let variant = derive_model_variant(center, |dx, dy, dz| {
            (dx == 1 && dy == 0 && dz == 0).then_some(stone)
        });
        assert!(variant.connected(HorizontalDirection::East));
        assert!(!variant.connected(HorizontalDirection::West));
    }

    #[test]
    fn persisted_fence_side_overrides_neighbor_fallback() {
        let center = NeighborBlockDescriptor::from_state(
            &BlockStateQuery::new("minecraft:oak_fence")
                .with_state("minecraft:connection_east", false),
        );
        let stone = desc("minecraft:stone");
        let variant = derive_model_variant(center, |dx, dy, dz| {
            (dx == 1 && dy == 0 && dz == 0).then_some(stone)
        });
        assert!(!variant.connected(HorizontalDirection::East));
    }

    #[test]
    fn stair_weirdo_direction_uses_bedrock_specific_mapping() {
        let expected = [
            HorizontalDirection::East,
            HorizontalDirection::West,
            HorizontalDirection::South,
            HorizontalDirection::North,
        ];
        for (value, expected) in expected.into_iter().enumerate() {
            let descriptor = NeighborBlockDescriptor::from_state(
                &BlockStateQuery::new("minecraft:oak_stairs")
                    .with_state("weirdo_direction", value as i32),
            );
            assert_eq!(descriptor.facing, Some(expected));
        }
    }

    #[test]
    fn redstone_climbs_over_conductive_side_block() {
        let wire = desc("minecraft:redstone_wire");
        let stone = desc("minecraft:stone");
        let variant = derive_model_variant(wire, |dx, dy, dz| {
            if (dx, dy, dz) == (1, 0, 0) {
                Some(stone)
            } else if (dx, dy, dz) == (1, 1, 0) {
                Some(wire)
            } else {
                None
            }
        });
        assert_eq!(
            variant.redstone_connection(HorizontalDirection::East),
            RedstoneConnection::Up
        );
    }

    #[test]
    fn redstone_does_not_climb_when_ceiling_blocks_wire() {
        let wire = desc("minecraft:redstone_wire");
        let stone = desc("minecraft:stone");
        let variant = derive_model_variant(wire, |dx, dy, dz| match (dx, dy, dz) {
            (1, 0, 0) | (0, 1, 0) => Some(stone),
            (1, 1, 0) => Some(wire),
            _ => None,
        });
        assert_eq!(
            variant.redstone_connection(HorizontalDirection::East),
            RedstoneConnection::None
        );
    }

    #[test]
    fn repeater_ports_follow_its_axis() {
        let repeater = NeighborBlockDescriptor::from_state(
            &BlockStateQuery::new("minecraft:unpowered_repeater")
                .with_state("minecraft:cardinal_direction", "north"),
        );
        assert!(repeater.redstone_ports.contains(HorizontalDirection::North));
        assert!(repeater.redstone_ports.contains(HorizontalDirection::South));
        assert!(!repeater.redstone_ports.contains(HorizontalDirection::East));
    }
}
