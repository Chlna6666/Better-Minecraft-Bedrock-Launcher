use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{BlockFacing, block_facing};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("piston") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

/// Bedrock splits the piston into two independent blocks:
///
/// * `piston` / `sticky_piston` — the full-block base. Its pushed face carries
///   `piston_top_normal` (or `piston_top_sticky`), the other faces use the side/bottom art.
/// * `piston_arm_collision` / `sticky_piston_arm_collision` — the head, a 4 px plate that has
///   its own `facing_direction` and is *not* tied to the base's orientation.
///
/// Rendering the head as a full cube and painting the base with one texture made extended
/// pistons look like two stacked boxes.
pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    if name.contains("arm_collision") {
        return arm_shape(facing, name.starts_with("sticky"));
    }
    base_shape(facing, name.starts_with("sticky"))
}

fn base_shape(facing: BlockFacing, sticky: bool) -> ModelShape {
    let front_slot = if sticky { "top_sticky" } else { "top_normal" };
    // The base is a 3/4 cell: the pushed face is recessed by 4 px so an extended piston's head
    // occupies that quarter instead of overlapping the base. Measured from the client
    // (e.g. `facing = up` occupies y 0.25..1, `facing = down` y 0..0.75).
    let (min, max) = match facing {
        BlockFacing::North => ([0.0, 0.0, 0.25], [1.0, 1.0, 1.0]),
        BlockFacing::South => ([0.0, 0.0, 0.0], [1.0, 1.0, 0.75]),
        BlockFacing::East => ([0.0, 0.0, 0.0], [0.75, 1.0, 1.0]),
        BlockFacing::West => ([0.25, 0.0, 0.0], [1.0, 1.0, 1.0]),
        BlockFacing::Up => ([0.0, 0.0, 0.0], [1.0, 0.75, 1.0]),
        BlockFacing::Down => ([0.0, 0.25, 0.0], [1.0, 1.0, 1.0]),
    };
    let cuboid = ModelCuboid::new(min, max);
    let mut cuboid = cuboid
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "bottom")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side");
    // The pushed face gets the piston face art.
    cuboid = cuboid.with_face_material_slot(facing_face(facing), front_slot);
    ModelShape::from_cuboids([crate::model_family::shape::detail_cuboid_with_local_uv(
        cuboid,
    )])
}

fn arm_shape(facing: BlockFacing, sticky: bool) -> ModelShape {
    let head_slot = if sticky { "top_sticky" } else { "top_normal" };
    // The plate sits at the cell face that touches the base (opposite the push direction).
    let (min, max) = match facing {
        BlockFacing::North => ([0.0, 0.0, 0.75], [1.0, 1.0, 1.0]),
        BlockFacing::South => ([0.0, 0.0, 0.0], [1.0, 1.0, 0.25]),
        BlockFacing::East => ([0.0, 0.0, 0.0], [0.25, 1.0, 1.0]),
        BlockFacing::West => ([0.75, 0.0, 0.0], [1.0, 1.0, 1.0]),
        BlockFacing::Up => ([0.0, 0.0, 0.0], [1.0, 0.25, 1.0]),
        BlockFacing::Down => ([0.0, 0.75, 0.0], [1.0, 1.0, 1.0]),
    };
    let mut cuboid = ModelCuboid::new(min, max)
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "bottom")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side");
    cuboid = cuboid.with_face_material_slot(facing_face(facing), head_slot);
    ModelShape::from_cuboids([crate::model_family::shape::detail_cuboid_with_local_uv(
        cuboid,
    )])
}

const fn facing_face(facing: BlockFacing) -> BlockFace {
    match facing {
        BlockFacing::Down => BlockFace::Down,
        BlockFacing::Up => BlockFace::Up,
        BlockFacing::North => BlockFace::North,
        BlockFacing::South => BlockFace::South,
        BlockFacing::West => BlockFace::West,
        BlockFacing::East => BlockFace::East,
    }
}
