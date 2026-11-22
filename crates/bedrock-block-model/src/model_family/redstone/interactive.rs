use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    BlockFacing, CardinalDirection, block_facing, cardinal_direction, state_bool, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, rotate_shape_cardinal, rotate_shape_x,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_button") || matches!(name, "button" | "stone_button" | "wooden_button") {
        return Some(ModelFamily::Button);
    }
    if name.ends_with("_pressure_plate")
        || matches!(
            name,
            "pressure_plate"
                | "stone_pressure_plate"
                | "wooden_pressure_plate"
                | "light_weighted_pressure_plate"
                | "heavy_weighted_pressure_plate"
        )
    {
        return Some(ModelFamily::PressurePlate);
    }
    if name == "lever" || name == "tripwire_hook" {
        return Some(ModelFamily::RedstoneDevice);
    }
    None
}

pub(crate) fn button_shape(state: &BlockStateQuery) -> ModelShape {
    let pressed = state_bool(state, "button_pressed_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    // The vanilla button is a 6 x 4 px plate, 2 px thick (1 px when pressed), authored on the
    // floor and pointing north; every other mount turns that plate.
    let (x_turn, y_turn) = match facing {
        BlockFacing::Up => (0, CardinalDirection::North),
        BlockFacing::Down => (2, CardinalDirection::North),
        facing => (1, facing.cardinal().unwrap_or(CardinalDirection::North)),
    };
    let shape = ModelShape::from_cuboids([canonical_button(pressed)]);
    rotate_shape_cardinal(rotate_shape_x(shape, x_turn), y_turn)
}

/// Vanilla `button` / `button_pressed`: 6 x 4 px footprint on the face, 2 px deep when idle.
fn canonical_button(pressed: bool) -> ModelCuboid {
    let depth = if pressed { 1.0 } else { 2.0 };
    let sides_v0 = if pressed { 14.0 } else { 14.0 };
    let sides_v1 = if pressed { 15.0 } else { 16.0 };
    ModelCuboid::new([5.0, 0.0, 6.0].map(px), [11.0, depth, 10.0].map(px))
        .with_face_uv(BlockFace::Up, uv16(5.0, 6.0, 11.0, 10.0))
        .with_face_uv(BlockFace::Down, uv16(5.0, 6.0, 11.0, 10.0))
        .with_face_uv(BlockFace::North, uv16(5.0, sides_v0, 11.0, sides_v1))
        .with_face_uv(BlockFace::South, uv16(5.0, sides_v0, 11.0, sides_v1))
        .with_face_uv(BlockFace::West, uv16(6.0, sides_v0, 10.0, sides_v1))
        .with_face_uv(BlockFace::East, uv16(6.0, sides_v0, 10.0, sides_v1))
}

pub(crate) fn pressure_plate_shape() -> ModelShape {
    ModelShape::from_cuboids([ModelCuboid::new(
        [0.0625, 0.0, 0.0625],
        [0.9375, 0.0625, 0.9375],
    )])
}

pub(crate) fn lever_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = state_string(state, "lever_direction").unwrap_or("up_north_south");
    let open = state_bool(state, "open_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false);
    // The model is authored on the floor with the handle swinging along Z, matching
    // `lever_direction = up_north_south`; every other mount is that model turned.
    let (x_turn, y_turn) = lever_mount(direction);
    let shape = rotate_shape_cardinal(rotate_shape_x(canonical_lever(open), x_turn), y_turn);
    shape
}

/// Vanilla lever, transcribed from the game's own captured mesh (BedrockSceneCapture
/// `1.26.60.5`, floor states ref 725 closed / 733 open). Every corner, normal and UV window
/// below is the engine's decoded quad — not a JE model port. JE's lever puts the base on the
/// floor with a rigidly tilted handle; Bedrock shears the handle and binds the base to
/// `lever_particle` (cobblestone). Crucially, the per-face UV corners differ per quad: the
/// long handle faces read the stick strip `lever[7..9, 6..16]`, the pivot/tip caps only
/// `lever[7..9, 6..8]`, and the two side sheets mirror the strip. Sharing one UV quad across
/// the faces (the previous approach) smeared the art into grey/white stripes.
fn canonical_lever(open: bool) -> ModelShape {
    let base = "lever_particle";
    let stick = "lever";
    // Each row: (slot, face normal, 4 corners in px, 4 uv corners in 0..1). UVs are the
    // atlas-decoded local coordinates from the capture (v is bottom-up).
    let q = |slot: &str, n: [i32; 3], c: [[f32; 3]; 4], uv: [[f32; 2]; 4]| {
        ModelPlane::new(
            c.map(|p| p.map(px)),
            n,
        )
        .with_material_slot(slot)
        .with_uv(uv)
    };
    // The capture's own four uv corners per quad (atlas-decoded local coordinates).
    let uv = |a: [f32;2], b: [f32;2], c: [f32;2], d: [f32;2]| [a, b, c, d];
    // Base plate: x 5..11 px, y 0..3, z 4..12. Exact captured uv corners per quad.
    let base_planes = [
        q(base, [-1, 0, 0], [[5.,0.,4.],[5.,0.,12.],[5.,3.,12.],[5.,3.,4.]], uv([0.25,1.],[0.75,1.],[0.75,0.81],[0.25,0.81])),
        q(base, [0, 0,-1], [[11.,0.,4.],[5.,0.,4.],[5.,3.,4.],[11.,3.,4.]], uv([0.31,1.],[0.69,1.],[0.69,0.81],[0.31,0.81])),
        q(base, [0,-1, 0], [[5.,0.,12.],[5.,0.,4.],[11.,0.,4.],[11.,0.,12.]], uv([0.31,0.25],[0.31,0.75],[0.69,0.75],[0.69,0.25])),
        q(base, [0, 0, 1], [[5.,3.,12.],[5.,0.,12.],[11.,0.,12.],[11.,3.,12.]], uv([0.31,0.81],[0.31,1.],[0.69,1.],[0.69,0.81])),
        q(base, [0, 1, 0], [[5.,3.,4.],[5.,3.,12.],[11.,3.,12.],[11.,3.,4.]], uv([0.31,0.25],[0.31,0.75],[0.69,0.75],[0.69,0.25])),
        q(base, [1, 0, 0], [[11.,0.,12.],[11.,0.,4.],[11.,3.,4.],[11.,3.,12.]], uv([0.25,1.],[0.75,1.],[0.75,0.81],[0.25,0.81])),
    ];
    // Sheared handle: x 7..9 px; pivot (y2,z8) to tip. Closed leans +z, open leans -z.
    let handle_planes = if open {
        [
            q(stick, [-1, 0, 0], [[7.,0.71,6.47],[7.,2.,8.],[7.,9.66,1.57],[7.,8.37,0.04]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0,-1,-1], [[9.,0.71,6.47],[7.,0.71,6.47],[7.,8.37,0.04],[9.,8.37,0.04]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0,-1, 1], [[7.,0.71,6.47],[9.,0.71,6.47],[9.,2.,8.],[7.,2.,8.]], uv([0.44,0.5],[0.56,0.5],[0.56,0.38],[0.44,0.38])),
            q(stick, [0, 1, 1], [[7.,2.,8.],[9.,2.,8.],[9.,9.66,1.57],[7.,9.66,1.57]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0, 1,-1], [[9.,8.37,0.04],[7.,8.37,0.04],[7.,9.66,1.57],[9.,9.66,1.57]], uv([0.56,0.38],[0.44,0.38],[0.44,0.5],[0.56,0.5])),
            q(stick, [1, 0, 0], [[9.,2.,8.],[9.,0.71,6.47],[9.,8.37,0.04],[9.,9.66,1.57]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
        ]
    } else {
        [
            q(stick, [0,-1,-1], [[9.,0.71,9.53],[7.,0.71,9.53],[7.,2.,8.],[9.,2.,8.]], uv([0.56,0.38],[0.44,0.38],[0.44,0.5],[0.56,0.5])),
            q(stick, [-1,0, 0], [[7.,2.,8.],[7.,0.71,9.53],[7.,8.37,15.96],[7.,9.66,14.43]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0,-1, 1], [[7.,0.71,9.53],[9.,0.71,9.53],[9.,8.37,15.96],[7.,8.37,15.96]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0, 1,-1], [[9.,2.,8.],[7.,2.,8.],[7.,9.66,14.43],[9.,9.66,14.43]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
            q(stick, [0, 1, 1], [[7.,8.37,15.96],[9.,8.37,15.96],[9.,9.66,14.43],[7.,9.66,14.43]], uv([0.44,0.5],[0.56,0.5],[0.56,0.38],[0.44,0.38])),
            q(stick, [1, 0, 0], [[9.,0.71,9.53],[9.,2.,8.],[9.,9.66,14.43],[9.,8.37,15.96]], uv([0.44,1.],[0.56,1.],[0.56,0.38],[0.44,0.38])),
        ]
    };
    ModelShape::from_cuboids([]).with_planes(base_planes.into_iter().chain(handle_planes).collect::<Vec<_>>())
}

/// Turns the floor / north-south authored lever onto its mount.
///
/// Bedrock names the mount in `lever_direction`: `up_*` / `down_*` swing along the named axis
/// while the plain cardinal values mount the lever on that wall.
fn lever_mount(direction: &str) -> (i32, CardinalDirection) {
    match direction.trim() {
        "up_north_south" => (0, CardinalDirection::North),
        "up_east_west" => (0, CardinalDirection::East),
        "down_north_south" => (2, CardinalDirection::North),
        "down_east_west" => (2, CardinalDirection::East),
        "north" => (1, CardinalDirection::North),
        "south" => (1, CardinalDirection::South),
        "east" => (1, CardinalDirection::East),
        "west" => (1, CardinalDirection::West),
        _ => (0, CardinalDirection::North),
    }
}

fn px(value: f32) -> f32 {
    value / 16.0
}

fn uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    // Bottom-left corner first, matching the renderer's face order; the authored rects measure
    // V downwards, so the pair is emitted inverted.
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

/// Bedrock tripwire hook. Geometry and UVs come from the official JE
/// `tripwire_hook[_attached][_on]` models element by element (BE ships the same
/// `trip_wire_source` art, so the windows transfer 1:1). The model is authored facing north
/// with the plate against the south wall; it turns onto the state's facing like the repeaters.
/// Four variants keyed by `(attached_bit, powered_bit)`:
///
/// * idle: the head box leans back 45° about X, the arm leans forward 45°.
/// * powered (`powered_bit`): the head drops flat (y 4.2..5.0) and the arm settles to -22.5°.
/// * attached (`attached_bit`): the head sits at -22.5° and a sagging wire runs from the north
///   edge of the cell to the hook tip.
/// * attached + powered: flat head at y 3.4..4.2, arm at -22.5°, wire from y 0.46.
///
/// The rotated boxes cannot be expressed as axis-aligned cuboids, so every rotated face is
/// emitted as a quad with the element's own UV window; the plate stays a cuboid. Material
/// slots follow the `blocks.json` face map: hook parts read `north` (`trip_wire_source`), the
/// plate reads `down` (`trip_wire_base` = oak planks), the attached wire reads `south`
/// (`trip_wire`).
pub(crate) fn tripwire_hook_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let attached = state_bool(state, "attached_bit").unwrap_or(false);
    let powered = state_bool(state, "powered_bit").unwrap_or(false);

    let hook_uv = |uv: [f32; 4]| {
        crate::model_family::shape::rect_uv(
            uv[0] / 16.0,
            uv[1] / 16.0,
            uv[2] / 16.0,
            uv[3] / 16.0,
        )
    };
    const PLATE_SLOT: &str = "down";
    const WIRE_SLOT: &str = "south";
    let plane = |corners: [[f32; 3]; 4], normal: [i32; 3], uv: [f32; 4], slot: &'static str| {
        // The official model numbers are authoring pixels; the shape space is blocks.
        let corners = corners.map(|corner| [px(corner[0]), px(corner[1]), px(corner[2])]);
        ModelPlane::new(corners, normal)
            .with_uv(hook_uv(uv))
            .with_material_slot(slot)
    };

    // Head box (6.2,?,6.7)-(9.8,?,10.3) under each variant's tilt; `head_y` is the bottom.
    // The rotation origins are the official per-variant `rotation.origin` values: the idle
    // dangle turns about (y 6, z 5.2), the attached tilt about (y 4.2, z 6.7).
    let (head_y, head_deg, head_origin_y, head_origin_z) = match (attached, powered) {
        (false, false) => (3.8, -45.0, 6.0, 5.2),
        (false, true) => (4.2, 0.0, 6.0, 5.2),
        (true, false) => (4.2, -22.5, 4.2, 6.7),
        (true, true) => (3.4, 0.0, 4.2, 6.7),
    };
    let arm_deg = match (attached, powered) {
        // Official per-variant arm rotations: idle hangs 45° up, the attached upright arm
        // settles to 0°, and a powered hook pulls the arm to -22.5° in both attach states.
        (_, true) => -22.5,
        (true, false) => 0.0,
        (false, false) => 45.0,
    };
    let mut planes = Vec::new();
    push_rotated_box(
        &mut planes,
        &plane,
        [6.2, head_y, 6.7],
        [9.8, head_y + 0.8, 10.3],
        head_deg,
        head_origin_y,
        head_origin_z,
    );
    if head_deg != 0.0 {
        push_head_edge_cards(&mut planes, &plane, head_y, head_deg, head_origin_y, head_origin_z);
    }
    // Arm box (7.4,5.2,10)-(8.8,6.8,14), rotated about its top-front edge (y 6, z 14).
    push_rotated_box(
        &mut planes,
        &plane,
        [7.4, 5.2, 10.0],
        [8.8, 6.8, 14.0],
        arm_deg,
        6.0,
        14.0,
    );
    if attached {
        // The sagging wire: a 2 px band rotated -22.5° about the cell's bottom-north edge,
        // running from the north edge to the hook tip. Its two cap faces carry the 16 x 2
        // wire band windows; the wire hangs at the variant's sag height.
        let wire_y = if powered { 0.5 } else { 1.5 };
        let (down, up) = wire_quads(wire_y);
        planes.push(plane(down, [0, -1, 0], [16.0, 6.0, 0.0, 8.0], WIRE_SLOT));
        planes.push(plane(up, [0, 1, 0], [0.0, 6.0, 16.0, 8.0], WIRE_SLOT));
    }
    // Mounting plate: an axis-aligned cuboid against the south wall with the JE plate
    // windows (the plate element (6,1,14)-(10,9,16)).
    let plate = ModelCuboid::new([0.375, 0.0625, 0.875], [0.625, 0.5625, 1.0])
        .with_face_material_slot(BlockFace::Up, PLATE_SLOT)
        .with_face_material_slot(BlockFace::Down, PLATE_SLOT)
        .with_face_material_slot(BlockFace::North, PLATE_SLOT)
        .with_face_material_slot(BlockFace::South, PLATE_SLOT)
        .with_face_material_slot(BlockFace::East, PLATE_SLOT)
        .with_face_material_slot(BlockFace::West, PLATE_SLOT)
        .with_face_uv(BlockFace::Down, hook_uv([6.0, 14.0, 10.0, 16.0]))
        .with_face_uv(BlockFace::Up, hook_uv([6.0, 0.0, 10.0, 2.0]))
        .with_face_uv(BlockFace::North, hook_uv([6.0, 7.0, 10.0, 15.0]))
        .with_face_uv(BlockFace::South, hook_uv([6.0, 7.0, 10.0, 15.0]))
        .with_face_uv(BlockFace::West, hook_uv([0.0, 7.0, 2.0, 15.0]))
        .with_face_uv(BlockFace::East, hook_uv([14.0, 7.0, 16.0, 15.0]));
    let shape = ModelShape::from_cuboids(vec![plate]).with_planes(planes);
    rotate_shape_cardinal(shape, direction)
}

/// Emits the six faces of a box rotated `deg` about the X axis through `(origin_y, origin_z)`,
/// each face sampling the official hook-art windows. Corner winding matches the cuboid faces.
fn push_rotated_box(
    planes: &mut Vec<ModelPlane>,
    plane: &impl Fn([[f32; 3]; 4], [i32; 3], [f32; 4], &'static str) -> ModelPlane,
    min: [f32; 3],
    max: [f32; 3],
    deg: f32,
    origin_y: f32,
    origin_z: f32,
) {
    let rotate = |corner: [f32; 3]| {
        let radians = deg.to_radians();
        let (sin, cos) = radians.sin_cos();
        let (dy, dz) = (corner[1] - origin_y, corner[2] - origin_z);
        [
            corner[0],
            origin_y + dy * cos - dz * sin,
            origin_z + dy * sin + dz * cos,
        ]
    };
    // The GPU voxel templates only accept the six axis neighbour offsets, and the opaque
    // culling projects that neighbour one cell out. A tilted face's true normal is diagonal,
    // so pick its dominant axis (ties break toward the horizontal component, whose neighbour
    // is air instead of the mount block above/below) — keeping the authored axis normal
    // culled the grey head art against whatever sits straight above/below the hook.
    let face_normal = |authored: [i32; 3]| -> [i32; 3] {
        if deg.abs() < 0.001 {
            return authored;
        }
        let radians = deg.to_radians();
        let (sin, cos) = radians.sin_cos();
        let ry = authored[1] as f32 * cos - authored[2] as f32 * sin;
        let rz = authored[1] as f32 * sin + authored[2] as f32 * cos;
        if ry.abs() > rz.abs() {
            [0, if ry > 0.0 { 1 } else { -1 }, 0]
        } else {
            [0, 0, if rz > 0.0 { 1 } else { -1 }]
        }
    };
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let faces: [([i32; 3], [f32; 4], [[f32; 3]; 4]); 6] = [
        (
            [0, -1, 0],
            [5.0, 3.0, 11.0, 9.0],
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        ),
        (
            [0, 1, 0],
            [5.0, 3.0, 11.0, 9.0],
            [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
        ),
        (
            [0, 0, -1],
            [5.0, 3.0, 11.0, 4.0],
            [[x0, y1, z0], [x1, y1, z0], [x1, y0, z0], [x0, y0, z0]],
        ),
        (
            [0, 0, 1],
            [5.0, 8.0, 11.0, 9.0],
            [[x1, y1, z1], [x0, y1, z1], [x0, y0, z1], [x1, y0, z1]],
        ),
        (
            [-1, 0, 0],
            [5.0, 8.0, 11.0, 9.0],
            [[x0, y1, z1], [x0, y1, z0], [x0, y0, z0], [x0, y0, z1]],
        ),
        (
            [1, 0, 0],
            [5.0, 3.0, 11.0, 4.0],
            [[x1, y1, z0], [x1, y1, z1], [x1, y0, z1], [x1, y0, z0]],
        ),
    ];
    for (normal, uv, corners) in faces {
        planes.push(plane(corners.map(rotate), face_normal(normal), uv, "north"));
    }
}

/// The head's four zero-thickness hole-wall cards (JE elements 2..5): the stick-hole through
/// the ring — a north plane at z 9.1, a south plane at z 7.9, an east plane at x 7.4 and a
/// west plane at x 8.6, each a 2 x 2 px window riding the rotated plane. The official UVs
/// alternate rows: north/east read row 8, south/west row 3.
fn push_head_edge_cards(
    planes: &mut Vec<ModelPlane>,
    plane: &impl Fn([[f32; 3]; 4], [i32; 3], [f32; 4], &'static str) -> ModelPlane,
    head_y: f32,
    deg: f32,
    origin_y: f32,
    origin_z: f32,
) {
    let rotate = |corner: [f32; 3]| {
        let radians = deg.to_radians();
        let (sin, cos) = radians.sin_cos();
        let (dy, dz) = (corner[1] - origin_y, corner[2] - origin_z);
        [
            corner[0],
            origin_y + dy * cos - dz * sin,
            origin_z + dy * sin + dz * cos,
        ]
    };
    let y0 = head_y;
    let y1 = head_y + 0.8;
    let cards: [([f32; 4], [i32; 3], [[f32; 3]; 4]); 4] = [
        (
            [7.0, 8.0, 9.0, 9.0],
            [0, 0, -1],
            [[7.4, y1, 9.1], [8.6, y1, 9.1], [8.6, y0, 9.1], [7.4, y0, 9.1]],
        ),
        (
            [7.0, 3.0, 9.0, 4.0],
            [0, 0, 1],
            [[7.4, y1, 7.9], [8.6, y1, 7.9], [8.6, y0, 7.9], [7.4, y0, 7.9]],
        ),
        (
            [7.0, 8.0, 9.0, 9.0],
            [1, 0, 0],
            [[7.4, y1, 9.1], [7.4, y1, 7.9], [7.4, y0, 7.9], [7.4, y0, 9.1]],
        ),
        (
            [7.0, 3.0, 9.0, 4.0],
            [-1, 0, 0],
            [[8.6, y1, 9.1], [8.6, y1, 7.9], [8.6, y0, 7.9], [8.6, y0, 9.1]],
        ),
    ];
    for (uv, normal, corners) in cards {
        planes.push(plane(corners.map(rotate), normal, uv, "north"));
    }
}

/// The attached tripwire line (JE element 0): a 0.5 px wide, 6.7 px long band rotated
/// -22.5° about X through the cell's bottom-north corner, running from the north edge down
/// to the hook tip. Its up/down faces sample the string band (trip_wire rows 6..8) rotated
/// 90° so the art runs along the wire, the north copy mirrored.
fn wire_quads(wire_y: f32) -> ([[f32; 3]; 4], [[f32; 3]; 4]) {
    let radians = (-22.5f32).to_radians();
    let (sin, cos) = radians.sin_cos();
    let rotated_y = |z: f32| wire_y * cos - z * sin;
    let rotated_z = |z: f32| wire_y * sin + z * cos;
    // The wire box spans x 7.75..8.25 (0.5 px wide), y wire_y..wire_y+0.5, z 0..6.7.
    // Corners run length-first (z 0→6.7 then across the width) so the quad's U axis is the
    // wire's length — the official rot-90 band windows then read the string art along it.
    let band = |delta_y: f32| {
        [
            [7.75, rotated_y(0.0) + delta_y, rotated_z(0.0)],
            [7.75, rotated_y(6.7) + delta_y, rotated_z(6.7)],
            [8.25, rotated_y(6.7) + delta_y, rotated_z(6.7)],
            [8.25, rotated_y(0.0) + delta_y, rotated_z(0.0)],
        ]
    };
    (band(0.0), band(0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lever handle reproduces the captured sheared cross-section (refs 725 closed /
    /// 733 open): a rigidly rotated box cannot match, so the corner set is asserted directly.
    #[test]
    fn lever_handle_matches_the_captured_cross_section() {
        let closed = lever_shape(&BlockStateQuery::new("minecraft:lever"));
        // 12 planes = 6 base faces + 6 handle faces (all from the captured mesh).
        assert_eq!(closed.planes.len(), 12);
        let handle = |shape: &ModelShape| {
            let ys: Vec<f32> = shape
                .planes
                .iter()
                .filter(|p| p.material_slot.as_deref() == Some("lever"))
                .flat_map(|plane| plane.corners.iter().map(|corner| corner[1]))
                .collect();
            let zs: Vec<f32> = shape
                .planes
                .iter()
                .filter(|p| p.material_slot.as_deref() == Some("lever"))
                .flat_map(|plane| plane.corners.iter().map(|corner| corner[2]))
                .collect();
            (ys, zs)
        };
        let (ys, zs) = handle(&closed);
        let (y_min, y_max) = (
            ys.iter().cloned().fold(f32::INFINITY, f32::min),
            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        let (z_min, z_max) = (
            zs.iter().cloned().fold(f32::INFINITY, f32::min),
            zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        // Captured extents in pixels: y 0.72..9.66, z 8.0..15.96.
        assert!((y_min - 0.7152 / 16.0).abs() < 0.001, "y_min {y_min}");
        assert!((y_max - 9.6616 / 16.0).abs() < 0.001, "y_max {y_max}");
        assert!((z_min - 8.0 / 16.0).abs() < 0.001, "z_min {z_min}");
        assert!((z_max - 15.96 / 16.0).abs() < 0.001, "z_max {z_max}");

        // The open lever leans north instead of south.
        let open = lever_shape(
            &BlockStateQuery::new("minecraft:lever").with_state("open_bit", true),
        );
        let (_, open_zs) = handle(&open);
        let open_z_max = open_zs.into_iter().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (open_z_max - 8.0 / 16.0).abs() < 0.001,
            "open handle should stop at the pivot plane, got {open_z_max}"
        );
    }
}
