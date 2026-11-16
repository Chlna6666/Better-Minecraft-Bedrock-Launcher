use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    BlockFacing, CardinalDirection, block_facing, cardinal_direction, state_bool, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, rotate_shape_cardinal, rotate_shape_x,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_button") || matches!(name, "button" | "stone_button" | "wooden_button") {
        return Some(ModelFamily::Button);
    }
    if name.ends_with("_pressure_plate")
        || matches!(
            name,
            "pressure_plate"
                | "stone_pressure_plate"
                | "wooden_pressure_plate"
                | "light_weighted_pressure_plate"
                | "heavy_weighted_pressure_plate"
        )
    {
        return Some(ModelFamily::PressurePlate);
    }
    if name == "lever" || name == "tripwire_hook" {
        return Some(ModelFamily::RedstoneDevice);
    }
    None
}

pub(crate) fn button_shape(state: &BlockStateQuery) -> ModelShape {
    let pressed = state_bool(state, "button_pressed_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    // The vanilla button is a 6 x 4 px plate, 2 px thick (1 px when pressed), authored on the
    // floor and pointing north; every other mount turns that plate.
    let (x_turn, y_turn) = match facing {
        BlockFacing::Up => (0, CardinalDirection::North),
        BlockFacing::Down => (2, CardinalDirection::North),
        facing => (1, facing.cardinal().unwrap_or(CardinalDirection::North)),
    };
    let shape = ModelShape::from_cuboids([canonical_button(pressed)]);
    rotate_shape_cardinal(rotate_shape_x(shape, x_turn), y_turn)
}

/// Vanilla `button` / `button_pressed`: 6 x 4 px footprint on the face, 2 px deep when idle.
fn canonical_button(pressed: bool) -> ModelCuboid {
    let depth = if pressed { 1.0 } else { 2.0 };
    let sides_v0 = if pressed { 14.0 } else { 14.0 };
    let sides_v1 = if pressed { 15.0 } else { 16.0 };
    ModelCuboid::new([5.0, 0.0, 6.0].map(px), [11.0, depth, 10.0].map(px))
        .with_face_uv(BlockFace::Up, uv16(5.0, 6.0, 11.0, 10.0))
        .with_face_uv(BlockFace::Down, uv16(5.0, 6.0, 11.0, 10.0))
        .with_face_uv(BlockFace::North, uv16(5.0, sides_v0, 11.0, sides_v1))
        .with_face_uv(BlockFace::South, uv16(5.0, sides_v0, 11.0, sides_v1))
        .with_face_uv(BlockFace::West, uv16(6.0, sides_v0, 10.0, sides_v1))
        .with_face_uv(BlockFace::East, uv16(6.0, sides_v0, 10.0, sides_v1))
}

pub(crate) fn pressure_plate_shape() -> ModelShape {
    ModelShape::from_cuboids([ModelCuboid::new(
        [0.0625, 0.0, 0.0625],
        [0.9375, 0.0625, 0.9375],
    )])
}

pub(crate) fn lever_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = state_string(state, "lever_direction").unwrap_or("up_north_south");
    let open = state_bool(state, "open_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    // The model is authored on the floor with the handle swinging along Z, matching
    // `lever_direction = up_north_south`; every other mount is that model turned.
    let (x_turn, y_turn) = lever_mount(direction);
    let shape = rotate_shape_cardinal(rotate_shape_x(canonical_lever(open), x_turn), y_turn);
    shape
}

/// Vanilla lever: a 6 x 3 x 8 px base plate and a 2 x 10 x 2 px handle tilted 45 degrees.
///
/// Geometry and UVs follow the Java `lever` / `lever_on` models; the handle tilts towards north
/// when the lever is on (`powered=true` uses the -45 degree model).
fn canonical_lever(open: bool) -> ModelShape {
    let base = ModelCuboid::new([5.0, 0.0, 4.0].map(px), [11.0, 3.0, 12.0].map(px))
        .with_face_uv(BlockFace::Up, uv16(5.0, 4.0, 11.0, 12.0))
        .with_face_uv(BlockFace::Down, uv16(5.0, 4.0, 11.0, 12.0))
        .with_face_uv(BlockFace::North, uv16(5.0, 0.0, 11.0, 3.0))
        .with_face_uv(BlockFace::South, uv16(5.0, 0.0, 11.0, 3.0))
        .with_face_uv(BlockFace::West, uv16(4.0, 0.0, 12.0, 3.0))
        .with_face_uv(BlockFace::East, uv16(4.0, 0.0, 12.0, 3.0));

    let handle = ModelCuboid::new([7.0, 1.0, 7.0].map(px), [9.0, 11.0, 9.0].map(px))
        .with_face_uv(BlockFace::Up, uv16(7.0, 6.0, 9.0, 8.0))
        .with_face_uv(BlockFace::North, uv16(7.0, 6.0, 9.0, 16.0))
        .with_face_uv(BlockFace::South, uv16(7.0, 6.0, 9.0, 16.0))
        .with_face_uv(BlockFace::West, uv16(7.0, 6.0, 9.0, 16.0))
        .with_face_uv(BlockFace::East, uv16(7.0, 6.0, 9.0, 16.0));

    // Open levers use the -45 degree model; closed levers lean the other way.
    let angle = if open { -45.0 } else { 45.0 };
    let planes = tilted_handle_planes(&handle, angle, [1.0, 8.0]);
    ModelShape::from_cuboids([base]).with_planes(planes)
}

/// Turns the floor / north-south authored lever onto its mount.
///
/// Bedrock names the mount in `lever_direction`: `up_*` / `down_*` swing along the named axis
/// while the plain cardinal values mount the lever on that wall.
fn lever_mount(direction: &str) -> (i32, CardinalDirection) {
    match direction.trim() {
        "up_north_south" => (0, CardinalDirection::North),
        "up_east_west" => (0, CardinalDirection::East),
        "down_north_south" => (2, CardinalDirection::North),
        "down_east_west" => (2, CardinalDirection::East),
        "north" => (1, CardinalDirection::North),
        "south" => (1, CardinalDirection::South),
        "east" => (1, CardinalDirection::East),
        "west" => (1, CardinalDirection::West),
        _ => (0, CardinalDirection::North),
    }
}

/// The six quads of a box tilted about the X axis through `pivot` (pixels, y/z).
fn tilted_handle_planes(
    cuboid: &ModelCuboid,
    angle_degrees: f32,
    pivot: [f32; 2],
) -> Vec<ModelPlane> {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    let pivot = [pivot[0] / 16.0, pivot[1] / 16.0];
    let rotate = |corner: [f32; 3]| {
        let (dy, dz) = (corner[1] - pivot[0], corner[2] - pivot[1]);
        [
            corner[0],
            pivot[0] + dy * cos - dz * sin,
            pivot[1] + dy * sin + dz * cos,
        ]
    };
    [
        (BlockFace::Up, [0, 1, 0]),
        (BlockFace::Down, [0, -1, 0]),
        (BlockFace::North, [0, 0, -1]),
        (BlockFace::South, [0, 0, 1]),
        (BlockFace::East, [1, 0, 0]),
        (BlockFace::West, [-1, 0, 0]),
    ]
    .into_iter()
    .map(|(face, normal)| {
        let corners =
            crate::model_family::shape::box_face_corners(cuboid.min, cuboid.max, face).map(rotate);
        let mut plane = ModelPlane::new(corners, normal).front_only();
        if let Some(uv) = cuboid.face_uvs.get(&face).copied() {
            plane = plane.with_uv(uv);
        }
        if let Some(slot) = cuboid.face_material_slots.get(&face) {
            plane = plane.with_material_slot(slot);
        }
        plane
    })
    .collect()
}

fn px(value: f32) -> f32 {
    value / 16.0
}

fn uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    // Bottom-left corner first, matching the renderer's face order; the authored rects measure
    // V downwards, so the pair is emitted inverted.
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

/// Bedrock tripwire hook: a mounting plate, a pair of slanted hook arms, and — when a wire is
/// attached — a straight line running away from the block.
///
/// Verified against the client: the idle hook has 24 vertices (plate `y 0.05..0.55`, `z 0..0.125`,
/// plus two slanted arm pairs at `x 0.4062/0.5938` and `0.4531/0.5469`), and the attached hook has
/// 30 — the extra six are the `y 0.2188` wire strip reaching `z 1.0`. Modelling it as two plain
/// boxes lost the arms entirely.
pub(crate) fn tripwire_hook_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let attached = state_bool(state, "attached_bit").unwrap_or(false);
    let mut cuboids = vec![
        // Mounting plate against the block the hook is fixed to.
        ModelCuboid::new([0.375, 0.05, 0.0], [0.625, 0.55, 0.125]),
        // The two slanted hook arms; each pair is a thin plate leaning away from the plate.
        ModelCuboid::new([0.4062, 0.35, 0.0543], [0.4531, 0.4215, 0.3833]),
        ModelCuboid::new([0.5469, 0.35, 0.0543], [0.5938, 0.4215, 0.3833]),
        // The hook tips.
        ModelCuboid::new([0.4062, 0.3197, 0.3642], [0.4531, 0.3942, 0.5543]),
        ModelCuboid::new([0.5469, 0.3197, 0.3642], [0.5938, 0.3942, 0.5543]),
    ];
    if attached {
        // The wire leaves the hook along the block's own axis.
        cuboids.push(ModelCuboid::new(
            [0.4844, 0.2188, 0.5],
            [0.5156, 0.3197, 1.0],
        ));
    }
    let shape = ModelShape::from_cuboids(cuboids);
    rotate_shape_cardinal(shape, direction)
}

fn wall_attached_cuboid(
    direction: CardinalDirection,
    thickness: f32,
    min_y: f32,
    max_y: f32,
) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.0, min_y, 0.0], [1.0, max_y, thickness]),
        CardinalDirection::South => {
            ModelCuboid::new([0.0, min_y, 1.0 - thickness], [1.0, max_y, 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([1.0 - thickness, min_y, 0.0], [1.0, max_y, 1.0])
        }
        CardinalDirection::West => ModelCuboid::new([0.0, min_y, 0.0], [thickness, max_y, 1.0]),
    }
}
