use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{state_bool, state_i64, state_string};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv,
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
        "chorus_plant" => Some(ModelFamily::ChorusPlant),
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
    // Tilted 22.5 degrees about the block's middle; the sheet spans 0.3..0.7 at the bottom and
    // widens to 0.1..0.9 at the top (measured from the captured mesh).
    let (low_bottom, low_top) = (0.3_f32, 0.9);
    let (high_bottom, high_top) = (0.7_f32, 0.1);
    let top = 1.4_f32;
    let uv = super::super::shape::upright_full_texture_uv();
    let mut planes = Vec::with_capacity(4);
    for (bottom, tip, normal, on_x) in [
        (low_bottom, low_top, [0, 0, 1], false),
        (high_bottom, high_top, [0, 0, -1], false),
        (low_bottom, low_top, [1, 0, 0], true),
        (high_bottom, high_top, [-1, 0, 0], true),
    ] {
        let corners = if on_x {
            [
                [bottom, 0.0, 0.0],
                [bottom, 0.0, 1.0],
                [tip, top, 1.0],
                [tip, top, 0.0],
            ]
        } else {
            [
                [0.0, 0.0, bottom],
                [1.0, 0.0, bottom],
                [1.0, top, tip],
                [0.0, top, tip],
            ]
        };
        planes.push(
            crate::model_family::shape::ModelPlane::new(corners, normal)
                .front_only()
                .with_material_slot("side")
                .with_uv(uv),
        );
    }
    ModelShape::default().with_planes(planes)
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

pub(crate) fn chorus_plant_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(4.0), 0.0, px(4.0)],
            [px(12.0), 1.0, px(12.0)],
        )),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(3.0), px(4.0), px(4.0)],
            [px(13.0), px(12.0), px(12.0)],
        )),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(5.0), px(5.0), px(2.0)],
            [px(11.0), px(11.0), px(14.0)],
        )),
    ])
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

pub(crate) fn mangrove_propagule_shape(state: &BlockStateQuery) -> ModelShape {
    if !state_bool(state, "hanging").unwrap_or(false) {
        // Standing propagules grow upwards, so their art must stay upright; the generic
        // `diagonal_plane_between` default points it down for hanging shapes.
        let uv = crate::model_family::shape::upright_full_texture_uv();
        return ModelShape::default().with_planes([
            diagonal_plane_between(0.0, 1.0, true, None).with_uv(uv),
            diagonal_plane_between(0.0, 1.0, false, None).with_uv(uv),
        ]);
    }

    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), px(12.0), px(7.0)],
        [px(9.0), px(16.0), px(9.0)],
    ))]);
    // A hanging propagule points its bud downwards, so its art is flipped.
    let uv = crate::model_family::shape::hanging_full_texture_uv();
    shape.planes.extend([
        diagonal_plane_between(px(4.0), px(14.0), true, None).with_uv(uv),
        diagonal_plane_between(px(4.0), px(14.0), false, None).with_uv(uv),
    ]);
    shape
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
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, px(8.0), 1.0],
    ))]);
    shape.planes.extend([
        tendril_plane_z(px(2.0)),
        tendril_plane_z(px(14.0)),
        tendril_plane_x(px(2.0)),
        tendril_plane_x(px(14.0)),
    ]);
    if name == "calibrated_sculk_sensor" {
        shape.planes.extend([
            vertical_plane_z(px(8.0), "south"),
            vertical_plane_x(px(8.0), "south"),
        ]);
    }
    shape
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

fn small_dripleaf_shape(state: &BlockStateQuery) -> ModelShape {
    let upper = state_bool(state, "upper_block_bit").unwrap_or(false);
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), 0.0, px(7.0)],
        [px(9.0), 1.0, px(9.0)],
    ))]);
    if upper {
        shape.cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new([px(1.0), px(12.0), px(1.0)], [px(15.0), px(14.0), px(15.0)])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Side, "side"),
        ));
    }
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

fn diagonal_plane_between(
    bottom: f32,
    top: f32,
    forward: bool,
    material_slot: Option<&'static str>,
) -> ModelPlane {
    let plane = if forward {
        ModelPlane::new(
            [
                [0.0, bottom, 0.0],
                [1.0, bottom, 1.0],
                [1.0, top, 1.0],
                [0.0, top, 0.0],
            ],
            [-1, 0, 1],
        )
    } else {
        ModelPlane::new(
            [
                [1.0, bottom, 0.0],
                [0.0, bottom, 1.0],
                [0.0, top, 1.0],
                [1.0, top, 0.0],
            ],
            [1, 0, 1],
        )
    }
    .with_uv(full_texture_uv());
    if let Some(slot) = material_slot {
        plane.with_material_slot(slot)
    } else {
        plane
    }
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
    .with_uv(full_texture_uv())
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
    .with_uv(full_texture_uv())
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
