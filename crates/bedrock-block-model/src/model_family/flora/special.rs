use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv, ground_plane,
    rect_uv, upright_full_texture_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    match name {
        "cactus" => Some(ModelFamily::Cactus),
        "bamboo" => Some(ModelFamily::Bamboo),
        "cocoa" => Some(ModelFamily::Cocoa),
        "azalea" | "flowering_azalea" => Some(ModelFamily::Azalea),
        "mangrove_roots" | "muddy_mangrove_roots" => Some(ModelFamily::MangroveRoots),
        "leaf_litter" | "pink_petals" | "wildflowers" | "pale_moss_carpet" | "frog_spawn"
        | "waterlily" | "lily_pad" => Some(ModelFamily::GroundCover),
        "resin_clump" | "glow_lichen" | "sculk_vein" => Some(ModelFamily::MultiFace),
        _ => None,
    }
}

pub(crate) fn cactus_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(15.0), 1.0, px(15.0)])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::North, "side")
            .with_face_material_slot(BlockFace::South, "side")
            .with_face_material_slot(BlockFace::East, "side")
            .with_face_material_slot(BlockFace::West, "side"),
    )])
}

pub(crate) fn bamboo_shape(state: &BlockStateQuery) -> ModelShape {
    let thick = state_string(state, "bamboo_stalk_thickness").is_some_and(|value| value == "thick");
    let leaf_size = state_string(state, "bamboo_leaf_size").unwrap_or("no_leaves");
    let (min, max) = if thick {
        ([px(7.0), 0.0, px(7.0)], [px(10.0), 1.0, px(10.0)])
    } else {
        ([px(7.0), 0.0, px(7.0)], [px(9.0), 1.0, px(9.0)])
    };

    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new(min, max).with_material_slot("north"),
    )]);

    match leaf_size {
        "small_leaves" => shape.planes.extend(bamboo_leaf_planes("south")),
        "large_leaves" => shape.planes.extend(bamboo_leaf_planes("up")),
        _ => {}
    }

    shape
}

pub(crate) fn cocoa_shape(state: &BlockStateQuery) -> ModelShape {
    let age = state_i64(state, "age").unwrap_or(2).clamp(0, 2) as usize;
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);

    // Per-stage pod body from the JE `cocoa_stage{0,1,2}` models (geometry only — JE's from/to
    // match the Bedrock capture geo 716 at stage 2 exactly: [4,3,7]..[12,12,15]). All stages are
    // authored on the +Z (south) face; the stem is a zero-thickness 2-sided sheet above it.
    // UVs are JE's 16-pixel rects, which map 1:1 onto the Bedrock cocoa_stage_*.png 16x16 tiles.
    let (from, to, cap_uv, side_uv) = match age {
        0 => (
            [6.0, 7.0, 11.0],
            [10.0, 12.0, 15.0],
            [0.0, 0.0, 4.0, 4.0],
            [11.0, 4.0, 15.0, 9.0],
        ),
        1 => (
            [5.0, 5.0, 9.0],
            [11.0, 12.0, 15.0],
            [0.0, 0.0, 6.0, 6.0],
            [9.0, 4.0, 15.0, 11.0],
        ),
        _ => (
            [4.0, 3.0, 7.0],
            [12.0, 12.0, 15.0],
            [0.0, 0.0, 8.0, 8.0],
            [8.0, 4.0, 16.0, 13.0],
        ),
    };
    let uv16 = |r: [f32; 4]| rect_uv(r[0] / 16.0, r[1] / 16.0, r[2] / 16.0, r[3] / 16.0);
    let cap = uv16(cap_uv);
    let side = uv16(side_uv);
    // `cocoa` is a 3-frame terrain array [stage0, stage1, stage2]; the slot suffix carries the
    // age so obj.rs binds the right frame (otherwise every stage samples stage0's near-empty art).
    let slot: &'static str = match age {
        0 => "cocoa_0",
        1 => "cocoa_1",
        _ => "cocoa_2",
    };
    let body = ModelCuboid::new(from.map(px), to.map(px))
        .with_face_uv(BlockFace::Up, cap)
        .with_face_uv(BlockFace::Down, cap)
        .with_face_uv(BlockFace::North, side)
        .with_face_uv(BlockFace::South, side)
        .with_face_uv(BlockFace::East, side)
        .with_face_uv(BlockFace::West, side)
        .with_material_slot(slot);

    // The stem is the same for every stage: a zero-thickness sheet at x=8..8, z=12..16, y=12..16.
    let stem_uv = uv16([12.0, 0.0, 16.0, 4.0]);
    let stem = ModelPlane::new(
        [
            [px(8.0), px(16.0), px(12.0)],
            [px(8.0), px(16.0), px(16.0)],
            [px(8.0), px(12.0), px(16.0)],
            [px(8.0), px(12.0), px(12.0)],
        ],
        [1, 0, 0],
    )
    .with_uv(stem_uv)
    .with_material_slot(slot);

    // Rotate from the south-authored model to the block's direction. `quarter_turn_clockwise`
    // maps +Z(south) → -X(west) → -Z(north) → +X(east), so the count from south is:
    // South 0, West 1, North 2, East 3.
    let turns = match direction {
        CardinalDirection::South => 0usize,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    let rotated_body = (0..turns).fold(body, |c, _| rotate_cuboid_south_clockwise(&c));
    let stem = (0..turns).fold(stem, |p, _| rotate_plane_south_clockwise(&p));
    let mut shape = ModelShape::from_cuboids([rotated_body]);
    shape.planes.push(stem);
    shape
}

/// One clockwise quarter turn of a south-authored cuboid about the cell centre: `(x,z) -> (1-z, x)`.
fn rotate_cuboid_south_clockwise(c: &ModelCuboid) -> ModelCuboid {
    let mut r = ModelCuboid::new(
        [1.0 - c.max[2], c.min[1], c.min[0]],
        [1.0 - c.min[2], c.max[1], c.max[0]],
    );
    r.material_slot = c.material_slot.clone();
    for (face, slot) in &c.face_material_slots {
        r.face_material_slots.insert(cw_face(*face), slot.clone());
    }
    for (face, uv) in &c.face_uvs {
        r.face_uvs.insert(cw_face(*face), *uv);
    }
    r
}

/// One clockwise quarter turn of a south-authored plane.
fn rotate_plane_south_clockwise(p: &ModelPlane) -> ModelPlane {
    let corners = p.corners.map(|[x, y, z]| [1.0 - z, y, x]);
    let mut r = ModelPlane::new(corners, cw_normal(p.normal));
    r.material_slot = p.material_slot.clone();
    r.uv = p.uv;
    r
}

fn cw_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

fn cw_normal(n: [i32; 3]) -> [i32; 3] {
    [n[2], n[1], -n[0]]
}

pub(crate) fn azalea_shape() -> ModelShape {
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, px(5.0), 0.0], [1.0, 1.0, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::North, "side")
            .with_face_material_slot(BlockFace::South, "side")
            .with_face_material_slot(BlockFace::East, "side")
            .with_face_material_slot(BlockFace::West, "side"),
    )]);
    shape.planes.extend(cross_planes(1.0, Some("east")));
    shape
}

pub(crate) fn mangrove_roots_shape() -> ModelShape {
    let cuboid = detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    );
    ModelShape::from_cuboids([cuboid]).with_planes([
        ModelPlane::new(
            [
                [0.5, 0.0, 0.0],
                [0.5, 0.0, 1.0],
                [0.5, 1.0, 1.0],
                [0.5, 1.0, 0.0],
            ],
            [1, 0, 0],
        )
        .with_material_slot("side")
        .with_uv(upright_full_texture_uv()),
        ModelPlane::new(
            [
                [0.0, 0.0, 0.5],
                [1.0, 0.0, 0.5],
                [1.0, 1.0, 0.5],
                [0.0, 1.0, 0.5],
            ],
            [0, 0, 1],
        )
        .with_material_slot("side")
        .with_uv(upright_full_texture_uv()),
    ])
}

/// Ground-cover plants that fill their cell with one to four flat cards.
///
/// The capture shows two different families here, so they are built differently:
///
/// * `leaf_litter` (refs 9-21) lays down **quarter-cell cards** that tile outward from the
///   corner the state's `cardinal_direction` names. Growth 0 is a single quarter, growth 1-2
///   adds the neighbouring quarters, and growth 3 fills the cell.
/// * `pink_petals` and `wildflowers` (refs 483-514, 860-891) stand a few **flower sprites** on
///   the floor plus a thin top card at `y 0.1875`. Growth only changes how many sprites are
///   present.
///
/// Both read the growth axis, which the previous shape ignored beyond 4 quarter cards, so every
/// growth value above 3 rendered the same full cell.
pub(crate) fn ground_cover_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "pale_moss_carpet" {
        return pale_moss_carpet_shape(state);
    }
    if matches!(name, "frog_spawn" | "waterlily" | "lily_pad") {
        return full_ground_cover_shape(px(0.125));
    }
    if name == "leaf_litter" {
        return leaf_litter_shape(state);
    }
    flower_litter_shape(state)
}

/// Leaf litter: quarter-cell cards tiling outward from the state's facing corner.
///
/// Growth 0..3 select one to four quarters. The capture only carries refs for growth 0..3; the
/// higher values are the same four quarters plus a second scattered layer the client builds per
/// position, so the family stops at the full cell.
fn leaf_litter_shape(state: &BlockStateQuery) -> ModelShape {
    let growth = state_i64(state, "growth").unwrap_or(0).clamp(0, 3);
    let count = growth as usize + 1;
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    // Quarter cards in south-facing order, then turned by the state.
    let quarters = ground_cover_quadrants();
    let mut planes = Vec::with_capacity(count);
    for corners in quarters.iter().take(count) {
        planes.push(ground_plane(
            rotate_ground_corners(*corners, direction),
            None,
            full_texture_uv(),
        ));
    }
    ModelShape::default().with_planes(planes)
}

/// Pink petals and wildflowers: flower sprites standing on the floor plus one thin top card.
///
/// The capture's sprites are pairs of crossed quads at `y 0..0.1875`, and the top card covers the
/// occupied quarters. Growth 0..3 add quarters; above 3 the client scatters extra sprites per
/// position, so the family keeps the full cell.
fn flower_litter_shape(state: &BlockStateQuery) -> ModelShape {
    let growth = state_i64(state, "growth").unwrap_or(0).clamp(0, 3);
    let count = growth as usize + 1;
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    let quarters = ground_cover_quadrants();
    let mut planes = Vec::with_capacity(count * 3);
    for corners in quarters.iter().take(count) {
        let corners = rotate_ground_corners(*corners, direction);
        planes.push(ground_plane(corners, None, full_texture_uv()));
        planes.push(flower_sprite(corners, [1, 0, 1]));
        planes.push(flower_sprite(corners, [1, 0, -1]));
    }
    ModelShape::default().with_planes(planes)
}

/// One upright flower quad standing on a ground card, rotated 45 degrees in plan.
fn flower_sprite(card: [[f32; 3]; 4], normal: [i32; 3]) -> ModelPlane {
    let (min_x, max_x, min_z, max_z) = card_bounds(&card);
    let (centre_x, centre_z) = ((min_x + max_x) * 0.5, (min_z + max_z) * 0.5);
    let reach = (max_x - min_x).min(max_z - min_z) * 0.5;
    let (dx, dz) = (reach, reach * if normal[2] > 0 { 1.0 } else { -1.0 });
    ModelPlane::new(
        [
            [centre_x - dx, 0.0, centre_z - dz],
            [centre_x + dx, 0.0, centre_z + dz],
            [centre_x + dx, 0.1875, centre_z + dz],
            [centre_x - dx, 0.1875, centre_z - dz],
        ],
        normal,
    )
    .with_uv(upright_full_texture_uv())
}

fn card_bounds(card: &[[f32; 3]; 4]) -> (f32, f32, f32, f32) {
    let mut bounds = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for corner in card {
        bounds.0 = bounds.0.min(corner[0]);
        bounds.1 = bounds.1.max(corner[0]);
        bounds.2 = bounds.2.min(corner[2]);
        bounds.3 = bounds.3.max(corner[2]);
    }
    bounds
}

/// Turns a ground card authored facing south onto the state's cardinal.
fn rotate_ground_corners(
    corners: [[f32; 3]; 4],
    direction: CardinalDirection,
) -> [[f32; 3]; 4] {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    let mut corners = corners;
    for _ in 0..turns {
        for corner in &mut corners {
            let (x, z) = (corner[0], corner[2]);
            corner[0] = z;
            corner[2] = 1.0 - x;
        }
    }
    corners
}

pub(crate) fn multi_face_shape(state: &BlockStateQuery) -> ModelShape {
    let bits = state_i64(state, "multi_face_direction_bits").unwrap_or(1);
    let mut planes = Vec::with_capacity(6);
    if bits & 1 != 0 {
        planes.push(face_plane(BlockFace::Down));
    }
    if bits & 2 != 0 {
        planes.push(face_plane(BlockFace::Up));
    }
    if bits & 4 != 0 {
        planes.push(face_plane(BlockFace::South));
    }
    if bits & 8 != 0 {
        planes.push(face_plane(BlockFace::East));
    }
    if bits & 16 != 0 {
        planes.push(face_plane(BlockFace::North));
    }
    if bits & 32 != 0 {
        planes.push(face_plane(BlockFace::West));
    }
    if planes.is_empty() {
        planes.push(face_plane(BlockFace::Up));
    }
    ModelShape::default().with_planes(planes)
}

fn pale_moss_carpet_shape(state: &BlockStateQuery) -> ModelShape {
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, px(1.0), 1.0],
    ))]);

    for (state_key, direction) in [
        ("pale_moss_carpet_side_north", CardinalDirection::North),
        ("pale_moss_carpet_side_south", CardinalDirection::South),
        ("pale_moss_carpet_side_east", CardinalDirection::East),
        ("pale_moss_carpet_side_west", CardinalDirection::West),
    ] {
        let Some(value) = state_string(state, state_key) else {
            continue;
        };
        let height = match value {
            "short" => px(6.0),
            "tall" => 1.0,
            _ => continue,
        };
        shape.planes.push(side_drape_plane(direction, height));
    }

    shape
}

fn full_ground_cover_shape(y: f32) -> ModelShape {
    ModelShape::default().with_planes([ground_plane(
        [[0.0, y, 0.0], [1.0, y, 0.0], [1.0, y, 1.0], [0.0, y, 1.0]],
        None,
        full_texture_uv(),
    )])
}

fn bamboo_leaf_planes(material_slot: &'static str) -> [ModelPlane; 2] {
    [
        ModelPlane::new(
            [
                [0.0, 0.0, 0.5],
                [1.0, 0.0, 0.5],
                [1.0, 1.0, 0.5],
                [0.0, 1.0, 0.5],
            ],
            [0, 0, 1],
        )
        .with_material_slot(material_slot),
        ModelPlane::new(
            [
                [0.5, 0.0, 0.0],
                [0.5, 0.0, 1.0],
                [0.5, 1.0, 1.0],
                [0.5, 1.0, 0.0],
            ],
            [1, 0, 0],
        )
        .with_material_slot(material_slot),
    ]
}

fn cross_planes(height: f32, material_slot: Option<&'static str>) -> [ModelPlane; 2] {
    let mut first = ModelPlane::new(
        [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, height, 1.0],
            [0.0, height, 0.0],
        ],
        [-1, 0, 1],
    );
    let mut second = ModelPlane::new(
        [
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, height, 1.0],
            [1.0, height, 0.0],
        ],
        [1, 0, 1],
    );
    if let Some(slot) = material_slot {
        first = first.with_material_slot(slot);
        second = second.with_material_slot(slot);
    }
    [first, second]
}

fn ground_cover_quadrants() -> [[[f32; 3]; 4]; 4] {
    let y = 0.01;
    [
        [[0.5, y, 0.5], [1.0, y, 0.5], [1.0, y, 1.0], [0.5, y, 1.0]],
        [[0.5, y, 0.0], [1.0, y, 0.0], [1.0, y, 0.5], [0.5, y, 0.5]],
        [[0.0, y, 0.0], [0.5, y, 0.0], [0.5, y, 0.5], [0.0, y, 0.5]],
        [[0.0, y, 0.5], [0.5, y, 0.5], [0.5, y, 1.0], [0.0, y, 1.0]],
    ]
}

fn face_plane(face: BlockFace) -> ModelPlane {
    let offset = 0.01;
    match face {
        BlockFace::Up => ModelPlane::new(
            [
                [0.0, 1.0 - offset, 0.0],
                [1.0, 1.0 - offset, 0.0],
                [1.0, 1.0 - offset, 1.0],
                [0.0, 1.0 - offset, 1.0],
            ],
            [0, -1, 0],
        ),
        BlockFace::Down => ModelPlane::new(
            [
                [0.0, offset, 0.0],
                [0.0, offset, 1.0],
                [1.0, offset, 1.0],
                [1.0, offset, 0.0],
            ],
            [0, 1, 0],
        ),
        BlockFace::North => ModelPlane::new(
            [
                [1.0, 0.0, offset],
                [0.0, 0.0, offset],
                [0.0, 1.0, offset],
                [1.0, 1.0, offset],
            ],
            [0, 0, 1],
        ),
        BlockFace::South => ModelPlane::new(
            [
                [0.0, 0.0, 1.0 - offset],
                [1.0, 0.0, 1.0 - offset],
                [1.0, 1.0, 1.0 - offset],
                [0.0, 1.0, 1.0 - offset],
            ],
            [0, 0, -1],
        ),
        BlockFace::East => ModelPlane::new(
            [
                [1.0 - offset, 0.0, 1.0],
                [1.0 - offset, 0.0, 0.0],
                [1.0 - offset, 1.0, 0.0],
                [1.0 - offset, 1.0, 1.0],
            ],
            [-1, 0, 0],
        ),
        BlockFace::West => ModelPlane::new(
            [
                [offset, 0.0, 0.0],
                [offset, 0.0, 1.0],
                [offset, 1.0, 1.0],
                [offset, 1.0, 0.0],
            ],
            [1, 0, 0],
        ),
        BlockFace::Side | BlockFace::All | BlockFace::Default => face_plane(BlockFace::North),
    }
}

fn side_drape_plane(direction: CardinalDirection, height: f32) -> ModelPlane {
    match direction {
        CardinalDirection::North => ModelPlane::new(
            [
                [1.0, 0.0, 0.01],
                [0.0, 0.0, 0.01],
                [0.0, height, 0.01],
                [1.0, height, 0.01],
            ],
            [0, 0, 1],
        ),
        CardinalDirection::South => ModelPlane::new(
            [
                [0.0, 0.0, 0.99],
                [1.0, 0.0, 0.99],
                [1.0, height, 0.99],
                [0.0, height, 0.99],
            ],
            [0, 0, -1],
        ),
        CardinalDirection::East => ModelPlane::new(
            [
                [0.99, 0.0, 1.0],
                [0.99, 0.0, 0.0],
                [0.99, height, 0.0],
                [0.99, height, 1.0],
            ],
            [-1, 0, 0],
        ),
        CardinalDirection::West => ModelPlane::new(
            [
                [0.01, 0.0, 0.0],
                [0.01, 0.0, 1.0],
                [0.01, height, 1.0],
                [0.01, height, 0.0],
            ],
            [1, 0, 0],
        ),
    }
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
