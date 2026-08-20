use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, state_bool, state_i64, state_string};
use crate::model_family::shape::{ModelPlane, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if is_vine_like_block(name) {
        Some(ModelFamily::Vine)
    } else {
        None
    }
}

pub(crate) fn shape_for_vine(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if !is_vine_like_block(name) {
        return None;
    }

    let mut planes = Vec::new();
    if matches!(name, "vine" | "weeping_vines" | "twisting_vines") {
        for direction in CardinalDirection::ALL {
            let key = direction.state_key();
            if state_bool(state, key)
                .or_else(|| state_bool(state, &format!("{key}_bit")))
                .unwrap_or(false)
            {
                planes.push(vine_plane(direction));
            }
        }
    }

    if planes.is_empty()
        && let Some(face) = state_string(state, "minecraft:block_face")
            .or_else(|| state_string(state, "block_face"))
            .or_else(|| state_string(state, "vine_direction"))
        && let Some(direction) = vine_direction_from_string(face)
    {
        planes.push(vine_plane(direction));
    }

    if planes.is_empty()
        && let Some(direction) = state_i64(state, "minecraft:coral_direction")
            .or_else(|| state_i64(state, "coral_direction"))
            .or_else(|| state_i64(state, "minecraft:direction"))
            .or_else(|| state_i64(state, "direction"))
            .and_then(coral_direction_from_int)
    {
        planes.push(vine_plane(direction));
    }

    if planes.is_empty() {
        planes.push(vine_plane(CardinalDirection::North));
    }

    Some(ModelShape::default().with_planes(planes))
}

fn is_vine_like_block(name: &str) -> bool {
    name.ends_with("_wall_fan")
        || matches!(
            name,
            "vine"
                | "weeping_vines"
                | "twisting_vines"
                | "coral_fan_hang"
                | "coral_fan_hang2"
                | "coral_fan_hang3"
        )
}

fn vine_direction_from_string(value: &str) -> Option<CardinalDirection> {
    match value {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

fn coral_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::East),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::South),
        3 => Some(CardinalDirection::North),
        _ => None,
    }
}

fn vine_plane(direction: CardinalDirection) -> ModelPlane {
    let (corners, normal) = match direction {
        CardinalDirection::North => (
            [
                [1.0, 0.0, 0.01],
                [0.0, 0.0, 0.01],
                [0.0, 1.0, 0.01],
                [1.0, 1.0, 0.01],
            ],
            [0, 0, 1],
        ),
        CardinalDirection::South => (
            [
                [0.0, 0.0, 0.99],
                [1.0, 0.0, 0.99],
                [1.0, 1.0, 0.99],
                [0.0, 1.0, 0.99],
            ],
            [0, 0, -1],
        ),
        CardinalDirection::East => (
            [
                [0.99, 0.0, 1.0],
                [0.99, 0.0, 0.0],
                [0.99, 1.0, 0.0],
                [0.99, 1.0, 1.0],
            ],
            [-1, 0, 0],
        ),
        CardinalDirection::West => (
            [
                [0.01, 0.0, 0.0],
                [0.01, 0.0, 1.0],
                [0.01, 1.0, 1.0],
                [0.01, 1.0, 0.0],
            ],
            [1, 0, 0],
        ),
    };
    ModelPlane::new(corners, normal)
}
