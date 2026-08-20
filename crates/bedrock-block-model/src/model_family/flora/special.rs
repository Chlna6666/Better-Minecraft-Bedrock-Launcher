use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv, ground_plane,
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
    let age = state_i64(state, "age").unwrap_or(2).clamp(0, 2);
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::South);
    let (width, height, depth) = match age {
        0 => (px(4.0), px(5.0), px(4.0)),
        1 => (px(6.0), px(7.0), px(6.0)),
        _ => (px(8.0), px(9.0), px(8.0)),
    };
    let cuboid = cocoa_pod_cuboid(direction, width, height, depth);
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        cuboid
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::North, "side")
            .with_face_material_slot(BlockFace::South, "side")
            .with_face_material_slot(BlockFace::East, "side")
            .with_face_material_slot(BlockFace::West, "side"),
    )])
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
        .with_uv(full_texture_uv()),
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
        .with_uv(full_texture_uv()),
    ])
}

pub(crate) fn ground_cover_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "pale_moss_carpet" {
        return pale_moss_carpet_shape(state);
    }
    if matches!(name, "frog_spawn" | "waterlily" | "lily_pad") {
        return full_ground_cover_shape(px(0.125));
    }

    let count = match state_i64(state, "growth").unwrap_or(0).clamp(0, 3) {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => 4,
    };
    let mut planes = Vec::with_capacity(count);
    for corners in ground_cover_quadrants().iter().take(count) {
        planes.push(ground_plane(*corners, None, full_texture_uv()));
    }
    ModelShape::default().with_planes(planes)
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

fn cocoa_pod_cuboid(
    direction: CardinalDirection,
    width: f32,
    height: f32,
    depth: f32,
) -> ModelCuboid {
    let min_y = px(12.0) - height;
    let max_y = px(12.0);
    let half_width = width * 0.5;
    match direction {
        CardinalDirection::North => ModelCuboid::new(
            [0.5 - half_width, min_y, 0.0],
            [0.5 + half_width, max_y, depth],
        ),
        CardinalDirection::South => ModelCuboid::new(
            [0.5 - half_width, min_y, 1.0 - depth],
            [0.5 + half_width, max_y, 1.0],
        ),
        CardinalDirection::East => ModelCuboid::new(
            [1.0 - depth, min_y, 0.5 - half_width],
            [1.0, max_y, 0.5 + half_width],
        ),
        CardinalDirection::West => ModelCuboid::new(
            [0.0, min_y, 0.5 - half_width],
            [depth, max_y, 0.5 + half_width],
        ),
    }
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
