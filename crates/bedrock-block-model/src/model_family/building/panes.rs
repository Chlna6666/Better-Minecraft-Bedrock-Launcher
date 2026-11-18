use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, connected};
use crate::model_family::shape::{ModelCuboid, ModelPlane, ModelShape, uv16};
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

/// Vanilla draws panes as a multipart model, not as one box per connection.
///
/// `glass_pane` always applies a 2 px post, adds one 2 px `side` slab per **connected** direction
/// and one 2 px `noside` card per **disconnected** direction. That last part is what makes a lone
/// pane a single thin card through the cell: without it the four missing directions left the
/// block looking like a hollow frame. `iron_bars` uses the same four-way structure with its own
/// cap pieces.
///
/// The `noside` piece is a **single quad** (JE draws only the outward face), so it is emitted as a
/// plane. A zero-thickness box would be dropped by the face emitter and the card would vanish.
pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let kind = PaneKind::from_block_name(name);
    let mut cuboids = Vec::with_capacity(5);
    let mut planes = Vec::with_capacity(4);
    cuboids.push(pane_post_cuboid(kind));
    for direction in CardinalDirection::ALL {
        if connected(state, direction) {
            cuboids.push(pane_arm_cuboid(direction, kind));
        } else {
            planes.push(pane_open_plane(direction, kind));
        }
    }
    ModelShape::from_cuboids(cuboids).with_planes(planes)
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

    /// Slot of the pane's 2 px rim art.
    ///
    /// `blocks.json` maps `glass_pane` east to `glass_pane_top` and `iron_bars` east to
    /// `iron_bars_edge`; both families therefore read their rim from `east`.
    const fn edge_slot(self) -> &'static str {
        "east"
    }

    /// Slot of the pane's flat faces.
    const fn face_slot(self) -> &'static str {
        "side"
    }
}

/// The 2 px post that every pane has at its centre.
fn pane_post_cuboid(kind: PaneKind) -> ModelCuboid {
    // Capture (geo 290 / app 3075, standalone pane): the 2 px post sides sample the u7..9,
    // v0..16 strip of the glass sheet; the caps take the u7..9, v7..9 patch. Leaving side UVs
    // unset made the renderer stretch the whole 16x16 tile over the 2 px faces ("拉扯").
    let side_uv = uv16(7.0, 0.0, 9.0, 16.0);
    ModelCuboid::new([0.4375, 0.0, 0.4375], [0.5625, 1.0, 0.5625])
        .with_face_uv(BlockFace::Up, uv16(7.0, 7.0, 9.0, 9.0))
        .with_face_uv(BlockFace::Down, uv16(7.0, 7.0, 9.0, 9.0))
        .with_face_uv(BlockFace::North, side_uv)
        .with_face_uv(BlockFace::South, side_uv)
        .with_face_uv(BlockFace::West, side_uv)
        .with_face_uv(BlockFace::East, side_uv)
        .with_face_material_slot(BlockFace::Up, kind.edge_slot())
        .with_face_material_slot(BlockFace::Down, kind.edge_slot())
        .with_face_material_slot(BlockFace::North, kind.face_slot())
        .with_face_material_slot(BlockFace::South, kind.face_slot())
        .with_face_material_slot(BlockFace::West, kind.face_slot())
        .with_face_material_slot(BlockFace::East, kind.face_slot())
}

/// The `noside` card: one quad on the post's outward face.
fn pane_open_plane(direction: CardinalDirection, kind: PaneKind) -> ModelPlane {
    // `template_glass_pane_noside` is `from [7,0,7] to [9,16,9]` with only one face drawn.
    let (corners, normal) = match direction {
        CardinalDirection::North => (
            [
                [0.4375, 0.0, 0.4375],
                [0.5625, 0.0, 0.4375],
                [0.5625, 1.0, 0.4375],
                [0.4375, 1.0, 0.4375],
            ],
            [0, 0, -1],
        ),
        CardinalDirection::South => (
            [
                [0.5625, 0.0, 0.5625],
                [0.4375, 0.0, 0.5625],
                [0.4375, 1.0, 0.5625],
                [0.5625, 1.0, 0.5625],
            ],
            [0, 0, 1],
        ),
        CardinalDirection::East => (
            [
                [0.5625, 0.0, 0.4375],
                [0.5625, 0.0, 0.5625],
                [0.5625, 1.0, 0.5625],
                [0.5625, 1.0, 0.4375],
            ],
            [1, 0, 0],
        ),
        CardinalDirection::West => (
            [
                [0.4375, 0.0, 0.5625],
                [0.4375, 0.0, 0.4375],
                [0.4375, 1.0, 0.4375],
                [0.4375, 1.0, 0.5625],
            ],
            [-1, 0, 0],
        ),
    };
    ModelPlane::new(corners, normal)
        .with_material_slot(kind.face_slot())
        .with_uv(uv16(7.0, 0.0, 9.0, 16.0))
}

/// One 2 px arm from the post to a connected side.
fn pane_arm_cuboid(direction: CardinalDirection, kind: PaneKind) -> ModelCuboid {
    let cuboid = match direction {
        CardinalDirection::North => ModelCuboid::new([0.4375, 0.0, 0.0], [0.5625, 1.0, 0.4375]),
        CardinalDirection::South => ModelCuboid::new([0.4375, 0.0, 0.5625], [0.5625, 1.0, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.5625, 0.0, 0.4375], [1.0, 1.0, 0.5625]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.0, 0.4375], [0.4375, 1.0, 0.5625]),
    };
    let side_uv = uv16(0.0, 0.0, 16.0, 16.0);
    let rim_uv = uv16(7.0, 0.0, 9.0, 16.0);
    let mut cuboid = cuboid
        .with_face_uv(BlockFace::Up, uv16(7.0, 0.0, 9.0, 7.0))
        .with_face_uv(BlockFace::Down, uv16(7.0, 0.0, 9.0, 7.0))
        .with_face_material_slot(BlockFace::Up, kind.edge_slot())
        .with_face_material_slot(BlockFace::Down, kind.edge_slot());
    let (long_faces, rim_face) = match direction {
        CardinalDirection::North => ([BlockFace::East, BlockFace::West], BlockFace::North),
        CardinalDirection::South => ([BlockFace::East, BlockFace::West], BlockFace::South),
        CardinalDirection::East => ([BlockFace::North, BlockFace::South], BlockFace::East),
        CardinalDirection::West => ([BlockFace::North, BlockFace::South], BlockFace::West),
    };
    for face in long_faces {
        cuboid = cuboid.with_face_uv(face, side_uv);
    }
    cuboid = cuboid.with_face_uv(rim_face, rim_uv);
    for face in [
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        let slot = if face == rim_face {
            kind.edge_slot()
        } else {
            kind.face_slot()
        };
        cuboid = cuboid.with_face_material_slot(face, slot);
    }
    cuboid
}
