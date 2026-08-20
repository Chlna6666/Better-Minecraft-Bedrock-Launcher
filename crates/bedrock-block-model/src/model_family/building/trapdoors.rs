use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, cardinal_direction_from_string, state_bool, state_i64,
    state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_trapdoor") || name == "trapdoor" {
        Some(ModelFamily::Trapdoor)
    } else {
        None
    }
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    if trapdoor_is_open(state) {
        let direction = trapdoor_direction(state).unwrap_or(CardinalDirection::North);
        let cuboid =
            trapdoor_cuboid_with_uv(wall_attached_cuboid(direction.opposite(), 0.1875, 0.0, 1.0));
        let cuboid = if trapdoor_is_top(state) {
            flip_open_top_side_uvs(cuboid)
        } else {
            cuboid
        };
        return ModelShape::from_cuboids([cuboid]);
    }

    let top = trapdoor_is_top(state);
    let (min_y, max_y) = if top { (0.8125, 1.0) } else { (0.0, 0.1875) };
    ModelShape::from_cuboids([trapdoor_cuboid_with_uv(ModelCuboid::new(
        [0.0, min_y, 0.0],
        [1.0, max_y, 1.0],
    ))])
}

fn flip_open_top_side_uvs(mut cuboid: ModelCuboid) -> ModelCuboid {
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        if cuboid
            .face_material_slots
            .get(&face)
            .is_none_or(|slot| slot != "side")
        {
            continue;
        }
        let Some(uv) = cuboid.face_uvs.get(&face).copied() else {
            continue;
        };
        cuboid.face_uvs.insert(face, [uv[2], uv[3], uv[0], uv[1]]);
    }
    cuboid
}

fn trapdoor_direction(state: &BlockStateQuery) -> Option<CardinalDirection> {
    state_i64(state, "direction")
        .and_then(trapdoor_direction_from_int)
        .or_else(|| {
            state_string(state, "direction").and_then(|value| {
                value
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .and_then(trapdoor_direction_from_int)
                    .or_else(|| cardinal_direction_from_string(value))
            })
        })
        .or_else(|| cardinal_direction(state))
}

fn trapdoor_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::West),
        1 => Some(CardinalDirection::East),
        2 => Some(CardinalDirection::North),
        3 => Some(CardinalDirection::South),
        _ => None,
    }
}

fn trapdoor_is_open(state: &BlockStateQuery) -> bool {
    state_bool(state, "open")
        .or_else(|| state_bool(state, "open_bit"))
        .or_else(|| state_bool(state, "minecraft:open"))
        .or_else(|| state_bool(state, "minecraft:open_bit"))
        .unwrap_or(false)
}

fn trapdoor_is_top(state: &BlockStateQuery) -> bool {
    state_string(state, "half")
        .or_else(|| state_string(state, "vertical_half"))
        .or_else(|| state_string(state, "minecraft:half"))
        .or_else(|| state_string(state, "minecraft:vertical_half"))
        .is_some_and(|value| value == "top" || value == "upper")
        || state_bool(state, "upside_down_bit").unwrap_or(false)
}

fn trapdoor_cuboid_with_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let mut cuboid = detail_cuboid_with_local_uv(cuboid);
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::West,
        BlockFace::East,
    ] {
        let slot = trapdoor_face_material_slot(&cuboid, face);
        cuboid = cuboid.with_face_material_slot(face, slot);
    }
    cuboid
}

fn trapdoor_face_material_slot(cuboid: &ModelCuboid, face: BlockFace) -> &'static str {
    let size = [
        cuboid.max[0] - cuboid.min[0],
        cuboid.max[1] - cuboid.min[1],
        cuboid.max[2] - cuboid.min[2],
    ];
    if size[1] <= size[0] && size[1] <= size[2] {
        return match face {
            BlockFace::Up => "up",
            BlockFace::Down => "down",
            _ => "side",
        };
    }
    if size[2] <= size[0] && size[2] <= size[1] {
        return match face {
            BlockFace::North => "up",
            BlockFace::South => "down",
            _ => "side",
        };
    }
    match face {
        BlockFace::West => "up",
        BlockFace::East => "down",
        _ => "side",
    }
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
