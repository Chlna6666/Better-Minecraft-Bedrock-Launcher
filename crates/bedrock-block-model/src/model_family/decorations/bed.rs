use crate::geometry::BlockGeometry;
use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{ModelPlane, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name == "bed" || name.ends_with("_bed") {
        Some(ModelFamily::Bed)
    } else {
        None
    }
}

/// The official `geometry.bed` identifier from the pack's `models/mobs.json`.
pub const BED_GEOMETRY_IDENTIFIER: &str = "geometry.bed";

/// Bed art is the 64x64 entity sprite `textures/entity/bed/<colour>`.
const BED_TEXTURE_SPAN: f32 = 64.0;
/// Model pixels per block.
const PX: f32 = 16.0;
/// Height of the laid-flat bed (mattress top).
const BED_TOP_PX: f32 = 9.0;
/// One bed half along the model's length axis.
const HALF_LENGTH_PX: f32 = 16.0;

/// Fallback bed used only when the pack ships no `geometry.bed`.
pub(crate) fn bed_shape(state: &BlockStateQuery) -> ModelShape {
    build_bed(state, default_bed_cubes()).unwrap_or_default()
}

/// Converts the pack's `geometry.bed` entity model into this block's half of the bed.
///
/// `geometry.bed` is authored standing on end: `+Y` runs the bed's 32 px length and `+Z` its
/// thickness (mattress 0..6, legs 6..9). The block-entity renderer lays it flat, mapping a model
/// point `(x, y, z)` to block space as `x/16`, `(9 - z)/16`, `y/16`. Each bed block draws one half
/// of that length, so cubes are clipped at 16 px before the transform.
pub fn bed_shape_from_geometry(
    geometry: &BlockGeometry,
    state: &BlockStateQuery,
) -> Option<ModelShape> {
    build_bed(state, geometry_cubes(geometry))
}

fn geometry_cubes(geometry: &BlockGeometry) -> Vec<([f32; 3], [f32; 3], [f32; 2])> {
    let mut cubes = Vec::new();
    for bone in &geometry.bones {
        for cube in &bone.cubes {
            let (Some(origin), Some(size)) = (cube.origin, cube.size) else {
                continue;
            };
            let uv = cube
                .uv
                .as_ref()
                .and_then(|uv| uv.raw.as_array())
                .map(|raw| {
                    [
                        raw.first()
                            .and_then(serde_json::Value::as_f64)
                            .map_or(0.0, |v| v as f32),
                        raw.get(1)
                            .and_then(serde_json::Value::as_f64)
                            .map_or(0.0, |v| v as f32),
                    ]
                })
                .unwrap_or([0.0, 0.0]);
            cubes.push((
                origin,
                [
                    origin[0] + size[0],
                    origin[1] + size[1],
                    origin[2] + size[2],
                ],
                uv,
            ));
        }
    }
    cubes
}

fn build_bed(
    state: &BlockStateQuery,
    cubes: Vec<([f32; 3], [f32; 3], [f32; 2])>,
) -> Option<ModelShape> {
    let is_head = bed_is_head(state);
    let direction = bed_direction(state);
    let (half_low, half_high) = if is_head {
        (HALF_LENGTH_PX, 2.0 * HALF_LENGTH_PX)
    } else {
        (0.0, HALF_LENGTH_PX)
    };

    let mut planes = Vec::new();
    for (min, max, uv) in cubes {
        if max[1] <= half_low || min[1] >= half_high {
            continue;
        }
        push_cube_planes(min, max, uv, half_low, half_high, direction, &mut planes);
    }
    (!planes.is_empty()).then_some(ModelShape {
        cuboids: Vec::new(),
        planes,
    })
}

fn bed_is_head(state: &BlockStateQuery) -> bool {
    state_bool(state, "head_piece_bit")
        .or_else(|| {
            crate::model_family::direction::state_string(state, "bed_part").map(|s| s == "head")
        })
        .unwrap_or(true)
}

/// Bed `direction` is an integer 0..=3; other spellings cover pack variants.
fn bed_direction(state: &BlockStateQuery) -> CardinalDirection {
    if let Some(value) = crate::model_family::direction::state_i64(state, "direction") {
        return match value & 3 {
            0 => CardinalDirection::South,
            1 => CardinalDirection::West,
            2 => CardinalDirection::North,
            _ => CardinalDirection::East,
        };
    }
    cardinal_direction(state).unwrap_or(CardinalDirection::South)
}

/// Emits the six faces of one authored cube, clipped to this block's half of the bed.
fn push_cube_planes(
    min: [f32; 3],
    max: [f32; 3],
    uv: [f32; 2],
    half_low: f32,
    half_high: f32,
    direction: CardinalDirection,
    planes: &mut Vec<ModelPlane>,
) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let low = y0.max(half_low);
    let high = y1.min(half_high);
    if high <= low {
        return;
    }

    // Model-space faces with the official box unwrap. `v_along_y` marks the four long faces
    // whose V axis is the bed's length, so their UVs follow the clip.
    let faces = [
        (
            BlockFace::Up,
            [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            false,
        ),
        (
            BlockFace::Down,
            [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
            false,
        ),
        (
            BlockFace::North,
            [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
            true,
        ),
        (
            BlockFace::South,
            [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
            true,
        ),
        (
            BlockFace::East,
            [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
            true,
        ),
        (
            BlockFace::West,
            [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
            true,
        ),
    ];

    for (face, corners, v_along_y) in faces {
        // Up/Down are the bed's end caps: each sits at one length, so it belongs to exactly one
        // half. Keeping the far cap made the head block stretch two blocks along the bed.
        if !v_along_y {
            let cap_y = if face == BlockFace::Up { y1 } else { y0 };
            if cap_y < half_low || cap_y > half_high {
                continue;
            }
        }
        let mut clipped = corners;
        let mut uvs = [[0.0f32; 2]; 4];
        for (index, corner) in clipped.iter_mut().enumerate() {
            corner[1] = corner[1].clamp(low, high);
            uvs[index] = model_uv(face, uv, min, max, *corner);
        }
        let block = clipped.map(|point| bed_point_to_block(point, half_low));
        let block = rotate_corners(block, direction);
        let normal = plane_normal(block);
        planes.push(
            ModelPlane::new(block, normal)
                .with_material_slot("bed")
                .with_uv(uvs),
        );
    }
}

/// Box-unwrap UV of one model-space point, normalised to the sprite.
fn model_uv(
    face: BlockFace,
    uv: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
    point: [f32; 3],
) -> [f32; 2] {
    let [u0, v0] = uv;
    let [x0, _, z0] = min;
    let [x1, y1, z1] = max;
    let width = x1 - x0;
    let depth = z1 - z0;
    let (u, v) = match face {
        // +Y end cap: u across X, v across Z.
        BlockFace::Up => (u0 + depth + (point[0] - x0), v0 + (point[2] - z0)),
        // -Y end cap, mirrored in Z.
        BlockFace::Down => (u0 + depth + width + (point[0] - x0), v0 + (z1 - point[2])),
        // -Z long face: u across X, v along the length measured from the far end.
        BlockFace::North => (u0 + depth + (point[0] - x0), v0 + depth + (y1 - point[1])),
        // +Z long face: u mirrored in X.
        BlockFace::South => (
            u0 + depth + width + depth + (x1 - point[0]),
            v0 + depth + (y1 - point[1]),
        ),
        // +X long face: u runs from the bed's underside up to the mattress top, matching the
        // sprite's wood frame at the bottom edge.
        BlockFace::East => (
            u0 + depth + width + (point[2] - z0),
            v0 + depth + (y1 - point[1]),
        ),
        // -X long face: same direction as the east side.
        _ => (u0 + (z1 - point[2]), v0 + depth + (y1 - point[1])),
    };
    [u / BED_TEXTURE_SPAN, v / BED_TEXTURE_SPAN]
}

/// Model point to bed block space: lay flat, then put this half inside the cell.
fn bed_point_to_block(point: [f32; 3], half_low: f32) -> [f32; 3] {
    let [x, y, z] = point;
    [x / PX, (BED_TOP_PX - z) / PX, (y - half_low) / PX]
}

fn plane_normal(corners: [[f32; 3]; 4]) -> [i32; 3] {
    let a = [
        corners[1][0] - corners[0][0],
        corners[1][1] - corners[0][1],
        corners[1][2] - corners[0][2],
    ];
    let b = [
        corners[3][0] - corners[0][0],
        corners[3][1] - corners[0][1],
        corners[3][2] - corners[0][2],
    ];
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    [
        if cross[0] >= 0.0 { 1 } else { -1 },
        if cross[1] >= 0.0 { 1 } else { -1 },
        if cross[2] >= 0.0 { 1 } else { -1 },
    ]
}

fn rotate_corners(corners: [[f32; 3]; 4], direction: CardinalDirection) -> [[f32; 3]; 4] {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    let mut corners = corners;
    for _ in 0..turns {
        corners = corners.map(|[x, y, z]| [z, y, 1.0 - x]);
    }
    corners
}

/// Official `geometry.bed` cubes in authoring pixels, used when a pack ships no geometry.
fn default_bed_cubes() -> Vec<([f32; 3], [f32; 3], [f32; 2])> {
    vec![
        ([0.0, 0.0, 0.0], [16.0, 32.0, 6.0], [0.0, 0.0]),
        ([3.0, 31.0, 6.0], [13.0, 32.0, 9.0], [38.0, 2.0]),
        ([3.0, 0.0, 6.0], [13.0, 1.0, 9.0], [38.0, 38.0]),
        ([15.0, 3.0, 6.0], [16.0, 29.0, 9.0], [52.0, 6.0]),
        ([0.0, 3.0, 6.0], [1.0, 29.0, 9.0], [44.0, 6.0]),
        ([13.0, 29.0, 6.0], [16.0, 32.0, 9.0], [12.0, 38.0]),
        ([0.0, 29.0, 6.0], [3.0, 32.0, 9.0], [0.0, 38.0]),
        ([13.0, 0.0, 6.0], [16.0, 3.0, 9.0], [12.0, 44.0]),
        ([0.0, 0.0, 6.0], [3.0, 3.0, 9.0], [0.0, 44.0]),
    ]
}
