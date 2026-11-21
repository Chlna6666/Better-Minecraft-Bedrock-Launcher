use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{BlockFacing, block_facing, state_bool, state_i64};
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
/// The piston's own `facing_direction` mapping. Bedrock's six-way int state is **not**
/// uniform across blocks: pistons point `0=down, 1=up, 2=south, 3=north, 4=east, 5=west`
/// (minecraft.wiki/w/Piston block states), the horizontal values opposite the shared
/// `facing_direction_from_int` table.
fn piston_facing(state: &BlockStateQuery) -> BlockFacing {
    if let Some(value) = state_i64(state, "facing_direction") {
        return match value {
            0 => BlockFacing::Down,
            1 => BlockFacing::Up,
            2 => BlockFacing::South,
            3 => BlockFacing::North,
            4 => BlockFacing::East,
            5 => BlockFacing::West,
            _ => BlockFacing::North,
        };
    }
    block_facing(state).unwrap_or(BlockFacing::North)
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let facing = piston_facing(state);
    if name.contains("arm_collision") {
        return arm_shape(facing, name.starts_with("sticky"));
    }
    base_shape(
        facing,
        name.starts_with("sticky"),
        state_bool(state, "extended_bit").unwrap_or(false),
    )
}

fn base_shape(facing: BlockFacing, sticky: bool, extended: bool) -> ModelShape {
    // The official `template_piston` authored facing north: the platform on the north
    // (push) face, `piston_bottom` on the south face, and the side tile with the official
    // per-face UV rotations — down rot 180, east rot 90, west rot 270 (the side art turns
    // with the model, which is why each facing shows the frame at its own orientation).
    // An extended piston's base shows the dark inner frame on its pushed face (the shaft
    // covers its centre); an idle one keeps the wooden platform.
    let platform_slot = if sticky { "top_sticky" } else { "top_normal" };
    let pushed_slot = if extended { "top" } else { platform_slot };
    let full = crate::model_family::shape::full_texture_uv();
    let turned = |quarters: i32| crate::model_family::shape::rotate_uv_quarter(full, quarters);
    let authored = ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .with_face_material_slot(BlockFace::North, pushed_slot)
        .with_face_uv(BlockFace::North, full)
        .with_face_material_slot(BlockFace::South, "bottom")
        .with_face_uv(BlockFace::South, full)
        .with_face_material_slot(BlockFace::Up, "side")
        .with_face_uv(BlockFace::Up, turned(0))
        .with_face_material_slot(BlockFace::Down, "side")
        .with_face_uv(BlockFace::Down, turned(2))
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_uv(BlockFace::East, turned(1))
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_uv(BlockFace::West, turned(3))]);
    let rotated = match facing {
        BlockFacing::North => authored,
        BlockFacing::South => crate::model_family::shape::rotate_shape_cardinal(
            authored,
            crate::model_family::direction::CardinalDirection::South,
        ),
        BlockFacing::East => crate::model_family::shape::rotate_shape_cardinal(
            authored,
            crate::model_family::direction::CardinalDirection::East,
        ),
        BlockFacing::West => crate::model_family::shape::rotate_shape_cardinal(
            authored,
            crate::model_family::direction::CardinalDirection::West,
        ),
        BlockFacing::Up => crate::model_family::shape::rotate_shape_x(authored, 1),
        BlockFacing::Down => crate::model_family::shape::rotate_shape_x(authored, 3),
    };
    rotated
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
        // All four side bands read the side tile's top band (rows 0..4), like the official
        // `template_piston_head` ([0,0,16,4] with 90/270/180 rotations — the rotations only
        // flip the band's orientation, never its row).
        .with_face_uv(BlockFace::East, side_band(0.0, 0.0))
        .with_face_uv(BlockFace::West, side_band(0.0, 0.0))
        .with_face_uv(BlockFace::Up, side_band(0.0, 0.0))
        .with_face_uv(BlockFace::Down, side_band(0.0, 0.0));
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
        // The official shaft faces all read the same top band [0,0,16,4] (the up/down ones
        // rotated 90/270, the west one mirrored) — one band for all four.
        .with_face_uv(BlockFace::East, shaft_long(0.0))
        .with_face_uv(BlockFace::West, shaft_long(0.0))
        .with_face_uv(BlockFace::Up, shaft_long(0.0))
        .with_face_uv(BlockFace::Down, shaft_long(0.0))
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

