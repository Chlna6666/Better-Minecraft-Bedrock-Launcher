use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, rotate_cuboid_cardinal, upright_full_texture_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("campfire") {
        Some(ModelFamily::Campfire)
    } else {
        None
    }
}

/// One vanilla campfire box: pixel bounds plus the four vertex UVs of each face.
///
/// The table is the north-facing arrangement from the vanilla 26.30 model (as compiled by
/// cinnabar's `visuals::campfire`). Every other facing is this layout turned about the block
/// centre, so the UVs travel with the geometry.
struct CampfireFace {
    face: BlockFace,
    /// Authored rect in the 16 x 16 frame space, top row first.
    uv: [f32; 4],
    /// Whether this face samples the lit strip instead of the plain bark tile.
    lit: bool,
    /// Region of `campfire_log` the extinguished face shows.
    dark: CampfireDark,
}

/// Part of the plain `campfire_log` tile a face reads.
///
/// The 16 x 16 tile is one log cut up: rows 0..4 are the bark strip, `(0, 4)..(4, 8)` is the
/// cut end, rows 8..14 the ember bed and rows 15..16 its charred edge. The rest of rows 4..8 is
/// transparent black filler, so an authored rect that spans a full width there would sample
/// holes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CampfireDark {
    Bark,
    End,
    Ash,
    Char,
}

impl CampfireDark {
    /// Texture window of the `campfire_log` tile, as `[u0, v0, u1, v1]` in 16 px space.
    fn window(self) -> [f32; 4] {
        match self {
            Self::Bark => [0.0, 0.0, 16.0, 4.0],
            Self::End => [0.0, 4.0, 4.0, 8.0],
            Self::Ash => [0.0, 8.0, 16.0, 14.0],
            Self::Char => [0.0, 15.0, 16.0, 16.0],
        }
    }
}

struct CampfireBox {
    min: [f32; 3],
    max: [f32; 3],
    faces: &'static [CampfireFace],
}

const fn face(face: BlockFace, uv: [f32; 4], lit: bool, dark: CampfireDark) -> CampfireFace {
    CampfireFace {
        face,
        uv,
        lit,
        dark,
    }
}

/// Vanilla `template_campfire` elements and UVs, authored facing south.
///
/// `lit` follows the JE model's `#lit_log` faces — the ones that face the fire — and `dark`
/// says which window of the plain tile the same face shows once the campfire is out.
const CAMPFIRE_BOXES: [CampfireBox; 5] = {
    use BlockFace::{Down, East, North, South, Up, West};
    use CampfireDark::{Ash, Bark, Char, End};
    [
        CampfireBox {
            min: [1.0, 0.0, 0.0],
            max: [5.0, 4.0, 16.0],
            faces: &[
                face(North, [0.0, 4.0, 4.0, 8.0], false, End),
                face(East, [0.0, 1.0, 16.0, 5.0], true, Bark),
                face(South, [0.0, 4.0, 4.0, 8.0], false, End),
                face(West, [16.0, 0.0, 0.0, 4.0], false, Bark),
                face(Up, [0.0, 0.0, 16.0, 4.0], false, Bark),
                face(Down, [0.0, 0.0, 16.0, 4.0], false, Bark),
            ],
        },
        CampfireBox {
            min: [0.0, 3.0, 11.0],
            max: [16.0, 7.0, 15.0],
            faces: &[
                face(North, [16.0, 0.0, 0.0, 4.0], true, Bark),
                face(East, [0.0, 4.0, 4.0, 8.0], false, End),
                face(South, [0.0, 0.0, 16.0, 4.0], true, Bark),
                face(West, [0.0, 4.0, 4.0, 8.0], false, End),
                face(Up, [0.0, 0.0, 16.0, 4.0], false, Bark),
                face(Down, [0.0, 4.0, 16.0, 8.0], true, Bark),
            ],
        },
        CampfireBox {
            min: [11.0, 0.0, 0.0],
            max: [15.0, 4.0, 16.0],
            faces: &[
                face(North, [0.0, 4.0, 4.0, 8.0], false, End),
                face(East, [0.0, 0.0, 16.0, 4.0], false, Bark),
                face(South, [0.0, 4.0, 4.0, 8.0], false, End),
                face(West, [16.0, 1.0, 0.0, 5.0], true, Bark),
                face(Up, [0.0, 0.0, 16.0, 4.0], false, Bark),
                face(Down, [0.0, 0.0, 16.0, 4.0], false, Bark),
            ],
        },
        CampfireBox {
            min: [0.0, 3.0, 1.0],
            max: [16.0, 7.0, 5.0],
            faces: &[
                face(North, [0.0, 0.0, 16.0, 4.0], true, Bark),
                face(East, [0.0, 4.0, 4.0, 8.0], false, End),
                face(South, [16.0, 0.0, 0.0, 4.0], true, Bark),
                face(West, [0.0, 4.0, 4.0, 8.0], false, End),
                face(Up, [0.0, 0.0, 16.0, 4.0], false, Bark),
                face(Down, [0.0, 4.0, 16.0, 8.0], true, Bark),
            ],
        },
        CampfireBox {
            min: [5.0, 0.0, 0.0],
            max: [11.0, 1.0, 16.0],
            faces: &[
                face(North, [0.0, 15.0, 6.0, 16.0], false, Char),
                face(South, [10.0, 15.0, 16.0, 16.0], false, Char),
                face(Up, [0.0, 8.0, 16.0, 14.0], true, Ash),
                face(Down, [0.0, 8.0, 16.0, 14.0], false, Ash),
            ],
        },
    ]
};

/// Campfire logs, ember bed and the two crossed flame sheets.
///
/// `campfire_log_lit` is a 16 x 64 flipbook (four lit frames) and `campfire` is a 16 x 128
/// flipbook, so every UV stays inside one frame; the renderer picks the first frame.
pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let lit = state_bool(state, "extinguished")
        .map(|extinguished| !extinguished)
        .unwrap_or(true);
    // Soul campfires ship only the lit strip, so every face reads that texture.
    let strip_only = name.starts_with("soul");
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);

    let mut cuboids = Vec::with_capacity(5);
    for model in CAMPFIRE_BOXES {
        let mut cuboid = ModelCuboid::new(model.min.map(px), model.max.map(px));
        for entry in model.faces {
            let (slot, uv) = face_texture(&model, entry, lit, strip_only);
            cuboid = cuboid
                .with_face_uv(entry.face, uv)
                .with_face_material_slot(entry.face, slot);
        }
        // The authored layout faces south, so the block direction turns it from there.
        cuboids.push(rotate_cuboid_cardinal(&cuboid, direction.opposite()));
    }

    let mut shape = ModelShape::from_cuboids(cuboids);
    if lit {
        // Two crossed sheets carrying the `campfire` fire art, sampled inside the first frame.
        let reach = 0.5_f32 * std::f32::consts::FRAC_1_SQRT_2;
        let (lo, hi) = (0.5 - reach, 0.5 + reach);
        shape.planes.extend([
            ModelPlane::new(
                [
                    [lo, px(1.0), lo],
                    [hi, px(1.0), hi],
                    [hi, 1.0625, hi],
                    [lo, 1.0625, lo],
                ],
                [-1, 0, 1],
            )
            .with_material_slot("up")
            .with_uv(upright_full_texture_uv()),
            ModelPlane::new(
                [
                    [hi, px(1.0), lo],
                    [lo, px(1.0), hi],
                    [lo, 1.0625, hi],
                    [hi, 1.0625, lo],
                ],
                [1, 0, 1],
            )
            .with_material_slot("up")
            .with_uv(upright_full_texture_uv()),
        ]);
    }
    shape
}

/// Resolves one face's material slot and texture coordinates.
///
/// `blocks.json` gives the whole campfire one role set (`up` = fire sheet, `down` = plain log,
/// `side` = the lit strip), and the vanilla client switches the plain tile in for `side` when the
/// campfire is out. So a burning campfire reads the strip on every log face — inner faces grab
/// the authored flame rects, outer faces and log tops the bark-over-flame band that keeps bark on
/// the outside — while an extinguished one reads the matching window of `campfire_log`. The cut
/// ends always come from the plain tile: the strip has no end art.
fn face_texture(
    model: &CampfireBox,
    entry: &CampfireFace,
    lit: bool,
    strip_only: bool,
) -> (&'static str, [[f32; 2]; 4]) {
    let min = model.min.map(px);
    let max = model.max.map(px);
    let strip = strip_only || lit;
    match entry.dark {
        CampfireDark::End if !strip_only => (
            "log",
            region_uv(min, max, entry.face, CampfireDark::End.window()),
        ),
        CampfireDark::Char if !strip_only => (
            "log",
            region_uv(min, max, entry.face, CampfireDark::Char.window()),
        ),
        CampfireDark::Ash => (
            if strip { "lit_log" } else { "log" },
            region_uv(min, max, entry.face, CampfireDark::Ash.window()),
        ),
        _ => {
            if !strip {
                return (
                    "log",
                    region_uv(min, max, entry.face, entry.dark.window()),
                );
            }
            if entry.lit {
                // The strip's flames sit where the authored rects point.
                ("lit_log", frame_uv(entry.uv))
            } else {
                (
                    "lit_log",
                    region_uv(min, max, entry.face, CampfireDark::Bark.window()),
                )
            }
        }
    }
}

/// Spreads a texture window across one face, with U running along the face's longer in-plane
/// axis so the log grain follows the box.
fn region_uv(min: [f32; 3], max: [f32; 3], face: BlockFace, region: [f32; 4]) -> [[f32; 2]; 4] {
    let corners = crate::model_family::shape::box_face_corners(min, max, face);
    let (first, second) = match face {
        BlockFace::Up | BlockFace::Down => (0, 2),
        BlockFace::North | BlockFace::South => (0, 1),
        BlockFace::East | BlockFace::West => (2, 1),
        BlockFace::Side | BlockFace::All | BlockFace::Default => (0, 1),
    };
    let extent = |axis: usize| (max[axis] - min[axis]).abs().max(f32::EPSILON);
    let (u_axis, v_axis) = if extent(first) >= extent(second) {
        (first, second)
    } else {
        (second, first)
    };
    let [u0, v0, u1, v1] = region.map(|value| value / 16.0);
    corners.map(|corner| {
        let u = (corner[u_axis] - min[u_axis]) / extent(u_axis);
        // The window's first row is its top edge, so the face's bottom edge reads `v1`.
        let t = (corner[v_axis] - min[v_axis]) / extent(v_axis);
        [u0 + (u1 - u0) * u, v1 + (v0 - v1) * t]
    })
}

/// Turns one JE face rect into the renderer's bottom-left-first quad.
fn frame_uv(uv: [f32; 4]) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = uv;
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
