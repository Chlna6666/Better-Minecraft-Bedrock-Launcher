use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction};
use crate::model_family::shape::{ModelCuboid, ModelShape, rect_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_standing_sign")
        || name.ends_with("_wall_sign")
        || name.ends_with("_hanging_sign")
        || matches!(name, "standing_sign" | "wall_sign" | "hanging_sign")
    {
        return Some(ModelFamily::Sign);
    }
    if name.ends_with("_bed") || name == "bed" {
        return Some(ModelFamily::Bed);
    }
    if name.ends_with("_banner") || matches!(name, "banner" | "standing_banner" | "wall_banner") {
        return Some(ModelFamily::Banner);
    }
    if name == "bell" {
        return Some(ModelFamily::Bell);
    }
    None
}

pub(crate) fn sign_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.ends_with("_wall_sign") || name == "wall_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([sign_board_uv(wall_sign_board_cuboid(direction))]);
    }

    if name.ends_with("_wall_hanging_sign") || name == "wall_hanging_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([hanging_sign_board_uv(wall_hanging_sign_board_cuboid(
            direction,
        ))]);
    }

    if name.ends_with("_hanging_sign") || name == "hanging_sign" {
        let mut cuboids = vec![hanging_sign_board_uv(ModelCuboid::new(
            [px(1.0), 0.0, px(7.0)],
            [px(15.0), px(10.0), px(9.0)],
        ))];
        if crate::model_family::direction::state_bool(state, "attached_bit").unwrap_or(false) {
            cuboids.push(
                hanging_sign_chain_uv(ModelCuboid::new(
                    [0.0, px(10.0), px(7.5)],
                    [1.0, 1.0, px(8.5)],
                ))
                .with_face_uv(BlockFace::South, sign_uv(12.0, 6.0, 16.0, 6.0))
                .with_face_uv(BlockFace::North, sign_uv(12.0, 6.0, 16.0, 6.0)),
            );
        } else {
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(1.5), px(10.0), px(7.5)],
                [px(4.5), 1.0, px(8.5)],
            )));
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(11.5), px(10.0), px(7.5)],
                [px(14.5), 1.0, px(8.5)],
            )));
        }
        if !crate::model_family::direction::state_bool(state, "hanging").unwrap_or(false) {
            cuboids.push(hanging_sign_bar_uv(ModelCuboid::new(
                [0.0, px(14.0), px(6.0)],
                [1.0, 1.0, px(10.0)],
            )));
        }
        return ModelShape::from_cuboids(cuboids);
    }

    ModelShape::from_cuboids([
        sign_board_uv(ModelCuboid::new(
            [px(0.25), px(8.25), px(7.25)],
            [px(15.75), 1.0, px(8.75)],
        )),
        sign_post_uv(ModelCuboid::new(
            [px(7.25), 0.0, px(7.25)],
            [px(8.75), px(8.25), px(8.75)],
        )),
    ])
}

fn sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 0.0, 24.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 0.0, 24.0, 2.0))
}

fn sign_post_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::Side, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 16.0, 2.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 16.0, 2.0, 2.0))
}

fn hanging_sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 12.0, 14.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(16.0, 12.0, 14.0, 2.0))
}

fn hanging_sign_chain_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("chain")
        .with_face_uv(BlockFace::South, sign_uv(0.0, 6.0, 6.0, 6.0))
        .with_face_uv(BlockFace::North, sign_uv(6.0, 6.0, 6.0, 6.0))
}

fn hanging_sign_bar_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("bar")
        .with_face_uv(BlockFace::North, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::South, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::Up, sign_uv(4.0, 0.0, 16.0, 4.0))
        .with_face_uv(BlockFace::Down, sign_uv(20.0, 0.0, 16.0, 4.0))
}

fn wall_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(0.25), px(4.25), 0.0], [px(15.75), px(12.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(0.25), px(4.25), px(14.5)], [px(15.75), px(12.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), px(4.25), px(0.25)], [1.0, px(12.0), px(15.75)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, px(4.25), px(0.25)], [px(1.5), px(12.0), px(15.75)])
        }
    }
}

fn wall_hanging_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(1.0), 0.0, 0.0], [px(15.0), px(10.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(1.0), 0.0, px(14.5)], [px(15.0), px(10.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), 0.0, px(1.0)], [1.0, px(10.0), px(15.0)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, 0.0, px(1.0)], [px(1.5), px(10.0), px(15.0)])
        }
    }
}

fn sign_uv(u: f32, v: f32, width: f32, height: f32) -> [[f32; 2]; 4] {
    rect_uv(u / 64.0, v / 32.0, (u + width) / 64.0, (v + height) / 32.0)
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

pub(crate) fn bed_shape(state: &BlockStateQuery) -> ModelShape {
    let is_head = crate::model_family::direction::state_bool(state, "head_piece_bit")
        .or_else(|| {
            crate::model_family::direction::state_string(state, "bed_part").map(|s| s == "head")
        })
        .unwrap_or(true);

    let mut cuboids = vec![ModelCuboid::new([0.0, 0.1875, 0.0], [1.0, 0.5625, 1.0])];
    if is_head {
        cuboids.push(ModelCuboid::new([0.0, 0.5625, 0.0], [1.0, 0.6875, 0.3125]));
        cuboids.push(ModelCuboid::new([0.0, 0.0, 0.0], [0.1875, 0.1875, 0.1875]));
        cuboids.push(ModelCuboid::new([0.8125, 0.0, 0.0], [1.0, 0.1875, 0.1875]));
    } else {
        cuboids.push(ModelCuboid::new([0.0, 0.0, 0.8125], [0.1875, 0.1875, 1.0]));
        cuboids.push(ModelCuboid::new([0.8125, 0.0, 0.8125], [1.0, 0.1875, 1.0]));
    }
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn banner_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.contains("wall_banner") || name == "wall_banner" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([wall_attached_banner(direction)]);
    }
    ModelShape::from_cuboids([
        ModelCuboid::new([0.4375, 0.0, 0.4375], [0.5625, 0.875, 0.5625]),
        ModelCuboid::new([0.0625, 0.25, 0.4375], [0.9375, 1.25, 0.5625]),
    ])
}

pub(crate) fn bell_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let along_x = matches!(
        direction,
        CardinalDirection::North | CardinalDirection::South
    );
    if along_x {
        ModelShape::from_cuboids([
            ModelCuboid::new([0.3125, 0.25, 0.3125], [0.6875, 0.6875, 0.6875]),
            ModelCuboid::new([0.25, 0.6875, 0.4375], [0.75, 0.8125, 0.5625]),
        ])
    } else {
        ModelShape::from_cuboids([
            ModelCuboid::new([0.3125, 0.25, 0.3125], [0.6875, 0.6875, 0.6875]),
            ModelCuboid::new([0.4375, 0.6875, 0.25], [0.5625, 0.8125, 0.75]),
        ])
    }
}

fn wall_attached_banner(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.0625, 0.0, 0.0], [0.9375, 1.0, 0.125]),
        CardinalDirection::South => ModelCuboid::new([0.0625, 0.0, 0.875], [0.9375, 1.0, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.875, 0.0, 0.0625], [1.0, 1.0, 0.9375]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.0, 0.0625], [0.125, 1.0, 0.9375]),
    }
}
