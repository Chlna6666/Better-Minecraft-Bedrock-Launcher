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

/// Bedrock tripwire: two flat strips lying at `y = 0.2188`, one per horizontal axis.
///
/// Measured from the client: the strip is a zero-height quad at `y 0.2188`, `x 0.4844..0.5156`,
/// running the full cell. The old 1..2 px box sat too low and read as a solid bar. The wire only
/// runs along the axes its state connects, so a wire with no neighbours draws nothing.
pub(crate) fn tripwire_shape(state: &BlockStateQuery) -> ModelShape {
    let y = 0.2188_f32;
    let (lo, hi) = (0.4844_f32, 0.5156_f32);
    let along_x = state_bool(state, "minecraft:east")
        .or_else(|| state_bool(state, "east"))
        .unwrap_or(true)
        || state_bool(state, "minecraft:west")
            .or_else(|| state_bool(state, "west"))
            .unwrap_or(true);
    let along_z = state_bool(state, "minecraft:north")
        .or_else(|| state_bool(state, "north"))
        .unwrap_or(true)
        || state_bool(state, "minecraft:south")
            .or_else(|| state_bool(state, "south"))
            .unwrap_or(true);
    let mut planes = Vec::with_capacity(2);
    if along_z {
        planes.push(
            ModelPlane::new(
                [[lo, y, 0.0], [hi, y, 0.0], [hi, y, 1.0], [lo, y, 1.0]],
                [0, 1, 0],
            )
            .with_uv(upright_full_texture_uv()),
        );
    }
    if along_x {
        planes.push(
            ModelPlane::new(
                [[0.0, y, lo], [1.0, y, lo], [1.0, y, hi], [0.0, y, hi]],
                [0, 1, 0],
            )
            .with_uv(upright_full_texture_uv()),
        );
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
        return ModelShape::from_cuboids(hanging_propagule_cuboids().map(|cuboid| {
            plant_cuboid_with_upright_art(cuboid)
        }));
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

/// The hanging propagule (ref 772): stem, body, four collar plates and a top cap.
fn hanging_propagule_cuboids() -> [ModelCuboid; 7] {
    // The hanging seedling samples the sprite's own art regions: the long hypocotyl takes the
    // stem column (x 7..9, y 7..16), the seed body and the four collar plates take the sprout
    // (x 4..11, y 1..7), and the tip takes the sprout's top. Whole-tile UVs sampled mostly
    // transparent pixels and rendered the seedling as sparse ghost wisps.
    [
        plant_slice_cuboid(
            ModelCuboid::new([0.5725, 0.0, 0.3725], [0.6609, 0.625, 0.4609]),
            (7.0, 7.0, 9.0, 16.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.5542, 0.625, 0.3542], [0.6792, 0.8125, 0.4792]),
            (4.0, 1.0, 11.0, 7.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.5725, 0.875, 0.3725], [0.6609, 1.0, 0.4609]),
            (7.0, 1.0, 9.0, 7.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.4387, 0.7647, 0.3542], [0.5542, 0.8125, 0.4792]),
            (4.0, 1.0, 11.0, 7.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.6792, 0.7647, 0.3542], [0.7947, 0.8125, 0.4792]),
            (4.0, 1.0, 11.0, 7.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.5542, 0.7647, 0.2387], [0.6792, 0.8125, 0.3542]),
            (4.0, 1.0, 11.0, 7.0),
        ),
        plant_slice_cuboid(
            ModelCuboid::new([0.5542, 0.7647, 0.4792], [0.6792, 0.8125, 0.5947]),
            (4.0, 1.0, 11.0, 7.0),
        ),
    ]
}

/// Wraps a small plant box so every face samples an upright art slice of the sprite instead of
/// the whole (mostly transparent) tile.
fn plant_slice_cuboid(cuboid: ModelCuboid, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> ModelCuboid {
    use crate::model_family::shape::full_texture_face_uv;
    let upright = full_texture_face_uv(BlockFace::North)
        .iter()
        .map(|corner| {
            [
                u0 / 16.0 + corner[0] * (u1 - u0) / 16.0,
                v0 / 16.0 + corner[1] * (v1 - v0) / 16.0,
            ]
        })
        .collect::<Vec<_>>();
    let upright: [[f32; 2]; 4] = [upright[0], upright[1], upright[2], upright[3]];
    let flat = full_texture_uv()
        .iter()
        .map(|corner| {
            [
                u0 / 16.0 + corner[0] * (u1 - u0) / 16.0,
                v0 / 16.0 + corner[1] * (v1 - v0) / 16.0,
            ]
        })
        .collect::<Vec<_>>();
    let flat: [[f32; 2]; 4] = [flat[0], flat[1], flat[2], flat[3]];
    cuboid
        .with_face_uv(BlockFace::Up, flat)
        .with_face_uv(BlockFace::Down, flat)
        .with_face_uv(BlockFace::North, upright)
        .with_face_uv(BlockFace::South, upright)
        .with_face_uv(BlockFace::East, upright)
        .with_face_uv(BlockFace::West, upright)
}

/// Gives a box whole-tile art on its caps and upright art on its sides.
///
/// Plant textures are single tiles, so a side that sampled the tile top-down would show the
/// sprout hanging from the top of the face instead of standing on it.
fn plant_cuboid_with_upright_art(cuboid: ModelCuboid) -> ModelCuboid {
    use crate::model_family::shape::{full_texture_face_uv, full_texture_uv};
    cuboid
        .with_face_uv(BlockFace::Up, full_texture_uv())
        .with_face_uv(BlockFace::Down, full_texture_uv())
        .with_face_uv(BlockFace::North, full_texture_face_uv(BlockFace::North))
        .with_face_uv(BlockFace::South, full_texture_face_uv(BlockFace::South))
        .with_face_uv(BlockFace::East, full_texture_face_uv(BlockFace::East))
        .with_face_uv(BlockFace::West, full_texture_face_uv(BlockFace::West))
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

fn vertical_plane_z(z: f32, material_slot: &'static str) -> ModelPlane {
    ModelPlane::new(
        [[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
        [0, 0, 1],
    )
    .with_material_slot(material_slot)
    .with_uv(upright_full_texture_uv())
}

fn vertical_plane_x(x: f32, material_slot: &'static str) -> ModelPlane {
    ModelPlane::new(
        [[x, 0.0, 0.0], [x, 0.0, 1.0], [x, 1.0, 1.0], [x, 1.0, 0.0]],
        [1, 0, 0],
    )
    .with_material_slot(material_slot)
    .with_uv(upright_full_texture_uv())
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

fn tendril_plane_z(z: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [px(3.0), px(8.0), z],
            [px(13.0), px(8.0), z],
            [px(13.0), px(16.0), z],
            [px(3.0), px(16.0), z],
        ],
        [0, 0, 1],
    )
    .with_uv(upright_full_texture_uv())
}

fn tendril_plane_x(x: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [x, px(8.0), px(3.0)],
            [x, px(8.0), px(13.0)],
            [x, px(16.0), px(13.0)],
            [x, px(16.0), px(3.0)],
        ],
        [1, 0, 0],
    )
    .with_uv(upright_full_texture_uv())
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
