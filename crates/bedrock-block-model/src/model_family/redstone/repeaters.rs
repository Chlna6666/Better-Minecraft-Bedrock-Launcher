use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, rotate_cuboid_cardinal, rotate_plane_cardinal,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("repeater") || name.contains("comparator") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let is_comparator = name.contains("comparator");

    // Official block state (minecraft.wiki/w/Redstone_Repeater):
    // `cardinal_direction` "points from the output side to the input side", so
    // the output faces the opposite direction. The frame below is the official
    // model (JE `repeater_<n>tick` / `comparator`), whose output faces
    // model-north; the whole assembly turns model-north onto the output.
    let facing = repeater_facing_direction(state);
    let output = facing.opposite();

    let powered = (name.contains("powered") && !name.contains("unpowered"))
        || state_bool(state, "powered_bit").unwrap_or(false)
        || state_bool(state, "powered").unwrap_or(false)
        || state_bool(state, "output_lit_bit").unwrap_or(false);
    // The comparator's subtract mode lights the front (output) post only; the
    // two input posts light with the powered output (JE `comparator_subtract`
    // binds the front torch to `#lit` while the inputs stay `#unlit`).
    let is_subtract = is_comparator
        && (state_bool(state, "output_subtract_bit").unwrap_or(false)
            || state_string(state, "comparator_mode")
                .is_some_and(|mode| mode == "subtract" || mode == "1"));

    let top_slot = if powered { "top_on" } else { "top_off" };
    let floor_slot = if is_comparator {
        "comparator_floor"
    } else {
        "repeater_floor"
    };

    // Base slab: the top carries the mechanism art (`repeater_up` /
    // `comparator_up`, two-frame [off, on]); the sides show the bottom 2 px
    // band of the stone slab and the underside the whole slab tile.
    let slab_band = flipped_uv(0.0, 14.0, 16.0, 16.0);
    let mut cuboids = vec![
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])
            .with_face_uv(BlockFace::Up, flipped_uv(0.0, 0.0, 16.0, 16.0))
            .with_face_uv(BlockFace::Down, flipped_uv(0.0, 0.0, 16.0, 16.0))
            .with_face_uv(BlockFace::North, slab_band)
            .with_face_uv(BlockFace::South, slab_band)
            .with_face_uv(BlockFace::East, slab_band)
            .with_face_uv(BlockFace::West, slab_band)
            .with_face_material_slot(BlockFace::Up, top_slot)
            .with_face_material_slot(BlockFace::Down, floor_slot)
            .with_face_material_slot(BlockFace::North, floor_slot)
            .with_face_material_slot(BlockFace::South, floor_slot)
            .with_face_material_slot(BlockFace::East, floor_slot)
            .with_face_material_slot(BlockFace::West, floor_slot),
    ];
    let mut planes = Vec::new();

    // Torch posts (official models, authored in 16 px units): every post is a
    // 2 x ? x 2 column standing on the base slab. The repeater's output post
    // sits at the output edge (z 2..4) and the delay post slides 2 px per
    // delay step away from it; the comparator has a short output post (3 px)
    // flanked by the two 5 px input posts at the back. Lit posts also grow the
    // official glow cross around themselves.
    let mut push_post = |cuboid: ModelCuboid, lit: bool, side_uv_v1: f32| {
        let x0 = cuboid.min[0];
        let x1 = cuboid.max[0];
        let z0 = cuboid.min[2];
        let z1 = cuboid.max[2];
        let y_top = cuboid.max[1];
        let slot = if lit { "torch_on" } else { "torch_off" };
        let mut post = cuboid.with_material_slot(slot);
        let side = torch_slice_uv(7.0, 6.0, 9.0, side_uv_v1);
        let head = torch_slice_uv(7.0, 6.0, 9.0, 8.0);
        for (face, uv) in [
            (BlockFace::North, side),
            (BlockFace::South, side),
            (BlockFace::East, side),
            (BlockFace::West, side),
            (BlockFace::Up, head),
        ] {
            post = post.with_face_uv(face, uv);
        }
        cuboids.push(post);
        if lit {
            planes.extend(torch_glow_planes(x0, x1, y_top, z0, z1));
        }
    };

    if is_comparator {
        let back_lit = powered;
        let front_lit = is_subtract;
        // Output post (3 px tall): (7,2,2)-(9,5,4), side window [7,6,9,9].
        push_post(
            ModelCuboid::new([px(7.0), px(2.0), px(2.0)], [px(9.0), px(5.0), px(4.0)]),
            front_lit,
            9.0,
        );
        // Input posts (5 px tall): (4,2,11)-(6,7,13) and (10,2,11)-(12,7,13).
        push_post(
            ModelCuboid::new([px(4.0), px(2.0), px(11.0)], [px(6.0), px(7.0), px(13.0)]),
            back_lit,
            11.0,
        );
        push_post(
            ModelCuboid::new([px(10.0), px(2.0), px(11.0)], [px(12.0), px(7.0), px(13.0)]),
            back_lit,
            11.0,
        );
    } else {
        let tick = state_i64(state, "repeater_delay")
            .or_else(|| state_i64(state, "delay"))
            .unwrap_or(0)
            .clamp(0, 3);
        // Output post fixed at the output edge; the delay post starts 4 px
        // away and slides 2 px per delay step (1tick z 6..8 .. 4tick z 12..14).
        push_post(
            ModelCuboid::new([px(7.0), px(2.0), px(2.0)], [px(9.0), px(7.0), px(4.0)]),
            powered,
            11.0,
        );
        let delay_z0 = px(6.0 + 2.0 * tick as f32);
        push_post(
            ModelCuboid::new(
                [px(7.0), px(2.0), delay_z0],
                [px(9.0), px(7.0), delay_z0 + px(2.0)],
            ),
            powered,
            11.0,
        );
    }

    let rotated_cuboids = cuboids
        .iter()
        .map(|cuboid| rotate_cuboid_cardinal(cuboid, output))
        .collect::<Vec<_>>();
    let rotated_planes = planes
        .iter()
        .map(|plane| rotate_plane_cardinal(plane, output))
        .collect::<Vec<_>>();
    ModelShape {
        cuboids: rotated_cuboids,
        planes: rotated_planes,
    }
}

/// The official lit-torch glow cross (JE lit models bind six 3 x 3 single-face
/// quads one half-pixel outside the post). Each quad samples a 1 px window of
/// the lit torch art.
fn torch_glow_planes(x0: f32, x1: f32, y_top: f32, z0: f32, z1: f32) -> Vec<ModelPlane> {
    let (u0, u1) = (x0 - px(0.5), x1 + px(0.5));
    let (w0, w1) = (z0 - px(0.5), z1 + px(0.5));
    let mid_up = y_top - px(2.5);
    let glow_top = y_top + px(0.5);
    let glow_bottom = y_top - px(2.5);
    let slot = "torch_on";
    let plane = |corners: [[f32; 3]; 4], normal: [i32; 3], uv: [f32; 4]| {
        ModelPlane::new(corners, normal)
            .with_material_slot(slot)
            .with_uv(torch_slice_uv(uv[0], uv[1], uv[2], uv[3]))
    };
    vec![
        // Horizontal glow quads: one crossing the post's height, one above it.
        plane(
            [
                [u0, mid_up, w0],
                [u1, mid_up, w0],
                [u1, mid_up, w1],
                [u0, mid_up, w1],
            ],
            [0, 1, 0],
            [8.0, 5.0, 9.0, 6.0],
        ),
        plane(
            [
                [u0, glow_top, w1],
                [u1, glow_top, w1],
                [u1, glow_top, w0],
                [u0, glow_top, w0],
            ],
            [0, -1, 0],
            [7.0, 5.0, 8.0, 6.0],
        ),
        // Four vertical quads wrapping the post, each half a pixel outside.
        plane(
            [
                [u1, glow_bottom, w0],
                [u0, glow_bottom, w0],
                [u0, glow_top, w0],
                [u1, glow_top, w0],
            ],
            [0, 0, 1],
            [9.0, 6.0, 10.0, 7.0],
        ),
        plane(
            [
                [u0, glow_bottom, w1],
                [u1, glow_bottom, w1],
                [u1, glow_top, w1],
                [u0, glow_top, w1],
            ],
            [0, 0, -1],
            [6.0, 6.0, 7.0, 7.0],
        ),
        plane(
            [
                [u0, glow_bottom, w1],
                [u0, glow_bottom, w0],
                [u0, glow_top, w0],
                [u0, glow_top, w1],
            ],
            [1, 0, 0],
            [9.0, 7.0, 10.0, 8.0],
        ),
        plane(
            [
                [u1, glow_bottom, w0],
                [u1, glow_bottom, w1],
                [u1, glow_top, w1],
                [u1, glow_top, w0],
            ],
            [-1, 0, 0],
            [6.0, 7.0, 7.0, 8.0],
        ),
    ]
}

/// A flipped sprite slice: bottom-left corner first, matching the renderer's face order while
/// the authored rect measures V downwards.
fn torch_slice_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

/// Resolves the side the repeater's state points towards: from the output side
/// to the input side (minecraft.wiki/w/Redstone_Repeater). The deprecated
/// integer state uses the legacy data order 0=south / 1=west / 2=north /
/// 3=east, which `cardinal_direction` already applies.
fn repeater_facing_direction(state: &BlockStateQuery) -> CardinalDirection {
    cardinal_direction(state).unwrap_or(CardinalDirection::South)
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

