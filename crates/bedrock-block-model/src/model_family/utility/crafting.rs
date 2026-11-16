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
    // Java Edition `block/cauldron`: four 2 px walls from y=3, a 2x1 inner floor, and eight
    // corner feet. The previous shape used a 16x3 slab plus four walls, so the silhouette was
    // a solid box rather than an open cauldron.
    let px = |value: f32| value / 16.0;
    let mut boxes = Vec::with_capacity(13);
    // Walls (side art outside, top rim on top, inner floor underneath).
    for (min, max) in [
        ([0.0, 3.0, 0.0], [2.0, 16.0, 16.0]),
        ([14.0, 3.0, 0.0], [16.0, 16.0, 16.0]),
        ([2.0, 3.0, 0.0], [14.0, 16.0, 2.0]),
        ([2.0, 3.0, 14.0], [14.0, 16.0, 16.0]),
    ] {
        boxes.push(
            crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
                min.map(px),
                max.map(px),
            ))
            .with_face_material_slot(BlockFace::Up, "top")
            .with_face_material_slot(BlockFace::Down, "inner")
            .with_face_material_slot(BlockFace::Side, "side"),
        );
    }
    // Inner floor.
    boxes.push(
        crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
            [2.0, 3.0, 2.0].map(px),
            [14.0, 4.0, 14.0].map(px),
        ))
        .with_face_material_slot(BlockFace::Up, "inner")
        .with_face_material_slot(BlockFace::Down, "inner"),
    );
    // Corner feet.
    for (min, max) in [
        ([0.0, 0.0, 0.0], [4.0, 3.0, 2.0]),
        ([0.0, 0.0, 2.0], [2.0, 3.0, 4.0]),
        ([12.0, 0.0, 0.0], [16.0, 3.0, 2.0]),
        ([14.0, 0.0, 2.0], [16.0, 3.0, 4.0]),
        ([0.0, 0.0, 14.0], [4.0, 3.0, 16.0]),
        ([0.0, 0.0, 12.0], [2.0, 3.0, 14.0]),
        ([12.0, 0.0, 14.0], [16.0, 3.0, 16.0]),
        ([14.0, 0.0, 12.0], [16.0, 3.0, 14.0]),
    ] {
        boxes.push(
            crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
                min.map(px),
                max.map(px),
            ))
            .with_face_material_slot(BlockFace::Down, "bottom")
            .with_face_material_slot(BlockFace::Side, "side"),
        );
    }
    ModelShape::from_cuboids(boxes)
}

use crate::model_family::shape::ModelPlane;

pub(crate) fn flower_pot_shape(name: &str) -> ModelShape {
    // Measured from the client: a 6 x 6 px body to y = 0.249 and a 4 px rim band to y = 0.375,
    // both 0.3125..0.6875 across. The previous 0.3125 / 0.4375 heights were too tall.
    let mut shape = ModelShape::from_cuboids([
        ModelCuboid::new([0.3125, 0.0, 0.3125], [0.6875, 0.249, 0.6875]),
        ModelCuboid::new([0.3125, 0.249, 0.3125], [0.6875, 0.375, 0.6875]),
    ]);
    if let Some(plant) = name.strip_prefix("potted_") {
        // Each potted plant keeps its own silhouette: a cactus is a 6 px column, a bamboo shoot a
        // thin post, a mushroom a stem plus cap. Drawing every one of them as the same oversized
        // cross plane (the previous model) made the whole family look alike.
        shape.cuboids.extend(potted_plant_cuboids(plant));
        if potted_plant_uses_cross(plant) {
            shape.planes.extend(cross_plant_planes());
        }
    }
    shape
}

/// The solid part of a potted plant, sized from its own block model.
fn potted_plant_cuboids(plant: &str) -> Vec<ModelCuboid> {
    match plant {
        // A potted cactus is a 6 px column rising out of the pot.
        "cactus" => vec![ModelCuboid::new(
            [px(5.0), px(6.0), px(5.0)],
            [px(11.0), px(13.0), px(11.0)],
        )],
        // Bamboo keeps its thin 2 px shoot.
        "bamboo" => vec![ModelCuboid::new(
            [px(7.0), px(6.0), px(7.0)],
            [px(9.0), px(13.0), px(9.0)],
        )],
        // Mushrooms are a narrow stem under a wider cap.
        "red_mushroom" | "brown_mushroom" => vec![
            ModelCuboid::new([px(7.0), px(6.0), px(7.0)], [px(9.0), px(10.0), px(9.0)]),
            ModelCuboid::new([px(5.0), px(10.0), px(5.0)], [px(11.0), px(12.0), px(11.0)]),
        ],
        // A dead bush is bare twigs; the cross plane carries it.
        "deadbush" => Vec::new(),
        // Everything else is a cross-shaped plant.
        _ => Vec::new(),
    }
}

/// Whether the potted plant draws as a cross (flowers, saplings, ferns, dead bush, ...).
fn potted_plant_uses_cross(plant: &str) -> bool {
    !matches!(plant, "cactus" | "bamboo" | "red_mushroom" | "brown_mushroom")
}

/// The cross plane a potted plant uses, sitting inside the pot rim.
fn cross_plant_planes() -> [ModelPlane; 2] {
    [
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
    ]
}

/// Bedrock decorated pot: a block-actor model built from three stacked boxes.
///
/// The pot is **not** a cross model — the client renders it as an entity with five separate
/// model keys (`decorated_pot_base` plus one per side), each sampling its own sherd art. Its
/// `decorated_pot_base` texture is 32 x 32 and lays the parts out unfolded:
///
/// ```text
/// u 8..24,  v 0..8    neck top   16 x 8  -> an 8 px square lid
/// u 0..32,  v 8..12   shoulder   32 x 4  -> four 8 px faces
/// u 0..28,  v 13..27  body       28 x 14 -> four 7 px faces, 14 px tall
/// ```
///
/// So the pot is a slim 7 px column that flares to 8 px at the shoulder and closes with an 8 px
/// lid — the previous single 12 px box plus a rim ring ignored that profile.
pub(crate) fn decorated_pot_shape() -> ModelShape {
    // Body: 7 px wide, 14 px tall, standing on the block floor.
    let body = pot_face_uv(ModelCuboid::new(
        [px(4.5), 0.0, px(4.5)],
        [px(11.5), px(14.0), px(11.5)],
    ), [0.0, 13.0, 7.0, 27.0], [0.0, 13.0, 7.0, 27.0]);
    // Shoulder: 8 px wide, 4 px tall, one px proud of the body.
    let shoulder = pot_face_uv(ModelCuboid::new(
        [px(4.0), px(14.0), px(4.0)],
        [px(12.0), px(18.0), px(12.0)],
    ), [0.0, 8.0, 8.0, 12.0], [0.0, 8.0, 8.0, 12.0]);
    // The lid is the shoulder box's own top face: the unfolded sheet draws the 16 x 8 neck art
    // above the shoulder band, so the cap samples that rect instead of adding a second box.
    let shoulder = shoulder
        .with_face_uv(BlockFace::Up, [
            [8.0 / 32.0, 8.0 / 32.0],
            [24.0 / 32.0, 8.0 / 32.0],
            [24.0 / 32.0, 0.0],
            [8.0 / 32.0, 0.0],
        ])
        .with_face_uv(BlockFace::Down, [
            [8.0 / 32.0, 8.0 / 32.0],
            [24.0 / 32.0, 8.0 / 32.0],
            [24.0 / 32.0, 0.0],
            [8.0 / 32.0, 0.0],
        ]);
    ModelShape::from_cuboids([body, shoulder])
}

/// Maps one box's four sides onto a strip of `sides` (four equal columns) and its caps onto
/// `caps`.
fn pot_face_uv(
    cuboid: ModelCuboid,
    sides: [f32; 4],
    _caps: [f32; 4],
) -> ModelCuboid {
    let [u0, v0, u1, v1] = sides;
    let width = (u1 - u0) / 4.0;
    let rect = |index: usize| {
        let left = u0 + width * index as f32;
        [
            [left / 32.0, v1 / 32.0],
            [(left + width) / 32.0, v1 / 32.0],
            [(left + width) / 32.0, v0 / 32.0],
            [left / 32.0, v0 / 32.0],
        ]
    };
    cuboid
        .with_face_uv(BlockFace::North, rect(0))
        .with_face_uv(BlockFace::East, rect(1))
        .with_face_uv(BlockFace::South, rect(2))
        .with_face_uv(BlockFace::West, rect(3))
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
