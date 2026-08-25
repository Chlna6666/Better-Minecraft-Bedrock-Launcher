use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv, rect_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_chest")
        || matches!(name, "chest" | "trapped_chest" | "ender_chest" | "barrel")
    {
        return Some(ModelFamily::Container);
    }
    if name.ends_with("_shulker_box") || matches!(name, "shulker_box" | "undyed_shulker_box") {
        return Some(ModelFamily::ShulkerBox);
    }
    if name == "chiseled_bookshelf" {
        return Some(ModelFamily::ChiseledBookshelf);
    }
    None
}

pub(crate) fn container_shape(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if name == "barrel" {
        return Some(barrel_shape(state));
    }
    if name == "chest" || name.ends_with("_chest") {
        return Some(chest_shape(state));
    }
    Some(ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    )
    .with_face_material_slot(BlockFace::Up, "up")
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "side")
    .with_face_material_slot(BlockFace::South, "side")
    .with_face_material_slot(BlockFace::East, "side")
    .with_face_material_slot(BlockFace::West, "side")]))
}

fn barrel_shape(state: &BlockStateQuery) -> ModelShape {
    let is_open = crate::model_family::direction::state_bool(state, "open_bit").unwrap_or(false);
    let top_slot = if is_open { "barrel_top_open" } else { "up" };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ))
    .with_face_material_slot(BlockFace::Up, top_slot)
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "side")
    .with_face_material_slot(BlockFace::South, "side")
    .with_face_material_slot(BlockFace::East, "side")
    .with_face_material_slot(BlockFace::West, "side")])
}

pub(crate) fn shulker_box_shape() -> ModelShape {
    ModelShape::from_cuboids([
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.5, 1.0])
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Side, "side"),
        ModelCuboid::new([0.0, 0.25, 0.0], [1.0, 1.0, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Side, "side"),
    ])
}

pub(crate) fn chiseled_bookshelf_shape(state: &BlockStateQuery) -> ModelShape {
    let books = state_i64(state, "books_stored").unwrap_or(63);
    let mut cuboids = vec![detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, px(1.0)], [1.0, 1.0, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side")
            .with_face_material_slot(BlockFace::North, "front"),
    )];

    for slot in 0..6 {
        if books & (1 << slot) == 0 {
            continue;
        }
        let row = slot / 3;
        let column = slot % 3;
        let (x0, x1) = match column {
            0 => (0.0, 5.0),
            1 => (5.0, 11.0),
            _ => (11.0, 16.0),
        };
        let (y0, y1) = if row == 0 { (8.0, 16.0) } else { (0.0, 8.0) };
        cuboids.push(
            detail_cuboid_with_local_uv(ModelCuboid::new(
                [px(x0), px(y0), 0.0],
                [px(x1), px(y1), px(1.0)],
            ))
            .with_material_slot("front"),
        );
    }

    ModelShape::from_cuboids(cuboids)
}

fn chest_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let front_face = block_face_from_direction(direction);
    let back_face = opposite_block_face(front_face);
    let (left_face, right_face) = left_right_faces(direction);
    let pair_direction = state_string(state, "pair_direction").and_then(cardinal_from_string);
    let (body_min, body_max) = chest_pair_bounds(pair_direction);
    let mut cuboids = vec![
        chest_box_uv(ModelCuboid::new(
            body_min,
            [body_max[0], 0.625, body_max[2]],
        ))
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(front_face, "front")
        .with_face_material_slot(back_face, "side")
        .with_face_material_slot(left_face, "side")
        .with_face_material_slot(right_face, "side"),
        chest_lid_uv(ModelCuboid::new(
            [body_min[0], 0.625, body_min[2]],
            [body_max[0], 0.875, body_max[2]],
        ))
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(front_face, "front")
        .with_face_material_slot(back_face, "side")
        .with_face_material_slot(left_face, "side")
        .with_face_material_slot(right_face, "side"),
        chest_latch_uv(face_centered_cuboid(
            direction, 0.1875, 0.0625, 0.3125, 0.5625,
        ))
        .with_face_material_slot(front_face, "front"),
    ];
    ModelShape::from_cuboids(std::mem::take(&mut cuboids))
}

fn chest_pair_bounds(pair_direction: Option<CardinalDirection>) -> ([f32; 3], [f32; 3]) {
    let mut min = [0.0625, 0.0, 0.0625];
    let mut max = [0.9375, 1.0, 0.9375];
    match pair_direction {
        Some(CardinalDirection::North) => min[2] = 0.0,
        Some(CardinalDirection::South) => max[2] = 1.0,
        Some(CardinalDirection::East) => max[0] = 1.0,
        Some(CardinalDirection::West) => min[0] = 0.0,
        None => {}
    }
    (min, max)
}

fn cardinal_from_string(value: &str) -> Option<CardinalDirection> {
    match value
        .trim()
        .strip_prefix("minecraft:")
        .unwrap_or(value.trim())
    {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

fn block_face_from_direction(direction: CardinalDirection) -> BlockFace {
    match direction {
        CardinalDirection::North => BlockFace::North,
        CardinalDirection::South => BlockFace::South,
        CardinalDirection::East => BlockFace::East,
        CardinalDirection::West => BlockFace::West,
    }
}

fn opposite_block_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::South,
        BlockFace::South => BlockFace::North,
        BlockFace::East => BlockFace::West,
        BlockFace::West => BlockFace::East,
        other => other,
    }
}

fn left_right_faces(direction: CardinalDirection) -> (BlockFace, BlockFace) {
    match direction {
        CardinalDirection::North | CardinalDirection::South => (BlockFace::East, BlockFace::West),
        CardinalDirection::East | CardinalDirection::West => (BlockFace::North, BlockFace::South),
    }
}

fn face_centered_cuboid(
    direction: CardinalDirection,
    width: f32,
    thickness: f32,
    min_y: f32,
    max_y: f32,
) -> ModelCuboid {
    let half_width = width * 0.5;
    match direction {
        CardinalDirection::North => ModelCuboid::new(
            [0.5 - half_width, min_y, 0.0],
            [0.5 + half_width, max_y, thickness],
        ),
        CardinalDirection::South => ModelCuboid::new(
            [0.5 - half_width, min_y, 1.0 - thickness],
            [0.5 + half_width, max_y, 1.0],
        ),
        CardinalDirection::East => ModelCuboid::new(
            [1.0 - thickness, min_y, 0.5 - half_width],
            [1.0, max_y, 0.5 + half_width],
        ),
        CardinalDirection::West => ModelCuboid::new(
            [0.0, min_y, 0.5 - half_width],
            [thickness, max_y, 0.5 + half_width],
        ),
    }
}

fn chest_box_uv(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_uv(BlockFace::Down, uv64(28.0, 19.0, 14.0, 14.0))
        .with_face_uv(BlockFace::North, uv64(14.0, 33.0, 14.0, 10.0))
        .with_face_uv(BlockFace::South, uv64(14.0, 33.0, 14.0, 10.0))
        .with_face_uv(BlockFace::West, uv64(0.0, 33.0, 14.0, 10.0))
        .with_face_uv(BlockFace::East, uv64(0.0, 33.0, 14.0, 10.0))
}

fn chest_lid_uv(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_uv(BlockFace::Up, uv64(14.0, 0.0, 14.0, 14.0))
        .with_face_uv(BlockFace::Down, uv64(28.0, 0.0, 14.0, 14.0))
        .with_face_uv(BlockFace::North, uv64(14.0, 14.0, 14.0, 4.0))
        .with_face_uv(BlockFace::South, uv64(14.0, 14.0, 14.0, 4.0))
        .with_face_uv(BlockFace::West, uv64(0.0, 14.0, 14.0, 4.0))
        .with_face_uv(BlockFace::East, uv64(0.0, 14.0, 14.0, 4.0))
}

fn chest_latch_uv(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_uv(BlockFace::South, uv64(0.0, 1.0, 2.0, 4.0))
        .with_face_uv(BlockFace::North, uv64(0.0, 1.0, 2.0, 4.0))
        .with_face_uv(BlockFace::West, uv64(0.0, 1.0, 2.0, 4.0))
        .with_face_uv(BlockFace::East, uv64(0.0, 1.0, 2.0, 4.0))
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

fn uv64(u: f32, v: f32, width: f32, height: f32) -> [[f32; 2]; 4] {
    rect_uv(u / 64.0, v / 64.0, (u + width) / 64.0, (v + height) / 64.0)
}
