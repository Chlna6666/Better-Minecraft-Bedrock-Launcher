use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, rotate_shape_cardinal, uv16,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_anvil") || name == "anvil" {
        return Some(ModelFamily::Anvil);
    }
    if name == "stonecutter" || name == "stonecutter_block" {
        return Some(ModelFamily::Stonecutter);
    }
    if name == "hopper" {
        return Some(ModelFamily::Hopper);
    }
    if name == "grindstone" {
        return Some(ModelFamily::Grindstone);
    }
    if name == "lectern" {
        return Some(ModelFamily::Lectern);
    }
    None
}

pub(crate) fn anvil_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let turns = match direction {
        CardinalDirection::North => 0,
        CardinalDirection::East => 1,
        CardinalDirection::South => 2,
        CardinalDirection::West => 3,
    };
    // Every anvil box is symmetric, so the facing only shows in the texture: rotate each face's
    // UV quad inside the face instead of relying on the geometry turn.
    let mut shape = rotate_shape_cardinal(
        ModelShape::from_cuboids([
            anvil_side_cuboid_with_uv(ModelCuboid::new([0.125, 0.0, 0.125], [0.875, 0.25, 0.875])),
            anvil_side_cuboid_with_uv(ModelCuboid::new(
                [0.25, 0.25, 0.1875],
                [0.75, 0.3125, 0.8125],
            )),
            anvil_cuboid_with_uv(
                ModelCuboid::new([0.375, 0.3125, 0.25], [0.625, 0.625, 0.75]),
                uv16(6.0, 5.0, 10.0, 13.0),
                uv16(0.0, 0.0, 16.0, 16.0),
            ),
            anvil_cuboid_with_uv(
                ModelCuboid::new([0.1875, 0.625, 0.0], [0.8125, 1.0, 1.0]),
                uv16(0.0, 0.0, 16.0, 10.0),
                uv16(0.0, 0.0, 16.0, 16.0),
            ),
        ]),
        direction,
    );
    for cuboid in &mut shape.cuboids {
        for face in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            if let Some(uv) = cuboid.face_uvs.get(&face).copied() {
                cuboid.face_uvs.insert(
                    face,
                    crate::model_family::shape::rotate_uv_quarter(uv, turns),
                );
            }
        }
    }
    shape
}

pub(crate) fn stonecutter_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    // Authored facing north; `blocks.json` puts the saw art on the north/south band, so the
    // blade sheet crosses the cell at z = 0.5 and turns with the block.
    let shape = ModelShape::from_cuboids([
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.5625, 1.0]),
        ModelCuboid::new([0.4375, 0.5625, 0.125], [0.5625, 0.9375, 0.875]),
    ])
    .with_planes([ModelPlane::new(
        [
            [0.25, 0.5625, 0.5],
            [0.75, 0.5625, 0.5],
            [0.75, 1.0, 0.5],
            [0.25, 1.0, 0.5],
        ],
        [0, 0, 1],
    )
    .with_material_slot("saw")]);
    rotate_shape_cardinal(shape, direction)
}

pub(crate) fn hopper_shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = vec![
        hopper_shell_cuboid(ModelCuboid::new(
            [0.125, 0.625, 0.125],
            [0.875, 0.6875, 0.875],
        ))
        .with_face_material_slot(BlockFace::Up, "down"),
        hopper_shell_cuboid(ModelCuboid::new([0.0, 0.625, 0.0], [1.0, 1.0, 0.125]))
            .with_face_material_slot(BlockFace::South, "north"),
        hopper_shell_cuboid(ModelCuboid::new([0.0, 0.625, 0.875], [1.0, 1.0, 1.0])),
        hopper_shell_cuboid(ModelCuboid::new([0.0, 0.625, 0.125], [0.125, 1.0, 0.875]))
            .with_face_material_slot(BlockFace::West, "north"),
        hopper_shell_cuboid(ModelCuboid::new([0.875, 0.625, 0.125], [1.0, 1.0, 0.875]))
            .with_face_material_slot(BlockFace::East, "north"),
        hopper_body_cuboid(ModelCuboid::new([0.25, 0.25, 0.25], [0.75, 0.625, 0.75])),
        hopper_body_cuboid(hopper_spout_cuboid(state)),
    ];
    ModelShape::from_cuboids(std::mem::take(&mut cuboids))
}

fn hopper_shell_cuboid(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
}

fn hopper_body_cuboid(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_material_slot(BlockFace::Up, "north")
        .with_face_material_slot(BlockFace::Down, "north")
        .with_face_material_slot(BlockFace::Side, "north")
}

fn hopper_spout_cuboid(state: &BlockStateQuery) -> ModelCuboid {
    match hopper_side_direction(state) {
        Some(CardinalDirection::North) => ModelCuboid::new([0.375, 0.25, 0.0], [0.625, 0.5, 0.375]),
        Some(CardinalDirection::South) => ModelCuboid::new([0.375, 0.25, 0.625], [0.625, 0.5, 1.0]),
        Some(CardinalDirection::East) => ModelCuboid::new([0.625, 0.25, 0.375], [1.0, 0.5, 0.625]),
        Some(CardinalDirection::West) => ModelCuboid::new([0.0, 0.25, 0.375], [0.375, 0.5, 0.625]),
        None => ModelCuboid::new([0.375, 0.0, 0.375], [0.625, 0.25, 0.625]),
    }
}

fn hopper_side_direction(state: &BlockStateQuery) -> Option<CardinalDirection> {
    for key in [
        "minecraft:cardinal_direction",
        "facing_direction",
        "minecraft:block_face",
        "block_face",
    ] {
        if let Some(value) = crate::model_family::direction::state_string(state, key) {
            if value == "down" {
                return None;
            }
            return crate::model_family::direction::from_string(value);
        }
    }
    crate::model_family::direction::state_i64(state, "facing_direction")
        .or_else(|| crate::model_family::direction::state_i64(state, "minecraft:facing_direction"))
        .and_then(|value| match value {
            2 => Some(CardinalDirection::North),
            3 => Some(CardinalDirection::South),
            4 => Some(CardinalDirection::West),
            5 => Some(CardinalDirection::East),
            _ => None,
        })
}

pub(crate) fn grindstone_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    // `attachment` selects the mount (standing = floor, hanging = ceiling, side = wall) and the
    // model is authored on the floor facing north.
    let x_turn = match crate::model_family::direction::state_string(state, "attachment") {
        Some("hanging") => 2,
        Some("side") | Some("wall") => 1,
        _ => 0,
    };
    let shape = rotate_shape_cardinal(
        ModelShape::from_cuboids([
            // Wheel: the grinding face reads `grindstone_round`, its rim `grindstone_side`.
            grind_box(
                [4.0, 4.0, 2.0],
                [12.0, 16.0, 14.0],
                &[
                    (BlockFace::North, [0.0, 0.0, 8.0, 12.0], "up"),
                    (BlockFace::South, [0.0, 0.0, 8.0, 12.0], "up"),
                    (BlockFace::Up, [0.0, 0.0, 8.0, 12.0], "up"),
                    (BlockFace::Down, [0.0, 0.0, 8.0, 12.0], "up"),
                    (BlockFace::East, [0.0, 0.0, 12.0, 12.0], "south"),
                    (BlockFace::West, [0.0, 0.0, 12.0, 12.0], "south"),
                ],
            ),
            // Legs (`grindstone_leg`) and pivots (`grindstone_pivot`) flank the wheel.
            grind_box(
                [12.0, 0.0, 6.0],
                [14.0, 7.0, 10.0],
                &[
                    (BlockFace::North, [2.0, 9.0, 4.0, 16.0], "down"),
                    (BlockFace::South, [12.0, 9.0, 14.0, 16.0], "down"),
                    (BlockFace::East, [10.0, 16.0, 6.0, 9.0], "down"),
                    (BlockFace::West, [6.0, 9.0, 10.0, 16.0], "down"),
                    (BlockFace::Down, [12.0, 6.0, 14.0, 10.0], "down"),
                ],
            ),
            grind_box(
                [2.0, 0.0, 6.0],
                [4.0, 7.0, 10.0],
                &[
                    (BlockFace::North, [12.0, 9.0, 14.0, 16.0], "down"),
                    (BlockFace::South, [2.0, 9.0, 4.0, 16.0], "down"),
                    (BlockFace::East, [10.0, 16.0, 6.0, 9.0], "down"),
                    (BlockFace::West, [6.0, 9.0, 10.0, 16.0], "down"),
                    (BlockFace::Down, [2.0, 6.0, 4.0, 10.0], "down"),
                ],
            ),
            grind_box(
                [12.0, 7.0, 5.0],
                [14.0, 13.0, 11.0],
                &[
                    (BlockFace::North, [6.0, 0.0, 8.0, 6.0], "north"),
                    (BlockFace::South, [6.0, 0.0, 8.0, 6.0], "north"),
                    (BlockFace::East, [0.0, 0.0, 6.0, 6.0], "north"),
                    (BlockFace::Up, [8.0, 0.0, 10.0, 6.0], "north"),
                    (BlockFace::Down, [8.0, 0.0, 10.0, 6.0], "north"),
                ],
            ),
            grind_box(
                [2.0, 7.0, 5.0],
                [4.0, 13.0, 11.0],
                &[
                    (BlockFace::North, [6.0, 0.0, 8.0, 6.0], "north"),
                    (BlockFace::South, [6.0, 0.0, 8.0, 6.0], "north"),
                    (BlockFace::West, [0.0, 0.0, 6.0, 6.0], "north"),
                    (BlockFace::Up, [8.0, 0.0, 10.0, 6.0], "north"),
                    (BlockFace::Down, [8.0, 0.0, 10.0, 6.0], "north"),
                ],
            ),
        ]),
        direction,
    );
    crate::model_family::shape::rotate_shape_x(shape, x_turn)
}

/// One grindstone box with per-face UVs (authored top-down) and Bedrock material slots.
fn grind_box(
    min: [f32; 3],
    max: [f32; 3],
    faces: &[(BlockFace, [f32; 4], &'static str)],
) -> ModelCuboid {
    let mut cuboid = ModelCuboid::new(
        [min[0] / 16.0, min[1] / 16.0, min[2] / 16.0],
        [max[0] / 16.0, max[1] / 16.0, max[2] / 16.0],
    );
    for (face, uv, slot) in faces {
        let [u0, v0, u1, v1] = *uv;
        let quad = [
            [u0 / 16.0, v1 / 16.0],
            [u1 / 16.0, v1 / 16.0],
            [u1 / 16.0, v0 / 16.0],
            [u0 / 16.0, v0 / 16.0],
        ];
        cuboid = cuboid
            .with_face_uv(*face, quad)
            .with_face_material_slot(*face, *slot);
    }
    cuboid
}

pub(crate) fn lectern_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let has_book = crate::model_family::direction::state_bool(state, "has_book_bit")
        .or_else(|| crate::model_family::direction::state_bool(state, "has_book"))
        .unwrap_or(false);

    // A full-footprint 2 px base, a 8x13 px post, and a 16x4 px board tilted -22.5 degrees
    // about X. `blocks.json` gives the lectern one material per face (up=lectern_top,
    // down=lectern_bottom, north=lectern_front, south/west=lectern_sides, east=lectern_base), so
    // both boxes keep the face-driven materials instead of naming parts the pack does not have.
    let base = crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, 0.125, 1.0],
    ));
    let post = crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.25, 0.125, 0.25],
        [0.75, 0.9375, 0.75],
    ));
    let mut shape = ModelShape::from_cuboids([base, post]);
    // Board: vanilla `LECTERN_BOARD` (x ±7.9 px, y 11..15 px, z -6..7 px) tilted -22.5 degrees
    // about the pivot (0, 7, -1) px and offset by (0, 1.05, 1.0) px. Sampling the tilted top
    // face as a quad reproduces the slope without needing rotated cuboids.
    let mut board_corners = [[0.0f32; 3]; 4];
    for (index, (x, z)) in [(-7.9f32, -6.0f32), (7.9, -6.0), (7.9, 7.0), (-7.9, 7.0)]
        .into_iter()
        .enumerate()
    {
        board_corners[index] = lectern_board_point([x, 15.0, z]);
    }
    let normal = plane_normal(board_corners);
    shape.planes.push(
        ModelPlane::new(board_corners, normal)
            .with_material_slot("top")
            .with_uv(crate::model_family::shape::full_texture_uv()),
    );
    if has_book {
        shape.cuboids.push(
            crate::model_family::shape::detail_cuboid_with_local_uv(ModelCuboid::new(
                [0.1875, 0.78125, 0.3125],
                [0.8125, 0.90625, 0.875],
            ))
            .with_material_slot("north"),
        );
    }

    rotate_shape_cardinal(shape, direction)
}

/// Tilts a lectern board point around the vanilla pivot, then applies the model offset.
///
/// Input and output are block coordinates; the source table is in authoring pixels centred on
/// the block, so the caller passes pixel values offset from the centre.
fn lectern_board_point(point: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = point;
    // Pivot (0, 7, -1) px, offset (0, 1.05, 1.0) px, slope -22.5 degrees about X.
    let (pivot_y, pivot_z) = (7.0f32, -1.0f32);
    let (offset_y, offset_z) = (1.05f32, 1.0f32);
    let (sin, cos) = (-22.5f32).to_radians().sin_cos();
    let local_y = y - pivot_y;
    let local_z = z - pivot_z;
    [
        0.5 + x / 16.0,
        (cos * local_y - sin * local_z + pivot_y + offset_y) / 16.0,
        (sin * local_y + cos * local_z + pivot_z + offset_z) / 16.0,
    ]
}

/// Signed normal of a quad, used so the tilted board faces the same way as its slope.
fn plane_normal(corners: [[f32; 3]; 4]) -> [i32; 3] {
    let edge_a = [
        corners[1][0] - corners[0][0],
        corners[1][1] - corners[0][1],
        corners[1][2] - corners[0][2],
    ];
    let edge_b = [
        corners[3][0] - corners[0][0],
        corners[3][1] - corners[0][1],
        corners[3][2] - corners[0][2],
    ];
    let cross = [
        edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
        edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
        edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
    ];
    [
        if cross[0] >= 0.0 { 1 } else { -1 },
        if cross[1] >= 0.0 { 1 } else { -1 },
        if cross[2] >= 0.0 { 1 } else { -1 },
    ]
}
fn anvil_cuboid_with_uv(
    cuboid: ModelCuboid,
    top_uv: [[f32; 2]; 4],
    side_uv: [[f32; 2]; 4],
) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_face_uv(BlockFace::Up, top_uv)
        .with_face_uv(BlockFace::Down, top_uv)
        .with_face_uv(BlockFace::North, side_uv)
        .with_face_uv(BlockFace::South, side_uv)
        .with_face_uv(BlockFace::West, side_uv)
        .with_face_uv(BlockFace::East, side_uv)
}

fn anvil_side_cuboid_with_uv(cuboid: ModelCuboid) -> ModelCuboid {
    detail_cuboid_with_local_uv(cuboid)
        .with_material_slot("side")
        .with_face_uv(BlockFace::Up, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::Down, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::North, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::South, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::West, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::East, uv16(0.0, 0.0, 16.0, 16.0))
}
