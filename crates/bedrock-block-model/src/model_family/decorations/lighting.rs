use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, block_face, cardinal_direction, state_bool, state_i64,
};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_torch")
        || matches!(name, "torch" | "redstone_torch" | "unlit_redstone_torch")
    {
        return Some(ModelFamily::Torch);
    }
    if name.ends_with("_lantern") || name == "lantern" {
        return Some(ModelFamily::Lantern);
    }
    if name.ends_with("_candle") || name == "candle" {
        return Some(ModelFamily::Candle);
    }
    if matches!(name, "portal" | "nether_portal" | "end_portal") {
        return Some(ModelFamily::Portal);
    }
    None
}

pub(crate) fn torch_shape(state: &BlockStateQuery) -> ModelShape {
    if block_face(state).is_some_and(|face| face == "ceiling") {
        return ModelShape::from_cuboids([ModelCuboid::new(
            [0.40625, 0.375, 0.40625],
            [0.59375, 1.0, 0.59375],
        )]);
    }
    if block_face(state).is_some_and(|face| face == "wall") {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([wall_torch_cuboid(direction)]);
    }
    ModelShape::from_cuboids([ModelCuboid::new(
        [0.40625, 0.0, 0.40625],
        [0.59375, 0.625, 0.59375],
    )])
}

pub(crate) fn lantern_shape(state: &BlockStateQuery) -> ModelShape {
    let hanging = state_bool(state, "hanging").unwrap_or(false);
    let mut cuboids = Vec::with_capacity(4);
    cuboids.push(ModelCuboid::new([0.25, 0.0, 0.25], [0.75, 0.625, 0.75]));
    cuboids.push(ModelCuboid::new(
        [0.3125, 0.625, 0.3125],
        [0.6875, 0.75, 0.6875],
    ));
    if hanging {
        cuboids.push(ModelCuboid::new(
            [0.4375, 0.75, 0.4375],
            [0.5625, 1.0, 0.5625],
        ));
    } else {
        cuboids.push(ModelCuboid::new(
            [0.375, 0.75, 0.375],
            [0.625, 0.9375, 0.625],
        ));
    }
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn candle_shape(state: &BlockStateQuery) -> ModelShape {
    let candle_count = state_i64(state, "candles")
        .or_else(|| state_i64(state, "cluster_count"))
        .unwrap_or(1)
        .clamp(1, 4);
    let positions: &[[f32; 2]] = match candle_count {
        1 => &[[0.5, 0.5]],
        2 => &[[0.40625, 0.5], [0.59375, 0.5]],
        3 => &[[0.375, 0.375], [0.625, 0.375], [0.5, 0.625]],
        _ => &[
            [0.375, 0.375],
            [0.625, 0.375],
            [0.375, 0.625],
            [0.625, 0.625],
        ],
    };
    let cuboids = positions
        .iter()
        .map(|[x, z]| {
            ModelCuboid::new(
                [x - 0.0625, 0.0, z - 0.0625],
                [x + 0.0625, 0.4375, z + 0.0625],
            )
        })
        .collect::<Vec<_>>();
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn portal_shape(state: &BlockStateQuery) -> ModelShape {
    let axis = crate::model_family::direction::state_string(state, "portal_axis")
        .or_else(|| crate::model_family::direction::state_string(state, "axis"))
        .or_else(|| crate::model_family::direction::state_string(state, "pillar_axis"))
        .unwrap_or("x");
    let thickness = 0.03125;
    let cuboid = if axis == "z" {
        ModelCuboid::new([0.5 - thickness, 0.0, 0.0], [0.5 + thickness, 1.0, 1.0])
    } else {
        ModelCuboid::new([0.0, 0.0, 0.5 - thickness], [1.0, 1.0, 0.5 + thickness])
    };
    ModelShape::from_cuboids([cuboid])
}

fn wall_torch_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.40625, 0.25, 0.0], [0.59375, 0.875, 0.5]),
        CardinalDirection::South => ModelCuboid::new([0.40625, 0.25, 0.5], [0.59375, 0.875, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5, 0.25, 0.40625], [1.0, 0.875, 0.59375]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.25, 0.40625], [0.5, 0.875, 0.59375]),
    }
}
