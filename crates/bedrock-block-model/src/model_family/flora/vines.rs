use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, state_bool, state_i64, state_string};
use crate::model_family::shape::{ModelPlane, ModelShape, upright_full_texture_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if is_vine_like_block(name) {
        Some(ModelFamily::Vine)
    } else {
        None
    }
}

pub(crate) fn vine_shape(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if !is_vine_like_block(name) {
        return None;
    }

    let mut planes = Vec::new();
    // Bedrock stores the vine sides in one integer bitmask (`vine_direction_bits`:
    // 1=south, 2=west, 4=north, 8=east). The boolean probes below never match that state,
    // so without this branch every vine fell back to the north face and appeared detached
    // from its support.
    if name == "vine"
        && let Some(bits) = state_i64(state, "vine_direction_bits")
            .or_else(|| state_i64(state, "minecraft:vine_direction_bits"))
    {
        for (mask, direction) in [
            (1, CardinalDirection::South),
            (2, CardinalDirection::West),
            (4, CardinalDirection::North),
            (8, CardinalDirection::East),
        ] {
            if bits & mask != 0 {
                planes.push(vine_plane(direction));
            }
        }
        if planes.is_empty() {
            // Value 0 has no side faces; the game shows a ceiling vine instead.
            planes.push(vine_ceiling_plane());
        }
        return Some(ModelShape::default().with_planes(planes));
    }
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
        // Wall fans are NOT an upright card: the capture (refs 233..236) is a 12.8x2.4 px plate
        // lying at half height (y 8..10.4 px) against the named wall.
        return Some(coral_wall_fan_shape(direction));
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

/// The wall-mounted coral fan: a 12.8 x 2.4 px plate lying at half height against the named
/// wall (capture refs 233..236, `coral_direction` 0..3). The slight 1.6 px tilt of the real
/// sheet is dropped for the flat plate; its top face carries the fan art upright.
fn coral_wall_fan_shape(direction: CardinalDirection) -> ModelShape {
    let (min, max, normal) = match direction {
        CardinalDirection::East => ([0.19_f32, 0.5, 0.05], [0.99, 0.65, 0.95], [1, 0, 0]),
        CardinalDirection::West => ([0.01, 0.5, 0.05], [0.81, 0.65, 0.95], [-1, 0, 0]),
        CardinalDirection::South => ([0.05, 0.5, 0.19], [0.95, 0.65, 0.99], [0, 0, 1]),
        CardinalDirection::North => ([0.05, 0.5, 0.01], [0.95, 0.65, 0.81], [0, 0, -1]),
    };
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let (corners, normal) = match normal[0] {
        1 | -1 => (
            [
                [x1, y1, z0],
                [x1, y1, z1],
                [x0, y1, z1],
                [x0, y1, z0],
            ],
            [0, 1, 0],
        ),
        _ => (
            [
                [x0, y1, z1],
                [x1, y1, z1],
                [x1, y1, z0],
                [x0, y1, z0],
            ],
            [0, 1, 0],
        ),
    };
    ModelShape::default().with_planes([ModelPlane::new(corners, normal).with_uv(
        upright_full_texture_uv(),
    )])
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
    ModelPlane::new(corners, normal).with_uv(upright_full_texture_uv())
}

fn vine_ceiling_plane() -> ModelPlane {
    ModelPlane::new(
        [
            [0.0, 0.99, 1.0],
            [1.0, 0.99, 1.0],
            [1.0, 0.99, 0.0],
            [0.0, 0.99, 0.0],
        ],
        [0, -1, 0],
    )
    .with_uv(upright_full_texture_uv())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vine_planes(bits: i64) -> Vec<ModelPlane> {
        let state = BlockStateQuery::new("minecraft:vine").with_state("vine_direction_bits", bits);
        vine_shape("vine", &state).expect("vine shape").planes
    }

    #[test]
    fn vine_direction_bits_place_planes_on_the_matching_faces() {
        for (bits, normal, axis, expected) in [
            (1, [0, 0, -1], 2, 0.99_f32),
            (2, [1, 0, 0], 0, 0.01),
            (4, [0, 0, 1], 2, 0.01),
            (8, [-1, 0, 0], 0, 0.99),
        ] {
            let planes = vine_planes(bits);
            assert_eq!(planes.len(), 1, "bits {bits}");
            assert_eq!(planes[0].normal, normal, "bits {bits}");
            for corner in &planes[0].corners {
                assert!(
                    (corner[axis] - expected).abs() < 0.001,
                    "bits {bits} corner {corner:?}"
                );
            }
        }
    }

    #[test]
    fn vine_direction_bits_combine_multiple_faces() {
        let planes = vine_planes(1 | 4);
        assert_eq!(planes.len(), 2);
        assert!(planes.iter().any(|plane| plane.normal == [0, 0, -1]));
        assert!(planes.iter().any(|plane| plane.normal == [0, 0, 1]));
    }

    #[test]
    fn zero_vine_direction_bits_render_a_ceiling_plane() {
        let planes = vine_planes(0);
        assert_eq!(planes.len(), 1);
        assert_eq!(planes[0].normal, [0, -1, 0]);
    }
}
