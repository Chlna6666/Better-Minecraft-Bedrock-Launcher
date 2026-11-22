use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, rect_uv, rotate_shape_cardinal, rotate_shape_y_degrees,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_standing_sign")
        || name.ends_with("_wall_sign")
        || name.ends_with("_hanging_sign")
        || matches!(name, "standing_sign" | "wall_sign" | "hanging_sign")
    {
        return Some(ModelFamily::Sign);
    }
    if name.ends_with("_bed") || name == "bed" {
        return Some(ModelFamily::Bed);
    }
    if name.ends_with("_banner") || matches!(name, "banner" | "standing_banner" | "wall_banner") {
        return Some(ModelFamily::Banner);
    }
    if name == "bell" {
        return Some(ModelFamily::Bell);
    }
    None
}

pub(crate) fn sign_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.ends_with("_wall_sign") || name == "wall_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([sign_board_uv(wall_sign_board_cuboid(direction))]);
    }

    if name.ends_with("_wall_hanging_sign") || name == "wall_hanging_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([hanging_sign_board_uv(wall_hanging_sign_board_cuboid(
            direction,
        ))]);
    }

    if name.ends_with("_hanging_sign") || name == "hanging_sign" {
        // Official JE `template_hanging_sign_rot_{0..3}` / `template_attached_hanging_sign_rot_0`
        // (mcmeta@assets), with UVs in the 16x16 hanging-sign sheet (Bedrock's
        // `textures/ui/hanging_sign.png` is also 16x16, so the JE windows map directly —
        // the old `sign_uv()` used a 64x32 divisor and shrank every window to a quarter).
        // `ground_sign_direction` 0..15 maps to rot_N (value % 4) plus a 90-degree step
        // per group of four, exactly like the blockstate's 4 models x 4 yaw variants.
        let value = crate::model_family::direction::state_i64(state, "ground_sign_direction")
            .unwrap_or(0)
            .rem_euclid(16);
        let attached = crate::model_family::direction::state_bool(state, "attached_bit")
            .or_else(|| crate::model_family::direction::state_bool(state, "attached"))
            .unwrap_or(false);
        let uv16 = |u0: f32, v0: f32, u1: f32, v1: f32| {
            crate::model_family::shape::rect_uv(u0 / 16.0, v1 / 16.0, u1 / 16.0, v0 / 16.0)
        };
        // Board: [1,0,7]-[15,10,9], exact per-face windows from the template.
        let board = ModelCuboid::new([px(1.0), 0.0, px(7.0)], [px(15.0), px(10.0), px(9.0)])
            .with_face_material_slot(BlockFace::North, "front")
            .with_face_material_slot(BlockFace::South, "front")
            .with_face_material_slot(BlockFace::East, "side")
            .with_face_material_slot(BlockFace::West, "side")
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_uv(BlockFace::North, uv16(9.0, 8.0, 16.0, 13.0))
            .with_face_uv(BlockFace::East, uv16(8.0, 8.0, 9.0, 13.0))
            .with_face_uv(BlockFace::South, uv16(1.0, 8.0, 8.0, 13.0))
            .with_face_uv(BlockFace::West, uv16(0.0, 8.0, 1.0, 13.0))
            .with_face_uv(BlockFace::Up, uv16(1.0, 7.0, 8.0, 8.0))
            .with_face_uv(BlockFace::Down, uv16(1.0, 13.0, 8.0, 14.0));
        let mut cuboids = vec![board];
        if attached {
            // Attached: one zero-thickness crossbar [2,10,8]-[14,16,8], the ∧ chain
            // folded into a bar. Both faces sample the chain band.
            let bar = ModelPlane::new(
                [
                    [px(2.0), px(10.0), 0.5],
                    [px(14.0), px(10.0), 0.5],
                    [px(14.0), 1.0, 0.5],
                    [px(2.0), 1.0, 0.5],
                ],
                [0, 0, 1],
            )
            .with_material_slot("chain")
            .with_uv(uv16(10.0, 0.0, 16.0, 3.0));
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(2.0), px(10.0), px(7.75)],
                [px(14.0), 1.0, px(8.25)],
            )));
            let shape = ModelShape::from_cuboids(cuboids).with_planes([bar]);
            return rotate_shape_y_degrees(shape, value as f32 * 22.5);
        }
        // Hanging: four chains at the board's top corners. The template rotates each
        // chain element around (8,0,8) by the model's local angle; we emit the same four
        // narrow quads with the two official chain windows.
        let long = uv16(11.0, 3.5, 12.5, 6.5);
        let short = uv16(14.0, 4.0, 15.5, 6.0);
        for (x, uv) in [(2.96447_f32, short), (10.03553, long)] {
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(x), px(10.0), px(4.46447)],
                [px(x + 3.0), 1.0, px(4.56447)],
            )).with_face_uv(BlockFace::North, uv).with_face_uv(BlockFace::South, uv));
        }
        let shape = ModelShape::from_cuboids(cuboids);
        return rotate_shape_y_degrees(shape, value as f32 * 22.5);
    }

    // Standing signs have sixteen 22.5 degree orientations. Value 0 faces south and the value
    // grows clockwise seen from above, so the authored south-facing board just turns.
    let degrees = ground_sign_degrees(state);
    rotate_shape_y_degrees(
        ModelShape::from_cuboids([
            sign_board_uv(ModelCuboid::new(
                [px(0.25), px(8.25), px(7.25)],
                [px(15.75), 1.0, px(8.75)],
            )),
            sign_post_uv(ModelCuboid::new(
                [px(7.25), 0.0, px(7.25)],
                [px(8.75), px(8.25), px(8.75)],
            )),
        ]),
        degrees,
    )
}

/// `ground_sign_direction` (0..15) as degrees clockwise from the south-facing authored model.
fn ground_sign_degrees(state: &BlockStateQuery) -> f32 {
    let value = crate::model_family::direction::state_i64(state, "ground_sign_direction")
        .unwrap_or(0)
        .rem_euclid(16);
    value as f32 * 22.5
}

fn sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 0.0, 24.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 0.0, 24.0, 2.0))
}

fn sign_post_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::Side, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 16.0, 2.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 16.0, 2.0, 2.0))
}

fn hanging_sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 12.0, 14.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(16.0, 12.0, 14.0, 2.0))
}

fn hanging_sign_chain_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("chain")
        .with_face_uv(BlockFace::South, sign_uv(0.0, 6.0, 6.0, 6.0))
        .with_face_uv(BlockFace::North, sign_uv(6.0, 6.0, 6.0, 6.0))
}

fn hanging_sign_bar_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("bar")
        .with_face_uv(BlockFace::North, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::South, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::Up, sign_uv(4.0, 0.0, 16.0, 4.0))
        .with_face_uv(BlockFace::Down, sign_uv(20.0, 0.0, 16.0, 4.0))
}

fn wall_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(0.25), px(4.25), 0.0], [px(15.75), px(12.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(0.25), px(4.25), px(14.5)], [px(15.75), px(12.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), px(4.25), px(0.25)], [1.0, px(12.0), px(15.75)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, px(4.25), px(0.25)], [px(1.5), px(12.0), px(15.75)])
        }
    }
}

fn wall_hanging_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(1.0), 0.0, 0.0], [px(15.0), px(10.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(1.0), 0.0, px(14.5)], [px(15.0), px(10.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), 0.0, px(1.0)], [1.0, px(10.0), px(15.0)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, 0.0, px(1.0)], [px(1.5), px(10.0), px(15.0)])
        }
    }
}

fn sign_uv(u: f32, v: f32, width: f32, height: f32) -> [[f32; 2]; 4] {
    rect_uv(u / 64.0, v / 32.0, (u + width) / 64.0, (v + height) / 32.0)
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

/// Banners are ONE block each in Bedrock (`standing_banner` / `wall_banner`; there is no plain
/// `banner` block). The official entity model (JE `BannerModel` / `BannerFlagModel`, applied by
/// the renderer's `(0.5, 0, 0.5)` translation and `(2/3, -2/3, -2/3)` scale; the vanilla
/// Bedrock sheet paints exactly that box unwrap) stands UP from the ground: a 2 x 42 px pole
/// rises the full banner height, the 20 x 40 px cloth hangs from the 2 px bar capping the top,
/// and the banner reaches 1.83 blocks while its hitbox stays one block. Bedrock only ships two
/// forms — the standing banner (`ground_sign_direction`, 0 = south, 22.5 degrees per step
/// clockwise) and the wall banner (`facing_direction` 2..5 names the side the art faces; the
/// bar mounts on the opposite edge at mid height and the cloth hangs from it, dipping below
/// the block line). There is no compressed variant: the banner simply overlaps whatever shares
/// its space, which is why solid blocks may cover half of one.
pub(crate) fn banner_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.ends_with("wall_banner") {
        // `facing_direction` 0/1 never occurs in vanilla data; they fall through to the
        // default north facing like any unmapped state.
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        // The official wall assembly is authored facing south; turning model-north onto the
        // opposite edge points the cloth front onto `facing_direction`.
        return rotate_shape_cardinal(wall_banner_shape(), direction.opposite());
    }
    // Standing banner (and the legacy `banner` spelling): the official assembly authored
    // south-facing, turned by `ground_sign_direction`.
    rotate_shape_y_degrees(standing_banner_shape(), ground_sign_degrees(state))
}

pub(crate) fn bell_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let attachment = crate::model_family::direction::state_string(state, "attachment")
        .unwrap_or("standing")
        .trim();
    // `bell_side` is one unfolded piece: a 6 x 7 px body column at `(1, 0)..(7, 7)` over an
    // 8 x 2 px skirt at `(0, 7)..(8, 9)`, while `bell_top` / `bell_bottom` are 8 x 8 squares in
    // the tile corner. Sampling the whole 16 x 16 tile would shrink the bell to a quarter of
    // each face and open transparent gutters around it.
    let body_side = bell_uv(1.0, 0.0, 7.0, 7.0);
    let skirt_side = bell_uv(0.0, 7.0, 8.0, 9.0);
    let cap = bell_uv(0.0, 0.0, 8.0, 8.0);
    let body = ModelCuboid::new([px(5.0), px(6.0), px(5.0)], [px(11.0), px(13.0), px(11.0)])
        .with_face_uv(BlockFace::North, body_side)
        .with_face_uv(BlockFace::South, body_side)
        .with_face_uv(BlockFace::East, body_side)
        .with_face_uv(BlockFace::West, body_side)
        .with_face_uv(BlockFace::Up, bell_uv(1.0, 1.0, 7.0, 7.0))
        .with_face_uv(BlockFace::Down, cap)
        // `blocks.json` only maps north/south to `bell_side`: east/west are the wooden bar and
        // the stone posts, so the metal sides have to name their slot explicitly.
        .with_face_material_slot(BlockFace::East, "north")
        .with_face_material_slot(BlockFace::West, "north")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down");
    let skirt = ModelCuboid::new([px(4.0), px(4.0), px(4.0)], [px(12.0), px(6.0), px(12.0)])
        .with_face_uv(BlockFace::North, skirt_side)
        .with_face_uv(BlockFace::South, skirt_side)
        .with_face_uv(BlockFace::East, skirt_side)
        .with_face_uv(BlockFace::West, skirt_side)
        .with_face_uv(BlockFace::Up, cap)
        .with_face_uv(BlockFace::Down, cap)
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "north")
        .with_face_material_slot(BlockFace::East, "north")
        .with_face_material_slot(BlockFace::West, "north")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down");

    // Frames follow the Java bell models: bar `[2,13,7]..[14,15,9]` on dark oak planks, posts
    // `[0/14,0,6]..[2/16,16,10]` on the stone slot, a stub for ceilings and one bracket for
    // walls (floor posts double as the between-walls frame).
    let bar = |min: [f32; 3], max: [f32; 3]| ModelCuboid::new(min, max).with_material_slot("east");
    let post = |min: [f32; 3], max: [f32; 3]| ModelCuboid::new(min, max).with_material_slot("west");
    let mut cuboids = vec![body, skirt];
    match attachment {
        "hanging" => cuboids.push(bar([px(7.0), px(13.0), px(7.0)], [px(9.0), 1.0, px(9.0)])),
        "side" => cuboids.push(bar([px(3.0), px(13.0), px(7.0)], [1.0, px(15.0), px(9.0)])),
        "multiple" => cuboids.push(bar([0.0, px(13.0), px(7.0)], [1.0, px(15.0), px(9.0)])),
        _ => {
            cuboids.push(bar(
                [px(2.0), px(13.0), px(7.0)],
                [px(14.0), px(15.0), px(9.0)],
            ));
            cuboids.push(post([0.0, 0.0, px(6.0)], [px(2.0), 1.0, px(10.0)]));
            cuboids.push(post([px(14.0), 0.0, px(6.0)], [1.0, 1.0, px(10.0)]));
        }
    }
    rotate_shape_cardinal(ModelShape::from_cuboids(cuboids), direction)
}

/// One rect of a bell sheet: the art sits in the top-left corner of a 16 x 16 tile.
fn bell_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    // Authored rows measure V from the texture top; face corner 0 is the bottom-left one.
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

/// One model pixel of the banner entity at the official 2/3 block scale: the 44 px assembly
/// spans 44/24 blocks.
const fn banner_px(value: f32) -> f32 {
    value / 24.0
}

/// The official standing assembly — pole, cloth, top bar — authored facing south.
fn standing_banner_shape() -> ModelShape {
    ModelShape::from_cuboids([
        banner_pole_cuboid(),
        banner_flag_cuboid(
            banner_px(4.0),
            banner_px(44.0),
            0.5 + banner_px(1.0),
            0.5 + banner_px(2.0),
        ),
        banner_bar_cuboid(
            banner_px(42.0),
            banner_px(44.0),
            0.5 - banner_px(1.0),
            0.5 + banner_px(1.0),
        ),
    ])
}

/// The official wall assembly — cloth hanging from the mount bar — authored facing south; the
/// caller turns model-north onto the wall side. The cloth hangs from the bar at mid height and
/// dips 19.5 model px below the block line, the official wall-banner silhouette.
fn wall_banner_shape() -> ModelShape {
    ModelShape::from_cuboids([
        banner_flag_cuboid(
            -banner_px(19.5),
            banner_px(20.5),
            0.5 - banner_px(9.5),
            0.5 - banner_px(8.5),
        ),
        banner_bar_cuboid(
            banner_px(18.5),
            banner_px(20.5),
            0.5 - banner_px(11.5),
            0.5 - banner_px(9.5),
        ),
    ])
}

/// The 2 x 42 px pole rising from the ground to the top bar. The vanilla sheet paints every
/// box-UV window of the official pole cube (`texOffs(44, 0)`); the renderer's Y/Z mirror swaps
/// which world face reads each window, so the south face reads the north window and the up cap
/// reads the down cap.
fn banner_pole_cuboid() -> ModelCuboid {
    ModelCuboid::new(
        [0.5 - banner_px(1.0), 0.0, 0.5 - banner_px(1.0)],
        [0.5 + banner_px(1.0), banner_px(42.0), 0.5 + banner_px(1.0)],
    )
    .with_material_slot("front")
    .with_face_uv(BlockFace::South, banner_sheet(46.0, 2.0, 48.0, 44.0))
    .with_face_uv(BlockFace::North, banner_sheet(50.0, 2.0, 52.0, 44.0))
    .with_face_uv(BlockFace::East, banner_sheet(48.0, 2.0, 50.0, 44.0))
    .with_face_uv(BlockFace::West, banner_sheet(44.0, 2.0, 46.0, 44.0))
    .with_face_uv(BlockFace::Up, banner_sheet(48.0, 0.0, 50.0, 2.0))
    .with_face_uv(BlockFace::Down, banner_sheet(46.0, 0.0, 48.0, 2.0))
}

/// The 20 x 40 px cloth panel: the front art on the south face and the shaded back panel on
/// the north face, with the official 1 px edge and cap windows on the rest. Every window is
/// painted in the vanilla sheet; the panel art reads un-mirrored from outside both faces.
fn banner_flag_cuboid(y0: f32, y1: f32, z0: f32, z1: f32) -> ModelCuboid {
    ModelCuboid::new([0.5 - banner_px(10.0), y0, z0], [0.5 + banner_px(10.0), y1, z1])
        .with_material_slot("front")
        .with_face_uv(BlockFace::South, banner_sheet(1.0, 1.0, 21.0, 41.0))
        .with_face_uv(BlockFace::North, banner_sheet(22.0, 1.0, 42.0, 41.0))
        .with_face_uv(BlockFace::East, banner_sheet(21.0, 1.0, 22.0, 41.0))
        .with_face_uv(BlockFace::West, banner_sheet(0.0, 1.0, 1.0, 41.0))
        .with_face_uv(BlockFace::Up, banner_sheet(21.0, 0.0, 41.0, 1.0))
        .with_face_uv(BlockFace::Down, banner_sheet(1.0, 0.0, 21.0, 1.0))
}

/// The 20 x 2 x 2 px bar capping the banner — the top of the standing form, the mount bar of
/// the wall form. Official cube windows from `texOffs(0, 42)`, mirrored like the pole's.
fn banner_bar_cuboid(y0: f32, y1: f32, z0: f32, z1: f32) -> ModelCuboid {
    ModelCuboid::new([0.5 - banner_px(10.0), y0, z0], [0.5 + banner_px(10.0), y1, z1])
        .with_material_slot("front")
        .with_face_uv(BlockFace::South, banner_sheet(2.0, 44.0, 22.0, 46.0))
        .with_face_uv(BlockFace::North, banner_sheet(24.0, 44.0, 44.0, 46.0))
        .with_face_uv(BlockFace::East, banner_sheet(22.0, 44.0, 24.0, 46.0))
        .with_face_uv(BlockFace::West, banner_sheet(0.0, 44.0, 2.0, 46.0))
        .with_face_uv(BlockFace::Up, banner_sheet(22.0, 42.0, 42.0, 44.0))
        .with_face_uv(BlockFace::Down, banner_sheet(2.0, 42.0, 22.0, 44.0))
}

/// One rect of the 64 x 64 banner entity sheet, emitted bottom-left first (the authored rect
/// measures V from the texture top).
fn banner_sheet(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 64.0, v1 / 64.0],
        [u1 / 64.0, v1 / 64.0],
        [u1 / 64.0, v0 / 64.0],
        [u0 / 64.0, v0 / 64.0],
    ]
}
