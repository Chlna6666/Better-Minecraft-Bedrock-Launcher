use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, rotate_cuboid_cardinal};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("repeater") || name.contains("comparator") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let output = repeater_output_direction(state);
    let mut cuboids = vec![ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])];

    if name.contains("comparator") {
        let is_subtract = state_bool(state, "output_subtract_bit")
            .or_else(|| state_string(state, "comparator_mode").map(|s| s == "subtract" || s == "1"))
            .unwrap_or(false);

        let front_torch_y_max = if is_subtract { 0.3125 } else { 0.375 };
        cuboids.push(torch_cuboid(ModelCuboid::new(
            [0.4375, 0.125, 0.1875],
            [0.5625, front_torch_y_max, 0.3125],
        )));
        cuboids.push(torch_cuboid(ModelCuboid::new(
            [0.25, 0.125, 0.6875],
            [0.375, 0.375, 0.8125],
        )));
        cuboids.push(torch_cuboid(ModelCuboid::new(
            [0.625, 0.125, 0.6875],
            [0.75, 0.375, 0.8125],
        )));
    } else {
        let delay = state_i64(state, "repeater_delay")
            .or_else(|| state_i64(state, "delay"))
            .unwrap_or(0)
            .clamp(0, 3) as f32;

        let base_z = 0.4375 + delay * 0.125;
        cuboids.push(torch_cuboid(ModelCuboid::new(
            [0.4375, 0.125, 0.1875],
            [0.5625, 0.375, 0.3125],
        )));
        cuboids.push(torch_cuboid(ModelCuboid::new(
            [0.4375, 0.125, base_z],
            [0.5625, 0.375, base_z + 0.125],
        )));
    }

    // The torches above are authored with the output torch on the north side; the whole
    // assembly turns with the block.
    ModelShape::from_cuboids(
        cuboids
            .iter()
            .map(|cuboid| rotate_cuboid_cardinal(cuboid, output))
            .collect::<Vec<_>>(),
    )
}

/// The torch posts sample the shared `repeater_torch` / `comparator_torch` art, which is not
/// part of the block's own texture map.
fn torch_cuboid(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid.with_material_slot("torch")
}

/// Resolves the side the repeater drives power towards.
///
/// Bedrock's modern `minecraft:cardinal_direction` names the direction *from the output side
/// towards the input side*, so the output is the opposite face. The deprecated `direction`
/// integer names the output side directly (0=north, 1=east, 2=south, 3=west). Worlds may carry
/// both spellings, and the modern one wins because it describes the current state.
fn repeater_output_direction(state: &BlockStateQuery) -> CardinalDirection {
    if let Some(direction) = state_string(state, "minecraft:cardinal_direction")
        .or_else(|| state_string(state, "cardinal_direction"))
        .and_then(crate::model_family::direction::from_string)
    {
        return direction.opposite();
    }
    if let Some(value) = state_i64(state, "direction") {
        return match value.rem_euclid(4) {
            0 => CardinalDirection::North,
            1 => CardinalDirection::East,
            2 => CardinalDirection::South,
            _ => CardinalDirection::West,
        };
    }
    cardinal_direction(state)
        .map(CardinalDirection::opposite)
        .unwrap_or(CardinalDirection::North)
}
