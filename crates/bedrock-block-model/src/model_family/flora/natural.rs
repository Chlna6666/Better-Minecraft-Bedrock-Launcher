use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{state_bool, state_i64};
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv, full_texture_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    match name {
        "farmland" | "grass_path" => Some(ModelFamily::Farmland),
        "fire" | "soul_fire" => Some(ModelFamily::Fire),
        "water" | "flowing_water" | "lava" | "flowing_lava" => Some(ModelFamily::Liquid),
        "bubble_column" => Some(ModelFamily::BubbleColumn),
        "trip_wire" => Some(ModelFamily::Tripwire),
        "chorus_plant" => Some(ModelFamily::ChorusPlant),
        "pointed_dripstone" => Some(ModelFamily::PointedDripstone),
        "mangrove_propagule" => Some(ModelFamily::MangrovePropagule),
        "big_dripleaf" | "small_dripleaf_block" => Some(ModelFamily::Dripleaf),
        "spore_blossom" => Some(ModelFamily::SporeBlossom),
        "sculk_sensor" | "calibrated_sculk_sensor" => Some(ModelFamily::SculkSensor),
        "sculk_shrieker" => Some(ModelFamily::SculkShrieker),
        _ => None,
    }
}

pub(crate) fn farmland_shape() -> ModelShape {
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(15.0), 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )])
}

pub(crate) fn fire_shape() -> ModelShape {
    ModelShape::default().with_planes([
        vertical_plane_z(px(1.0), "side"),
        vertical_plane_z(px(15.0), "side"),
        vertical_plane_x(px(1.0), "side"),
        vertical_plane_x(px(15.0), "side"),
        diagonal_plane(true, "side"),
        diagonal_plane(false, "side"),
    ])
}

pub(crate) fn liquid_shape(state: &BlockStateQuery) -> ModelShape {
    let height = match state_i64(state, "liquid_depth").unwrap_or(8) {
        0 => px(14.0),
        1 => px(12.5),
        2 => px(10.5),
        3 => px(9.0),
        4 => px(7.0),
        5 => px(5.5),
        6 => px(3.5),
        7 => px(1.5),
        _ => 1.0,
    };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, height, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )])
}

pub(crate) fn bubble_column_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(1.0), 0.0, px(1.0)], [px(15.0), 1.0, px(15.0)])
                .with_face_material_slot(BlockFace::Side, "north"),
        ),
        detail_cuboid_with_local_uv(
            ModelCuboid::new([px(5.0), 0.0, px(5.0)], [px(11.0), 1.0, px(11.0)])
                .with_face_material_slot(BlockFace::Side, "south"),
        ),
    ])
}

pub(crate) fn tripwire_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [0.0, px(1.0), px(7.0)],
            [1.0, px(2.0), px(9.0)],
        )),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(7.0), px(1.0), 0.0],
            [px(9.0), px(2.0), 1.0],
        )),
    ])
}

pub(crate) fn chorus_plant_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(4.0), 0.0, px(4.0)],
            [px(12.0), 1.0, px(12.0)],
        )),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(3.0), px(4.0), px(4.0)],
            [px(13.0), px(12.0), px(12.0)],
        )),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(5.0), px(5.0), px(2.0)],
            [px(11.0), px(11.0), px(14.0)],
        )),
    ])
}

pub(crate) fn pointed_dripstone_shape(_state: &BlockStateQuery) -> ModelShape {
    ModelShape::default().with_planes([
        diagonal_plane_between(0.0, 1.0, true, None),
        diagonal_plane_between(0.0, 1.0, false, None),
    ])
}

pub(crate) fn mangrove_propagule_shape(state: &BlockStateQuery) -> ModelShape {
    if !state_bool(state, "hanging").unwrap_or(false) {
        return ModelShape::default().with_planes([
            diagonal_plane_between(0.0, 1.0, true, None),
            diagonal_plane_between(0.0, 1.0, false, None),
        ]);
    }

    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), px(12.0), px(7.0)],
        [px(9.0), px(16.0), px(9.0)],
    ))]);
    shape.planes.extend([
        diagonal_plane_between(px(4.0), px(14.0), true, None),
        diagonal_plane_between(px(4.0), px(14.0), false, None),
    ]);
    shape
}

pub(crate) fn dripleaf_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "small_dripleaf_block" {
        return small_dripleaf_shape(state);
    }
    big_dripleaf_shape(state)
}

pub(crate) fn spore_blossom_shape() -> ModelShape {
    let mut shape = ModelShape::default().with_planes([ModelPlane::new(
        [
            [0.0, px(15.9), 0.0],
            [1.0, px(15.9), 0.0],
            [1.0, px(15.9), 1.0],
            [0.0, px(15.9), 1.0],
        ],
        [0, -1, 0],
    )
    .with_material_slot("up")
    .with_uv(full_texture_uv())]);
    shape.planes.extend([
        drooping_plane_z(px(7.0)),
        drooping_plane_z(px(9.0)),
        drooping_plane_x(px(7.0)),
        drooping_plane_x(px(9.0)),
    ]);
    shape
}

pub(crate) fn sculk_sensor_shape(name: &str) -> ModelShape {
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, px(8.0), 1.0],
    ))]);
    shape.planes.extend([
        tendril_plane_z(px(2.0)),
        tendril_plane_z(px(14.0)),
        tendril_plane_x(px(2.0)),
        tendril_plane_x(px(14.0)),
    ]);
    if name == "calibrated_sculk_sensor" {
        shape.planes.extend([
            vertical_plane_z(px(8.0), "south"),
            vertical_plane_x(px(8.0), "south"),
        ]);
    }
    shape
}

pub(crate) fn sculk_shrieker_shape() -> ModelShape {
    ModelShape::from_cuboids([
        detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, px(8.0), 1.0])),
        detail_cuboid_with_local_uv(ModelCuboid::new(
            [px(1.0), px(8.0), px(1.0)],
            [px(15.0), px(15.0), px(15.0)],
        )),
    ])
}

fn big_dripleaf_shape(state: &BlockStateQuery) -> ModelShape {
    let head = state_bool(state, "big_dripleaf_head").unwrap_or(false);
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), 0.0, px(7.0)],
        [px(9.0), px(if head { 13.0 } else { 16.0 }), px(9.0)],
    ))]);
    if head {
        shape.cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new([0.0, px(12.0), 0.0], [1.0, px(15.0), 1.0])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Side, "side"),
        ));
    }
    shape
}

fn small_dripleaf_shape(state: &BlockStateQuery) -> ModelShape {
    let upper = state_bool(state, "upper_block_bit").unwrap_or(false);
    let mut shape = ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [px(7.0), 0.0, px(7.0)],
        [px(9.0), 1.0, px(9.0)],
    ))]);
    if upper {
        shape.cuboids.push(detail_cuboid_with_local_uv(
            ModelCuboid::new([px(1.0), px(12.0), px(1.0)], [px(15.0), px(14.0), px(15.0)])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Side, "side"),
        ));
    }
    shape
}

fn vertical_plane_z(z: f32, material_slot: &'static str) -> ModelPlane {
    ModelPlane::new(
        [[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
        [0, 0, 1],
    )
    .with_material_slot(material_slot)
    .with_uv(full_texture_uv())
}

fn vertical_plane_x(x: f32, material_slot: &'static str) -> ModelPlane {
    ModelPlane::new(
        [[x, 0.0, 0.0], [x, 0.0, 1.0], [x, 1.0, 1.0], [x, 1.0, 0.0]],
        [1, 0, 0],
    )
    .with_material_slot(material_slot)
    .with_uv(full_texture_uv())
}

fn diagonal_plane(forward: bool, material_slot: &'static str) -> ModelPlane {
    diagonal_plane_between(0.0, 1.0, forward, Some(material_slot))
}

fn diagonal_plane_between(
    bottom: f32,
    top: f32,
    forward: bool,
    material_slot: Option<&'static str>,
) -> ModelPlane {
    let plane = if forward {
        ModelPlane::new(
            [
                [0.0, bottom, 0.0],
                [1.0, bottom, 1.0],
                [1.0, top, 1.0],
                [0.0, top, 0.0],
            ],
            [-1, 0, 1],
        )
    } else {
        ModelPlane::new(
            [
                [1.0, bottom, 0.0],
                [0.0, bottom, 1.0],
                [0.0, top, 1.0],
                [1.0, top, 0.0],
            ],
            [1, 0, 1],
        )
    }
    .with_uv(full_texture_uv());
    if let Some(slot) = material_slot {
        plane.with_material_slot(slot)
    } else {
        plane
    }
}

fn drooping_plane_z(z: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [0.0, px(15.7), z],
            [1.0, px(15.7), z],
            [1.0, px(8.0), px(8.0)],
            [0.0, px(8.0), px(8.0)],
        ],
        [0, -1, 1],
    )
    .with_uv(full_texture_uv())
}

fn drooping_plane_x(x: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [x, px(15.7), 0.0],
            [x, px(15.7), 1.0],
            [px(8.0), px(8.0), 1.0],
            [px(8.0), px(8.0), 0.0],
        ],
        [1, -1, 0],
    )
    .with_uv(full_texture_uv())
}

fn tendril_plane_z(z: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [px(3.0), px(8.0), z],
            [px(13.0), px(8.0), z],
            [px(13.0), px(16.0), z],
            [px(3.0), px(16.0), z],
        ],
        [0, 0, 1],
    )
    .with_uv(full_texture_uv())
}

fn tendril_plane_x(x: f32) -> ModelPlane {
    ModelPlane::new(
        [
            [x, px(8.0), px(3.0)],
            [x, px(8.0), px(13.0)],
            [x, px(16.0), px(13.0)],
            [x, px(16.0), px(3.0)],
        ],
        [1, 0, 0],
    )
    .with_uv(full_texture_uv())
}

const fn px(value: f32) -> f32 {
    value / 16.0
}
