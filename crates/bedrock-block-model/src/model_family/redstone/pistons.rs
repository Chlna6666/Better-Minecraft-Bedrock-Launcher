use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("piston") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let extended = state_bool(state, "extended_bit")
        .or_else(|| state_bool(state, "is_extended"))
        .or_else(|| state_bool(state, "extended"))
        .unwrap_or(false);

    if !extended {
        return ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])]);
    }

    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let (base_max_z, shaft_min_z, shaft_max_z, head_min_z) = (0.75, 0.75, 1.0, 1.0);

    let cuboids = match direction {
        CardinalDirection::North => vec![
            ModelCuboid::new([0.0, 0.0, 1.0 - base_max_z], [1.0, 1.0, 1.0]),
            ModelCuboid::new(
                [0.375, 0.375, 1.0 - shaft_max_z],
                [0.625, 0.625, 1.0 - shaft_min_z],
            ),
            ModelCuboid::new([0.0, 0.0, -0.25], [1.0, 1.0, 0.0]),
        ],
        CardinalDirection::South => vec![
            ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, base_max_z]),
            ModelCuboid::new([0.375, 0.375, shaft_min_z], [0.625, 0.625, shaft_max_z]),
            ModelCuboid::new([0.0, 0.0, head_min_z], [1.0, 1.0, 1.25]),
        ],
        CardinalDirection::East => vec![
            ModelCuboid::new([0.0, 0.0, 0.0], [base_max_z, 1.0, 1.0]),
            ModelCuboid::new([shaft_min_z, 0.375, 0.375], [shaft_max_z, 0.625, 0.625]),
            ModelCuboid::new([head_min_z, 0.0, 0.0], [1.25, 1.0, 1.0]),
        ],
        CardinalDirection::West => vec![
            ModelCuboid::new([1.0 - base_max_z, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ModelCuboid::new(
                [1.0 - shaft_max_z, 0.375, 0.375],
                [1.0 - shaft_min_z, 0.625, 0.625],
            ),
            ModelCuboid::new([-0.25, 0.0, 0.0], [0.0, 1.0, 1.0]),
        ],
    };

    ModelShape::from_cuboids(cuboids)
}
