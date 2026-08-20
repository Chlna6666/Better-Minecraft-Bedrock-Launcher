use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, DirectionConnection, direction_connection, state_bool,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_wall") || name == "cobblestone_wall" {
        Some(ModelFamily::Wall)
    } else {
        None
    }
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = Vec::with_capacity(5);
    if state_bool(state, "wall_post_bit").unwrap_or(true) {
        cuboids.push(wall_cuboid_with_uv(ModelCuboid::new(
            [0.25, 0.0, 0.25],
            [0.75, 1.0, 0.75],
        )));
    }
    for direction in CardinalDirection::ALL {
        if let Some(max_y) = wall_arm_height(state, direction) {
            cuboids.push(wall_cuboid_with_uv(wall_arm_cuboid(direction, max_y)));
        }
    }
    if cuboids.is_empty() {
        cuboids.push(wall_cuboid_with_uv(ModelCuboid::new(
            [0.25, 0.0, 0.25],
            [0.75, 1.0, 0.75],
        )));
    }
    ModelShape::from_cuboids(cuboids)
}

fn wall_arm_height(state: &BlockStateQuery, direction: CardinalDirection) -> Option<f32> {
    match direction_connection(state, direction) {
        Some(DirectionConnection::Disconnected) => None,
        Some(DirectionConnection::Tall) => Some(1.0),
        Some(DirectionConnection::Short) => Some(short_wall_height()),
        None => None,
    }
}

fn wall_arm_cuboid(direction: CardinalDirection, max_y: f32) -> ModelCuboid {
    let max_y = max_y.clamp(0.0, 1.0);
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(5.0), 0.0, 0.0], [px(11.0), max_y, px(8.0)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(5.0), 0.0, px(8.0)], [px(11.0), max_y, 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([0.0, 0.0, px(5.0)], [px(8.0), max_y, px(11.0)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([px(8.0), 0.0, px(5.0)], [1.0, max_y, px(11.0)])
        }
    }
}

fn wall_cuboid_with_uv(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::East, "side")
}

fn short_wall_height() -> f32 {
    px(13.0)
}

fn px(value: f32) -> f32 {
    value / 16.0
}
