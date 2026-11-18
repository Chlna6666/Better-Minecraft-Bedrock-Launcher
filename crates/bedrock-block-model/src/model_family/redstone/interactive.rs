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

/// Vanilla lever: a 6 x 3 x 8 px base plate and a 2 x 10 x 2 px handle leaning off the floor.
///
/// Geometry follows the capture (floor lever closed = ref 725, open = 733): the handle is **not**
/// a rigidly rotated box — Bedrock shears it, so the tip is pushed further sideways than a
/// 45-degree turn would. The captured cross-section corners (y, z) in pixels are
/// closed `{(2, 8), (0.72, 9.53), (9.66, 14.43), (8.37, 15.96)}` and open
/// `{(2, 8), (0.72, 6.47), (9.66, 1.57), (8.37, 0.04)}`; the handle leans south when off and
/// north when on, off a pivot at the plate's top centre. Every handle face samples the stick
/// strip `lever[7..9, 6..16]` (the base plate samples cobblestone through the block's own map).
fn canonical_lever(open: bool) -> ModelShape {
    // The plate is cobblestone (`blocks.json` east = lever_particle → cobblestone), sampled
    // through the capture's own windows: the 6 x 8 top/down patch and the bottom 3 px band on
    // the sides. The previous map took the band from rows 0..3 and left the plate on the
    // lever sheet, which is what read as a stretched handle texture.
    let base = ModelCuboid::new([5.0, 0.0, 4.0].map(px), [11.0, 3.0, 12.0].map(px))
        .with_face_uv(BlockFace::Up, uv16(5.0, 4.0, 11.0, 12.0))
        .with_face_uv(BlockFace::Down, uv16(5.0, 4.0, 11.0, 12.0))
        .with_face_uv(BlockFace::North, uv16(5.0, 13.0, 11.0, 16.0))
        .with_face_uv(BlockFace::South, uv16(5.0, 13.0, 11.0, 16.0))
        .with_face_uv(BlockFace::West, uv16(4.0, 13.0, 12.0, 16.0))
        .with_face_uv(BlockFace::East, uv16(4.0, 13.0, 12.0, 16.0))
        .with_material_slot("lever_base");

    // Captured cross-section (y, z) in pixels, pivot (2, 8) first, then pivot->tip edges.
    let section: [[f32; 2]; 4] = if open {
        [
            [2.0, 8.0],
            [0.7152, 6.4688],
            [9.6616, 1.5728],
            [8.3744, 0.04],
        ]
    } else {
        [
            [2.0, 8.0],
            [0.7152, 9.5312],
            [9.6616, 14.4272],
            [8.3744, 15.96],
        ]
    };
    // The capture binds the handle's long faces to the stick column's lower 10 px and its two
    // END faces to just the top 2 px (uv 7..9, 6..8); stretching the full column onto the end
    // caps smeared the stick art across them.
    let section_uv = uv16(7.0, 6.0, 9.0, 16.0);
    let end_uv = uv16(7.0, 6.0, 9.0, 8.0);
    let (x0, x1) = (px(7.0), px(9.0));
    let at = |index: usize, x: f32| {
        [x, section[index][0] / 16.0, section[index][1] / 16.0]
    };
    let mut planes = Vec::with_capacity(6);
    // The two side sheets (x = 7 px and x = 9 px).
    for (x, normal) in [(x0, [-1_i32, 0, 0]), (x1, [1, 0, 0])] {
        planes.push(
            ModelPlane::new(
                [at(0, x), at(1, x), at(2, x), at(3, x)],
                normal,
            )
            .with_uv(section_uv),
        );
    }
    // The four strips connecting the sheets along the handle's length. Edges (0,1) and (2,3)
    // are the pivot and tip ends (2 px tall); the other two run the handle's full length.
    for (a, b) in [(0usize, 1usize), (1, 2), (2, 3), (3, 0)] {
        let normal = strip_normal(section[a], section[b]);
        let uv = if matches!((a, b), (0, 1) | (2, 3)) {
            end_uv
        } else {
            section_uv
        };
        planes.push(
            ModelPlane::new([at(a, x0), at(a, x1), at(b, x1), at(b, x0)], normal).with_uv(uv),
        );
    }
    ModelShape::from_cuboids([base]).with_planes(planes)
}

/// Outward normal of one handle strip, from the direction its section edge runs in (y, z).
fn strip_normal(a: [f32; 2], b: [f32; 2]) -> [i32; 3] {
    let edge = [b[0] - a[0], b[1] - a[1]];
    // The renderer's face order satisfies `cross(c1-c0, c3-c0) == -normal`; for a strip
    // [a@7px, a@9px, b@9px, b@7px] that resolves to (0, dz, -dy), pointing away from the
    // handle's lean.
    [
        0,
        if edge[1] > 0.0 {
            1
        } else if edge[1] < 0.0 {
            -1
        } else {
            0
        },
        if edge[0] > 0.0 {
            -1
        } else if edge[0] < 0.0 {
            1
        } else {
            0
        },
    ]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The lever handle reproduces the captured sheared cross-section (refs 725 closed /
    /// 733 open): a rigidly rotated box cannot match, so the corner set is asserted directly.
    #[test]
    fn lever_handle_matches_the_captured_cross_section() {
        let closed = lever_shape(&BlockStateQuery::new("minecraft:lever"));
        assert_eq!(closed.planes.len(), 6);
        let ys: Vec<f32> = closed
            .planes
            .iter()
            .flat_map(|plane| plane.corners.iter().map(|corner| corner[1]))
            .collect();
        let zs: Vec<f32> = closed
            .planes
            .iter()
            .flat_map(|plane| plane.corners.iter().map(|corner| corner[2]))
            .collect();
        let (y_min, y_max) = (
            ys.iter().cloned().fold(f32::INFINITY, f32::min),
            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        let (z_min, z_max) = (
            zs.iter().cloned().fold(f32::INFINITY, f32::min),
            zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        // Captured extents in pixels: y 0.72..9.66, z 8.0..15.96.
        assert!((y_min - 0.7152 / 16.0).abs() < 0.001, "y_min {y_min}");
        assert!((y_max - 9.6616 / 16.0).abs() < 0.001, "y_max {y_max}");
        assert!((z_min - 8.0 / 16.0).abs() < 0.001, "z_min {z_min}");
        assert!((z_max - 15.96 / 16.0).abs() < 0.001, "z_max {z_max}");

        // The open lever leans north instead of south.
        let open = lever_shape(
            &BlockStateQuery::new("minecraft:lever").with_state("open_bit", true),
        );
        let open_z_max = open
            .planes
            .iter()
            .flat_map(|plane| plane.corners.iter().map(|corner| corner[2]))
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (open_z_max - 8.0 / 16.0).abs() < 0.001,
            "open handle should stop at the pivot plane, got {open_z_max}"
        );
    }
}
