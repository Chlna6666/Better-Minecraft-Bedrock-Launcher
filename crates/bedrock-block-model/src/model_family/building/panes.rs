use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, direction_connected};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, apply_jmc_box_uv, detail_cuboid_with_local_uv, uv16,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_pane")
        || name.ends_with("_bars")
        || matches!(name, "glass_pane" | "iron_bars")
    {
        Some(ModelFamily::Pane)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let kind = PaneKind::from_block_name(name);
    pane_shape_from_connections(
        CardinalDirection::ALL
            .into_iter()
            .filter(|direction| direction_connected(state, *direction)),
        kind,
    )
}

#[derive(Clone, Copy)]
enum PaneKind {
    Glass,
    Bars,
}

impl PaneKind {
    fn from_block_name(name: &str) -> Self {
        if name.ends_with("_bars") || name == "iron_bars" {
            Self::Bars
        } else {
            Self::Glass
        }
    }

    const fn top_edge_slot(self) -> &'static str {
        match self {
            Self::Glass => "east",
            Self::Bars => "up",
        }
    }

    const fn bottom_edge_slot(self) -> &'static str {
        match self {
            Self::Glass => "east",
            Self::Bars => "down",
        }
    }
}

fn pane_shape_from_connections(
    connected: impl IntoIterator<Item = CardinalDirection>,
    kind: PaneKind,
) -> ModelShape {
    let connected = connected.into_iter().collect::<Vec<_>>();
    let mut cuboids = Vec::with_capacity(connected.len().max(2).saturating_add(1));
    cuboids.push(pane_post_cuboid_with_uv(kind));
    if connected.is_empty() {
        cuboids.push(pane_center_card_cuboid_with_uv(true, kind));
        cuboids.push(pane_center_card_cuboid_with_uv(false, kind));
        return ModelShape::from_cuboids(cuboids);
    }
    for direction in connected {
        cuboids.push(pane_arm_cuboid_with_uv(direction, kind));
    }
    ModelShape::from_cuboids(cuboids)
}

fn pane_post_cuboid_with_uv(kind: PaneKind) -> ModelCuboid {
    detail_slots(
        apply_jmc_box_uv(
            ModelCuboid::new([0.4375, 0.0, 0.4375], [0.5625, 1.0, 0.5625]),
            uv16(7.0, 7.0, 9.0, 9.0),
            uv16(7.0, 0.0, 9.0, 16.0),
        ),
        kind,
    )
}

fn pane_center_card_cuboid_with_uv(along_x: bool, kind: PaneKind) -> ModelCuboid {
    let cuboid = if along_x {
        ModelCuboid::new([0.0, 0.0, 0.46875], [1.0, 1.0, 0.53125])
    } else {
        ModelCuboid::new([0.46875, 0.0, 0.0], [0.53125, 1.0, 1.0])
    };
    detail_slots(detail_cuboid_with_local_uv(cuboid), kind)
        .with_face_uv(BlockFace::North, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::South, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::East, uv16(0.0, 0.0, 16.0, 16.0))
        .with_face_uv(BlockFace::West, uv16(0.0, 0.0, 16.0, 16.0))
}

fn pane_arm_cuboid_with_uv(direction: CardinalDirection, kind: PaneKind) -> ModelCuboid {
    let cuboid = pane_arm_cuboid(direction);
    let edge_top_uv = uv16(7.0, 8.0, 9.0, 16.0);
    let edge_bottom_uv = uv16(7.0, 0.0, 9.0, 8.0);
    let side_uv = uv16(0.0, 0.0, 8.0, 16.0);
    let end_uv = uv16(7.0, 0.0, 9.0, 16.0);
    let mut cuboid = detail_slots(detail_cuboid_with_local_uv(cuboid), kind)
        .with_face_uv(BlockFace::Up, edge_top_uv)
        .with_face_uv(BlockFace::Down, edge_bottom_uv);
    match direction {
        CardinalDirection::North => {
            cuboid = cuboid
                .with_face_uv(BlockFace::East, side_uv)
                .with_face_uv(BlockFace::West, side_uv)
                .with_face_uv(BlockFace::North, end_uv)
                .with_face_material_slot(BlockFace::North, "east");
        }
        CardinalDirection::South => {
            cuboid = cuboid
                .with_face_uv(BlockFace::East, side_uv)
                .with_face_uv(BlockFace::West, side_uv)
                .with_face_uv(BlockFace::South, end_uv)
                .with_face_material_slot(BlockFace::South, "east");
        }
        CardinalDirection::East => {
            cuboid = cuboid
                .with_face_uv(BlockFace::North, side_uv)
                .with_face_uv(BlockFace::South, side_uv)
                .with_face_uv(BlockFace::East, end_uv)
                .with_face_material_slot(BlockFace::East, "east");
        }
        CardinalDirection::West => {
            cuboid = cuboid
                .with_face_uv(BlockFace::North, side_uv)
                .with_face_uv(BlockFace::South, side_uv)
                .with_face_uv(BlockFace::West, end_uv)
                .with_face_material_slot(BlockFace::West, "east");
        }
    }
    cuboid
}

fn pane_arm_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.4375, 0.0, 0.0], [0.5625, 1.0, 0.4375]),
        CardinalDirection::South => ModelCuboid::new([0.4375, 0.0, 0.5625], [0.5625, 1.0, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5625, 0.0, 0.4375], [1.0, 1.0, 0.5625]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.0, 0.4375], [0.4375, 1.0, 0.5625]),
    }
}

fn detail_slots(cuboid: ModelCuboid, kind: PaneKind) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::Up, kind.top_edge_slot())
        .with_face_material_slot(BlockFace::Down, kind.bottom_edge_slot())
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::East, "side")
}
