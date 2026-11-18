use crate::material::BlockFace;
use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_i64};
use crate::model_family::shape::{ModelCuboid, ModelShape, rect_uv, rotate_cuboid_cardinal};
use crate::state::BlockStateQuery;

#[derive(Clone, Copy)]
struct EntityCube {
    origin: [f32; 3],
    size: [f32; 3],
    uv: [f32; 2],
    uv_size: [f32; 3],
}

pub(super) fn family_for(name: &str) -> bool {
    is_copper_golem_statue_name(name)
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let cubes = match pose(state) {
        1 => SITTING,
        2 => RUNNING,
        3 => STAR,
        _ => STANDING,
    };
    ModelShape::from_cuboids(
        cubes
            .iter()
            .map(|cube| rotate_cuboid_cardinal(&entity_cuboid(*cube), direction))
            .collect::<Vec<_>>(),
    )
}

fn is_copper_golem_statue_name(name: &str) -> bool {
    let name = name.strip_prefix("waxed_").unwrap_or(name);
    matches!(
        name,
        "copper_golem_statue"
            | "exposed_copper_golem_statue"
            | "weathered_copper_golem_statue"
            | "oxidized_copper_golem_statue"
    )
}

fn pose(state: &BlockStateQuery) -> i64 {
    [
        "entity.Pose",
        "Pose",
        "pose",
        "block_entity_data.Pose",
        "minecraft:pose",
    ]
    .into_iter()
    .find_map(|key| state_i64(state, key))
    .unwrap_or(0)
    .clamp(0, 3)
}

fn entity_cuboid(cube: EntityCube) -> ModelCuboid {
    let origin = cube.origin;
    let size = cube.size;
    entity_box_uv(
        ModelCuboid::new(
            [
                entity_xz_to_block(origin[0]),
                px(origin[1]),
                entity_xz_to_block(origin[2]),
            ],
            [
                entity_xz_to_block(origin[0] + size[0]),
                px(origin[1] + size[1]),
                entity_xz_to_block(origin[2] + size[2]),
            ],
        )
        .with_material_slot("body"),
        cube.uv,
        cube.uv_size,
    )
}

fn entity_box_uv(cuboid: ModelCuboid, uv: [f32; 2], size: [f32; 3]) -> ModelCuboid {
    let [width, height, depth] = size;
    let [u, v] = uv;
    // Standard Bedrock box unwrap (uv = top-left origin). The four side faces run, left to
    // right: east(+X), south(+Z), west(-X), north(-Z) — with the caps above them:
    //   up      (u+d,       v)     [w × d]
    //   down    (u+d+w,     v)     [w × d]
    //   east    (u,         v+d)   [d × h]
    //   south   (u+d,       v+d)   [w × h]
    //   west    (u+d+w,     v+d)   [d × h]
    //   north   (u+2d+w,    v+d)   [w × h]
    // The previous code put north at u+d and south at u+2d+w — swapped. The golem's front
    // (south, +Z) therefore sampled the back panel art and its face read as mirrored strips.
    cuboid
        .with_face_uv(BlockFace::Up, uv64(u + depth, v, width, depth))
        .with_face_uv(BlockFace::Down, uv64(u + depth + width, v, width, depth))
        .with_face_uv(BlockFace::East, uv64(u, v + depth, depth, height))
        .with_face_uv(BlockFace::South, uv64(u + depth, v + depth, width, height))
        .with_face_uv(BlockFace::West, uv64(u + depth + width, v + depth, depth, height))
        .with_face_uv(
            BlockFace::North,
            uv64(u + depth * 2.0 + width, v + depth, width, height),
        )
}

fn uv64(u: f32, v: f32, width: f32, height: f32) -> [[f32; 2]; 4] {
    rect_uv(u / 64.0, v / 64.0, (u + width) / 64.0, (v + height) / 64.0)
}

fn entity_xz_to_block(value: f32) -> f32 {
    px(value + 8.0)
}

const fn entity_cube(origin: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> EntityCube {
    EntityCube {
        origin,
        size,
        uv,
        uv_size: size,
    }
}

const fn inflated_entity_cube(
    origin: [f32; 3],
    size: [f32; 3],
    uv: [f32; 2],
    inflate: f32,
) -> EntityCube {
    EntityCube {
        origin: [
            origin[0] - inflate,
            origin[1] - inflate,
            origin[2] - inflate,
        ],
        size: [
            size[0] + inflate * 2.0,
            size[1] + inflate * 2.0,
            size[2] + inflate * 2.0,
        ],
        uv,
        uv_size: size,
    }
}

const STANDING: &[EntityCube] = &[
    entity_cube([-4.0, 5.0, -3.0], [8.0, 6.0, 6.0], [0.0, 15.0]),
    entity_cube([-4.0, 11.0, -5.0], [8.0, 5.0, 10.0], [0.0, 0.0]),
    entity_cube([-1.0, 10.0, -6.0], [2.0, 3.0, 2.0], [56.0, 0.0]),
    inflated_entity_cube([-1.0, 16.0, -1.0], [2.0, 4.0, 2.0], [37.0, 8.0], -0.01),
    inflated_entity_cube([-2.0, 20.0, -2.0], [4.0, 4.0, 4.0], [37.0, 0.0], -0.01),
    entity_cube([-7.0, 2.0, -2.0], [3.0, 10.0, 4.0], [36.0, 16.0]),
    entity_cube([4.0, 2.0, -2.0], [3.0, 10.0, 4.0], [50.0, 16.0]),
    entity_cube([-3.9, 0.0, -1.99], [4.0, 5.0, 4.0], [0.0, 27.0]),
    entity_cube([-0.1, 0.0, -2.0], [4.0, 5.0, 4.0], [16.0, 27.0]),
];

const SITTING: &[EntityCube] = &[
    entity_cube([-3.0, 6.0, -2.2], [6.0, 1.0, 6.0], [3.0, 19.0]),
    entity_cube([-4.0, 0.0, -1.2], [8.0, 6.0, 6.0], [0.0, 15.0]),
    entity_cube([-4.0, 1.0, -4.2], [8.0, 6.0, 3.0], [3.0, 18.0]),
    entity_cube([-1.0, 12.0, -1.175], [2.0, 4.0, 2.0], [37.0, 8.0]),
    entity_cube([-2.0, 16.0, -2.175], [4.0, 4.0, 4.0], [37.0, 0.0]),
    entity_cube([-4.0, 7.0, -5.2], [8.0, 5.0, 10.0], [0.0, 0.0]),
    entity_cube([-1.0, 6.0, -6.2], [2.0, 3.0, 2.0], [56.0, 0.0]),
    entity_cube([-7.075, -0.516, -1.3518], [3.0, 10.0, 4.0], [36.0, 16.0]),
    entity_cube([4.075, -0.35426, -1.35548], [3.0, 10.0, 4.0], [50.0, 16.0]),
    entity_cube([-4.05, -1.975, -1.0], [4.0, 5.0, 4.0], [0.0, 27.0]),
    entity_cube([0.05, -1.975, -1.0], [4.0, 5.0, 4.0], [16.0, 27.0]),
];

const RUNNING: &[EntityCube] = &[
    entity_cube([-4.0, 4.8, -3.0], [8.0, 6.0, 6.0], [0.0, 15.0]),
    entity_cube([-4.3, 10.6, -7.0], [8.0, 5.0, 10.0], [0.0, 0.0]),
    entity_cube([-1.3, 9.6, -8.0], [2.0, 3.0, 2.0], [56.0, 0.0]),
    inflated_entity_cube([-1.3, 15.6, -3.0], [2.0, 4.0, 2.0], [37.0, 8.0], -0.01),
    inflated_entity_cube([-2.3, 19.6, -4.0], [4.0, 4.0, 4.0], [37.0, 0.0], -0.01),
    entity_cube([-7.4, 2.0, -3.0], [3.0, 10.0, 4.0], [36.0, 16.0]),
    entity_cube([3.6, 2.0, -2.0], [3.0, 10.0, 4.0], [50.0, 16.0]),
    entity_cube([-3.9, 0.0, -1.99], [4.0, 5.0, 4.0], [0.0, 27.0]),
    entity_cube([-0.1, 0.0, -2.0], [4.0, 5.0, 4.0], [16.0, 27.0]),
];

const STAR: &[EntityCube] = &[
    entity_cube([-4.0, 5.0, -3.0], [8.0, 6.0, 6.0], [0.0, 15.0]),
    entity_cube([-4.0, 11.0, -5.0], [8.0, 5.0, 10.0], [0.0, 0.0]),
    entity_cube([-1.0, 10.0, -6.0], [2.0, 3.0, 2.0], [56.0, 0.0]),
    inflated_entity_cube([-1.0, 16.0, -1.0], [2.0, 4.0, 2.0], [37.0, 8.0], -0.01),
    inflated_entity_cube([-2.0, 20.0, -2.0], [4.0, 4.0, 4.0], [37.0, 0.0], -0.01),
    entity_cube([-4.5, 5.0, -2.0], [3.0, 10.0, 4.0], [36.0, 16.0]),
    entity_cube([1.5, 5.0, -2.0], [3.0, 10.0, 4.0], [50.0, 16.0]),
    entity_cube([-4.65, 0.5, -1.99], [4.0, 5.0, 4.0], [0.0, 27.0]),
    entity_cube([0.65, 0.5, -2.0], [4.0, 5.0, 4.0], [16.0, 27.0]),
];

const fn px(value: f32) -> f32 {
    value / 16.0
}
