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
    let cuboids = vec![
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

/// Official decorated pot (JE `DecoratedPotRenderer.createBaseLayer`/`createSidesLayer`,
/// applied 1:1 by the Bedrock client; the capture catalog marks every state `unsupported`).
/// Four 14 x 16 px sherd cards stand inset 1 px from each face, a 14 x 14 cap closes the
/// bottom and the top, and the mouth is an 8 x 3 x 8 neck inset inside the top with a
/// 6 x 1 x 6 rim grown 0.2 px above it — the 1.19.80 fix that stopped the old proud neck
/// from z-fighting. The vase-stack silhouette (14 px body + 12 px shoulder + proud neck)
/// this replaces was wrong on both geometry and UVs.
pub(crate) fn decorated_pot_shape() -> ModelShape {
    // Cards are zero-thickness planes in the official model; emit them as thin boxes so the
    // renderer's double-sided plane path is not required for solid shading. The sherd sheet
    // is 16 x 16 with the art painted at x 1..14; every outward face samples that band.
    let sherd = crate::model_family::shape::uv16(1.0, 0.0, 15.0, 16.0);
    let mut cuboids = vec![
        // Bottom and top caps sample the official cap windows (texOffs(-14,13): top face
        // u0..14, bottom face u14..28, both v13..27 on the 32 x 32 sheet).
        cap_cuboid([px(1.0), 0.0, px(1.0)], [px(15.0), 0.001, px(15.0)], 14.0, 13.0),
        cap_cuboid([px(1.0), 1.0, px(1.0)], [px(15.0), 1.001, px(15.0)], 14.0, 13.0),
        ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(1.001), 1.0, px(15.0)])
            .with_material_slot("pot_side"),
        ModelCuboid::new([px(15.0), 0.0, px(1.0)], [px(15.001), 1.0, px(15.0)])
            .with_material_slot("pot_side"),
        ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(15.0), 1.0, px(1.001)])
            .with_material_slot("pot_side"),
        ModelCuboid::new([px(1.0), 0.0, px(15.0)], [px(15.0), 1.0, px(15.001)])
            .with_material_slot("pot_side"),
    ];
    for cuboid in &mut cuboids {
        for face in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            *cuboid = cuboid.clone().with_face_uv(face, sherd);
        }
    }
    // Neck 8 x 3 x 8 at y 13..16, plus the 6 x 1 x 6 rim above it. The sheet windows are the
    // official box unwraps: neck texOffs(0,0) (caps u 8..16 / 16..24 at v 0..8, side strip
    // v 8..11 with west u0..8, north u8..16, east u16..24, south u24..32) and rim
    // texOffs(0,5) shifted down one row (caps v 5..11, side strip v 11..12).
    let neck = box_unwrap_cuboid(
        [px(4.0), px(13.0), px(4.0)],
        [px(12.0), 1.0, px(12.0)],
        [0.0, 0.0],
        [32.0, 32.0],
    )
    .with_material_slot("pot_base");
    let rim_growth = px(0.2);
    let rim = box_unwrap_cuboid(
        [px(5.0) - rim_growth, 1.0, px(5.0) - rim_growth],
        [px(11.0) + rim_growth, 1.0 + px(0.2), px(11.0) + rim_growth],
        [0.0, 5.0],
        [32.0, 32.0],
    )
    .with_material_slot("pot_base");
    cuboids.push(neck);
    cuboids.push(rim);
    ModelShape::from_cuboids(cuboids)
}

/// The 14 x 14 cap slab sampling the official cap windows of `decorated_pot_base`
/// (texOffs(-14, 13)): the up face reads u 0..14, the down face u 14..28, both v 13..27.
fn cap_cuboid(min: [f32; 3], max: [f32; 3], width: f32, v0: f32) -> ModelCuboid {
    let u = |value: f32| value / 32.0;
    let v = |value: f32| value / 32.0;
    let top = [
        [u(0.0), v(v0 + width)],
        [u(width), v(v0 + width)],
        [u(width), v(v0)],
        [u(0.0), v(v0)],
    ];
    let bottom = [
        [u(width), v(v0 + width)],
        [u(2.0 * width), v(v0 + width)],
        [u(2.0 * width), v(v0)],
        [u(width), v(v0)],
    ];
    ModelCuboid::new(min, max)
        .with_material_slot("pot_base")
        .with_face_uv(BlockFace::Up, top)
        .with_face_uv(BlockFace::Down, bottom)
}

/// One Bedrock box unwrap onto a 32 x 32 sheet: caps above the side strip, side faces in the
/// east/north/west/south strip order the entity box convention uses, normalized to `span`.
/// `origin` is the box's texOffs in authoring pixels.
fn box_unwrap_cuboid(
    min: [f32; 3],
    max: [f32; 3],
    origin: [f32; 2],
    span: [f32; 2],
) -> ModelCuboid {
    let uvs = crate::model_family::shape::box_unwrap_face_uvs(
        min.map(|v| v * 16.0),
        max.map(|v| v * 16.0),
        origin,
        span,
    );
    let mut cuboid = ModelCuboid::new(min, max);
    for (face, uv) in uvs {
        cuboid = cuboid.with_face_uv(face, uv);
    }
    cuboid
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
