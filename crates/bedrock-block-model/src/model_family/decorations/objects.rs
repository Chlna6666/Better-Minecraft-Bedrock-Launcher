use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_face_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name == "cake" || name == "candle_cake" || name.ends_with("_candle_cake") {
        return Some(ModelFamily::Cake);
    }
    if name == "scaffolding" {
        return Some(ModelFamily::Scaffolding);
    }
    if name.ends_with("_shelf") {
        return Some(ModelFamily::Shelf);
    }
    if name == "heavy_core" {
        return Some(ModelFamily::HeavyCore);
    }
    if name == "conduit" {
        return Some(ModelFamily::Conduit);
    }
    if name == "dried_ghast" {
        return Some(ModelFamily::DriedGhast);
    }
    if name == "end_rod" || is_lightning_rod_name(name) {
        return Some(ModelFamily::Rod);
    }
    if name == "sea_pickle" {
        return Some(ModelFamily::SeaPickle);
    }
    if name == "turtle_egg" || name == "sniffer_egg" {
        return Some(ModelFamily::Egg);
    }
    if name == "beacon" {
        return Some(ModelFamily::Beacon);
    }
    if name == "slime" || name == "honey_block" {
        return Some(ModelFamily::InsetBlock);
    }
    if name == "dragon_egg" {
        return Some(ModelFamily::DragonEgg);
    }
    if name == "frame" || name == "glow_frame" {
        return Some(ModelFamily::ItemFrame);
    }
    if is_head_block_name(name) {
        return Some(ModelFamily::Head);
    }
    None
}

pub(crate) fn cake_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let has_candle = name == "candle_cake" || name.ends_with("_candle_cake");
    let mut cuboids = vec![cake_body_cuboid(if has_candle {
        0
    } else {
        state_i64(state, "bite_counter").unwrap_or(0).clamp(0, 6)
    })];

    if has_candle {
        cuboids.push(
            detail_cuboid_with_local_uv(ModelCuboid::new(
                [px(7.0), px(8.0), px(7.0)],
                [px(9.0), px(15.0), px(9.0)],
            ))
            .with_material_slot("candle"),
        );
    }

    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn scaffolding_shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = vec![
        scaffolding_cuboid([0.0, px(14.0), 0.0], [1.0, 1.0, 1.0]),
        side_slot_cuboid([0.0, 0.0, 0.0], [px(2.0), px(14.0), px(2.0)]),
        side_slot_cuboid([px(14.0), 0.0, 0.0], [1.0, px(14.0), px(2.0)]),
        side_slot_cuboid([0.0, 0.0, px(14.0)], [px(2.0), px(14.0), 1.0]),
        side_slot_cuboid([px(14.0), 0.0, px(14.0)], [1.0, px(14.0), 1.0]),
    ];

    if state_i64(state, "stability").unwrap_or(1) > 0 {
        cuboids.extend([
            side_slot_cuboid([px(2.0), 0.0, 0.0], [px(14.0), px(2.0), px(2.0)]),
            side_slot_cuboid([px(2.0), 0.0, px(14.0)], [px(14.0), px(2.0), 1.0]),
            side_slot_cuboid([0.0, 0.0, px(2.0)], [px(2.0), px(2.0), px(14.0)]),
            side_slot_cuboid([px(14.0), 0.0, px(2.0)], [1.0, px(2.0), px(14.0)]),
        ]);
    }

    ModelShape::from_cuboids(cuboids)
}

/// Bedrock shelf: one 5 px slab standing against the back of its cell.
///
/// Measured from the client: a single box spanning the whole cell and 5 px deep
/// (`z 0..0.3125`), with the item pockets carved at `y 0.25..0.75`. Building it from three
/// separate rails (the previous model) made the shelf read as a fence rather than a board, and
/// the per-rail UVs never lined up with the pack's single shelf texture.
pub(crate) fn shelf_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    // The UVs here must come from the client's own tessellation (capture geometry 118-121 /
    // appearance 2376-2379). The earlier hand-estimated rects from the atlas-space appearance
    // stretched the art. Each of the four facings has its own captured 16-plane mesh.
    ModelShape::default().with_planes(match direction {
        CardinalDirection::South => super::shelf_capture::planes_south(),
        CardinalDirection::West => super::shelf_capture::planes_west(),
        CardinalDirection::North => super::shelf_capture::planes_north(),
        CardinalDirection::East => super::shelf_capture::planes_east(),
    })
}

pub(crate) fn heavy_core_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(4.0), 0.0, px(4.0)],
        [px(12.0), px(8.0), px(12.0)],
    ))])
}

pub(crate) fn conduit_shape() -> ModelShape {
    // `conduit_base.png` is a 24x12 atlas. The installed sheet reads: the top
    // 6 px band carries the box's up tile at x12..18 and its down tile at
    // x6..12, and the bottom 6 px band holds the four 6 px side tiles across
    // the full width. The old window (central 12x12) smeared all four regions
    // into one face.
    let px_w = 1.0 / 24.0;
    let px_h = 1.0 / 12.0;
    let window = |u0: f32, v0: f32, u1: f32, v1: f32| {
        [
            [u0 * px_w, v1 * px_h],
            [u1 * px_w, v1 * px_h],
            [u1 * px_w, v0 * px_h],
            [u0 * px_w, v0 * px_h],
        ]
    };
    let mut cuboid = ModelCuboid::new([px(5.0), 0.0, px(5.0)], [px(11.0), px(6.0), px(11.0)])
        .with_material_slot("conduit_base")
        .with_face_uv(BlockFace::Up, window(12.0, 0.0, 18.0, 6.0))
        .with_face_uv(BlockFace::Down, window(6.0, 0.0, 12.0, 6.0));
    // Side strips in engine order across the bottom band; rendered against the
    // game's own box these read as the four walls of the casing.
    for (face, u0) in [
        (BlockFace::North, 0.0),
        (BlockFace::West, 6.0),
        (BlockFace::South, 12.0),
        (BlockFace::East, 18.0),
    ] {
        cuboid = cuboid.with_face_uv(face, window(u0, 6.0, u0 + 6.0, 12.0));
    }
    ModelShape::from_cuboids([cuboid])
}

pub(crate) fn dried_ghast_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    // The ghast textures live in the vanilla_1.21.90 overlay as seven four-frame terrain
    // arrays named for the GHAST's own sides — `dried_ghast_front/back/left/right/top/bottom/
    // tentacles`, each [state_1..state_4] indexed by rehydration_level. The authored model
    // faces south (front = south), so the head slots carry those ghast-side names
    // (`ghast_<stage>_<face>`) and `obj.rs` binds `dried_ghast_<face>#<stage>`. The previous
    // slots used north/south/east/west/down — no such terrain keys — so every head face fell
    // back to the top tile and the tentacle slabs had no slot at all.
    let stage = state_i64(state, "rehydration_level").unwrap_or(0).clamp(0, 3);
    let slot = |face: &str| format!("ghast_{stage}_{face}");
    let mut cuboids = vec![
        ModelCuboid::new([px(3.0), 0.0, px(3.0)], [px(13.0), px(10.0), px(13.0)])
            .with_face_material_slot(BlockFace::Up, &slot("top"))
            .with_face_material_slot(BlockFace::Down, &slot("bottom"))
            // Authored facing south: the face looks south, the back is north, and the ghast's
            // own left hand points east.
            .with_face_material_slot(BlockFace::South, &slot("front"))
            .with_face_material_slot(BlockFace::North, &slot("back"))
            .with_face_material_slot(BlockFace::East, &slot("left"))
            .with_face_material_slot(BlockFace::West, &slot("right")),
    ];
    cuboids.extend([
        ModelCuboid::new([0.0, 0.0, px(5.0)], [px(3.0), px(1.0), px(7.0)]),
        ModelCuboid::new([0.0, 0.0, px(9.0)], [px(3.0), px(1.0), px(11.0)]),
        ModelCuboid::new([px(13.0), 0.0, px(5.0)], [1.0, px(1.0), px(7.0)]),
        ModelCuboid::new([px(13.0), 0.0, px(9.0)], [1.0, px(1.0), px(11.0)]),
        ModelCuboid::new([px(5.0), 0.0, 0.0], [px(7.0), px(1.0), px(3.0)]),
        ModelCuboid::new([px(9.0), 0.0, 0.0], [px(11.0), px(1.0), px(3.0)]),
    ]);

    ModelShape::from_cuboids(
        cuboids
            .into_iter()
            .map(|cuboid| {
                detail_cuboid_with_local_uv(rotate_cuboid_from_south(cuboid, direction))
                    .with_material_slot(&slot("tentacles"))
            })
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn rod_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    // Vanilla `end_rod` / `lightning_rod` sample their own texture columns instead of the
    // block-space projection: the base plate and the shaft occupy separate rectangles.
    let local = if is_lightning_rod_name(name) {
        vec![
            rod_cuboid(
                ModelCuboid::new([px(7.0), 0.0, px(7.0)], [px(9.0), px(12.0), px(9.0)]),
                [0.0, 4.0, 2.0, 16.0],
                Some([0.0, 4.0, 2.0, 6.0]),
            ),
            rod_cuboid(
                ModelCuboid::new([px(6.0), px(12.0), px(6.0)], [px(10.0), 1.0, px(10.0)]),
                [0.0, 0.0, 4.0, 4.0],
                Some([0.0, 0.0, 4.0, 4.0]),
            ),
        ]
    } else {
        vec![
            rod_cuboid(
                ModelCuboid::new([px(6.0), 0.0, px(6.0)], [px(10.0), px(1.0), px(10.0)]),
                [2.0, 6.0, 6.0, 7.0],
                Some([6.0, 6.0, 2.0, 2.0]),
            ),
            rod_cuboid(
                ModelCuboid::new([px(7.0), px(1.0), px(7.0)], [px(9.0), 1.0, px(9.0)]),
                [0.0, 0.0, 2.0, 15.0],
                None,
            ),
        ]
    };

    ModelShape::from_cuboids(
        local
            .into_iter()
            .map(|cuboid| orient_cuboid_from_up(cuboid, facing_from_state(state)))
            .collect::<Vec<_>>(),
    )
}

fn rod_cuboid(cuboid: ModelCuboid, sides: [f32; 4], bottom: Option<[f32; 4]>) -> ModelCuboid {
    let side_uv = java_side_uv(sides);
    let cuboid = cuboid
        .with_face_uv(BlockFace::North, side_uv)
        .with_face_uv(BlockFace::South, side_uv)
        .with_face_uv(BlockFace::East, side_uv)
        .with_face_uv(BlockFace::West, side_uv)
        .with_face_uv(
            BlockFace::Up,
            java_top_uv([sides[0], sides[2], sides[1], sides[3]]),
        );
    match bottom {
        Some(bottom) => cuboid.with_face_uv(BlockFace::Down, java_top_uv(bottom)),
        None => cuboid,
    }
}

fn java_side_uv(rect: [f32; 4]) -> [[f32; 2]; 4] {
    let [u1, v1, u2, v2] = rect.map(|value| value / 16.0);
    [[u2, v2], [u1, v2], [u1, v1], [u2, v1]]
}

fn java_top_uv(rect: [f32; 4]) -> [[f32; 2]; 4] {
    let [u1, v1, u2, v2] = rect.map(|value| value / 16.0);
    [[u1, v1], [u2, v1], [u2, v2], [u1, v2]]
}

/// Sea pickles: one to four upright tubes, each capped by a narrower glowing crown.
///
/// Measured from the capture (refs 606-613). Every pickle is a square tube whose footprint and
/// body height depend on its slot in the cluster, and the crown is always a 1.28 px column
/// centred on the tube, running from `body_top - 0.8` to `body_top + 2.4` px. The four dead
/// variants (610-613) keep the tubes and drop the crowns, so `dead_bit` only removes the glow.
pub(crate) fn sea_pickle_shape(state: &BlockStateQuery) -> ModelShape {
    let count = state_i64(state, "cluster_count").unwrap_or(0).clamp(0, 3) as usize;
    let dead = state_i64(state, "dead_bit").unwrap_or(0) != 0;
    // Official JE `sea_pickle[_2/3/4]` models (the Bedrock sheet is the same
    // 16x16 layout): one body cuboid per pickle plus a flat cap quad at its
    // top and a small cross of two vertical sprout sheets above it. Dead
    // pickles drop the cap and sprouts. Every body face binds its own window
    // of the shared sheet (the old full-sheet local UVs squeezed the whole
    // layout onto each post).
    let bodies: &[&[(f32, f32, f32, f32, f32)]] = match count {
        0 => &[&[(6.0, 10.0, 6.0, 10.0, 6.0)]],
        1 => &[&[(3.0, 7.0, 3.0, 7.0, 6.0), (8.0, 12.0, 8.0, 12.0, 4.0)]],
        2 => &[&[(6.0, 10.0, 9.0, 13.0, 6.0), (2.0, 6.0, 2.0, 6.0, 4.0), (8.0, 12.0, 4.0, 8.0, 6.0)]],
        _ => &[&[(2.0, 6.0, 2.0, 6.0, 6.0), (9.0, 13.0, 10.0, 14.0, 4.0), (9.0, 13.0, 2.0, 6.0, 6.0), (2.0, 6.0, 8.0, 12.0, 7.0)]],
    };
    let uv16 = |u0: f32, v0: f32, u1: f32, v1: f32| {
        crate::model_family::shape::rect_uv(u0 / 16.0, v1 / 16.0, u1 / 16.0, v0 / 16.0)
    };
    // Body face windows from the official model (the sheet stores one pickle
    // spread across the tile: up [4,1,8,5], down [8,1,12,5], and the four
    // side strips along v5..16 keyed by facing). Side windows shrink with the
    // post height so the art never stretches (JE heights 4..7 px).
    let body_faces = |x0: f32, x1: f32, z0: f32, z1: f32, height: f32| {
        let v1 = 5.0 + height;
        ModelCuboid::new(
            [x0 / 16.0, 0.0, z0 / 16.0],
            [x1 / 16.0, height / 16.0, z1 / 16.0],
        )
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_uv(BlockFace::Up, uv16(4.0, 1.0, 8.0, 5.0))
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::Down, uv16(8.0, 1.0, 12.0, 5.0))
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_uv(BlockFace::North, uv16(4.0, 5.0, 8.0, v1))
        .with_face_material_slot(BlockFace::South, "south")
        .with_face_uv(BlockFace::South, uv16(0.0, 5.0, 4.0, v1))
        .with_face_material_slot(BlockFace::West, "west")
        .with_face_uv(BlockFace::West, uv16(8.0, 5.0, 12.0, v1))
        .with_face_material_slot(BlockFace::East, "east")
        .with_face_uv(BlockFace::East, uv16(12.0, 5.0, 16.0, v1))
    };
    let mut cuboids = Vec::new();
    let mut planes = Vec::new();
    for row in bodies.iter() {
        for &(x0, x1, z0, z1, height) in row.iter() {
            cuboids.push(body_faces(x0, x1, z0, z1, height));
            if dead {
                continue;
            }
            // Flat cap quad at the pickle's top (up window [8,1,12,5]).
            let cap = height / 16.0 - 0.05 / 16.0;
            planes.push(
                ModelPlane::new(
                    [
                        [x0 / 16.0, cap, z0 / 16.0],
                        [x1 / 16.0, cap, z0 / 16.0],
                        [x1 / 16.0, cap, z1 / 16.0],
                        [x0 / 16.0, cap, z1 / 16.0],
                    ],
                    [0, 1, 0],
                )
                .with_material_slot("up")
                .with_uv(uv16(8.0, 1.0, 12.0, 5.0)),
            );
            // Two vertical sprout sheets crossing above the cap.
            let (cx, cz) = ((x0 + x1) / 32.0, (z0 + z1) / 32.0);
            let base = (height + 0.2) / 16.0;
            let top = (height + 3.7) / 16.0;
            planes.push(
                ModelPlane::new(
                    [
                        [cx - 0.03125, base, cz],
                        [cx + 0.03125, base, cz],
                        [cx + 0.03125, top, cz],
                        [cx - 0.03125, top, cz],
                    ],
                    [0, 0, 1],
                )
                .with_material_slot("up")
                .with_uv(uv16(1.0, 0.0, 3.0, 5.0)),
            );
            planes.push(
                ModelPlane::new(
                    [
                        [cx, base, cz - 0.03125],
                        [cx, base, cz + 0.03125],
                        [cx, top, cz + 0.03125],
                        [cx, top, cz - 0.03125],
                    ],
                    [1, 0, 0],
                )
                .with_material_slot("up")
                .with_uv(uv16(13.0, 0.0, 15.0, 5.0)),
            );
        }
    }
    let shape = ModelShape { cuboids, planes };
    let direction = state_i64(state, "coral_fan_direction").unwrap_or(0);
    let _ = direction;
    shape
}

pub(crate) fn egg_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "sniffer_egg" {
        // Capture (ref 773): one 14x16x12 px block. Every crack stage shares this geometry and
        // swaps the sheet per face (`sniffer_egg_<stage>_<face>` arrays indexed by
        // cracked_state), so the stage travels in the material slot.
        let stage = egg_stage_suffix(state, "sniffer_egg");
        let slot = |face: &str| format!("sniffer_{stage}_{face}");
        let mut egg = ModelCuboid::new([px(1.0), 0.0, px(2.0)], [px(15.0), 1.0, px(14.0)]);
        let uv = crate::model_family::shape::full_texture_uv();
        for (face, name) in [
            (BlockFace::Up, "top"),
            (BlockFace::Down, "down"),
            (BlockFace::North, "north"),
            (BlockFace::South, "south"),
            (BlockFace::East, "east"),
            (BlockFace::West, "west"),
        ] {
            egg = egg.with_face_material_slot(face, &slot(name)).with_face_uv(face, uv);
        }
        return ModelShape::from_cuboids([egg]);
    }

    let count = match state_string(state, "turtle_egg_count").unwrap_or("one_egg") {
        "two_egg" => 2,
        "three_egg" => 3,
        "four_egg" => 4,
        _ => 1,
    };
    let stage = egg_stage_suffix(state, "turtle_egg");
    // Capture boxes (refs 899..902, px): one egg [5,0,4]..[10,7,9]; two adds [1,0,7]..[5,5,11];
    // three adds [11,0,7]..[14,4,10]; four adds [7,0,10]..[10,3,13].
    let mut cuboids = vec![egg_cuboid(px(5.0), px(10.0), px(7.0), px(4.0), px(9.0), &stage)];
    if count >= 2 {
        cuboids.push(egg_cuboid(px(1.0), px(5.0), px(5.0), px(7.0), px(11.0), &stage));
    }
    if count >= 3 {
        cuboids.push(egg_cuboid(px(11.0), px(14.0), px(4.0), px(7.0), px(10.0), &stage));
    }
    if count >= 4 {
        cuboids.push(egg_cuboid(px(7.0), px(10.0), px(3.0), px(10.0), px(13.0), &stage));
    }
    ModelShape::from_cuboids(cuboids)
}

/// One turtle egg box; every face samples the WHOLE 16x16 sheet. The tile is a complete egg
/// shell art (roughly x0..14, y0..15), so the per-box projection would crop each small egg to
/// a strip of it — the shell speckles and crack marks would never line up. `turtle_egg` is one
/// terrain key whose three array frames are the crack stages.
fn egg_cuboid(x0: f32, x1: f32, y_max: f32, z0: f32, z1: f32, stage: &str) -> ModelCuboid {
    let cuboid = ModelCuboid::new([x0, 0.0, z0], [x1, y_max, z1])
        .with_material_slot(&format!("turtle_{stage}"));
    let uv = crate::model_family::shape::full_texture_uv();
    [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ]
    .into_iter()
    .fold(cuboid, |cuboid, face| cuboid.with_face_uv(face, uv))
}

/// The crack-stage suffix for one egg state: `no_cracks` keeps the default slot, the two cracked
/// stages append the stage so `obj.rs` can bind the matching terrain array frame.
fn egg_stage_suffix(state: &BlockStateQuery, block: &str) -> String {
    let stage = state_string(state, "cracked_state").unwrap_or("no_cracks");
    let _ = block;
    match stage {
        "cracked" => "cracked".to_owned(),
        "max_cracked" => "max_cracked".to_owned(),
        _ => String::new(),
    }
}

/// Bedrock beacon: a translucent shell, a base slab and the glowing core inside it.
///
/// `blocks.json` gives exactly three slots — `up → beacon_core`, `down → beacon_base`,
/// `side → beacon_shell` — so the shell's six faces take `side`, the base takes `down`, and only
/// the core's top face may show `up`. Painting the core's *sides* with `up` (the previous model)
/// put the bright core art on every face, which is why the block read as a solid white cube.
pub(crate) fn beacon_shape() -> ModelShape {
    let shell = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "side")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side");
    let base = ModelCuboid::new([px(2.0), 0.0, px(2.0)], [px(14.0), px(3.0), px(14.0)])
        .with_face_material_slot(BlockFace::Up, "down")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "down")
        .with_face_material_slot(BlockFace::South, "down")
        .with_face_material_slot(BlockFace::East, "down")
        .with_face_material_slot(BlockFace::West, "down");
    let core = ModelCuboid::new([px(3.0), px(3.0), px(3.0)], [px(13.0), px(14.0), px(13.0)])
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "up")
        .with_face_material_slot(BlockFace::North, "up")
        .with_face_material_slot(BlockFace::South, "up")
        .with_face_material_slot(BlockFace::East, "up")
        .with_face_material_slot(BlockFace::West, "up");
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(shell),
        detail_cuboid_with_local_uv(base),
        detail_cuboid_with_local_uv(core),
    ])
}

pub(crate) fn inset_block_shape(name: &str) -> ModelShape {
    let inset = if name == "honey_block" { 1.0 } else { 3.0 };
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(inset), px(inset), px(inset)],
            [px(16.0 - inset), px(16.0 - inset), px(16.0 - inset)],
        )),
    ])
}

pub(crate) fn dragon_egg_shape() -> ModelShape {
    ModelShape::from_cuboids([
        egg_layer(6.0, 15.0, 4.0, 1.0),
        egg_layer(5.0, 13.0, 6.0, 2.0),
        egg_layer(3.0, 11.0, 10.0, 2.0),
        egg_layer(2.0, 8.0, 12.0, 3.0),
        egg_layer(1.0, 3.0, 14.0, 5.0),
        egg_layer(2.0, 1.0, 12.0, 2.0),
        egg_layer(3.0, 0.0, 10.0, 1.0),
    ])
}

pub(crate) fn item_frame_shape(state: &BlockStateQuery) -> ModelShape {
    let facing = facing_from_state(state);
    let cuboids = match facing {
        // Horizontal frames are the cardinal model rotated onto the floor/ceiling, so the border
        // ring, backing plate and content recess keep exactly the same depth as wall frames.
        Facing::Up => local_item_frame_cuboids()
            .into_iter()
            .map(|cuboid| rotate_cuboid_around_x(cuboid, 1))
            .collect::<Vec<_>>(),
        Facing::Down => local_item_frame_cuboids()
            .into_iter()
            .map(|cuboid| rotate_cuboid_around_x(cuboid, 3))
            .collect::<Vec<_>>(),
        _ => {
            // The authored cuboids sit on the block's +Z face. facing_direction is the side the
            // frame's content faces, so the plate must be rotated toward the opposite (support)
            // side and stay flush with the wall.
            let direction = cardinal_from_facing(facing).unwrap_or(CardinalDirection::South);
            local_item_frame_cuboids()
                .into_iter()
                .map(|cuboid| rotate_cuboid_from_south(cuboid, direction.opposite()))
                .collect::<Vec<_>>()
        }
    };
    ModelShape::from_cuboids(
        cuboids
            .into_iter()
            .map(item_frame_cuboid_uv)
            .collect::<Vec<_>>(),
    )
}

/// The item frame texture is a complete frame image (dark outer edge, wooden backing), so every
/// part samples the whole texture instead of a cuboid-local crop. Cropping would cut the dark
/// frame edge off and leave a flat copper plate.
fn item_frame_cuboid_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let mut cuboid = cuboid;
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        cuboid.face_uvs.insert(face, full_texture_face_uv(face));
    }
    cuboid
}

fn cake_body_cuboid(bites: i64) -> ModelCuboid {
    let bite_width = match bites {
        0 => 0.0,
        1 => 2.0,
        2 => 4.0,
        3 => 6.0,
        4 => 8.0,
        5 => 10.0,
        _ => 12.0,
    };
    // Vanilla `cake_sliceN` eats from the west side: the box keeps max x at 15 and raises
    // min x, so the exposed cut face is the WEST face and uses `cake_inner`.
    let cuboid = detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(1.0 + bite_width), 0.0, px(1.0)],
        [px(15.0), px(8.0), px(15.0)],
    ))
    .with_face_material_slot(BlockFace::Up, "up")
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "north")
    .with_face_material_slot(BlockFace::South, "south")
    .with_face_material_slot(BlockFace::East, "east")
    .with_face_material_slot(BlockFace::West, if bites > 0 { "inner" } else { "west" });
    cuboid
}

fn is_lightning_rod_name(name: &str) -> bool {
    name == "lightning_rod" || name.ends_with("_lightning_rod")
}

fn is_head_block_name(name: &str) -> bool {
    matches!(
        name,
        "skeleton_skull"
            | "wither_skeleton_skull"
            | "zombie_head"
            | "creeper_head"
            | "player_head"
            | "dragon_head"
            | "piglin_head"
    )
}

fn egg_layer(inset: f32, y: f32, size: f32, height: f32) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(inset), px(y), px(inset)],
        [px(inset + size), px(y + height), px(inset + size)],
    ))
}

fn local_item_frame_cuboids() -> Vec<ModelCuboid> {
    vec![
        ModelCuboid::new([px(3.0), px(3.0), px(15.5)], [px(13.0), px(13.0), 1.0]),
        ModelCuboid::new([px(2.0), px(2.0), px(15.0)], [px(14.0), px(3.0), 1.0]),
        ModelCuboid::new([px(2.0), px(13.0), px(15.0)], [px(14.0), px(14.0), 1.0]),
        ModelCuboid::new([px(2.0), px(3.0), px(15.0)], [px(3.0), px(13.0), 1.0]),
        ModelCuboid::new([px(13.0), px(3.0), px(15.0)], [px(14.0), px(13.0), 1.0]),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Facing {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

pub(super) fn facing_from_state(state: &BlockStateQuery) -> Facing {
    if let Some(value) = state_i64(state, "facing_direction")
        .or_else(|| state_i64(state, "minecraft:facing_direction"))
    {
        return match value {
            0 => Facing::Down,
            2 => Facing::North,
            3 => Facing::South,
            4 => Facing::West,
            5 => Facing::East,
            _ => Facing::Up,
        };
    }
    cardinal_direction(state).map_or(Facing::Up, facing_from_cardinal)
}

fn facing_from_cardinal(direction: CardinalDirection) -> Facing {
    match direction {
        CardinalDirection::North => Facing::North,
        CardinalDirection::South => Facing::South,
        CardinalDirection::West => Facing::West,
        CardinalDirection::East => Facing::East,
    }
}

pub(super) fn cardinal_from_facing(facing: Facing) -> Option<CardinalDirection> {
    match facing {
        Facing::North => Some(CardinalDirection::North),
        Facing::South => Some(CardinalDirection::South),
        Facing::West => Some(CardinalDirection::West),
        Facing::East => Some(CardinalDirection::East),
        Facing::Down | Facing::Up => None,
    }
}

fn orient_cuboid_from_up(cuboid: ModelCuboid, facing: Facing) -> ModelCuboid {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for x in [cuboid.min[0], cuboid.max[0]] {
        for y in [cuboid.min[1], cuboid.max[1]] {
            for z in [cuboid.min[2], cuboid.max[2]] {
                let point = orient_point_from_up([x, y, z], facing);
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
        }
    }
    let mut oriented = ModelCuboid::new(min, max);
    oriented.material_slot = cuboid.material_slot;
    // Rotation moves each face to a new slot; keeping the original per-face UVs and material
    // slots on the rotated box would sample the wrong texture region (rods, chains, signs).
    oriented.face_material_slots = cuboid
        .face_material_slots
        .into_iter()
        .map(|(face, slot)| (orient_face_from_up(face, facing), slot))
        .collect();
    oriented.face_uvs = cuboid
        .face_uvs
        .into_iter()
        .map(|(face, uv)| (orient_face_from_up(face, facing), uv))
        .collect();
    oriented
}

fn orient_point_from_up(point: [f32; 3], facing: Facing) -> [f32; 3] {
    let [x, y, z] = point;
    match facing {
        Facing::Up => [x, y, z],
        Facing::Down => [x, 1.0 - y, z],
        Facing::North => [x, z, 1.0 - y],
        Facing::South => [x, z, y],
        Facing::West => [1.0 - y, z, x],
        Facing::East => [y, z, x],
    }
}

/// Maps a block face to the face it becomes after [`orient_point_from_up`].
///
/// Derived from the same axis permutation the corner transform applies, so a rotated model
/// keeps its per-face UVs and material slots on the correct side.
fn orient_face_from_up(face: BlockFace, facing: Facing) -> BlockFace {
    use BlockFace::{Down, East, North, South, Up, West};
    match facing {
        Facing::Up => face,
        Facing::Down => match face {
            Up => Down,
            Down => Up,
            other => other,
        },
        // model +Y -> world -Z, model +Z -> world +Y
        Facing::North => match face {
            Up => North,
            Down => South,
            North => Down,
            South => Up,
            other => other,
        },
        // model +Y -> world +Z, model +Z -> world +Y
        Facing::South => match face {
            Up => South,
            Down => North,
            North => Down,
            South => Up,
            other => other,
        },
        // model +Y -> world -X, model +X -> world +Z, model +Z -> world +Y
        Facing::West => match face {
            Up => West,
            Down => East,
            East => South,
            West => North,
            North => Down,
            South => Up,
            other => other,
        },
        // model +Y -> world +X, model +X -> world +Z, model +Z -> world +Y
        Facing::East => match face {
            Up => East,
            Down => West,
            East => South,
            West => North,
            North => Down,
            South => Up,
            other => other,
        },
    }
}

fn scaffolding_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    side_slot_cuboid(min, max)
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
}

fn side_slot_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(min, max))
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
}

fn rotate_cuboid_around_x(cuboid: ModelCuboid, quarter_turns: u8) -> ModelCuboid {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for x in [cuboid.min[0], cuboid.max[0]] {
        for y in [cuboid.min[1], cuboid.max[1]] {
            for z in [cuboid.min[2], cuboid.max[2]] {
                let mut point = [x, y, z];
                for _ in 0..quarter_turns.rem_euclid(4) {
                    point = [point[0], 1.0 - point[2], point[1]];
                }
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
        }
    }
    let mut rotated = ModelCuboid::new(min, max);
    rotated.material_slot = cuboid.material_slot;
    rotated
}

fn rotate_cuboid_from_south(cuboid: ModelCuboid, direction: CardinalDirection) -> ModelCuboid {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    (0..turns).fold(cuboid, |cuboid, _| rotate_cuboid_clockwise(cuboid))
}

fn rotate_cuboid_clockwise(cuboid: ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [1.0 - cuboid.max[2], cuboid.min[1], cuboid.min[0]],
        [1.0 - cuboid.min[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot;
    for (face, slot) in cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(rotate_block_face_clockwise(face), slot);
    }
    for (face, uv) in cuboid.face_uvs {
        rotated
            .face_uvs
            .insert(rotate_block_face_clockwise(face), uv);
    }
    rotated
}

fn rotate_block_face_clockwise(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
