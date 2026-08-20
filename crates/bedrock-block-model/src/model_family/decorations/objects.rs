use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name == "cake" || name == "candle_cake" || name.ends_with("_candle_cake") {
        return Some(ModelFamily::Cake);
    }
    if name == "scaffolding" {
        return Some(ModelFamily::Scaffolding);
    }
    if name.ends_with("_shelf") {
        return Some(ModelFamily::Shelf);
    }
    if name == "heavy_core" {
        return Some(ModelFamily::HeavyCore);
    }
    if name == "conduit" {
        return Some(ModelFamily::Conduit);
    }
    if name == "dried_ghast" {
        return Some(ModelFamily::DriedGhast);
    }
    if name == "end_rod" || is_lightning_rod_name(name) {
        return Some(ModelFamily::Rod);
    }
    if name == "sea_pickle" {
        return Some(ModelFamily::SeaPickle);
    }
    if name == "turtle_egg" || name == "sniffer_egg" {
        return Some(ModelFamily::Egg);
    }
    if name == "beacon" {
        return Some(ModelFamily::Beacon);
    }
    if name == "slime" || name == "honey_block" {
        return Some(ModelFamily::InsetBlock);
    }
    if name == "dragon_egg" {
        return Some(ModelFamily::DragonEgg);
    }
    if name == "frame" || name == "glow_frame" {
        return Some(ModelFamily::ItemFrame);
    }
    if is_head_block_name(name) {
        return Some(ModelFamily::Head);
    }
    None
}

pub(crate) fn cake_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let has_candle = name == "candle_cake" || name.ends_with("_candle_cake");
    let mut cuboids = vec![cake_body_cuboid(if has_candle {
        0
    } else {
        state_i64(state, "bite_counter").unwrap_or(0).clamp(0, 6)
    })];

    if has_candle {
        cuboids.push(
            detail_cuboid_with_local_uv(ModelCuboid::new(
                [px(7.0), px(8.0), px(7.0)],
                [px(9.0), px(15.0), px(9.0)],
            ))
            .with_material_slot("candle"),
        );
    }

    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn scaffolding_shape(state: &BlockStateQuery) -> ModelShape {
    let mut cuboids = vec![
        scaffolding_cuboid([0.0, px(14.0), 0.0], [1.0, 1.0, 1.0]),
        side_slot_cuboid([0.0, 0.0, 0.0], [px(2.0), px(14.0), px(2.0)]),
        side_slot_cuboid([px(14.0), 0.0, 0.0], [1.0, px(14.0), px(2.0)]),
        side_slot_cuboid([0.0, 0.0, px(14.0)], [px(2.0), px(14.0), 1.0]),
        side_slot_cuboid([px(14.0), 0.0, px(14.0)], [1.0, px(14.0), 1.0]),
    ];

    if state_i64(state, "stability").unwrap_or(1) > 0 {
        cuboids.extend([
            side_slot_cuboid([px(2.0), 0.0, 0.0], [px(14.0), px(2.0), px(2.0)]),
            side_slot_cuboid([px(2.0), 0.0, px(14.0)], [px(14.0), px(2.0), 1.0]),
            side_slot_cuboid([0.0, 0.0, px(2.0)], [px(2.0), px(2.0), px(14.0)]),
            side_slot_cuboid([px(14.0), 0.0, px(2.0)], [1.0, px(2.0), px(14.0)]),
        ]);
    }

    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn shelf_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    let cuboids = [
        ModelCuboid::new([0.0, 0.0, px(13.0)], [1.0, 1.0, 1.0]),
        ModelCuboid::new([0.0, 0.0, px(11.0)], [1.0, px(4.0), px(13.0)]),
        ModelCuboid::new([0.0, px(12.0), px(11.0)], [1.0, 1.0, px(13.0)]),
    ]
    .into_iter()
    .map(|cuboid| detail_cuboid_with_local_uv(rotate_cuboid_from_south(cuboid, direction)))
    .collect::<Vec<_>>();

    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn heavy_core_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(4.0), 0.0, px(4.0)],
        [px(12.0), px(8.0), px(12.0)],
    ))])
}

pub(crate) fn conduit_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(5.0), 0.0, px(5.0)],
        [px(11.0), px(6.0), px(11.0)],
    ))])
}

pub(crate) fn dried_ghast_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    let mut cuboids = vec![face_slot_cuboid(
        [px(3.0), 0.0, px(3.0)],
        [px(13.0), px(10.0), px(13.0)],
    )];
    cuboids.extend([
        ModelCuboid::new([0.0, 0.0, px(5.0)], [px(3.0), px(1.0), px(7.0)]),
        ModelCuboid::new([0.0, 0.0, px(9.0)], [px(3.0), px(1.0), px(11.0)]),
        ModelCuboid::new([px(13.0), 0.0, px(5.0)], [1.0, px(1.0), px(7.0)]),
        ModelCuboid::new([px(13.0), 0.0, px(9.0)], [1.0, px(1.0), px(11.0)]),
        ModelCuboid::new([px(5.0), 0.0, 0.0], [px(7.0), px(1.0), px(3.0)]),
        ModelCuboid::new([px(9.0), 0.0, 0.0], [px(11.0), px(1.0), px(3.0)]),
    ]);

    ModelShape::from_cuboids(
        cuboids
            .into_iter()
            .map(|cuboid| detail_cuboid_with_local_uv(rotate_cuboid_from_south(cuboid, direction)))
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn rod_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let local = if is_lightning_rod_name(name) {
        vec![
            ModelCuboid::new([px(7.0), 0.0, px(7.0)], [px(9.0), px(12.0), px(9.0)]),
            ModelCuboid::new([px(6.0), px(12.0), px(6.0)], [px(10.0), 1.0, px(10.0)]),
        ]
    } else {
        vec![
            ModelCuboid::new([px(6.0), 0.0, px(6.0)], [px(10.0), px(1.0), px(10.0)]),
            ModelCuboid::new([px(7.0), px(1.0), px(7.0)], [px(9.0), 1.0, px(9.0)]),
        ]
    };

    ModelShape::from_cuboids(
        local
            .into_iter()
            .map(|cuboid| {
                detail_cuboid_with_local_uv(orient_cuboid_from_up(cuboid, facing_from_state(state)))
            })
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn sea_pickle_shape(state: &BlockStateQuery) -> ModelShape {
    let count = state_i64(state, "cluster_count").unwrap_or(0).clamp(0, 3) + 1;
    let pickles: &[(f32, f32, f32, f32)] = match count {
        1 => &[(8.0, 8.0, 6.0, 6.0)],
        2 => &[(10.5, 5.5, 6.0, 6.0), (5.5, 10.5, 4.0, 5.0)],
        3 => &[
            (6.5, 5.5, 6.0, 6.0),
            (8.0, 11.25, 4.0, 5.0),
            (11.25, 4.75, 7.0, 7.0),
        ],
        _ => &[
            (11.5, 5.0, 6.0, 6.0),
            (4.5, 12.0, 4.0, 5.0),
            (12.0, 10.75, 7.0, 7.0),
            (5.0, 4.5, 5.0, 5.0),
        ],
    };

    ModelShape::from_cuboids(
        pickles
            .iter()
            .map(|(x, z, width, height)| {
                let half = width * 0.5;
                detail_cuboid_with_local_uv(ModelCuboid::new(
                    [px(x - half), 0.0, px(z - half)],
                    [px(x + half), px(*height), px(z + half)],
                ))
            })
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn egg_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "sniffer_egg" {
        return ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(1.0), 0.0, px(2.0)],
            [px(15.0), 1.0, px(14.0)],
        ))]);
    }

    let count = match state_string(state, "turtle_egg_count").unwrap_or("one_egg") {
        "two_egg" => 2,
        "three_egg" => 3,
        "four_egg" => 4,
        _ => 1,
    };
    let mut cuboids = vec![detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(6.0), 0.0, px(4.0)],
        [px(11.0), px(7.0), px(9.0)],
    ))];
    if count >= 2 {
        cuboids.push(detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(11.0), 0.0, px(7.0)],
            [px(15.0), px(5.0), px(11.0)],
        )));
    }
    if count >= 3 {
        cuboids.push(detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(2.0), 0.0, px(7.0)],
            [px(5.0), px(4.0), px(10.0)],
        )));
    }
    if count >= 4 {
        cuboids.push(detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(6.0), 0.0, px(10.0)],
            [px(9.0), px(3.0), px(13.0)],
        )));
    }
    ModelShape::from_cuboids(cuboids)
}

pub(crate) fn beacon_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(
            ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
                .with_face_material_slot(BlockFace::Side, "side"),
        ),
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(2.0), 0.0, px(2.0)], [px(14.0), px(3.0), px(14.0)])
                .with_material_slot("down"),
        ),
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(3.0), px(3.0), px(3.0)], [px(13.0), px(14.0), px(13.0)])
                .with_material_slot("up"),
        ),
    ])
}

pub(crate) fn inset_block_shape(name: &str) -> ModelShape {
    let inset = if name == "honey_block" { 1.0 } else { 3.0 };
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(inset), px(inset), px(inset)],
            [px(16.0 - inset), px(16.0 - inset), px(16.0 - inset)],
        )),
    ])
}

pub(crate) fn dragon_egg_shape() -> ModelShape {
    ModelShape::from_cuboids([
        egg_layer(6.0, 15.0, 4.0, 1.0),
        egg_layer(5.0, 13.0, 6.0, 2.0),
        egg_layer(3.0, 11.0, 10.0, 2.0),
        egg_layer(2.0, 8.0, 12.0, 3.0),
        egg_layer(1.0, 3.0, 14.0, 5.0),
        egg_layer(2.0, 1.0, 12.0, 2.0),
        egg_layer(3.0, 0.0, 10.0, 1.0),
    ])
}

pub(crate) fn item_frame_shape(state: &BlockStateQuery) -> ModelShape {
    let facing = facing_from_state(state);
    let cuboids = match facing {
        Facing::Up => vec![
            ModelCuboid::new([px(2.0), 0.0, px(2.0)], [px(14.0), px(1.0), px(14.0)]),
            ModelCuboid::new([px(3.0), px(1.0), px(3.0)], [px(13.0), px(1.5), px(13.0)]),
        ],
        Facing::Down => vec![
            ModelCuboid::new([px(2.0), px(15.0), px(2.0)], [px(14.0), 1.0, px(14.0)]),
            ModelCuboid::new([px(3.0), px(14.5), px(3.0)], [px(13.0), px(15.0), px(13.0)]),
        ],
        _ => {
            let direction = cardinal_for_facing(facing).unwrap_or(CardinalDirection::South);
            local_item_frame_cuboids()
                .into_iter()
                .map(|cuboid| rotate_cuboid_from_south(cuboid, direction))
                .collect::<Vec<_>>()
        }
    };
    ModelShape::from_cuboids(
        cuboids
            .into_iter()
            .map(detail_cuboid_with_local_uv)
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn head_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let facing = facing_from_state(state);
    let size = if name == "dragon_head" { 12.0 } else { 8.0 };
    let standing = ModelCuboid::new(
        [px((16.0 - size) * 0.5), 0.0, px((16.0 - size) * 0.5)],
        [px((16.0 + size) * 0.5), px(size), px((16.0 + size) * 0.5)],
    );
    let cuboid = match facing {
        Facing::Up => standing,
        Facing::Down => ModelCuboid::new(
            [
                px((16.0 - size) * 0.5),
                px(16.0 - size),
                px((16.0 - size) * 0.5),
            ],
            [px((16.0 + size) * 0.5), 1.0, px((16.0 + size) * 0.5)],
        ),
        _ => {
            let direction = cardinal_for_facing(facing).unwrap_or(CardinalDirection::South);
            rotate_cuboid_from_south(
                ModelCuboid::new(
                    [px((16.0 - size) * 0.5), px(4.0), px(16.0 - size)],
                    [px((16.0 + size) * 0.5), px(4.0 + size), 1.0],
                ),
                direction,
            )
        }
    };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(cuboid)])
}

fn cake_body_cuboid(bites: i64) -> ModelCuboid {
    let bite_width = match bites {
        0 => 0.0,
        1 => 2.0,
        2 => 4.0,
        3 => 6.0,
        4 => 8.0,
        5 => 10.0,
        _ => 12.0,
    };
    face_slot_cuboid(
        [px(1.0), 0.0, px(1.0)],
        [px(15.0 - bite_width), px(8.0), px(15.0)],
    )
}

fn is_lightning_rod_name(name: &str) -> bool {
    name == "lightning_rod" || name.ends_with("_lightning_rod")
}

fn is_head_block_name(name: &str) -> bool {
    matches!(
        name,
        "skeleton_skull"
            | "wither_skeleton_skull"
            | "zombie_head"
            | "creeper_head"
            | "player_head"
            | "dragon_head"
            | "piglin_head"
    )
}

fn egg_layer(inset: f32, y: f32, size: f32, height: f32) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(inset), px(y), px(inset)],
        [px(inset + size), px(y + height), px(inset + size)],
    ))
}

fn local_item_frame_cuboids() -> Vec<ModelCuboid> {
    vec![
        ModelCuboid::new([px(3.0), px(3.0), px(15.5)], [px(13.0), px(13.0), 1.0]),
        ModelCuboid::new([px(2.0), px(2.0), px(15.0)], [px(14.0), px(3.0), 1.0]),
        ModelCuboid::new([px(2.0), px(13.0), px(15.0)], [px(14.0), px(14.0), 1.0]),
        ModelCuboid::new([px(2.0), px(3.0), px(15.0)], [px(3.0), px(13.0), 1.0]),
        ModelCuboid::new([px(13.0), px(3.0), px(15.0)], [px(14.0), px(13.0), 1.0]),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Facing {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

fn facing_from_state(state: &BlockStateQuery) -> Facing {
    if let Some(value) = state_i64(state, "facing_direction")
        .or_else(|| state_i64(state, "minecraft:facing_direction"))
    {
        return match value {
            0 => Facing::Down,
            2 => Facing::North,
            3 => Facing::South,
            4 => Facing::West,
            5 => Facing::East,
            _ => Facing::Up,
        };
    }
    cardinal_direction(state).map_or(Facing::Up, facing_from_cardinal)
}

fn facing_from_cardinal(direction: CardinalDirection) -> Facing {
    match direction {
        CardinalDirection::North => Facing::North,
        CardinalDirection::South => Facing::South,
        CardinalDirection::West => Facing::West,
        CardinalDirection::East => Facing::East,
    }
}

fn cardinal_for_facing(facing: Facing) -> Option<CardinalDirection> {
    match facing {
        Facing::North => Some(CardinalDirection::North),
        Facing::South => Some(CardinalDirection::South),
        Facing::West => Some(CardinalDirection::West),
        Facing::East => Some(CardinalDirection::East),
        Facing::Down | Facing::Up => None,
    }
}

fn orient_cuboid_from_up(cuboid: ModelCuboid, facing: Facing) -> ModelCuboid {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for x in [cuboid.min[0], cuboid.max[0]] {
        for y in [cuboid.min[1], cuboid.max[1]] {
            for z in [cuboid.min[2], cuboid.max[2]] {
                let point = orient_point_from_up([x, y, z], facing);
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
        }
    }
    let mut oriented = ModelCuboid::new(min, max);
    oriented.material_slot = cuboid.material_slot;
    oriented
}

fn orient_point_from_up(point: [f32; 3], facing: Facing) -> [f32; 3] {
    let [x, y, z] = point;
    match facing {
        Facing::Up => [x, y, z],
        Facing::Down => [x, 1.0 - y, z],
        Facing::North => [x, z, 1.0 - y],
        Facing::South => [x, z, y],
        Facing::West => [1.0 - y, z, x],
        Facing::East => [y, z, x],
    }
}

fn scaffolding_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    side_slot_cuboid(min, max)
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
}

fn face_slot_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(min, max))
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "south")
        .with_face_material_slot(BlockFace::East, "east")
        .with_face_material_slot(BlockFace::West, "west")
}

fn side_slot_cuboid(min: [f32; 3], max: [f32; 3]) -> ModelCuboid {
    detail_cuboid_with_local_uv(ModelCuboid::new(min, max))
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
}

fn rotate_cuboid_from_south(cuboid: ModelCuboid, direction: CardinalDirection) -> ModelCuboid {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    (0..turns).fold(cuboid, |cuboid, _| rotate_cuboid_clockwise(cuboid))
}

fn rotate_cuboid_clockwise(cuboid: ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [1.0 - cuboid.max[2], cuboid.min[1], cuboid.min[0]],
        [1.0 - cuboid.min[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot;
    for (face, slot) in cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(rotate_block_face_clockwise(face), slot);
    }
    for (face, uv) in cuboid.face_uvs {
        rotated
            .face_uvs
            .insert(rotate_block_face_clockwise(face), uv);
    }
    rotated
}

fn rotate_block_face_clockwise(face: BlockFace) -> BlockFace {
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
