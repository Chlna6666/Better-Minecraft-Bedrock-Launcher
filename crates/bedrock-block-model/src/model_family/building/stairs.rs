use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, stair_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv,
    upright_full_texture_uv,
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
    // facing_direction is the side the ladder's rungs face, so the board sits against the
    // opposite (support) boundary and its normal points toward the facing direction.
    let plane = match direction {
        CardinalDirection::North => ModelPlane::new(
            [
                [0.0, 0.0, 1.0 - offset],
                [1.0, 0.0, 1.0 - offset],
                [1.0, 1.0, 1.0 - offset],
                [0.0, 1.0, 1.0 - offset],
            ],
            [0, 0, -1],
        ),
        CardinalDirection::South => ModelPlane::new(
            [
                [1.0, 0.0, offset],
                [0.0, 0.0, offset],
                [0.0, 1.0, offset],
                [1.0, 1.0, offset],
            ],
            [0, 0, 1],
        ),
        CardinalDirection::East => ModelPlane::new(
            [
                [offset, 0.0, 0.0],
                [offset, 0.0, 1.0],
                [offset, 1.0, 1.0],
                [offset, 1.0, 0.0],
            ],
            [1, 0, 0],
        ),
        CardinalDirection::West => ModelPlane::new(
            [
                [1.0 - offset, 0.0, 1.0],
                [1.0 - offset, 0.0, 0.0],
                [1.0 - offset, 1.0, 0.0],
                [1.0 - offset, 1.0, 1.0],
            ],
            [-1, 0, 0],
        ),
    };
    ModelShape::default().with_planes([plane
        .with_material_slot("side")
        .with_uv(upright_full_texture_uv())])
}

pub(crate) fn carpet_shape(name: &str) -> ModelShape {
    let height = if name == "snow_layer" { 0.125 } else { 0.0625 };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(stair_cuboid_with_slots(
        [0.0, 0.0, 0.0],
        [1.0, height, 1.0],
    ))])
}

pub(crate) fn rail_shape(state: &BlockStateQuery) -> ModelShape {
    // Bedrock stores the six straight/sloped orientations and four curves in rail_direction.
    // The rail plane sits 1 px above the block floor; slopes rise one full block over the run
    // and curves use the turned texture.
    let direction = state_i64(state, "rail_direction")
        .or_else(|| state_i64(state, "rail_shape"))
        .unwrap_or(0)
        .clamp(0, 9);
    let (y_rotation, sloped) = match direction {
        0 => (0.0_f32, false),
        1 => (90.0, false),
        2 => (-90.0, true),
        3 => (90.0, true),
        4 => (180.0, true),
        5 => (0.0, true),
        6 => (90.0, false),
        7 => (180.0, false),
        8 => (-90.0, false),
        _ => (0.0, false),
    };
    let curved = direction > 5;
    let base = 1.0 / 16.0;
    let mut corners = if sloped {
        [
            [0.0, base, 0.0],
            [1.0, base, 0.0],
            [1.0, base + 1.0, 1.0],
            [0.0, base + 1.0, 1.0],
        ]
    } else {
        [
            [0.0, base, 0.0],
            [1.0, base, 0.0],
            [1.0, base, 1.0],
            [0.0, base, 1.0],
        ]
    };
    if y_rotation != 0.0 {
        let (sin, cos) = y_rotation.to_radians().sin_cos();
        for corner in &mut corners {
            let x = corner[0] - 0.5;
            let z = corner[2] - 0.5;
            corner[0] = 0.5 + x * cos + z * sin;
            corner[2] = 0.5 - x * sin + z * cos;
        }
    }
    // blocks.json defines up=turned and down/side=straight for rails, mirroring how the
    // client picks the straight or curved texture from the block state.
    let slot = if curved { "up" } else { "side" };
    ModelShape::default().with_planes([ModelPlane::new(corners, [0, 1, 0])
        .with_material_slot(slot)
        .with_uv(full_texture_uv())])
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let top = state_string(state, "minecraft:vertical_half")
        .map(is_top_half)
        .or_else(|| state_bool(state, "upside_down_bit"))
        .unwrap_or(false);
    let direction = stair_direction(state).unwrap_or(CardinalDirection::North);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_weirdo_direction_matches_bedrock_stairs_encoding() {
        let cases = [
            (0, [0.5, 0.5, 0.0], [1.0, 1.0, 1.0]),
            (1, [0.0, 0.5, 0.0], [0.5, 1.0, 1.0]),
            (2, [0.0, 0.5, 0.5], [1.0, 1.0, 1.0]),
            (3, [0.0, 0.5, 0.0], [1.0, 1.0, 0.5]),
        ];

        for (direction, expected_min, expected_max) in cases {
            let state = BlockStateQuery::new("minecraft:oak_stairs")
                .with_state("weirdo_direction", direction)
                .with_state("upside_down_bit", false);
            let shape = shape(&state);
            assert_eq!(shape.cuboids.len(), 2);
            assert_eq!(shape.cuboids[1].min, expected_min, "direction={direction}");
            assert_eq!(shape.cuboids[1].max, expected_max, "direction={direction}");
        }
    }
}
