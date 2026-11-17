use crate::geometry::BlockGeometry;
use crate::material::BlockFace;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{ModelPlane, ModelShape, box_unwrap_uv};
use crate::state::BlockStateQuery;

/// The official `geometry.bed` identifier from the pack's `models/mobs.json`.
pub const GEOMETRY_IDENTIFIER: &str = "geometry.bed";

/// Bed art is the 64x64 entity sprite `textures/entity/bed/<colour>`.
const BED_TEXTURE_SPAN: f32 = 64.0;
/// Model pixels per block.
const PX: f32 = 16.0;
/// Height of the laid-flat bed (mattress top).
const BED_TOP_PX: f32 = 9.0;
/// One bed half along the model's length axis.
const HALF_LENGTH_PX: f32 = 16.0;

/// Fallback bed used only when the pack ships no `geometry.bed`.
pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    build_bed(state, default_bed_cubes()).unwrap_or_default()
}

/// Converts the pack's `geometry.bed` entity model into this block's half of the bed.
///
/// `geometry.bed` is authored standing on end: `+Y` runs the bed's 32 px length and `+Z` its
/// thickness (mattress 0..6, legs 6..9). The block-entity renderer lays it flat, mapping a model
/// point `(x, y, z)` to block space as `x/16`, `(9 - z)/16`, `y/16`. Each bed block draws one half
/// of that length, so cubes are clipped at 16 px before the transform.
pub fn shape_from_geometry(
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
    let is_head = is_head(state);
    let direction = direction(state);
    let slot = bed_material_slot(state);
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
        push_cube_planes(
            min,
            max,
            uv,
            half_low,
            half_high,
            direction,
            &slot,
            &mut planes,
        );
    }
    (!planes.is_empty()).then_some(ModelShape {
        cuboids: Vec::new(),
        planes,
    })
}

/// The material slot a bed's planes bind.
///
/// Bedrock names the block `bed` for **every** colour — the dye lives in the `Bed` block entity's
/// `color` field, not in the block name — so deriving the sprite from the name (the old
/// `bed_colour` suffix strip) always fell back to white and every bed rendered white. The colour
/// therefore travels in the slot name, the same way banner dye does: `bed` keeps the default and
/// `bed_<colour>` selects one of the 16 `textures/entity/bed/<colour>` sprites. A legacy block
/// name that *does* carry the colour (`white_bed`, `red_bed`, …) still works.
fn bed_material_slot(state: &BlockStateQuery) -> String {
    let name = state
        .name
        .strip_prefix("minecraft:")
        .unwrap_or(&state.name);
    let from_name = name
        .strip_suffix("_bed")
        .filter(|colour| !colour.is_empty() && *colour != "bed");
    // A colour parsed from the block-entity NBT is attached to the query by the caller.
    let from_state = crate::model_family::direction::state_string(state, "bed_colour")
        .or_else(|| crate::model_family::direction::state_string(state, "color"));
    match from_state.or(from_name) {
        Some(colour) if is_bed_colour(colour) => format!("bed_{colour}"),
        _ => "bed".to_owned(),
    }
}

/// The 16 vanilla bed colours, which are also the `textures/entity/bed/` sprite names.
fn is_bed_colour(colour: &str) -> bool {
    matches!(
        colour,
        "white"
            | "orange"
            | "magenta"
            | "light_blue"
            | "yellow"
            | "lime"
            | "pink"
            | "gray"
            | "light_gray"
            | "cyan"
            | "purple"
            | "blue"
            | "brown"
            | "green"
            | "red"
            | "black"
    )
}

fn is_head(state: &BlockStateQuery) -> bool {
    state_bool(state, "head_piece_bit")
        .or_else(|| {
            crate::model_family::direction::state_string(state, "bed_part").map(|s| s == "head")
        })
        .unwrap_or(true)
}

/// Bed `direction` is an integer 0..=3; other spellings cover pack variants.
fn direction(state: &BlockStateQuery) -> CardinalDirection {
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
    slot: &str,
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
        // The bed's four **vertical** sides in world space are the model's East/West faces.
        //
        // `point_to_block` maps model `z` to world `Y`, so the model's `x`-normal faces (East and
        // West) become the upright sides, while the model's `z`-normal faces (North and South)
        // become the horizontal caps. The box unwrap gives East/West
        // `v = depth + (y1 - model_y)` — a V that follows the model's **length** (32 px) — but
        // that face is only `depth` (6 px) tall in world space, so the art is squashed by
        // `32 / depth` and the 1-texel frame rail at `v 6..7` covers a sixth of the side instead
        // of a sixteenth. That is what made the brown frame read as too big and in the wrong
        // place. Remap V onto the unwrap's own side band, `v0 + depth .. v0 + 2*depth`, using the
        // model `z` that actually became the world height.
        if matches!(face, BlockFace::East | BlockFace::West) {
            let depth = max[2] - min[2];
            let band_start = uv[1] + depth;
            for (index, corner) in clipped.iter().enumerate() {
                // model z 0 is the top (world Y highest), model z = depth is the underside.
                let height = (corner[2] - min[2]) / depth.max(1e-6);
                uvs[index][1] = (band_start + height * depth) / BED_TEXTURE_SPAN;
            }
        }
        let block = clipped.map(|point| point_to_block(point, half_low));
        let block = rotate_corners(block, direction);
        let normal = plane_normal(block);
        planes.push(
            ModelPlane::new(block, normal)
                .with_material_slot(slot)
                .with_uv(uvs),
        );
    }
}

/// Box-unwrap UV of one model-space point, normalised to the sprite.
///
/// Verified against the shipped 64x64 `textures/entity/bed/white.png`. With the mattress cube
/// `[0,0,0]..[16,32,6]` at `uv [0,0]` the unwrap puts:
///
/// * the **mattress top** (model -Z) on the white-sheet strip `u 6..22, v 6..38`,
/// * the **underside** (model +Z) on the all-brown frame `u 28..44, v 6..38`,
/// * the two **long sides** (model +-X) on `u 22..28` / `u 0..6`, whose outer columns are the
///   frame rail.
///
/// `point_to_block` maps model `-Z` to world **up**, so the sheet lands on top and the frame
/// underneath. The mapping is correct as authored; rewriting the V axis (an earlier attempt)
/// only rotated the sides and collapsed them to a line.
fn model_uv(
    face: BlockFace,
    uv: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
    point: [f32; 3],
) -> [f32; 2] {
    let [u, v] = box_unwrap_uv(face, uv, min, max, point);
    [u / BED_TEXTURE_SPAN, v / BED_TEXTURE_SPAN]
}

/// Model point to bed block space: lay flat, then put this half inside the cell.
fn point_to_block(point: [f32; 3], half_low: f32) -> [f32; 3] {
    let [x, y, z] = point;
    [x / PX, (BED_TOP_PX - z) / PX, (y - half_low) / PX]
}

/// The unit axis normal of a planar quad.
///
/// Two separate bugs lived here:
///
/// * Taking the sign of **each** cross-product component independently turned every zero
///   component into `+1`, producing normals like `[1, 1, -1]`. Those are not unit vectors, so the
///   renderer's face-shading lookup missed them and fell back to one flat shade.
/// * The renderer emits faces whose corner order gives `cross(c1 - c0, c3 - c0) == -normal` (see
///   `push_cuboid_faces`'s Up face), so the dominant component has to be **negated**. Without
///   that every bed face pointed into the bed, which flipped its shading and let backface culling
///   drop it.
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
    let axis = (0..3)
        .max_by(|left, right| {
            cross[*left]
                .abs()
                .partial_cmp(&cross[*right].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1);
    let mut normal = [0i32; 3];
    // The face emitter's corner order is opposite to its normal, so flip the sign here.
    normal[axis] = if cross[axis] >= 0.0 { -1 } else { 1 };
    normal
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
