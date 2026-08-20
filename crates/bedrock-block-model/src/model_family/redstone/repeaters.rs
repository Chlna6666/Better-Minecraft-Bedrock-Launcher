use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("repeater") || name.contains("comparator") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let along_x = matches!(
        direction,
        CardinalDirection::North | CardinalDirection::South
    );
    let mut cuboids = vec![ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])];

    if name.contains("comparator") {
        let is_subtract = state_bool(state, "output_subtract_bit")
            .or_else(|| state_string(state, "comparator_mode").map(|s| s == "subtract" || s == "1"))
            .unwrap_or(false);

        let front_torch_y_max = if is_subtract { 0.3125 } else { 0.375 };
        cuboids.push(ModelCuboid::new(
            [0.4375, 0.125, 0.1875],
            [0.5625, front_torch_y_max, 0.3125],
        ));
        cuboids.push(ModelCuboid::new(
            [0.25, 0.125, 0.6875],
            [0.375, 0.375, 0.8125],
        ));
        cuboids.push(ModelCuboid::new(
            [0.625, 0.125, 0.6875],
            [0.75, 0.375, 0.8125],
        ));
    } else {
        let delay = state_i64(state, "repeater_delay")
            .or_else(|| state_i64(state, "delay"))
            .unwrap_or(0)
            .clamp(0, 3) as f32;

        let base_z = 0.4375 + delay * 0.125;
        cuboids.push(ModelCuboid::new(
            [0.4375, 0.125, 0.1875],
            [0.5625, 0.375, 0.3125],
        ));
        cuboids.push(ModelCuboid::new(
            [0.4375, 0.125, base_z],
            [0.5625, 0.375, base_z + 0.125],
        ));
    }

    oriented_xz_shape(along_x, &cuboids)
}

fn oriented_xz_shape(along_x: bool, cuboids: &[ModelCuboid]) -> ModelShape {
    if along_x {
        return ModelShape::from_cuboids(cuboids.to_vec());
    }
    ModelShape::from_cuboids(cuboids.iter().map(rotate_cuboid_xz).collect::<Vec<_>>())
}

fn rotate_cuboid_xz(cuboid: &ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [cuboid.min[2], cuboid.min[1], cuboid.min[0]],
        [cuboid.max[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot.clone();
    for (face, slot) in &cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(rotate_block_face_xz(*face), slot.clone());
    }
    for (face, uv) in &cuboid.face_uvs {
        rotated.face_uvs.insert(rotate_block_face_xz(*face), *uv);
    }
    rotated
}

fn rotate_block_face_xz(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::West,
        BlockFace::South => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::West => BlockFace::North,
        _ => face,
    }
}
