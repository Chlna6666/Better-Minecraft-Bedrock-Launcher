use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv, ground_plane,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_stairs")
        || matches!(name, "stairs" | "stone_stairs" | "normal_stone_stairs")
    {
        return Some(ModelFamily::Stairs);
    }
    if name.ends_with("_ladder") || name == "ladder" {
        return Some(ModelFamily::Ladder);
    }
    if name == "snow_layer" || name.ends_with("_carpet") || name == "carpet" {
        return Some(ModelFamily::Carpet);
    }
    if name.ends_with("_rail") || matches!(name, "rail" | "golden_rail" | "detector_rail") {
        return Some(ModelFamily::Rail);
    }
    None
}

pub(crate) fn ladder_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let offset = 0.03125;
    let plane = match direction {
        CardinalDirection::North => ModelPlane::new(
            [
                [1.0, 0.0, offset],
                [0.0, 0.0, offset],
                [0.0, 1.0, offset],
                [1.0, 1.0, offset],
            ],
            [0, 0, 1],
        ),
        CardinalDirection::South => ModelPlane::new(
            [
                [0.0, 0.0, 1.0 - offset],
                [1.0, 0.0, 1.0 - offset],
                [1.0, 1.0, 1.0 - offset],
                [0.0, 1.0, 1.0 - offset],
            ],
            [0, 0, -1],
        ),
        CardinalDirection::East => ModelPlane::new(
            [
                [1.0 - offset, 0.0, 1.0],
                [1.0 - offset, 0.0, 0.0],
                [1.0 - offset, 1.0, 0.0],
                [1.0 - offset, 1.0, 1.0],
            ],
            [-1, 0, 0],
        ),
        CardinalDirection::West => ModelPlane::new(
            [
                [offset, 0.0, 0.0],
                [offset, 0.0, 1.0],
                [offset, 1.0, 1.0],
                [offset, 1.0, 0.0],
            ],
            [1, 0, 0],
        ),
    };
    ModelShape::default().with_planes([plane.with_material_slot("side")])
}

pub(crate) fn carpet_shape(name: &str) -> ModelShape {
    let height = if name == "snow_layer" { 0.125 } else { 0.0625 };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(stair_cuboid_with_slots(
        [0.0, 0.0, 0.0],
        [1.0, height, 1.0],
    ))])
}

pub(crate) fn rail_shape(state: &BlockStateQuery) -> ModelShape {
    let shape_value = state_string(state, "rail_direction")
        .or_else(|| state_string(state, "rail_shape"))
        .unwrap_or("north_south");
    let (corners, uv) = match shape_value {
        "east_west" => (
            [
                [0.0, 0.01, 0.0],
                [1.0, 0.01, 0.0],
                [1.0, 0.01, 1.0],
                [0.0, 0.01, 1.0],
            ],
            full_texture_uv(),
        ),
        "ascending_east" => (
            [
                [0.0, 0.01, 0.0],
                [1.0, 1.01, 0.0],
                [1.0, 1.01, 1.0],
                [0.0, 0.01, 1.0],
            ],
            full_texture_uv(),
        ),
        "ascending_west" => (
            [
                [0.0, 1.01, 0.0],
                [1.0, 0.01, 0.0],
                [1.0, 0.01, 1.0],
                [0.0, 1.01, 1.0],
            ],
            full_texture_uv(),
        ),
        "ascending_north" => (
            [
                [0.0, 1.01, 0.0],
                [1.0, 1.01, 0.0],
                [1.0, 0.01, 1.0],
                [0.0, 0.01, 1.0],
            ],
            full_texture_uv(),
        ),
        "ascending_south" => (
            [
                [0.0, 0.01, 0.0],
                [1.0, 0.01, 0.0],
                [1.0, 1.01, 1.0],
                [0.0, 1.01, 1.0],
            ],
            full_texture_uv(),
        ),
        _ => (
            [
                [0.0, 0.01, 0.0],
                [1.0, 0.01, 0.0],
                [1.0, 0.01, 1.0],
                [0.0, 0.01, 1.0],
            ],
            full_texture_uv(),
        ),
    };
    ModelShape::default().with_planes([ground_plane(corners, Some("up"), uv)])
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let top = state_string(state, "minecraft:vertical_half")
        .map(is_top_half)
        .or_else(|| state_bool(state, "upside_down_bit"))
        .unwrap_or(false);
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let shape_name = stairs_shape_name(state).unwrap_or("straight");
    let mut cuboids = Vec::with_capacity(3);
    if top {
        cuboids.push(detail_cuboid_with_local_uv(stair_cuboid_with_slots(
            [0.0, 0.5, 0.0],
            [1.0, 1.0, 1.0],
        )));
        push_stair_step_cuboids(&mut cuboids, direction, shape_name, 0.0, 0.5);
    } else {
        cuboids.push(detail_cuboid_with_local_uv(stair_cuboid_with_slots(
            [0.0, 0.0, 0.0],
            [1.0, 0.5, 1.0],
        )));
        push_stair_step_cuboids(&mut cuboids, direction, shape_name, 0.5, 1.0);
    }
    ModelShape::from_cuboids(cuboids)
}

fn is_top_half(value: &str) -> bool {
    value == "top" || value == "upper"
}

fn stairs_shape_name<'a>(state: &'a BlockStateQuery) -> Option<&'a str> {
    state_string(state, "shape")
        .or_else(|| state_string(state, "stairs_shape"))
        .or_else(|| state_string(state, "minecraft:shape"))
        .or_else(|| state_string(state, "minecraft:stairs_shape"))
}

fn stair_cuboid_with_slots(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    ModelCuboid::new(min, max)
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "south")
        .with_face_material_slot(BlockFace::East, "east")
        .with_face_material_slot(BlockFace::West, "west")
        .with_face_material_slot(BlockFace::Side, "side")
}

fn push_stair_step_cuboids(
    cuboids: &mut Vec<ModelCuboid>,
    direction: CardinalDirection,
    shape: &str,
    min_y: f32,
    max_y: f32,
) {
    match shape {
        "outer_left" => cuboids.push(detail_cuboid_with_local_uv(quarter_stair_cuboid(
            direction, true, true, min_y, max_y,
        ))),
        "outer_right" => cuboids.push(detail_cuboid_with_local_uv(quarter_stair_cuboid(
            direction, true, false, min_y, max_y,
        ))),
        "inner_left" => {
            cuboids.push(detail_cuboid_with_local_uv(back_half_cuboid(
                direction, min_y, max_y,
            )));
            cuboids.push(detail_cuboid_with_local_uv(quarter_stair_cuboid(
                direction, false, true, min_y, max_y,
            )));
        }
        "inner_right" => {
            cuboids.push(detail_cuboid_with_local_uv(back_half_cuboid(
                direction, min_y, max_y,
            )));
            cuboids.push(detail_cuboid_with_local_uv(quarter_stair_cuboid(
                direction, false, false, min_y, max_y,
            )));
        }
        _ => cuboids.push(detail_cuboid_with_local_uv(back_half_cuboid(
            direction, min_y, max_y,
        ))),
    }
}

fn quarter_stair_cuboid(
    direction: CardinalDirection,
    back: bool,
    left: bool,
    min_y: f32,
    max_y: f32,
) -> ModelCuboid {
    let (min_x, max_x, min_z, max_z) = match direction {
        CardinalDirection::North => {
            let (min_x, max_x) = if left { (0.0, 0.5) } else { (0.5, 1.0) };
            let (min_z, max_z) = if back { (0.0, 0.5) } else { (0.5, 1.0) };
            (min_x, max_x, min_z, max_z)
        }
        CardinalDirection::South => {
            let (min_x, max_x) = if left { (0.5, 1.0) } else { (0.0, 0.5) };
            let (min_z, max_z) = if back { (0.5, 1.0) } else { (0.0, 0.5) };
            (min_x, max_x, min_z, max_z)
        }
        CardinalDirection::East => {
            let (min_x, max_x) = if back { (0.5, 1.0) } else { (0.0, 0.5) };
            let (min_z, max_z) = if left { (0.0, 0.5) } else { (0.5, 1.0) };
            (min_x, max_x, min_z, max_z)
        }
        CardinalDirection::West => {
            let (min_x, max_x) = if back { (0.0, 0.5) } else { (0.5, 1.0) };
            let (min_z, max_z) = if left { (0.5, 1.0) } else { (0.0, 0.5) };
            (min_x, max_x, min_z, max_z)
        }
    };
    stair_cuboid_with_slots([min_x, min_y, min_z], [max_x, max_y, max_z])
}

fn back_half_cuboid(direction: CardinalDirection, min_y: f32, max_y: f32) -> ModelCuboid {
    let cuboid = match direction {
        CardinalDirection::North => ModelCuboid::new([0.0, min_y, 0.0], [1.0, max_y, 0.5]),
        CardinalDirection::South => ModelCuboid::new([0.0, min_y, 0.5], [1.0, max_y, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5, min_y, 0.0], [1.0, max_y, 1.0]),
        CardinalDirection::West => ModelCuboid::new([0.0, min_y, 0.0], [0.5, max_y, 1.0]),
    };
    stair_cuboid_with_slots(cuboid.min, cuboid.max)
}
