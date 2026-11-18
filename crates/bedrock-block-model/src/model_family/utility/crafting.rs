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

pub(crate) fn cauldron_shape(state: &BlockStateQuery) -> ModelShape {
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
    let liquid_surface = {
        let fill = crate::model_family::direction::state_i64(state, "fill_level").unwrap_or(0);
        let kind = crate::model_family::direction::state_string(state, "cauldron_liquid")
            .unwrap_or("water");
        if fill > 0 {
            // Interior spans x/z 2..14, floor at y4, rim at y16. Level 2/4/6 = 1/3, 2/3, full.
            let surface_y = 4.0 + (fill as f32 / 6.0) * 12.0;
            let slot = match kind {
                "lava" => "cauldron_lava",
                "powder_snow" => "cauldron_snow",
                _ => "cauldron_water",
            };
            Some(
                ModelCuboid::new(
                    [2.0 / 16.0, (surface_y - 1.0) / 16.0, 2.0 / 16.0],
                    [14.0 / 16.0, surface_y / 16.0, 14.0 / 16.0],
                )
                .with_material_slot(slot),
            )
        } else {
            None
        }
    };
    boxes.extend(liquid_surface);
    ModelShape::from_cuboids(boxes)
}

use crate::model_family::shape::ModelPlane;

pub(crate) fn flower_pot_shape(name: &str) -> ModelShape {
    // JE `flower_pot` model: five boxes whose per-face UVs name the exact drawn regions of the
    // 16x16 `flower_pot` tile (art only at x5..11, y5..16). The previous single solid box sampled
    // the WHOLE tile per face, so the pot rendered as a plain brown block. The tile's transparent
    // centre rows form the hollow rim through alpha-testing.
    let pot = |min: [f32; 3], max: [f32; 3], faces: &[(BlockFace, [f32; 4])]| {
        let mut cuboid = ModelCuboid::new(min.map(px), max.map(px));
        for (face, rect) in faces {
            let uv = if matches!(face, BlockFace::Up | BlockFace::Down) {
                pot_top_uv(*rect)
            } else {
                pot_side_uv(*rect)
            };
            cuboid = cuboid.with_face_uv(*face, uv);
        }
        cuboid
    };
    // uv rects are [u0, v0, u1, v1] in image pixels, measured from the texture top.
    let mut cuboids = vec![
        // west wall x5..6
        pot(
            [5.0, 0.0, 5.0],
            [6.0, 6.0, 11.0],
            &[
                (BlockFace::Down, [5.0, 5.0, 6.0, 11.0]),
                (BlockFace::Up, [5.0, 5.0, 6.0, 11.0]),
                (BlockFace::North, [10.0, 10.0, 11.0, 16.0]),
                (BlockFace::South, [5.0, 10.0, 6.0, 16.0]),
                (BlockFace::West, [5.0, 10.0, 11.0, 16.0]),
                (BlockFace::East, [5.0, 10.0, 11.0, 16.0]),
            ],
        ),
        // east wall x10..11
        pot(
            [10.0, 0.0, 5.0],
            [11.0, 6.0, 11.0],
            &[
                (BlockFace::Down, [10.0, 5.0, 11.0, 11.0]),
                (BlockFace::Up, [10.0, 5.0, 11.0, 11.0]),
                (BlockFace::North, [5.0, 10.0, 6.0, 16.0]),
                (BlockFace::South, [10.0, 10.0, 11.0, 16.0]),
                (BlockFace::West, [5.0, 10.0, 11.0, 16.0]),
                (BlockFace::East, [5.0, 10.0, 11.0, 16.0]),
            ],
        ),
        // north wall z5..6
        pot(
            [6.0, 0.0, 5.0],
            [10.0, 6.0, 6.0],
            &[
                (BlockFace::Down, [6.0, 10.0, 10.0, 11.0]),
                (BlockFace::Up, [6.0, 5.0, 10.0, 6.0]),
                (BlockFace::North, [6.0, 10.0, 10.0, 16.0]),
                (BlockFace::South, [6.0, 10.0, 10.0, 16.0]),
            ],
        ),
        // south wall z10..11
        pot(
            [6.0, 0.0, 10.0],
            [10.0, 6.0, 11.0],
            &[
                (BlockFace::Down, [6.0, 5.0, 10.0, 6.0]),
                (BlockFace::Up, [6.0, 10.0, 10.0, 11.0]),
                (BlockFace::North, [6.0, 10.0, 10.0, 16.0]),
                (BlockFace::South, [6.0, 10.0, 10.0, 16.0]),
            ],
        ),
        // inner floor (dirt on top)
        pot(
            [6.0, 0.0, 6.0],
            [10.0, 4.0, 10.0],
            &[
                (BlockFace::Down, [6.0, 12.0, 10.0, 16.0]),
            ],
        ),
    ];
    let mut shape = ModelShape::from_cuboids(cuboids);
    if let Some(plant) = name.strip_prefix("potted_") {
        // Each potted plant keeps its own silhouette and its OWN flower sprite: the two crossed
        // cards sample the plant's block texture (poppy → flower_rose, etc.) instead of the pot.
        let plant_uv = crate::model_family::shape::full_texture_uv();
        let cross = [
            ModelPlane::new(
                [
                    [0.1625, 0.25, 0.0],
                    [0.8375, 0.25, 1.0],
                    [0.8375, 1.0, 1.0],
                    [0.1625, 1.0, 0.0],
                ],
                [-1, 0, 1],
            )
            .with_uv(plant_uv),
            ModelPlane::new(
                [
                    [1.0, 0.25, 0.0],
                    [0.0, 0.25, 1.0],
                    [0.0, 1.0, 1.0],
                    [1.0, 1.0, 0.0],
                ],
                [1, 0, 1],
            )
            .with_uv(plant_uv),
        ];
        shape.planes.extend(cross.map(|plane| plane.with_material_slot(plant)));
        shape.cuboids.extend(potted_plant_cuboids(plant));
    }
    shape
}

/// The solid part of a potted plant, sized from its own block model. The cross cards above
/// carry flowers/saplings; cactus, bamboo and mushrooms have their own volume.
fn potted_plant_cuboids(plant: &str) -> Vec<ModelCuboid> {
    match plant {
        "cactus" => vec![ModelCuboid::new(
            [px(5.0), px(6.0), px(5.0)],
            [px(11.0), px(13.0), px(11.0)],
        )],
        "bamboo" => vec![ModelCuboid::new(
            [px(7.0), px(6.0), px(7.0)],
            [px(9.0), px(13.0), px(9.0)],
        )],
        "red_mushroom" | "brown_mushroom" => vec![
            ModelCuboid::new([px(7.0), px(6.0), px(7.0)], [px(9.0), px(10.0), px(9.0)]),
            ModelCuboid::new([px(5.0), px(10.0), px(5.0)], [px(11.0), px(12.0), px(11.0)]),
        ],
        _ => Vec::new(),
    }
}

/// A cap UV (up/down) from a JE-style rect measured from the texture top: our face corner 0 is
/// the bottom-left, so V is inverted.
fn pot_top_uv(rect: [f32; 4]) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = rect;
    [[u0 / 16.0, v1 / 16.0], [u1 / 16.0, v1 / 16.0], [u1 / 16.0, v0 / 16.0], [u0 / 16.0, v0 / 16.0]]
}

/// A side UV from a JE-style rect (V inverted like the caps, U mirrored for the renderer's
/// front-face convention).
fn pot_side_uv(rect: [f32; 4]) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = rect;
    [[u1 / 16.0, v1 / 16.0], [u0 / 16.0, v1 / 16.0], [u0 / 16.0, v0 / 16.0], [u1 / 16.0, v0 / 16.0]]
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
    // The block body comes from the client's own tessellated mesh (capture geometry 854 /
    // appearance 5381), decoded with its atlas UVs — see brewing_capture.rs. This replaces the
    // old two-cuboid guess (a flat slab + thin post) that missed the four feet and the stand's
    // sloped supports. The three potion bottles and the flame are block-entity art (not part of
    // this mesh) and are keyed by the `brewing_stand_slot_*_bit` states elsewhere.
    ModelShape::default().with_planes(super::brewing_capture::planes())
}

pub(crate) fn enchanting_table_shape() -> ModelShape {
    // The block base is a 0.75-tall box whose side UVs span only v 0.25..1; a full-tile
    // projection (detail_cuboid_with_local_uv) stretched the art. Use the client's decoded mesh
    // (capture geometry 403 / appearance 4607). The hovering book is block-entity art, separate.
    ModelShape::default().with_planes(super::enchanting_capture::planes())
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
        // `endframe_eye.png` paints the eye square at x4..12, y4..12 and a dark base band at
        // y0..3. Java's `end_portal_frame_filled` binds exactly those: the top face samples the
        // centred square uv [4,4,12,12] and the 3 px-tall sides take the top band uv
        // [4,0,12,3]. Sampling the whole painted region on every face squeezed 12 px of art
        // onto the 3 px sides — the striped bands the screenshots showed.
        let eye_top = crate::model_family::shape::rect_uv(
            4.0 / 16.0,
            4.0 / 16.0,
            12.0 / 16.0,
            12.0 / 16.0,
        );
        let eye_side = [
            [4.0 / 16.0, 3.0 / 16.0],
            [12.0 / 16.0, 3.0 / 16.0],
            [12.0 / 16.0, 0.0],
            [4.0 / 16.0, 0.0],
        ];
        let eye_cube = ModelCuboid::new([px(4.0), px(13.0), px(4.0)], [px(12.0), 1.0, px(12.0)])
            .with_material_slot("carried")
            .with_face_uv(BlockFace::Up, eye_top)
            .with_face_uv(BlockFace::Down, eye_top)
            .with_face_uv(BlockFace::North, eye_side)
            .with_face_uv(BlockFace::South, eye_side)
            .with_face_uv(BlockFace::East, eye_side)
            .with_face_uv(BlockFace::West, eye_side);
        cuboids.push(eye_cube);
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
