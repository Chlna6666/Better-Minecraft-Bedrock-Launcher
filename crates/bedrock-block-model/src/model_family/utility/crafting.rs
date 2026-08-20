use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{state_bool, state_i64};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_cauldron") || name == "cauldron" {
        return Some(ModelFamily::Cauldron);
    }
    if name == "flower_pot" || name.starts_with("potted_") {
        return Some(ModelFamily::FlowerPot);
    }
    if name == "decorated_pot" {
        return Some(ModelFamily::DecoratedPot);
    }
    if name == "brewing_stand" {
        return Some(ModelFamily::BrewingStand);
    }
    if name == "enchanting_table" {
        return Some(ModelFamily::EnchantingTable);
    }
    if name == "composter" {
        return Some(ModelFamily::Composter);
    }
    if name == "end_portal_frame" {
        return Some(ModelFamily::EndPortalFrame);
    }
    None
}

pub(crate) fn cauldron_shape() -> ModelShape {
    ModelShape::from_cuboids([
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.1875, 1.0]),
        ModelCuboid::new([0.0, 0.1875, 0.0], [0.125, 1.0, 1.0]),
        ModelCuboid::new([0.875, 0.1875, 0.0], [1.0, 1.0, 1.0]),
        ModelCuboid::new([0.125, 0.1875, 0.0], [0.875, 1.0, 0.125]),
        ModelCuboid::new([0.125, 0.1875, 0.875], [0.875, 1.0, 1.0]),
    ])
}

use crate::model_family::shape::ModelPlane;

pub(crate) fn flower_pot_shape(name: &str) -> ModelShape {
    let mut shape = ModelShape::from_cuboids([
        ModelCuboid::new([0.3125, 0.0, 0.3125], [0.6875, 0.3125, 0.6875]),
        ModelCuboid::new([0.25, 0.3125, 0.25], [0.75, 0.4375, 0.75]),
    ]);
    if name.starts_with("potted_") {
        shape.planes.extend([
            ModelPlane::new(
                [
                    [0.0625, 0.0, -0.0625],
                    [1.0625, 0.0, 0.9375],
                    [1.0625, 0.75, 0.9375],
                    [0.0625, 0.75, -0.0625],
                ],
                [-1, 0, 1],
            ),
            ModelPlane::new(
                [
                    [1.0625, 0.0, -0.0625],
                    [0.0625, 0.0, 0.9375],
                    [0.0625, 0.75, 0.9375],
                    [1.0625, 0.75, -0.0625],
                ],
                [1, 0, 1],
            ),
        ]);
    }
    shape
}

pub(crate) fn decorated_pot_shape() -> ModelShape {
    ModelShape::from_cuboids([
        ModelCuboid::new([0.125, 0.0, 0.125], [0.875, 1.0, 0.875]),
        ModelCuboid::new([0.1875, 1.0, 0.1875], [0.8125, 1.0625, 0.8125]),
    ])
}

pub(crate) fn brewing_stand_shape() -> ModelShape {
    ModelShape::from_cuboids([
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0]),
        ModelCuboid::new([0.4375, 0.125, 0.4375], [0.5625, 0.875, 0.5625]),
    ])
}

pub(crate) fn enchanting_table_shape() -> ModelShape {
    ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.75, 1.0])])
}

pub(crate) fn composter_shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = vec![
        composter_cuboid([px(2.0), 0.0, px(2.0)], [px(14.0), px(2.0), px(14.0)]),
        composter_cuboid([0.0, 0.0, 0.0], [px(2.0), 1.0, 1.0]),
        composter_cuboid([px(14.0), 0.0, 0.0], [1.0, 1.0, 1.0]),
        composter_cuboid([px(2.0), 0.0, 0.0], [px(14.0), 1.0, px(2.0)]),
        composter_cuboid([px(2.0), 0.0, px(14.0)], [px(14.0), 1.0, 1.0]),
    ];

    let fill_height = match state_i64(state, "composter_fill_level").unwrap_or(0) {
        1 => Some(px(3.0)),
        2 => Some(px(5.0)),
        3 => Some(px(7.0)),
        4 => Some(px(9.0)),
        5 => Some(px(11.0)),
        6 => Some(px(13.0)),
        7 | 8 => Some(px(15.0)),
        _ => None,
    };
    if let Some(fill_height) = fill_height {
        cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new(
                [px(2.0), px(2.0), px(2.0)],
                [px(14.0), fill_height, px(14.0)],
            )
            .with_material_slot("up"),
        ));
    }

    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn end_portal_frame_shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = vec![detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(13.0), 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )];
    if state_bool(state, "end_portal_eye_bit").unwrap_or(false) {
        cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new([px(4.0), px(13.0), px(4.0)], [px(12.0), 1.0, px(12.0)])
                .with_material_slot("carried"),
        ));
    }
    ModelShape::from_cuboids(cuboids)
}

fn composter_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(min, max))
        .with_face_material_slot(BlockFace::Up, "down")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::Side, "side")
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
