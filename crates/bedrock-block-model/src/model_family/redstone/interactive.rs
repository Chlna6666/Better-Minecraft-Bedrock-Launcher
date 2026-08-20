use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{ModelCuboid, ModelShape};
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
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let pressed = state_bool(state, "button_pressed_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    let thickness = if pressed { 0.0625 } else { 0.125 };
    ModelShape::from_cuboids([wall_attached_cuboid(direction, thickness, 0.3125, 0.6875)])
}

pub(crate) fn pressure_plate_shape() -> ModelShape {
    ModelShape::from_cuboids([ModelCuboid::new(
        [0.0625, 0.0, 0.0625],
        [0.9375, 0.0625, 0.9375],
    )])
}

pub(crate) fn lever_shape(state: &BlockStateQuery) -> ModelShape {
    let open = state_bool(state, "open_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);

    let handle = if open {
        ModelCuboid::new([0.4375, 0.125, 0.25], [0.5625, 0.4375, 0.5625])
    } else {
        ModelCuboid::new([0.4375, 0.1875, 0.4375], [0.5625, 0.625, 0.5625])
    };

    ModelShape::from_cuboids([
        ModelCuboid::new([0.3125, 0.0, 0.25], [0.6875, 0.1875, 0.75]),
        handle,
    ])
}

pub(crate) fn tripwire_hook_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let attached = state_bool(state, "attached_bit").unwrap_or(false);
    let mut cuboids = vec![wall_attached_cuboid(direction, 0.1875, 0.125, 0.625)];
    if attached {
        cuboids.push(wall_attached_cuboid(direction, 0.375, 0.25, 0.375));
    }
    ModelShape::from_cuboids(cuboids)
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
