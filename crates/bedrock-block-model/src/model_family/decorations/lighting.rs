use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, block_face, cardinal_direction, state_bool, state_i64,
};
use crate::model_family::shape::{ModelCuboid, ModelPlane, ModelShape, rotate_plane_cardinal};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_torch")
        || matches!(name, "torch" | "redstone_torch" | "unlit_redstone_torch")
    {
        return Some(ModelFamily::Torch);
    }
    if name.ends_with("_lantern") || name == "lantern" {
        return Some(ModelFamily::Lantern);
    }
    if name.ends_with("_candle") || name == "candle" {
        return Some(ModelFamily::Candle);
    }
    if matches!(name, "portal" | "nether_portal" | "end_portal") {
        return Some(ModelFamily::Portal);
    }
    None
}

/// Vanilla torch geometry sampled from the Bedrock 1.26.60.5 client through
/// BedrockSceneCapture: one 2 x 2 x 9.9 px post that samples only the torch column of its
/// texture (u 7.1..8.9 px) instead of stretching the whole 16x16 tile onto a sliver. A wall
/// torch is the same post tilted 22.5 degrees so its base sits on the mount face and its tip
/// leans into the cell.
const TORCH_COLUMN_U: [f32; 2] = [0.44390106, 0.55609894];
const TORCH_SIDE_V: [f32; 2] = [0.37820053, 0.99679947];
const TORCH_UP_V: [f32; 2] = [0.37820053, 0.5];
const TORCH_DOWN_V: [f32; 2] = [0.81570053, 0.93429947];

/// Captured `torch_facing_direction = "north"` planes: normal, corners, UVs, in block space.
const WALL_TORCH_PLANES: [([i32; 3], [[f32; 3]; 4], [[f32; 2]; 4]); 6] = [
    (
        [-1, 0, 0],
        [
            [0.4375, 0.81875, 0.1844],
            [0.4375, 0.2, -0.0631],
            [0.4375, 0.2, 0.0619],
            [0.4375, 0.81875, 0.3094],
        ],
        [
            [0.44390106, 0.37820053],
            [0.44390106, 0.99679947],
            [0.55609894, 0.99679947],
            [0.55609894, 0.37820053],
        ],
    ),
    (
        [0, 0, -1],
        [
            [0.4375, 0.2, -0.0631],
            [0.4375, 0.81875, 0.1844],
            [0.5625, 0.81875, 0.1844],
            [0.5625, 0.2, -0.0631],
        ],
        [
            [0.55609894, 0.99679947],
            [0.55609894, 0.37820053],
            [0.44390106, 0.37820053],
            [0.44390106, 0.99679947],
        ],
    ),
    (
        [0, -1, 0],
        [
            [0.4375, 0.2, -0.0631],
            [0.5625, 0.2, -0.0631],
            [0.5625, 0.2, 0.0619],
            [0.4375, 0.2, 0.0619],
        ],
        [
            [0.44390106, 0.81570053],
            [0.55609894, 0.81570053],
            [0.55609894, 0.93429947],
            [0.44390106, 0.93429947],
        ],
    ),
    (
        [0, 0, 1],
        [
            [0.4375, 0.81875, 0.3094],
            [0.4375, 0.2, 0.0619],
            [0.5625, 0.2, 0.0619],
            [0.5625, 0.81875, 0.3094],
        ],
        [
            [0.44390106, 0.37820053],
            [0.44390106, 0.99679947],
            [0.55609894, 0.99679947],
            [0.55609894, 0.37820053],
        ],
    ),
    (
        [0, 1, 0],
        [
            [0.4375, 0.81875, 0.185],
            [0.4375, 0.81875, 0.31],
            [0.5625, 0.81875, 0.31],
            [0.5625, 0.81875, 0.185],
        ],
        [
            [0.44390106, 0.37820053],
            [0.44390106, 0.5],
            [0.55609894, 0.5],
            [0.55609894, 0.37820053],
        ],
    ),
    (
        [1, 0, 0],
        [
            [0.5625, 0.2, -0.0631],
            [0.5625, 0.81875, 0.1844],
            [0.5625, 0.81875, 0.3094],
            [0.5625, 0.2, 0.0619],
        ],
        [
            [0.55609894, 0.99679947],
            [0.55609894, 0.37820053],
            [0.44390106, 0.37820053],
            [0.44390106, 0.99679947],
        ],
    ),
];

pub(crate) fn torch_shape(state: &BlockStateQuery) -> ModelShape {
    match torch_mount(state) {
        TorchMount::Wall(direction) => ModelShape::default().with_planes(
            WALL_TORCH_PLANES
                .iter()
                .map(|(normal, corners, uv)| {
                    rotate_plane_cardinal(
                        &ModelPlane::new(*corners, *normal).with_uv(*uv),
                        direction,
                    )
                })
                .collect::<Vec<_>>(),
        ),
        TorchMount::Ceiling => {
            // No vanilla torch hangs from a ceiling; keep the post mirrored for pack states.
            ModelShape::from_cuboids([torch_post_cuboid(0.38125, 1.0)])
        }
        TorchMount::Floor => ModelShape::from_cuboids([torch_post_cuboid(0.0, 0.61875)]),
    }
}

enum TorchMount {
    Floor,
    Ceiling,
    Wall(CardinalDirection),
}

/// Resolves where a torch is mounted from its state.
///
/// Bedrock spells the mount in `torch_facing_direction`: `top` stands on the block below, and
/// the four cardinal names mount the torch against the wall on that side. The value names the
/// side *towards* the supporting block, not the direction the flame points.
fn torch_mount(state: &BlockStateQuery) -> TorchMount {
    let Some(face) = block_face(state) else {
        return TorchMount::Floor;
    };
    match face.trim() {
        "top" | "up" => TorchMount::Floor,
        "bottom" | "down" | "ceiling" => TorchMount::Ceiling,
        _ => cardinal_direction(state).map_or(TorchMount::Floor, TorchMount::Wall),
    }
}

fn torch_post_cuboid(y0: f32, y1: f32) -> ModelCuboid {
    let side = uv_rect(TORCH_COLUMN_U, TORCH_SIDE_V);
    ModelCuboid::new([0.4375, y0, 0.4375], [0.5625, y1, 0.5625])
        .with_face_uv(BlockFace::Up, uv_rect(TORCH_COLUMN_U, TORCH_UP_V))
        .with_face_uv(BlockFace::Down, uv_rect(TORCH_COLUMN_U, TORCH_DOWN_V))
        .with_face_uv(BlockFace::North, side)
        .with_face_uv(BlockFace::South, side)
        .with_face_uv(BlockFace::East, side)
        .with_face_uv(BlockFace::West, side)
}

fn uv_rect(u: [f32; 2], v: [f32; 2]) -> [[f32; 2]; 4] {
    [[u[0], v[1]], [u[1], v[1]], [u[1], v[0]], [u[0], v[0]]]
}

pub(crate) fn lantern_shape(state: &BlockStateQuery) -> ModelShape {
    let hanging = state_bool(state, "hanging").unwrap_or(false);
    // Vanilla `template_lantern` / `template_hanging_lantern`: the texture is an unfolded
    // lantern, so every face samples its own rectangle instead of a block-space projection.
    let (body_min_y, body_max_y) = if hanging { (1.0, 8.0) } else { (0.0, 7.0) };
    let body = ModelCuboid::new(
        [5.0 / 16.0, body_min_y / 16.0, 5.0 / 16.0],
        [11.0 / 16.0, body_max_y / 16.0, 11.0 / 16.0],
    );
    let body = with_java_uvs(body, [0.0, 9.0, 6.0, 15.0], [0.0, 2.0, 6.0, 9.0]);
    let top = ModelCuboid::new(
        [6.0 / 16.0, body_max_y / 16.0, 6.0 / 16.0],
        [10.0 / 16.0, (body_max_y + 2.0) / 16.0, 10.0 / 16.0],
    );
    let top = with_java_uvs(top, [1.0, 10.0, 5.0, 14.0], [1.0, 0.0, 5.0, 2.0]);
    let (handle_min_y, handle_max_y) = if hanging { (11.0, 15.0) } else { (9.0, 11.0) };
    let handle_uv = if hanging {
        [11.0, 1.0, 14.0, 5.0]
    } else {
        [11.0, 1.0, 14.0, 3.0]
    };
    ModelShape::from_cuboids([body, top]).with_planes([
        lantern_handle_plane(handle_min_y, handle_max_y, true, handle_uv),
        lantern_handle_plane(handle_min_y, handle_max_y, false, handle_uv),
    ])
}

fn with_java_uvs(cuboid: ModelCuboid, top_bottom: [f32; 4], sides: [f32; 4]) -> ModelCuboid {
    cuboid
        .with_face_uv(BlockFace::Up, java_face_uv(true, top_bottom))
        .with_face_uv(BlockFace::Down, java_face_uv(true, top_bottom))
        .with_face_uv(BlockFace::North, java_face_uv(false, sides))
        .with_face_uv(BlockFace::South, java_face_uv(false, sides))
        .with_face_uv(BlockFace::East, java_face_uv(false, sides))
        .with_face_uv(BlockFace::West, java_face_uv(false, sides))
}

/// Converts a Java model `uv: [u1, v1, u2, v2]` rectangle (0..16, v from the texture top)
/// into the renderer's per-corner order.
fn java_face_uv(top_bottom: bool, rect: [f32; 4]) -> [[f32; 2]; 4] {
    let [u1, v1, u2, v2] = rect.map(|value| value / 16.0);
    if top_bottom {
        [[u1, v1], [u2, v1], [u2, v2], [u1, v2]]
    } else {
        [[u2, v2], [u1, v2], [u1, v1], [u2, v1]]
    }
}

fn lantern_handle_plane(
    min_y: f32,
    max_y: f32,
    forward: bool,
    uv: [f32; 4],
) -> crate::model_family::shape::ModelPlane {
    // Two 3x2 px vertical plates rotated 45 degrees around the block centre (the hanging
    // handle loop of `template_lantern`).
    let reach = 1.5 * std::f32::consts::FRAC_1_SQRT_2 / 16.0;
    let (start, end) = if forward {
        ((-reach, reach), (reach, -reach))
    } else {
        ((-reach, -reach), (reach, reach))
    };
    let a = [0.5 + start.0, min_y / 16.0, 0.5 + start.1];
    let b = [0.5 + end.0, min_y / 16.0, 0.5 + end.1];
    let c = [b[0], max_y / 16.0, b[2]];
    let d = [a[0], max_y / 16.0, a[2]];
    crate::model_family::shape::ModelPlane::new([a, b, c, d], [0, 0, 1])
        .with_uv(java_face_uv(false, uv))
}

pub(crate) fn candle_shape(state: &BlockStateQuery) -> ModelShape {
    let candle_count = state_i64(state, "candles")
        .or_else(|| state_i64(state, "cluster_count"))
        .unwrap_or(1)
        .clamp(1, 4);
    let lit = state_bool(state, "lit").unwrap_or(false);
    // Vanilla `template_candle*` uses one unfolded texture rectangle per face, so every candle
    // must sample its own 2 px column instead of projecting the whole texture onto the box.
    let columns: &[[f32; 2]] = match candle_count {
        1 => &[[7.0, 7.0]],
        2 => &[[5.0, 7.0], [9.0, 6.0]],
        3 => &[[5.0, 5.0], [9.0, 8.0], [8.0, 5.0]],
        _ => &[[6.0, 8.0], [9.0, 8.0], [5.0, 5.0], [8.0, 5.0]],
    };
    let heights: &[f32] = match candle_count {
        1 => &[6.0],
        2 => &[5.0, 6.0],
        3 => &[5.0, 5.0, 6.0],
        _ => &[3.0, 5.0, 5.0, 6.0],
    };
    let mut cuboids = Vec::with_capacity(candle_count as usize);
    for (index, [x, z]) in columns.iter().enumerate() {
        let height = heights.get(index).copied().unwrap_or(6.0);
        let cuboid = ModelCuboid::new(
            [x / 16.0, 0.0, z / 16.0],
            [(x + 2.0) / 16.0, height / 16.0, (z + 2.0) / 16.0],
        );
        let sides = java_face_uv(false, [0.0, 8.0, 2.0, 8.0 + height]);
        cuboids.push(
            cuboid
                .with_face_uv(BlockFace::Up, java_face_uv(true, [0.0, 6.0, 2.0, 8.0]))
                .with_face_uv(BlockFace::Down, java_face_uv(true, [0.0, 14.0, 2.0, 16.0]))
                .with_face_uv(BlockFace::North, sides)
                .with_face_uv(BlockFace::South, sides)
                .with_face_uv(BlockFace::East, sides)
                .with_face_uv(BlockFace::West, sides),
        );
    }
    let mut shape = ModelShape::from_cuboids(cuboids);
    if lit {
        // The flame is a 1 px sprite above each candle, drawn as two crossed quads.
        for (index, [x, z]) in columns.iter().enumerate() {
            let height = heights.get(index).copied().unwrap_or(6.0);
            shape
                .planes
                .push(candle_flame_plane(*x + 1.0, *z + 1.0, height, true));
            shape
                .planes
                .push(candle_flame_plane(*x + 1.0, *z + 1.0, height, false));
        }
    }
    shape
}

fn candle_flame_plane(
    x: f32,
    z: f32,
    base_height: f32,
    forward: bool,
) -> crate::model_family::shape::ModelPlane {
    let reach = 0.5 * std::f32::consts::FRAC_1_SQRT_2 / 16.0;
    let (start, end) = if forward {
        ((-reach, reach), (reach, -reach))
    } else {
        ((-reach, -reach), (reach, reach))
    };
    let a = [x / 16.0 + start.0, base_height / 16.0, z / 16.0 + start.1];
    let b = [x / 16.0 + end.0, base_height / 16.0, z / 16.0 + end.1];
    let c = [b[0], (base_height + 1.0) / 16.0, b[2]];
    let d = [a[0], (base_height + 1.0) / 16.0, a[2]];
    crate::model_family::shape::ModelPlane::new([a, b, c, d], [0, 0, 1])
        .with_uv(java_face_uv(false, [0.0, 5.0, 1.0, 6.0]))
}

pub(crate) fn portal_shape(state: &BlockStateQuery) -> ModelShape {
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if name == "end_portal" {
        // The End portal's starfield surface sits at 3/4 block height; it is not the
        // side-facing Nether portal sheet. A full-footprint 12/16 box is the vanilla shape.
        return ModelShape::from_cuboids([
            crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
                [0.0, 0.0, 0.0],
                [1.0, 0.75, 1.0],
            )),
        ]);
    }
    let axis = crate::model_family::direction::state_string(state, "portal_axis")
        .or_else(|| crate::model_family::direction::state_string(state, "axis"))
        .or_else(|| crate::model_family::direction::state_string(state, "pillar_axis"))
        .unwrap_or("x");
    let thickness = 0.03125;
    let cuboid = if axis == "z" {
        ModelCuboid::new([0.5 - thickness, 0.0, 0.0], [0.5 + thickness, 1.0, 1.0])
    } else {
        ModelCuboid::new([0.0, 0.0, 0.5 - thickness], [1.0, 1.0, 0.5 + thickness])
    };
    ModelShape::from_cuboids([cuboid])
}
