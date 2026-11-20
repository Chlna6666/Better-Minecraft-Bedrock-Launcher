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
/// * `piston` / `sticky_piston` — the base, always a FULL cube (Bedrock ships no recessed
///   variant; an extended piston's arm block covers the pushed face).
/// * `piston_arm_collision` / `sticky_piston_arm_collision` — the head, per the official JE
///   `template_piston_head`: a 16 x 16 x 4 plate at the push-end face plus a 4 x 4 x 16 shaft
///   running back through the cell (its last 4 px reach into the base block, where the full
///   cube hides them). Rendering the head as a bare plate is why extended pistons showed a
///   floating slab with no arm.
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
    let cuboid = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "bottom")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side");
    // The pushed face gets the piston face art.
    let cuboid = cuboid.with_face_material_slot(facing_face(facing), front_slot);
    ModelShape::from_cuboids([crate::model_family::shape::detail_cuboid_with_local_uv(
        cuboid,
    )])
}

fn arm_shape(facing: BlockFacing, sticky: bool) -> ModelShape {
    // `platform` carries the piston face art (normal/sticky); `inner` is the dark hole art on
    // the plate's back; the plate sides and the shaft sample `piston_side`.
    let platform_slot = if sticky { "platform_sticky" } else { "platform" };
    let px = |value: f32| value / 16.0;
    // Author the head facing north (plate at z 0..4, shaft behind it), then rotate.
    let authored = authoured_head(platform_slot, px);
    rotate_head_to_facing(authored, facing)
}

/// The head assembly authored facing north: the 16 x 16 x 4 plate at z 0..4 with the official
/// per-face windows, plus the 4 x 4 x 16 shaft (z 4..20 — the last 4 px reach into the base
/// block, where the full cube hides them).
fn authoured_head(platform_slot: &str, px: impl Fn(f32) -> f32) -> ModelShape {
    use crate::model_family::shape::{rect_uv, rotate_uv_quarter, upright_full_texture_uv};
    let _ = rect_uv;
    let _ = rotate_uv_quarter;
    let _ = upright_full_texture_uv;
    let side_band = |u0: f32, v0: f32| {
        [
            [px(u0), px(v0 + 4.0)],
            [px(u0 + 16.0), px(v0 + 4.0)],
            [px(u0 + 16.0), px(v0)],
            [px(u0), px(v0)],
        ]
    };
    // Plate: north face = the platform art; south face = the hole art; sides = 4 px bands of
    // piston_side (rot 90/270 in the official model → the V band across the side tile).
    let plate = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, px(4.0)])
        .with_face_material_slot(BlockFace::North, platform_slot)
        .with_face_material_slot(BlockFace::South, "inner")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "side")
        .with_face_uv(BlockFace::North, crate::model_family::shape::full_texture_uv())
        .with_face_uv(BlockFace::South, crate::model_family::shape::full_texture_uv())
        .with_face_uv(BlockFace::East, side_band(0.0, 12.0))
        .with_face_uv(BlockFace::West, side_band(0.0, 0.0))
        .with_face_uv(BlockFace::Up, side_band(0.0, 0.0))
        .with_face_uv(BlockFace::Down, side_band(0.0, 12.0));
    // Shaft: 4 x 4 px, running z 4..20 (16 px). Its long faces sample the side tile's shaft
    // band (rows 0..4); the two side faces mirror each other like the official east/west.
    let shaft_long = |v0: f32| {
        [
            [px(0.0), px(v0)],
            [px(16.0), px(v0)],
            [px(16.0), px(v0 + 4.0)],
            [px(0.0), px(v0 + 4.0)],
        ]
    };
    let shaft = ModelCuboid::new([px(6.0), px(6.0), px(4.0)], [px(10.0), px(10.0), px(20.0)])
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_material_slot(BlockFace::Down, "side")
        .with_face_uv(BlockFace::East, shaft_long(0.0))
        .with_face_uv(BlockFace::West, shaft_long(0.0))
        .with_face_uv(BlockFace::Up, shaft_long(4.0))
        .with_face_uv(BlockFace::Down, shaft_long(4.0))
        // The shaft's end cap sits inside the base block; give it the hole art.
        .with_face_material_slot(BlockFace::North, "inner")
        .with_face_uv(BlockFace::North, crate::model_family::shape::full_texture_uv())
        .with_face_material_slot(BlockFace::South, "inner")
        .with_face_uv(BlockFace::South, crate::model_family::shape::full_texture_uv());
    ModelShape::from_cuboids([plate, shaft])
}

/// Turns the north-authored head assembly onto the state's facing. The plate's outer face
/// (model north) lands on the push direction, so the model north face travels onto `facing`.
fn rotate_head_to_facing(shape: ModelShape, facing: BlockFacing) -> ModelShape {
    use crate::model_family::shape::{rotate_shape_cardinal, rotate_shape_x};
    match facing {
        BlockFacing::North => shape,
        BlockFacing::South => rotate_shape_cardinal(shape, crate::model_family::direction::CardinalDirection::South),
        BlockFacing::East => rotate_shape_cardinal(shape, crate::model_family::direction::CardinalDirection::East),
        BlockFacing::West => rotate_shape_cardinal(shape, crate::model_family::direction::CardinalDirection::West),
        BlockFacing::Up => rotate_shape_x(shape, 1),
        BlockFacing::Down => rotate_shape_x(shape, 3),
    }
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
