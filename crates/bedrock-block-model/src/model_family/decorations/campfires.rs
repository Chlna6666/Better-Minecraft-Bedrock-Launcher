use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_bool};
use crate::model_family::shape::{ModelCuboid, ModelPlane, ModelShape, upright_full_texture_uv};
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
struct CampfireBox {
    min: [f32; 3],
    max: [f32; 3],
    /// `[face][corner] = [u, v]` in texels; corner order follows the renderer's face order.
    uv: [(BlockFace, [[f32; 2]; 4]); 6],
    /// Faces that use the lit log art instead of the bark art.
    lit: &'static [BlockFace],
}

/// Vanilla `template_campfire` boxes. Lower logs, upper logs and the ember bed.
fn north_boxes() -> [CampfireBox; 5] {
    const L: BlockFace = BlockFace::Up;
    let _ = L;
    [
        CampfireBox {
            min: [1.0, 0.0, 0.0],
            max: [5.0, 4.0, 16.0],
            uv: [
                (BlockFace::Down, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::Up, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::North, rect(4.0, 4.0, 0.0, 8.0)),
                (BlockFace::South, rect(0.0, 4.0, 4.0, 8.0)),
                (BlockFace::West, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::East, rect(0.0, 1.0, 16.0, 5.0)),
            ],
            lit: &[BlockFace::East],
        },
        CampfireBox {
            min: [0.0, 3.0, 1.0],
            max: [16.0, 7.0, 5.0],
            uv: [
                (BlockFace::Down, rect(0.0, 4.0, 16.0, 8.0)),
                (BlockFace::Up, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::North, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::South, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::West, rect(4.0, 4.0, 0.0, 8.0)),
                (BlockFace::East, rect(0.0, 4.0, 4.0, 8.0)),
            ],
            lit: &[BlockFace::Down, BlockFace::North, BlockFace::South],
        },
        CampfireBox {
            min: [11.0, 0.0, 0.0],
            max: [15.0, 4.0, 16.0],
            uv: [
                (BlockFace::Down, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::Up, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::North, rect(4.0, 4.0, 0.0, 8.0)),
                (BlockFace::South, rect(0.0, 4.0, 4.0, 8.0)),
                (BlockFace::West, rect(0.0, 1.0, 16.0, 5.0)),
                (BlockFace::East, rect(0.0, 0.0, 16.0, 4.0)),
            ],
            lit: &[BlockFace::West],
        },
        CampfireBox {
            min: [0.0, 3.0, 11.0],
            max: [16.0, 7.0, 15.0],
            uv: [
                (BlockFace::Down, rect(0.0, 4.0, 16.0, 8.0)),
                (BlockFace::Up, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::North, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::South, rect(0.0, 0.0, 16.0, 4.0)),
                (BlockFace::West, rect(4.0, 4.0, 0.0, 8.0)),
                (BlockFace::East, rect(0.0, 4.0, 4.0, 8.0)),
            ],
            lit: &[BlockFace::Down, BlockFace::North, BlockFace::South],
        },
        CampfireBox {
            min: [5.0, 0.0, 0.0],
            max: [11.0, 1.0, 16.0],
            uv: [
                (BlockFace::Down, rect(0.0, 8.0, 16.0, 14.0)),
                (BlockFace::Up, rect(0.0, 8.0, 16.0, 14.0)),
                (BlockFace::North, rect(6.0, 15.0, 0.0, 16.0)),
                (BlockFace::South, rect(10.0, 15.0, 16.0, 16.0)),
                (BlockFace::West, rect(4.0, 4.0, 0.0, 8.0)),
                (BlockFace::East, rect(0.0, 4.0, 4.0, 8.0)),
            ],
            lit: &[BlockFace::Up],
        },
    ]
}

fn rect(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v0 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u0 / 16.0, v1 / 16.0],
    ]
}

/// Campfire logs, ember bed and the two crossed flame sheets.
///
/// The previous shape was four rail-like boxes with no flame, so campfires read as fences.
pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let lit = state_bool(state, "extinguished")
        .map(|extinguished| !extinguished)
        .unwrap_or(true);
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let turns = match direction {
        CardinalDirection::North => 0,
        CardinalDirection::West => 1,
        CardinalDirection::South => 2,
        CardinalDirection::East => 3,
    };

    let mut cuboids = Vec::with_capacity(5);
    for mut model in north_boxes() {
        let mut cuboid = ModelCuboid::new(model.min.map(px), model.max.map(px));
        for (face, uv) in model.uv {
            cuboid = cuboid.with_face_uv(face, uv);
            if model.lit.contains(&face) {
                cuboid = cuboid.with_face_material_slot(face, if lit { "lit_log" } else { "log" });
            } else {
                cuboid = cuboid.with_face_material_slot(face, "log");
            }
        }
        for _ in 0..turns {
            cuboid = rotate_cuboid_clockwise(cuboid);
        }
        cuboids.push(cuboid);
        model.lit = &[];
    }

    let mut shape = ModelShape::from_cuboids(cuboids);
    if lit {
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
            .with_material_slot("fire")
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
            .with_material_slot("fire")
            .with_uv(upright_full_texture_uv()),
        ]);
    }
    shape
}

fn rotate_cuboid_clockwise(cuboid: ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [1.0 - cuboid.max[2], cuboid.min[1], cuboid.min[0]],
        [1.0 - cuboid.min[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot;
    for (face, slot) in cuboid.face_material_slots {
        rotated.face_material_slots.insert(rotate_face(face), slot);
    }
    for (face, uv) in cuboid.face_uvs {
        rotated.face_uvs.insert(rotate_face(face), uv);
    }
    rotated
}

fn rotate_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
