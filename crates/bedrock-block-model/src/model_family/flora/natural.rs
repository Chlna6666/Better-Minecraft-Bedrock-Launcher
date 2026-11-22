use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv, uv16,
    upright_full_texture_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    match name {
        "farmland" | "grass_path" => Some(ModelFamily::Farmland),
        "fire" | "soul_fire" => Some(ModelFamily::Fire),
        "water" | "flowing_water" | "lava" | "flowing_lava" => Some(ModelFamily::Liquid),
        "bubble_column" => Some(ModelFamily::BubbleColumn),
        "trip_wire" => Some(ModelFamily::Tripwire),
        "chorus_plant" | "chorus_flower" => Some(ModelFamily::ChorusPlant),
        "pointed_dripstone" | "sulfur_spike" => Some(ModelFamily::PointedDripstone),
        "mangrove_propagule" => Some(ModelFamily::MangrovePropagule),
        "big_dripleaf" | "small_dripleaf_block" => Some(ModelFamily::Dripleaf),
        "spore_blossom" => Some(ModelFamily::SporeBlossom),
        "sculk_sensor" | "calibrated_sculk_sensor" => Some(ModelFamily::SculkSensor),
        "sculk_shrieker" => Some(ModelFamily::SculkShrieker),
        _ => None,
    }
}

pub(crate) fn farmland_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(15.0), 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )])
}

/// Bedrock fire: four sheets tilted 22.5 degrees that **spill above the cell**.
///
/// Verified against the real client: `BedrockSceneCapture` stores 28 vertices reaching
/// `y = 1.4` (22.4 px), with the bottom edges at `z 0.3 / 0.7` and the top edges spreading to
/// `0.1..0.2` and `0.8..0.9`. So Bedrock does **not** rescale the tilted element back to one
/// cell — the flames stand 0.4 above their block, and the pair opens upward in a shallow "V".
pub(crate) fn fire_shape() -> ModelShape {
    // Measured from the capture (ref 288): **eight** vertical planes, all spanning `y 0..1.4`
    // (22.4 px, so the flame spills above its cell), grouped as four facing +X, four facing +Z.
    // Each group is two "edge" sheets hugging the block boundary and two tilted sheets that cross
    // in the middle — the bottom edge sits near the centre and the top edge swings out to the
    // boundary, which is what gives the flame its tent silhouette.
    //
    // The previous shape emitted only four planes and used tops of `0.9`/`0.1` where the capture
    // has `0.8`/`0.2`, so the flame was missing its four edge walls and the tilt was too wide.
    let top = 1.4_f32;
    let uv = super::super::shape::upright_full_texture_uv();
    // (normal, bottom, top) — `bottom`/`top` are the plane's position along its own tilt axis.
    let planes = [
        ([1_i32, 0, 0], 1.0_f32, 0.9_f32),
        ([1, 0, 0], 0.7, 0.2),
        ([-1, 0, 0], 0.3, 0.8),
        ([-1, 0, 0], 0.0, 0.1),
        ([0, 0, 1], 1.0, 0.9),
        ([0, 0, 1], 0.7, 0.2),
        ([0, 0, -1], 0.3, 0.8),
        ([0, 0, -1], 0.0, 0.1),
    ];
    ModelShape::default().with_planes(planes
        .map(|(normal, bottom, tip)| {
            let corners = if normal[0] != 0 {
                // Faces +-X: spans the full Z extent, tilts along X.
                [
                    [bottom, 0.0, 0.0],
                    [bottom, 0.0, 1.0],
                    [tip, top, 1.0],
                    [tip, top, 0.0],
                ]
            } else {
                // Faces +-Z: spans the full X extent, tilts along Z.
                [
                    [0.0, 0.0, bottom],
                    [1.0, 0.0, bottom],
                    [1.0, top, tip],
                    [0.0, top, tip],
                ]
            };
            crate::model_family::shape::ModelPlane::new(corners, normal)
                .front_only()
                .with_material_slot("side")
                .with_uv(uv)
        }))
}

pub(crate) fn liquid_shape(state: &BlockStateQuery) -> ModelShape {
    let height = match state_i64(state, "liquid_depth").unwrap_or(8) {
        0 => px(14.0),
        1 => px(12.5),
        2 => px(10.5),
        3 => px(9.0),
        4 => px(7.0),
        5 => px(5.5),
        6 => px(3.5),
        7 => px(1.5),
        _ => 1.0,
    };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, height, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )])
}

pub(crate) fn bubble_column_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(15.0), 1.0, px(15.0)])
                .with_face_material_slot(BlockFace::Side, "north"),
        ),
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(5.0), 0.0, px(5.0)], [px(11.0), 1.0, px(11.0)])
                .with_face_material_slot(BlockFace::Side, "south"),
        ),
    ])
}

/// Bedrock tripwire: a flat 0.5 px-wide string plane at `y = 1.5` px, running the full cell
/// along each connected axis. The official `tripwire_n` model splits it into 4 px segments
/// that each read the string band `trip_wire` rows 4..6 along their length (`up
/// [0,4,16,6]` rot 90); one 16 px quad with that window is equivalent. The wire only runs
/// along the axes its state connects, so a wire with no neighbours draws nothing.
pub(crate) fn tripwire_shape(state: &BlockStateQuery) -> ModelShape {
    // Bedrock stores no per-direction state; the scene injects the four connection
    // booleans from the neighbours. A connected wire draws one 4 px segment from the
    // cell edge to the centre per connected side (JE `tripwire_*` multipart): no
    // connection → no wire at all. Previously the flags defaulted to true, so every
    // cell drew a full cross even with no neighbouring wire or hook.
    let connected = |dir: &str| {
        state_bool(state, &format!("minecraft:{dir}"))
            .or_else(|| state_bool(state, dir))
            .unwrap_or(false)
    };
    let north = connected("north");
    let south = connected("south");
    let east = connected("east");
    let west = connected("west");
    // Powered wires sit lower in the cell (the triggering animation); the art band
    // itself is the same `trip_wire` sheet.
    let powered = state_bool(state, "powered_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    let y = if powered { 0.5 / 16.0 } else { 1.5 / 16.0 };
    let (lo, hi) = (0.4844_f32, 0.5156_f32);
    let mid = 0.5_f32;
    // Length-first corners so U runs along each segment; the repeating 4 px dotted
    // band is sampled by a full-tile window (the texture repeats every 4 px).
    let band_uv = [[0.0, 6.0 / 16.0], [1.0, 6.0 / 16.0], [1.0, 4.0 / 16.0], [0.0, 4.0 / 16.0]];
    let mut planes = Vec::with_capacity(4);
    let segment = |corners: [[f32; 3]; 4]| {
        ModelPlane::new(corners, [0, 1, 0]).with_uv(band_uv)
    };
    // North edge (z=0) to centre, south edge (z=1) to centre.
    if north {
        planes.push(segment([[lo, y, 0.0], [lo, y, mid], [hi, y, mid], [hi, y, 0.0]]));
    }
    if south {
        planes.push(segment([[lo, y, 1.0], [lo, y, mid], [hi, y, mid], [hi, y, 1.0]]));
    }
    // West edge (x=0) to centre, east edge (x=1) to centre.
    if west {
        planes.push(segment([[0.0, y, lo], [mid, y, lo], [mid, y, hi], [0.0, y, hi]]));
    }
    if east {
        planes.push(segment([[1.0, y, lo], [mid, y, lo], [mid, y, hi], [1.0, y, hi]]));
    }
    ModelShape::default().with_planes(planes)
}

/// Chorus plant and chorus flower.
///
/// **Chorus plant** is a stateful connector: `blocks.json` exposes six
/// `minecraft:connection_*` booleans, and the captured geometry changes with them. The centre
/// cube plus one arm per connected side is the whole rule — the previous shape was a fixed
/// three-box plus that ignored the states entirely, so every plant looked the same whether it
/// was hanging in mid-air or joined to six neighbours.
///
/// **Chorus flower** is *not* a cross plant. The capture (ref 473) stores a solid 3-D cross:
/// three overlapping boxes that each span the full 0..16 extent of one axis and 2..14 of the
/// other two, which is the vanilla "centre cube with arms reaching the block edge" silhouette.
/// It was previously unclassified, so it fell through to a plain full cube.
pub(crate) fn chorus_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    // `canonical_block_name` returns the `minecraft:`-prefixed form.
    if name.ends_with("chorus_flower") {
        // Measured from capture ref 473: three full-span boxes clipped to 2..14 on the other
        // axes. Their union is the flower silhouette.
        return ModelShape::from_cuboids([
            detail_cuboid_with_local_uv(ModelCuboid::new([0.0, px(2.0), px(2.0)], [1.0, px(14.0), px(14.0)])),
            detail_cuboid_with_local_uv(ModelCuboid::new([px(2.0), 0.0, px(2.0)], [px(14.0), 1.0, px(14.0)])),
            detail_cuboid_with_local_uv(ModelCuboid::new([px(2.0), px(2.0), 0.0], [px(14.0), px(14.0), 1.0])),
        ]);
    }

    // Chorus plant: a centre cube with one arm per connected side.
    let mut cuboids = vec![detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(2.0), px(2.0), px(2.0)],
        [px(14.0), px(14.0), px(14.0)],
    ))];
    for (state_key, arm) in [
        (
            "minecraft:connection_west",
            ModelCuboid::new([0.0, px(2.0), px(2.0)], [px(14.0), px(14.0), px(14.0)]),
        ),
        (
            "minecraft:connection_east",
            ModelCuboid::new([px(14.0), px(2.0), px(2.0)], [1.0, px(14.0), px(14.0)]),
        ),
        (
            "minecraft:connection_down",
            ModelCuboid::new([px(2.0), 0.0, px(2.0)], [px(14.0), px(14.0), px(14.0)]),
        ),
        (
            "minecraft:connection_up",
            ModelCuboid::new([px(2.0), px(14.0), px(2.0)], [px(14.0), 1.0, px(14.0)]),
        ),
        (
            "minecraft:connection_north",
            ModelCuboid::new([px(2.0), px(2.0), 0.0], [px(14.0), px(14.0), px(14.0)]),
        ),
        (
            "minecraft:connection_south",
            ModelCuboid::new([px(2.0), px(2.0), px(14.0)], [px(14.0), px(14.0), 1.0]),
        ),
    ] {
        if state_bool(state, state_key).unwrap_or(false) {
            cuboids.push(detail_cuboid_with_local_uv(arm));
        }
    }
    ModelShape::from_cuboids(cuboids)
}

/// Vanilla spike segment: a tapered column anchored at the attached end.
///
/// `dripstone_thickness` selects the segment width and length (tip 2 x 6 px, frustum
/// 4 x 11 px, middle/merge 4 x 16 px, base 6 x 16 px) and `hanging` mirrors it so the free
/// end always points away from the support. The texture art for each segment is the
//// Bedrock pointed dripstone / sulfur spike: **two 45-degree cross planes**.
///
/// Verified against the real client: `BedrockSceneCapture` stores 16 vertices per segment state,
/// all on the `y = 0` and `y = 1` layers, spanning about 0.9 on both horizontal axes. That is the
/// signature of two rotated sheets (`0.8..15.2` px rotated 45 degrees gives `0.1818..0.8182`),
/// not a box — the segment's shape comes from the alpha-cut silhouette in the texture.
///
/// `blocks.json` still assigns one thickness art per face, but a cross only has two faces per
/// sheet; the front/back pair is what a viewer actually sees, so the sheet uses the art its own
/// orientation names and the remaining slots stay available for anything that samples them.
pub(crate) fn pointed_dripstone_shape(state: &BlockStateQuery) -> ModelShape {
    let thickness = state_string(state, "dripstone_thickness").unwrap_or("tip");
    let hanging = state_bool(state, "hanging").unwrap_or(false);
    // Bedrock spells the merged segment `merge`; Java calls it `tip_merge`.
    let segment = match thickness {
        "tip_merge" => "merge",
        other => other,
    };
    // `terrain_texture.json` lists each segment as a two-frame array `[down, up]`; the frame has
    // to travel in the slot name or a standing spike samples the hanging art.
    let frame = if hanging { "down" } else { "up" };
    let slot = format!("{segment}_{frame}");
    // The `_up_*` art is narrow at the top (the point faces up) and `_down_*` narrow at the
    // bottom, matching which end the state's tip belongs to. Each segment only paints the rows
    // its own art uses — `tip` covers y 4..15, so a standing tip must sample that band and not
    // stretch it over the whole cell, which is what made every stage look identical and inverted.
    let (v0, v1) = match (segment, hanging) {
        // A standing tip is the bottom 12 rows of the tile; a hanging tip is the top 12.
        ("tip", false) => (4.0_f32, 16.0),
        ("tip", true) => (0.0, 12.0),
        _ => (0.0, 16.0),
    };
    let uv = if hanging {
        crate::model_family::shape::hanging_full_texture_uv()
    } else {
        crate::model_family::shape::upright_full_texture_uv()
    };
    // Rescale the V range onto the band the art actually paints.
    let uv = if (v0, v1) == (0.0, 16.0) {
        uv
    } else {
        let (lo, hi) = (v0 / 16.0, v1 / 16.0);
        uv.map(|corner| [corner[0], hi - (1.0 - corner[1]) * (hi - lo)])
    };
    // `0.8..15.2` px rotated 45 degrees about the block centre.
    let reach = 0.45_f32 * std::f32::consts::FRAC_1_SQRT_2;
    let (lo, hi) = (0.5 - reach, 0.5 + reach);
    ModelShape::default().with_planes([
        crate::model_family::shape::ModelPlane::new(
            [
                [lo, 0.0, lo],
                [hi, 0.0, hi],
                [hi, 1.0, hi],
                [lo, 1.0, lo],
            ],
            [-1, 0, 1],
        )
        .with_material_slot(slot.as_str())
        .with_uv(uv),
        crate::model_family::shape::ModelPlane::new(
            [
                [hi, 0.0, lo],
                [lo, 0.0, hi],
                [lo, 1.0, hi],
                [hi, 1.0, lo],
            ],
            [1, 0, 1],
        )
        .with_material_slot(slot.as_str())
        .with_uv(uv),
    ])
}

/// Bedrock mangrove propagule: two crossed cards, each stepped from a narrow stalk to a wide leaf.
///
/// Measured from the client (refs 767-772). The plant is **not** a set of boxes: it is two
/// diagonal cards, and each card is two quads — a stalk section running `y 0..0.5625` between
/// `x/z 0.2837..0.4163`, and a leaf section running `y 0.5625..0.9375` that flares out to
/// `x/z 0.1511..0.5489`. The previous cross-plane model had a uniform width and the previous box
/// model added a depth the capture does not have, which is why the sprout never matched.
///
/// Two state axes move the plant:
///
/// * `propagule_stage` 0..4 slides the whole plant across the cell without rotating it. The
///   offsets are `(0, 0)`, `(0, +0.2666)`, `(+0.3667, +0.1333)`, `(-0.1, +0.1)` and
///   `(+0.2, +0.4)` in `(x, z)`, so the captured footprints walk around the cell while the height
///   stays identical.
/// * `hanging` (ref 772) is a separate layout: a narrow stem runs the full cell, a body sits at
///   `y 0.625..0.8125`, four collar plates fan out at `y 0.7647..0.8125` and a cap closes the top.
pub(crate) fn mangrove_propagule_shape(state: &BlockStateQuery) -> ModelShape {
    let hanging = state_bool(state, "hanging").unwrap_or(false);
    let stage = state_i64(state, "propagule_stage").unwrap_or(0).clamp(0, 4);
    let (stage_x, stage_z) = [(0.0, 0.0), (0.0, 0.2666), (0.3667, 0.1333), (-0.1, 0.1), (0.2, 0.4)]
        [stage as usize];
    if hanging {
        return hanging_propagule_shape(stage);
    }
    // The two cards, authored in place, then slid by the stage offset.
    //
    // UVs follow the JE `mangrove_propagule` model, which is the only reference that names the
    // sprite's painted regions. The 16x16 `mangrove_propagule` tile paints the **leaf** at
    // `u 4..11, v 1..7` and the **hypocotyl** (stem) at `u 7..9, v 7..16`, so the leaf and stem
    // sections of each card sample those two rects. Sampling the whole tile (the previous
    // `upright_full_texture_uv`) landed mostly on transparent pixels and stretched the stem art
    // across the leaf, which is what "红树木繁殖体 uv 错误" was.
    let leaf_uv = uv16(4.0, 1.0, 11.0, 7.0);
    let stem_uv = uv16(7.0, 7.0, 9.0, 16.0);
    let cards = [
        // Card A: lower-left to upper-right. Stem section then leaf section.
        (
            [
                [0.2837, 0.0, 0.2504],
                [0.4163, 0.0, 0.383],
                [0.4163, 0.5625, 0.383],
                [0.2837, 0.5625, 0.2504],
            ],
            [-1, 0, 1],
            stem_uv,
        ),
        (
            [
                [0.1511, 0.5625, 0.1178],
                [0.5489, 0.5625, 0.5155],
                [0.5489, 0.9375, 0.5155],
                [0.1511, 0.9375, 0.1178],
            ],
            [-1, 0, 1],
            leaf_uv,
        ),
        // Card B: upper-left to lower-right.
        (
            [
                [0.4163, 0.0, 0.2504],
                [0.2837, 0.0, 0.383],
                [0.2837, 0.5625, 0.383],
                [0.4163, 0.5625, 0.2504],
            ],
            [1, 0, 1],
            stem_uv,
        ),
        (
            [
                [0.5489, 0.5625, 0.1178],
                [0.1511, 0.5625, 0.5155],
                [0.1511, 0.9375, 0.5155],
                [0.5489, 0.9375, 0.1178],
            ],
            [1, 0, 1],
            leaf_uv,
        ),
    ];
    ModelShape::default().with_planes(cards.map(|(mut corners, normal, uv)| {
        for corner in &mut corners {
            corner[0] += stage_x;
            corner[2] += stage_z;
        }
        ModelPlane::new(corners, normal).with_uv(uv)
    }))
}

/// The hanging propagule hangs from the block above it, and each `propagule_stage` 1..4 grows
/// the hypocotyl longer. Numbers come from the official JE `mangrove_propagule_hanging_{1..4}`
/// models (y in model pixels maps 1:1 to world y): the leaf pair is a 2 px crossed card at the
/// attachment (y 14..16), the seed body is a 2 x 3 x 2 box at y 10..13, four collar plates fan
/// out at y ≈ 11.6..12.4, and the hypocotyl is a crossed 2 px pair running from the body down —
/// model y 7 (stage 2), 3 (stage 3) or 0 (stage 4). The plant samples the dedicated
/// `mangrove_propagule_hanging` sprite (the standing tile's art sits at different pixels), whose
/// painted columns are the stem pair at x 0..2 / 3..5 and the sprout at x 6..10.
///
/// Rotations: the leaf and stem boxes are 2 px planes turned ±45° about Y (diagonal cards,
/// corners at 7.2929..8.7071 px); the collars are near-horizontal plates tilted ±22.5°.
fn hanging_propagule_shape(stage: i64) -> ModelShape {
    let px = |value: f32| value / 16.0;
    // Sprite windows from the JE hanging models.
    let leaf_uv = uv16(0.0, 0.0, 2.0, 2.0);
    let cap_side_uv = uv16(0.0, 2.0, 2.0, 3.0);
    let cap_up_uv = uv16(0.0, 0.0, 2.0, 2.0);
    let cap_down_uv = uv16(0.0, 3.0, 2.0, 5.0);
    let body_side_uv = uv16(0.0, 7.0, 2.0, 10.0);
    let body_up_uv = uv16(0.0, 5.0, 2.0, 7.0);
    // Stage 2+ point the body's bottom face at the sprout tip art below the body column.
    let body_down_uv = if stage >= 2 {
        uv16(0.0, 10.0, 2.0, 12.0)
    } else {
        uv16(0.0, 5.0, 2.0, 7.0)
    };
    let collar_up_uv = uv16(8.0, 3.0, 10.0, 5.0);
    let collar_down_uv = uv16(6.0, 3.0, 8.0, 5.0);
    // The hypocotyl pair grows with the stage; the window rides the art column x3..5.
    let stem_v0 = match stage {
        0 | 1 => 7.0,
        2 => 7.0,
        3 => 3.0,
        _ => 0.0,
    };
    let stem_uv = uv16(3.0, stem_v0, 5.0, 10.0);

    let mut planes = Vec::new();
    // Leaf pair at the attachment: two diagonal cards, y 14..16 px.
    let diag_lo = px(7.2929);
    let diag_hi = px(8.7071);
    let leaf_top = px(16.0);
    let leaf_bottom = px(14.0);
    for (normal, corners) in [
        (
            [1, 0, 1],
            [
                [diag_lo, leaf_bottom, diag_hi],
                [diag_hi, leaf_bottom, diag_lo],
                [diag_hi, leaf_top, diag_lo],
                [diag_lo, leaf_top, diag_hi],
            ],
        ),
        (
            [-1, 0, 1],
            [
                [diag_lo, leaf_bottom, diag_lo],
                [diag_hi, leaf_bottom, diag_hi],
                [diag_hi, leaf_top, diag_hi],
                [diag_lo, leaf_top, diag_lo],
            ],
        ),
    ] {
        planes.push(ModelPlane::new(corners, normal).with_uv(leaf_uv));
    }
    // Hypocotyl pair: same diagonal cross, running from the body down to the stage depth
    // (stage 1 ships no stem; stages 2..4 start at model y 7 / 3 / 0).
    if stage >= 2 {
        let stem_top = px(10.0);
        let stem_bottom = px(match stage {
            2 => 7.0,
            3 => 3.0,
            _ => 0.0,
        });
        for (normal, corners) in [
            (
                [1, 0, 1],
                [
                    [diag_lo, stem_bottom, diag_hi],
                    [diag_hi, stem_bottom, diag_lo],
                    [diag_hi, stem_top, diag_lo],
                    [diag_lo, stem_top, diag_hi],
                ],
            ),
            (
                [-1, 0, 1],
                [
                    [diag_lo, stem_bottom, diag_lo],
                    [diag_hi, stem_bottom, diag_hi],
                    [diag_hi, stem_top, diag_hi],
                    [diag_lo, stem_top, diag_lo],
                ],
            ),
        ] {
            planes.push(ModelPlane::new(corners, normal).with_uv(stem_uv));
        }
    }
    // The four collar plates: near-horizontal quads tilted ±22.5°, fanning out of the body's
    // top corners (rotated corners computed from the official element boxes).
    let collars: [(&str, [f32; 3], [f32; 3], [f32; 3], [f32; 3]); 4] = [
        (
            "south",
            [px(7.0), px(12.391), px(12.0615)],
            [px(9.0), px(12.391), px(12.0615)],
            [px(9.0), px(11.6257), px(13.9092)],
            [px(7.0), px(11.6257), px(13.9092)],
        ),
        (
            "north",
            [px(7.0), px(11.6257), px(3.9385)],
            [px(9.0), px(11.6257), px(3.9385)],
            [px(9.0), px(12.391), px(2.0908)],
            [px(7.0), px(12.391), px(2.0908)],
        ),
        (
            "east",
            [px(12.0615), px(12.391), px(7.0)],
            [px(12.0615), px(12.391), px(9.0)],
            [px(13.9092), px(11.6257), px(9.0)],
            [px(13.9092), px(11.6257), px(7.0)],
        ),
        (
            "west",
            [px(3.9385), px(11.6257), px(7.0)],
            [px(3.9385), px(11.6257), px(9.0)],
            [px(2.0908), px(12.391), px(9.0)],
            [px(2.0908), px(12.391), px(7.0)],
        ),
    ];
    for (name, c0, c1, c2, c3) in collars {
        let normal = match name {
            "south" => [0, 0, 1],
            "north" => [0, 0, -1],
            "east" => [1, 0, 0],
            _ => [-1, 0, 0],
        };
        planes.push(
            ModelPlane::new([c0, c1, c2, c3], normal)
                .with_uv(collar_up_uv)
                .with_material_slot("collar"),
        );
        // The underside carries the sprout art; emit the flipped face too.
        planes.push(
            ModelPlane::new([c3, c2, c1, c0], [-normal[0], -normal[1], -normal[2]])
                .with_uv(collar_down_uv)
                .with_material_slot("collar"),
        );
    }
    // Body (2 x 3 x 2 at y 10..13) and cap (2 x 1 x 2 at y 13..14) as small cuboids with the
    // official per-face windows.
    let body = ModelCuboid::new([px(7.0), px(10.0), px(7.0)], [px(9.0), px(13.0), px(9.0)])
        .with_face_uv(BlockFace::North, body_side_uv)
        .with_face_uv(BlockFace::South, body_side_uv)
        .with_face_uv(BlockFace::East, body_side_uv)
        .with_face_uv(BlockFace::West, body_side_uv)
        .with_face_uv(BlockFace::Up, body_up_uv)
        .with_face_uv(BlockFace::Down, body_down_uv);
    let cap = ModelCuboid::new([px(7.0), px(13.0), px(7.0)], [px(9.0), px(14.0), px(9.0)])
        .with_face_uv(BlockFace::North, cap_side_uv)
        .with_face_uv(BlockFace::South, cap_side_uv)
        .with_face_uv(BlockFace::East, cap_side_uv)
        .with_face_uv(BlockFace::West, cap_side_uv)
        .with_face_uv(BlockFace::Up, cap_up_uv)
        .with_face_uv(BlockFace::Down, cap_down_uv);
    ModelShape::from_cuboids([body, cap]).with_planes(planes)
}

pub(crate) fn dripleaf_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "small_dripleaf_block" {
        return small_dripleaf_shape(state);
    }
    big_dripleaf_shape(state)
}

pub(crate) fn spore_blossom_shape() -> ModelShape {
    let mut shape = ModelShape::default().with_planes([ModelPlane::new(
        [
            [0.0, px(15.9), 0.0],
            [1.0, px(15.9), 0.0],
            [1.0, px(15.9), 1.0],
            [0.0, px(15.9), 1.0],
        ],
        [0, -1, 0],
    )
    .with_material_slot("up")
    .with_uv(full_texture_uv())]);
    shape.planes.extend([
        drooping_plane_z(px(7.0)),
        drooping_plane_z(px(9.0)),
        drooping_plane_x(px(7.0)),
        drooping_plane_x(px(9.0)),
    ]);
    shape
}

pub(crate) fn sculk_sensor_shape(name: &str) -> ModelShape {
    let calibrated = name == "calibrated_sculk_sensor";
    // Base slab 16 x 8 x 16 with per-face art (JE `sculk_sensor`): the four sides sample the
    // BOTTOM half of the side tile (uv 0,8..16), the calibrated sensor paints its south face
    // with the input-side sheet, and the caps use their own tiles. `blocks.json` has no
    // sculk_sensor entry, so the slot names are resolved explicitly in `obj.rs`.
    let side_uv = flipped_uv(0.0, 8.0, 16.0, 16.0);
    let cap_uv = crate::model_family::shape::full_texture_uv();
    let base = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(8.0), 1.0])
        .with_face_uv(BlockFace::Up, cap_uv)
        .with_face_uv(BlockFace::Down, cap_uv)
        .with_face_uv(BlockFace::North, side_uv)
        .with_face_uv(BlockFace::South, side_uv)
        .with_face_uv(BlockFace::East, side_uv)
        .with_face_uv(BlockFace::West, side_uv)
        .with_face_material_slot(BlockFace::Up, "sculk_top")
        .with_face_material_slot(BlockFace::Down, "sculk_bottom")
        .with_face_material_slot(BlockFace::North, "sculk_side")
        .with_face_material_slot(BlockFace::East, "sculk_side")
        .with_face_material_slot(BlockFace::West, "sculk_side")
        .with_face_material_slot(
            BlockFace::South,
            if calibrated {
                "sculk_input_side"
            } else {
                "sculk_side"
            },
        );
    let mut shape = ModelShape::from_cuboids([base]);

    // Tendrils (capture refs 470/471 normal, 603/604/605 calibrated; JE `sculk_sensor`): four
    // vertical diagonal sheets at the cell corners, y 8..24 px (0.5..1.5), overhanging the
    // corners by 2.76 px. Each sheet samples the 8 x 8 window of `sculk_sensor_tendril_inactive`
    // (uv 4..12, 8..16) — sampling the block's own side tile stretched dark slabs across the
    // blades. The detail-face emitter mirrors every plane, so one sheet covers both sides.
    let lo = 0.5_f32;
    let hi = 1.5_f32;
    let inner = 0.4525_f32;
    let outer = 1.1725_f32;
    let tendril_uv = flipped_uv(4.0, 8.0, 12.0, 16.0);
    let tendril = |bx0: f32, bz0: f32, bx1: f32, bz1: f32, normal: [i32; 3]| {
        ModelPlane::new(
            [
                [bx0, lo, bz0],
                [bx1, lo, bz1],
                [bx1, hi, bz1],
                [bx0, hi, bz0],
            ],
            normal,
        )
        .with_uv(tendril_uv)
        .with_material_slot("sculk_tendrils")
    };
    shape.planes.extend([
        tendril(outer, inner, inner, outer, [1, 0, 1]),
        tendril(outer, 0.5475, inner, -0.1725, [1, 0, -1]),
        tendril(0.5475, -0.1725, -0.1725, 0.5475, [-1, 0, 1]),
        tendril(0.5475, 1.1725, -0.1725, 0.4525, [-1, 0, 1]),
    ]);
    if calibrated {
        // The amethyst cross on top: two crossed diagonal sheets sampling the amethyst sheet
        // (uv 0,4..16,16), y 8..24 px per the capture.
        let amethyst_uv = flipped_uv(0.0, 4.0, 16.0, 16.0);
        shape.planes.extend([
            ModelPlane::new(
                [
                    [0.86, hi, 0.14],
                    [0.14, hi, 0.86],
                    [0.14, lo, 0.86],
                    [0.86, lo, 0.14],
                ],
                [1, 0, 1],
            )
            .with_uv(amethyst_uv)
            .with_material_slot("sculk_amethyst"),
            ModelPlane::new(
                [
                    [0.86, hi, 0.86],
                    [0.14, hi, 0.14],
                    [0.14, lo, 0.14],
                    [0.86, lo, 0.86],
                ],
                [-1, 0, 1],
            )
            .with_uv(amethyst_uv)
            .with_material_slot("sculk_amethyst"),
        ]);
    }
    shape
}

/// One rect of a 16 x 16 sheet, emitted bottom-left first (the authored rect measures V from
/// the texture top).
fn flipped_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

pub(crate) fn sculk_shrieker_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(8.0), 1.0])),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(1.0), px(8.0), px(1.0)],
            [px(15.0), px(15.0), px(15.0)],
        )),
    ])
}

fn big_dripleaf_shape(state: &BlockStateQuery) -> ModelShape {
    let head = state_bool(state, "big_dripleaf_head").unwrap_or(false);
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), 0.0, px(7.0)],
        [px(9.0), px(if head { 13.0 } else { 16.0 }), px(9.0)],
    ))]);
    if head {
        shape.cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new([0.0, px(12.0), 0.0], [1.0, px(15.0), 1.0])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Side, "side"),
        ));
    }
    shape
}

/// Small dripleaf: a crossed stem in the lower cell and a leaf fan in the upper cell.
///
/// Measured from the capture (refs 455-462). The state's `cardinal_direction` names the side the
/// plant leans toward, and `upper_block_bit` selects which half of the plant this cell draws:
///
/// * Lower cell (455/457/459/461): the same two crossed cards, `0.375` wide and `y 0..1`, slid
///   into the facing quarter of the cell. The cards do **not** rotate — the capture keeps both
///   diagonal directions identical and only moves the footprint (`south` at `z 0.0625`, `north`
///   at `z 0.5625`, `east` at `x 0.0625`, `west` at `x 0.5625`).
/// * Upper cell (456/458/460/462): a leaf fan filling the facing half of the cell plus the stem
///   continuing down. The fan hangs `0.3125` below the floor and overhangs `0.25` past the cell on
///   the facing side, which is why the upper half leaves its own cell.
///
/// The previous shape drew a 2 px post in both halves and ignored `cardinal_direction`, so every
/// orientation looked the same and the upper half never had a leaf.
fn small_dripleaf_shape(state: &BlockStateQuery) -> ModelShape {
    let upper = state_bool(state, "upper_block_bit").unwrap_or(false);
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    if upper {
        upper_dripleaf_leaf(direction)
    } else {
        lower_dripleaf_stem(direction)
    }
}

/// How far a small-dripleaf cell's geometry slides toward the side it faces.
///
/// The capture moves the footprint by a quarter cell without rotating it, so this is a plain
/// offset in `(x, z)` rather than a turn.
fn dripleaf_offset(direction: CardinalDirection) -> (f32, f32) {
    match direction {
        CardinalDirection::South => (0.0, 0.0),
        CardinalDirection::North => (0.0, 0.5),
        CardinalDirection::East => (-0.25, 0.25),
        CardinalDirection::West => (0.25, 0.25),
    }
}

/// The crossed stem cards of a lower small-dripleaf cell.
fn lower_dripleaf_stem(direction: CardinalDirection) -> ModelShape {
    let (offset_x, offset_z) = dripleaf_offset(direction);
    let cards = [
        (
            [
                [0.3125, 0.0, 0.0625],
                [0.6875, 0.0, 0.4375],
                [0.6875, 1.0, 0.4375],
                [0.3125, 1.0, 0.0625],
            ],
            [-1, 0, 1],
        ),
        (
            [
                [0.6875, 0.0, 0.0625],
                [0.3125, 0.0, 0.4375],
                [0.3125, 1.0, 0.4375],
                [0.6875, 1.0, 0.0625],
            ],
            [1, 0, 1],
        ),
    ];
    ModelShape::default().with_planes(cards.map(|(mut corners, normal)| {
        for corner in &mut corners {
            corner[0] += offset_x;
            corner[2] += offset_z;
        }
        ModelPlane::new(corners, normal).with_uv(upright_full_texture_uv())
    }))
}

/// The leaf fan of an upper small-dripleaf cell.
///
/// The capture's fan is a raised outer slab (`x 0.5..1`, `y 0.375..0.875`, `z 0.25..0.75`), a
/// drooping inner slab that hangs below the cell (`x 0.5..1`, `y -0.3125..0.1875`,
/// `z -0.25..0.25`), a quarter-cell ramp (`x 0..0.5`, `y 0..0.5`, `z 0.25..0.75`) and the crossed
/// stem cards running through the whole cell.
fn upper_dripleaf_leaf(direction: CardinalDirection) -> ModelShape {
    let (offset_x, offset_z) = dripleaf_offset(direction);
    let raised = ModelCuboid::new(
        [0.5 + offset_x, px(6.0), 0.25 + offset_z],
        [1.0 + offset_x, px(14.0), 0.75 + offset_z],
    );
    let drooping = ModelCuboid::new(
        [0.5 + offset_x, -0.3125, -0.25 + offset_z],
        [1.0 + offset_x, px(3.0), 0.25 + offset_z],
    );
    let ramp = ModelCuboid::new(
        [offset_x, 0.0, 0.25 + offset_z],
        [0.5 + offset_x, 0.5, 0.75 + offset_z],
    );
    let mut shape = ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(raised),
        detail_cuboid_with_local_uv(drooping),
        detail_cuboid_with_local_uv(ramp),
    ]);
    shape
        .planes
        .extend(lower_dripleaf_stem(direction).planes);
    shape
}

fn drooping_plane_z(z: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [0.0, px(15.7), z],
            [1.0, px(15.7), z],
            [1.0, px(8.0), px(8.0)],
            [0.0, px(8.0), px(8.0)],
        ],
        [0, -1, 1],
    )
    .with_uv(full_texture_uv())
}

fn drooping_plane_x(x: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [x, px(15.7), 0.0],
            [x, px(15.7), 1.0],
            [px(8.0), px(8.0), 1.0],
            [px(8.0), px(8.0), 0.0],
        ],
        [1, -1, 0],
    )
    .with_uv(full_texture_uv())
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
