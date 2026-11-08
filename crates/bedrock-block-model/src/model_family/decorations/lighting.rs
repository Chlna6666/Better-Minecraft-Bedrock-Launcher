use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, block_face, cardinal_direction, state_bool, state_i64,
};
use crate::model_family::shape::{ModelCuboid, ModelShape};
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

/// Vanilla `template_torch`: a 2x10 px post plus two full-texture cross sheets, not a thin box.
///
/// Mapping the whole 16x16 torch texture onto a 2 px box face squeezes it into a sliver, which
/// is why torches rendered as a thin line. The cross sheets carry the actual flame artwork.
pub(crate) fn torch_shape(state: &BlockStateQuery) -> ModelShape {
    let face = block_face(state);
    if face.is_some_and(|face| face == "wall") {
        // Wall torches sit against one face; keep the cross sheets but push them to the wall
        // and lift the base, which matches the vanilla silhouette without an arbitrary tilt.
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        let (min_x, max_x, min_z, max_z) = match direction {
            CardinalDirection::North => (7.0, 9.0, 0.0, 9.0),
            CardinalDirection::South => (7.0, 9.0, 7.0, 16.0),
            CardinalDirection::West => (0.0, 9.0, 7.0, 9.0),
            CardinalDirection::East => (7.0, 16.0, 7.0, 9.0),
        };
        return ModelShape::from_cuboids([
            torch_post_cuboid(min_x, 3.0, max_x, 13.0, min_z, max_z),
            torch_sheet_cuboid(true, 3.0, 16.0, 7.0, 9.0),
            torch_sheet_cuboid(false, 3.0, 16.0, 7.0, 9.0),
        ]);
    }
    if face.is_some_and(|face| face == "ceiling") {
        // Ceiling torches hang upside down from the block above.
        return ModelShape::from_cuboids([
            torch_post_cuboid(7.0, 6.0, 9.0, 10.0, 7.0, 9.0),
            torch_sheet_cuboid(true, 6.0, 16.0, 7.0, 9.0),
            torch_sheet_cuboid(false, 6.0, 16.0, 7.0, 9.0),
        ]);
    }
    ModelShape::from_cuboids([
        torch_post_cuboid(7.0, 0.0, 9.0, 10.0, 7.0, 9.0),
        torch_sheet_cuboid(true, 0.0, 16.0, 7.0, 9.0),
        torch_sheet_cuboid(false, 0.0, 16.0, 7.0, 9.0),
    ])
}

fn torch_post_cuboid(
    min_x: f32,
    y0: f32,
    max_x: f32,
    y1: f32,
    min_z: f32,
    max_z: f32,
) -> ModelCuboid {
    // Only the top and bottom sample the torch texture; the 2x2 px sides are transparent.
    ModelCuboid::new(
        [min_x / 16.0, y0 / 16.0, min_z / 16.0],
        [max_x / 16.0, y1 / 16.0, max_z / 16.0],
    )
    .with_face_uv(BlockFace::Up, java_face_uv(true, [7.0, 6.0, 9.0, 8.0]))
    .with_face_uv(BlockFace::Down, java_face_uv(true, [7.0, 13.0, 9.0, 15.0]))
}

fn torch_sheet_cuboid(
    along_z: bool,
    y0: f32,
    y1: f32,
    thin_min: f32,
    thin_max: f32,
) -> ModelCuboid {
    let (min, max) = if along_z {
        (
            [thin_min / 16.0, y0 / 16.0, 0.0],
            [thin_max / 16.0, y1 / 16.0, 1.0],
        )
    } else {
        (
            [0.0, y0 / 16.0, thin_min / 16.0],
            [1.0, y1 / 16.0, thin_max / 16.0],
        )
    };
    let cuboid = ModelCuboid::new(min, max);
    let full = java_face_uv(false, [0.0, 0.0, 16.0, 16.0]);
    if along_z {
        cuboid
            .with_face_uv(BlockFace::West, full)
            .with_face_uv(BlockFace::East, full)
    } else {
        cuboid
            .with_face_uv(BlockFace::North, full)
            .with_face_uv(BlockFace::South, full)
    }
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

fn wall_torch_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.40625, 0.25, 0.0], [0.59375, 0.875, 0.5]),
        CardinalDirection::South => ModelCuboid::new([0.40625, 0.25, 0.5], [0.59375, 0.875, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5, 0.25, 0.40625], [1.0, 0.875, 0.59375]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.25, 0.40625], [0.5, 0.875, 0.59375]),
    }
}
