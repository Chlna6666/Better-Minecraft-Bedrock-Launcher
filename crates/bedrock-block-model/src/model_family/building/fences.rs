use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, direction_connected, state_bool,
};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, apply_jmc_box_uv, detail_cuboid_with_local_uv,
    projected_cuboid_with_uv, uv16,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_fence_gate") || name == "fence_gate" {
        return Some(ModelFamily::FenceGate);
    }
    if name.ends_with("_fence") || matches!(name, "fence" | "nether_brick_fence") {
        return Some(ModelFamily::Fence);
    }
    if name.ends_with("_chain") || name == "chain" {
        return Some(ModelFamily::Chain);
    }
    None
}

pub(crate) fn chain_shape(state: &BlockStateQuery) -> ModelShape {
    let axis = crate::model_family::direction::state_string(state, "pillar_axis")
        .or_else(|| crate::model_family::direction::state_string(state, "axis"))
        .unwrap_or("y");
    let cuboids = match axis {
        "x" => vec![
            ModelCuboid::new([0.0, 0.375, 0.40625], [1.0, 0.625, 0.59375]),
            ModelCuboid::new([0.0, 0.40625, 0.375], [1.0, 0.59375, 0.625]),
        ],
        "z" => vec![
            ModelCuboid::new([0.40625, 0.375, 0.0], [0.59375, 0.625, 1.0]),
            ModelCuboid::new([0.375, 0.40625, 0.0], [0.625, 0.59375, 1.0]),
        ],
        _ => vec![
            ModelCuboid::new([0.375, 0.0, 0.40625], [0.625, 1.0, 0.59375]),
            ModelCuboid::new([0.40625, 0.0, 0.375], [0.59375, 1.0, 0.625]),
        ],
    };
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn fence_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let is_bamboo = name == "bamboo_fence";
    let mut cuboids = Vec::with_capacity(9);
    cuboids.push(fence_post_cuboid(is_bamboo));
    for direction in CardinalDirection::ALL {
        if direction_connected(state, direction) {
            cuboids.push(fence_arm_cuboid_with_uv(
                direction, 0.375, 0.5625, is_bamboo,
            ));
            cuboids.push(fence_arm_cuboid_with_uv(direction, 0.75, 0.9375, is_bamboo));
        }
    }
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn fence_gate_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let open = state_bool(state, "open_bit").unwrap_or(false);
    let axis = fence_gate_axis(direction, open);
    let y_offset = if state_bool(state, "in_wall_bit").unwrap_or(false) {
        -3.0
    } else {
        0.0
    };

    if name == "bamboo_fence_gate" {
        bamboo_fence_gate_shape(open, axis, y_offset)
    } else {
        normal_fence_gate_shape(open, axis, y_offset)
    }
}

#[derive(Clone, Copy)]
enum GateAxis {
    X,
    Z,
}

#[derive(Clone, Copy)]
enum BambooGateUv {
    All([f32; 2]),
    Sides {
        side: [f32; 2],
        up: [f32; 2],
        down: [f32; 2],
    },
}

fn fence_post_cuboid(is_bamboo: bool) -> ModelCuboid {
    let cuboid = ModelCuboid::new([0.375, 0.0, 0.375], [0.625, 1.0, 0.625]);
    if !is_bamboo {
        return detail_slots(projected_cuboid_with_uv(cuboid));
    }
    let top_uv = uv16(4.0, 0.0, 8.0, 4.0);
    let side_uv = uv16(0.0, 0.0, 4.0, 16.0);
    detail_slots(apply_jmc_box_uv(cuboid, top_uv, side_uv))
}

fn normal_fence_gate_shape(open: bool, axis: GateAxis, y_offset: f32) -> ModelShape {
    let mut cuboids = Vec::with_capacity(if open { 8 } else { 5 });
    push_normal_gate_piece(
        &mut cuboids,
        axis,
        y_offset,
        [0.0, 5.0, 7.0],
        [2.0, 11.0, 2.0],
    );
    push_normal_gate_piece(
        &mut cuboids,
        axis,
        y_offset,
        [14.0, 5.0, 7.0],
        [2.0, 11.0, 2.0],
    );

    if open {
        for (pos, size) in [
            ([0.0, 6.0, 9.0], [2.0, 3.0, 6.0]),
            ([0.0, 12.0, 9.0], [2.0, 3.0, 6.0]),
            ([0.0, 9.0, 13.0], [2.0, 3.0, 2.0]),
            ([14.0, 6.0, 9.0], [2.0, 3.0, 6.0]),
            ([14.0, 12.0, 9.0], [2.0, 3.0, 6.0]),
            ([14.0, 9.0, 13.0], [2.0, 3.0, 2.0]),
        ] {
            push_normal_gate_piece(&mut cuboids, axis, y_offset, pos, size);
        }
    } else {
        for (pos, size) in [
            ([2.0, 6.0, 7.0], [12.0, 3.0, 2.0]),
            ([2.0, 12.0, 7.0], [12.0, 3.0, 2.0]),
            ([6.0, 9.0, 7.0], [4.0, 3.0, 2.0]),
        ] {
            push_normal_gate_piece(&mut cuboids, axis, y_offset, pos, size);
        }
    }
    ModelShape::from_cuboids(cuboids)
}

fn bamboo_fence_gate_shape(open: bool, axis: GateAxis, y_offset: f32) -> ModelShape {
    let mut cuboids = Vec::with_capacity(8);
    push_bamboo_gate_posts(&mut cuboids, axis, y_offset);

    if open {
        push_open_bamboo_gate_pieces(&mut cuboids, axis, y_offset);
    } else {
        push_closed_bamboo_gate_pieces(&mut cuboids, axis, y_offset);
    }
    ModelShape::from_cuboids(cuboids)
}

fn push_bamboo_gate_posts(cuboids: &mut Vec<ModelCuboid>, axis: GateAxis, y_offset: f32) {
    for pos in [[0.0, 5.0, 7.0], [14.0, 5.0, 7.0]] {
        push_bamboo_gate_piece(
            cuboids,
            axis,
            y_offset,
            pos,
            [2.0, 11.0, 2.0],
            BambooGateUv::All([0.0, 2.0]),
        );
    }
}

fn push_open_bamboo_gate_pieces(cuboids: &mut Vec<ModelCuboid>, axis: GateAxis, y_offset: f32) {
    let rail_uv = BambooGateUv::Sides {
        side: [3.0, 9.0],
        up: [3.0, 1.0],
        down: [3.0, 1.0],
    };
    for (pos, size, uv) in [
        ([0.0, 6.0, 9.0], [2.0, 3.0, 4.0], rail_uv),
        ([0.0, 12.0, 9.0], [2.0, 3.0, 4.0], rail_uv),
        (
            [0.0, 6.0, 13.0],
            [2.0, 9.0, 2.0],
            BambooGateUv::All([8.0, 3.0]),
        ),
        ([14.0, 6.0, 9.0], [2.0, 3.0, 4.0], rail_uv),
        ([14.0, 12.0, 9.0], [2.0, 3.0, 4.0], rail_uv),
        (
            [14.0, 6.0, 13.0],
            [2.0, 9.0, 2.0],
            BambooGateUv::All([8.0, 3.0]),
        ),
    ] {
        push_bamboo_gate_piece(cuboids, axis, y_offset, pos, size, uv);
    }
}

fn push_closed_bamboo_gate_pieces(cuboids: &mut Vec<ModelCuboid>, axis: GateAxis, y_offset: f32) {
    for (pos, size, uv) in [
        (
            [2.0, 6.0, 7.0],
            [4.0, 3.0, 2.0],
            BambooGateUv::All([3.0, 9.0]),
        ),
        (
            [2.0, 12.0, 7.0],
            [4.0, 3.0, 2.0],
            BambooGateUv::All([3.0, 9.0]),
        ),
        (
            [6.0, 6.0, 7.0],
            [2.0, 9.0, 2.0],
            BambooGateUv::All([8.0, 3.0]),
        ),
        (
            [8.0, 6.0, 7.0],
            [2.0, 9.0, 2.0],
            BambooGateUv::All([8.0, 3.0]),
        ),
        (
            [10.0, 6.0, 7.0],
            [4.0, 3.0, 2.0],
            BambooGateUv::All([3.0, 9.0]),
        ),
        (
            [10.0, 12.0, 7.0],
            [4.0, 3.0, 2.0],
            BambooGateUv::All([3.0, 9.0]),
        ),
    ] {
        push_bamboo_gate_piece(cuboids, axis, y_offset, pos, size, uv);
    }
}

fn fence_gate_axis(direction: CardinalDirection, open: bool) -> GateAxis {
    let closed_along_x = matches!(
        direction,
        CardinalDirection::North | CardinalDirection::South
    );
    if closed_along_x == open {
        GateAxis::Z
    } else {
        GateAxis::X
    }
}

fn push_normal_gate_piece(
    cuboids: &mut Vec<ModelCuboid>,
    axis: GateAxis,
    y_offset: f32,
    pos: [f32; 3],
    size: [f32; 3],
) {
    let (cuboid, _) = gate_piece_cuboid(axis, y_offset, pos, size);
    cuboids.push(detail_slots(detail_cuboid_with_local_uv(cuboid)));
}

fn push_bamboo_gate_piece(
    cuboids: &mut Vec<ModelCuboid>,
    axis: GateAxis,
    y_offset: f32,
    pos: [f32; 3],
    size: [f32; 3],
    uv: BambooGateUv,
) {
    let (cuboid, oriented_size) = gate_piece_cuboid(axis, y_offset, pos, size);
    cuboids.push(bamboo_gate_cuboid_with_uv(cuboid, oriented_size, uv));
}

fn gate_piece_cuboid(
    axis: GateAxis,
    y_offset: f32,
    pos: [f32; 3],
    size: [f32; 3],
) -> (ModelCuboid, [f32; 3]) {
    let min = [pos[0], pos[1] + y_offset, pos[2]];
    let max = [
        pos[0] + size[0],
        pos[1] + y_offset + size[1],
        pos[2] + size[2],
    ];
    let (min, max, oriented_size) = match axis {
        GateAxis::X => (min, max, size),
        GateAxis::Z => (
            [min[2], min[1], min[0]],
            [max[2], max[1], max[0]],
            [size[2], size[1], size[0]],
        ),
    };
    (
        ModelCuboid::new(
            [px(min[0]), px(min[1]), px(min[2])],
            [px(max[0]), px(max[1]), px(max[2])],
        ),
        oriented_size,
    )
}

fn bamboo_gate_cuboid_with_uv(
    cuboid: ModelCuboid,
    pixel_size: [f32; 3],
    uv: BambooGateUv,
) -> ModelCuboid {
    let (side_origin, up_origin, down_origin) = match uv {
        BambooGateUv::All(origin) => (origin, origin, origin),
        BambooGateUv::Sides { side, up, down } => (side, up, down),
    };
    detail_slots(cuboid)
        .with_face_uv(
            BlockFace::Up,
            uv_rect(up_origin, pixel_size[0], pixel_size[2]),
        )
        .with_face_uv(
            BlockFace::Down,
            uv_rect(down_origin, pixel_size[0], pixel_size[2]),
        )
        .with_face_uv(
            BlockFace::North,
            uv_rect(side_origin, pixel_size[0], pixel_size[1]),
        )
        .with_face_uv(
            BlockFace::South,
            uv_rect(side_origin, pixel_size[0], pixel_size[1]),
        )
        .with_face_uv(
            BlockFace::West,
            uv_rect(side_origin, pixel_size[2], pixel_size[1]),
        )
        .with_face_uv(
            BlockFace::East,
            uv_rect(side_origin, pixel_size[2], pixel_size[1]),
        )
}

fn fence_arm_cuboid_with_uv(
    direction: CardinalDirection,
    min_y: f32,
    max_y: f32,
    is_bamboo: bool,
) -> ModelCuboid {
    let cuboid = fence_arm_cuboid(direction, min_y, max_y);
    if !is_bamboo {
        return detail_slots(projected_cuboid_with_uv(cuboid));
    }
    let top_uv = uv16(4.0, 0.0, 10.0, 2.0);
    let side_uv = uv16(0.0, 0.0, 8.0, 3.0);
    detail_slots(apply_jmc_box_uv(cuboid, top_uv, side_uv))
}

fn fence_arm_cuboid(direction: CardinalDirection, min_y: f32, max_y: f32) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.4375, min_y, 0.0], [0.5625, max_y, 0.5]),
        CardinalDirection::South => ModelCuboid::new([0.4375, min_y, 0.5], [0.5625, max_y, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5, min_y, 0.4375], [1.0, max_y, 0.5625]),
        CardinalDirection::West => ModelCuboid::new([0.0, min_y, 0.4375], [0.5, max_y, 0.5625]),
    }
}

fn detail_slots(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::East, "side")
}

fn uv_rect(origin: [f32; 2], width: f32, height: f32) -> [[f32; 2]; 4] {
    uv16(origin[0], origin[1], origin[0] + width, origin[1] + height)
}

fn px(value: f32) -> f32 {
    value / 16.0
}
