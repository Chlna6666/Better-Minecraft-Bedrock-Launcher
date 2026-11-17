use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, detail_cuboid_with_local_uv, full_texture_face_uv,
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
    // Capture (ref 118 south): a back panel spanning the full cell width and height 3 px deep,
    // plus two 2 px lips at y 0..4 and y 12..16 reaching z 3..5 — not one solid 5 px slab (the
    // previous model filled the shelf interior).
    let back = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, px(3.0)]);
    let top_lip = ModelCuboid::new([0.0, px(12.0), px(3.0)], [1.0, 1.0, px(5.0)]);
    let bottom_lip = ModelCuboid::new([0.0, 0.0, px(3.0)], [1.0, px(4.0), px(5.0)]);
    // The pack art is the 32x32 unfolded sheet (`oak_shelf.png`): the right half carries the
    // shelf interior, the left half the three south-facing bands (lips + back), and the thin
    // strips the rims. Rects below follow the captured appearance (appearance ref 2376).
    let sheet = |u0: f32, v0: f32, u1: f32, v1: f32| {
        crate::model_family::shape::rect_uv(u0 / 32.0, v0 / 32.0, u1 / 32.0, v1 / 32.0)
    };
    // South face pieces (the room-facing side): bottom lip band, back band, top lip band.
    let back = back
        .with_face_uv(BlockFace::South, sheet(0.0, 8.1, 15.8, 23.9))
        .with_face_uv(BlockFace::Up, sheet(16.1, 18.2, 32.0, 23.9))
        .with_face_uv(BlockFace::Down, sheet(16.1, 14.2, 32.0, 19.9))
        .with_face_uv(BlockFace::East, sheet(29.1, 0.0, 32.0, 32.0))
        .with_face_uv(BlockFace::West, sheet(16.1, 0.0, 18.9, 32.0))
        .with_face_uv(BlockFace::North, sheet(16.1, 0.0, 32.0, 32.0));
    let top_lip = top_lip
        .with_face_uv(BlockFace::South, sheet(0.0, 24.2, 15.8, 32.0))
        .with_face_uv(BlockFace::East, sheet(3.0, 24.2, 4.8, 32.0))
        .with_face_uv(BlockFace::West, sheet(11.0, 24.2, 12.9, 32.0))
        .with_face_uv(BlockFace::Down, sheet(0.0, 24.2, 15.8, 32.0));
    let bottom_lip = bottom_lip
        .with_face_uv(BlockFace::South, sheet(0.0, 0.0, 15.8, 7.8))
        .with_face_uv(BlockFace::East, sheet(3.0, 0.0, 4.8, 7.8))
        .with_face_uv(BlockFace::West, sheet(11.0, 0.0, 12.9, 7.8))
        .with_face_uv(BlockFace::Up, sheet(0.0, 0.0, 15.8, 7.8));
    let shape = ModelShape::from_cuboids([back, top_lip, bottom_lip]);
    ModelShape::from_cuboids(
        shape
            .cuboids
            .into_iter()
            .map(|cuboid| rotate_cuboid_from_south(cuboid, direction))
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn heavy_core_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(4.0), 0.0, px(4.0)],
        [px(12.0), px(8.0), px(12.0)],
    ))])
}

pub(crate) fn conduit_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(5.0), 0.0, px(5.0)],
        [px(11.0), px(6.0), px(11.0)],
    ))])
}

pub(crate) fn dried_ghast_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    // `dried_ghast_<face>` terrain keys are four-variant arrays indexed by rehydration_level + 1,
    // so the stage travels in the per-face slot (`ghast_<stage>_<face>`) and `obj.rs` binds the
    // matching frame; the geometry itself is identical across stages (capture refs 398..401).
    let stage = state_i64(state, "rehydration_level").unwrap_or(0).clamp(0, 3);
    let slot = |face: &str| format!("ghast_{stage}_{face}");
    let mut cuboids = vec![
        ModelCuboid::new([px(3.0), 0.0, px(3.0)], [px(13.0), px(10.0), px(13.0)])
            .with_face_material_slot(BlockFace::Up, &slot("top"))
            .with_face_material_slot(BlockFace::Down, &slot("down"))
            .with_face_material_slot(BlockFace::North, &slot("north"))
            .with_face_material_slot(BlockFace::South, &slot("south"))
            .with_face_material_slot(BlockFace::East, &slot("east"))
            .with_face_material_slot(BlockFace::West, &slot("west")),
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
            .map(|cuboid| detail_cuboid_with_local_uv(rotate_cuboid_from_south(cuboid, direction)))
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
    let count = state_i64(state, "cluster_count").unwrap_or(0).clamp(0, 3) + 1;
    let dead = state_i64(state, "dead_bit").unwrap_or(0) != 0;
    // (x0, x1, z0, z1, body_top) in authoring pixels.
    let bodies: &[(f32, f32, f32, f32, f32)] = match count {
        1 => &[(6.0, 10.0, 6.0, 10.0, 6.0)],
        2 => &[
            (3.6, 7.6, 3.6, 7.6, 6.0),
            (8.4, 12.4, 8.4, 12.4, 3.92),
        ],
        3 => &[
            (2.8, 6.8, 2.8, 6.8, 6.96),
            (6.0, 10.0, 9.2, 13.2, 3.92),
            (7.6, 11.6, 3.6, 7.6, 6.0),
        ],
        _ => &[
            (2.0, 6.0, 8.72, 12.72, 6.96),
            (2.64, 6.64, 2.96, 6.96, 6.0),
            (9.04, 13.04, 2.32, 6.32, 4.88),
            (9.36, 13.36, 10.16, 14.16, 3.92),
        ],
    };

    let mut cuboids = Vec::with_capacity(bodies.len() * 2);
    for (x0, x1, z0, z1, body_top) in bodies {
        cuboids.push(detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(*x0), 0.0, px(*z0)],
            [px(*x1), px(*body_top), px(*z1)],
        )));
        if dead {
            continue;
        }
        let (centre_x, centre_z) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
        cuboids.push(detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(centre_x - 0.64), px(body_top - 0.8), px(centre_z - 0.64)],
            [px(centre_x + 0.64), px(body_top + 2.4), px(centre_z + 0.64)],
        )));
    }
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn egg_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "sniffer_egg" {
        // Capture (ref 773): one 14x16x12 px block. Every crack stage shares this geometry and
        // swaps the sheet per face (`sniffer_egg_<stage>_<face>` arrays indexed by
        // cracked_state), so the stage travels in the material slot.
        let stage = egg_stage_suffix(state, "sniffer_egg");
        let slot = |face: &str| format!("sniffer_{stage}_{face}");
        return ModelShape::from_cuboids([detail_cuboid_with_local_uv(
            ModelCuboid::new([px(1.0), 0.0, px(2.0)], [px(15.0), 1.0, px(14.0)])
                .with_face_material_slot(BlockFace::Up, &slot("top"))
                .with_face_material_slot(BlockFace::Down, &slot("down"))
                .with_face_material_slot(BlockFace::North, &slot("north"))
                .with_face_material_slot(BlockFace::South, &slot("south"))
                .with_face_material_slot(BlockFace::East, &slot("east"))
                .with_face_material_slot(BlockFace::West, &slot("west")),
        )]);
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

/// One turtle egg box; every face samples the whole sheet (the tile is isotropic egg art, and
/// `turtle_egg` is one terrain key whose three array frames are the crack stages).
fn egg_cuboid(x0: f32, x1: f32, y_max: f32, z0: f32, z1: f32, stage: &str) -> ModelCuboid {
    detail_cuboid_with_local_uv(
        ModelCuboid::new([x0, 0.0, z0], [x1, y_max, z1])
            .with_material_slot(&format!("turtle_{stage}")),
    )
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

fn face_slot_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(min, max))
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "south")
        .with_face_material_slot(BlockFace::East, "east")
        .with_face_material_slot(BlockFace::West, "west")
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
