use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_door") || matches!(name, "wooden_door" | "iron_door") {
        Some(ModelFamily::Door)
    } else {
        None
    }
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let open = state_bool(state, "open")
        .or_else(|| state_bool(state, "open_bit"))
        .or_else(|| state_bool(state, "minecraft:open"))
        .unwrap_or(false);
    let hinge_right = state_bool(state, "door_hinge_bit")
        .or_else(|| state_bool(state, "hinge_bit"))
        .or_else(|| state_bool(state, "hinge"))
        .or_else(|| state_string(state, "hinge").map(|v| v == "right"))
        .unwrap_or(false);
    let upper = state_bool(state, "upper_block_bit")
        .or_else(|| state_bool(state, "upper_block"))
        .unwrap_or(false);

    let effective_dir = if open {
        if hinge_right {
            direction.clockwise()
        } else {
            direction.counter_clockwise()
        }
    } else {
        direction
    };
    // Doors are two blocks tall; each half samples its own `door_<wood>_lower/upper` tile.
    // Falling back to face-based naming gave both halves `door_upper` (blocks.json maps the
    // door's side faces to the upper tile), so the lower half showed the top artwork.
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let wood = name.strip_suffix("_door").unwrap_or("wooden_door");
    let wood = match wood {
        "wooden_door" | "wood" | "oak" => "wood",
        other => other,
    };
    let slot = format!("door_{wood}_{}", if upper { "upper" } else { "lower" });
    ModelShape::from_cuboids([
        wall_attached_cuboid(effective_dir, 0.1875, 0.0, 1.0).with_material_slot(slot.as_str())
    ])
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
