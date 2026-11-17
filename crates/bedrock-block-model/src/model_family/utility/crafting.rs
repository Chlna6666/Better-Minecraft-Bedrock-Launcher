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
    // Capture (ref 237): the native pot is ONE solid 6x6x6 box sampling the whole 16x16
    // `flower_pot` tile per face. The tile's centre rows 6..9 are fully transparent, and the
    // renderer alpha-tests binary-alpha textures, so the hollow rim comes from the texture —
    // the previous five-box JE ring fought that by sampling partial rects onto solid walls.
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(5.0), 0.0, px(5.0)],
        [px(11.0), px(6.0), px(11.0)],
    ))]);
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

/// Bedrock decorated pot: a block-actor with five model keys — `decorated_pot_base` (top,
/// bottom and the narrow neck) plus one sherd-textured side per face. The silhouette is a squat
/// pot: a **near-full-width 14 px body** rising ~10 px, tapering through a shorter 10 px upper
/// body to an 8 px neck/rim. The old 7 px-wide, 13 px-tall column read as a flower vase.
///
/// UVs: the body sides sample `decorated_pot_side` (16 x 16 sherd sheet, opaque x1..14), the
/// neck sides and its cap sample the 32 x 32 `decorated_pot_base` unfold (shoulder band v8..12,
/// top/bottom v0..8).
pub(crate) fn decorated_pot_shape() -> ModelShape {
    let side = crate::model_family::shape::uv16(1.0, 0.0, 15.0, 16.0);
    // Lower body: 14 px wide (x1..15), 10 px tall — the broad belly of the pot.
    let body = ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(15.0), px(10.0), px(15.0)])
        .with_face_uv(BlockFace::North, side)
        .with_face_uv(BlockFace::South, side)
        .with_face_uv(BlockFace::East, side)
        .with_face_uv(BlockFace::West, side)
        .with_material_slot("pot_side");
    // Upper body: 12 px wide, tapering from y10 to y13.
    let upper = ModelCuboid::new([px(2.0), px(10.0), px(2.0)], [px(14.0), px(13.0), px(14.0)])
        .with_material_slot("pot_base");
    let upper = pot_face_uv(upper, [0.0, 13.0, 7.0, 27.0], [0.0, 13.0, 7.0, 27.0]);
    // Neck/rim: 8 px wide, y13..16, one px proud of nothing (the mouth).
    let neck = ModelCuboid::new([px(4.0), px(13.0), px(4.0)], [px(12.0), 1.0, px(12.0)])
        .with_material_slot("pot_base");
    let neck = pot_face_uv(neck, [0.0, 8.0, 8.0, 12.0], [0.0, 8.0, 8.0, 12.0]);
    let neck = neck
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
    ModelShape::from_cuboids([body, upper, neck])
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
