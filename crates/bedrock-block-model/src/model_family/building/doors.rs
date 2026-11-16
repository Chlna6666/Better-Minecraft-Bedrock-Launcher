use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_door") || matches!(name, "wooden_door" | "iron_door") {
        Some(ModelFamily::Door)
    } else {
        None
    }
}

pub(crate) fn shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let open = state_bool(state, "open")
        .or_else(|| state_bool(state, "open_bit"))
        .or_else(|| state_bool(state, "minecraft:open"))
        .unwrap_or(false);
    let hinge_right = state_bool(state, "door_hinge_bit")
        .or_else(|| state_bool(state, "hinge_bit"))
        .or_else(|| state_bool(state, "hinge"))
        .or_else(|| state_string(state, "hinge").map(|v| v == "right"))
        .unwrap_or(false);
    let upper = state_bool(state, "upper_block_bit")
        .or_else(|| state_bool(state, "upper_block"))
        .unwrap_or(false);

    let effective_dir = if open {
        if hinge_right {
            direction.clockwise()
        } else {
            direction.counter_clockwise()
        }
    } else {
        direction
    };
    // Doors are two blocks tall; each half samples its own `door_<wood>_lower/upper` tile.
    // Falling back to face-based naming gave both halves `door_upper` (blocks.json maps the
    // door's side faces to the upper tile), so the lower half showed the top artwork.
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    // `wooden_door` strips to `"wooden"`, so it has to be mapped explicitly; leaving it out
    // produced the slot `door_wooden_lower`, which does not exist, and the fallback then put the
    // upper-half art on both halves.
    let wood = name.strip_suffix("_door").unwrap_or("wooden");
    let wood = match wood {
        "wooden" | "wood" | "oak" => "wood",
        other => other,
    };
    let slot = format!("door_{wood}_{}", if upper { "upper" } else { "lower" });
    // Each half only draws its own block: the lower half is rows 0..16 of the door art and the
    // upper half rows 16..32. Sampling the whole 16 x 32 tile on a 3 x 16 px box stretched one
    // half over both blocks, which is why a lone half looked like a complete door.
    let half_uv = door_face_uv(effective_dir);
    let mut cuboid =
        wall_attached_cuboid(effective_dir, 0.1875, 0.0, 1.0).with_material_slot(slot.as_str());
    for (face, uv) in half_uv {
        cuboid = cuboid.with_face_uv(face, uv);
    }
    ModelShape::from_cuboids([cuboid])
}

fn wall_attached_cuboid(
    direction: CardinalDirection,
    thickness: f32,
    min_y: f32,
    max_y: f32,
) -> ModelCuboid {
    match direction {
        CardinalDirection::North => ModelCuboid::new([0.0, min_y, 0.0], [1.0, max_y, thickness]),
        CardinalDirection::South => {
            ModelCuboid::new([0.0, min_y, 1.0 - thickness], [1.0, max_y, 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([1.0 - thickness, min_y, 0.0], [1.0, max_y, 1.0])
        }
        CardinalDirection::West => ModelCuboid::new([0.0, min_y, 0.0], [thickness, max_y, 1.0]),
    }
}

/// Per-face UVs for one door half.
///
/// Each half has its **own 16 x 16 tile** (`door_wood_lower.png`, `door_wood_upper.png`), not one
/// 16 x 32 sheet, so a half samples its whole tile with no V offset. The door is a 3 x 16 x 16 px
/// slab: the two wide faces take the full tile and the 3 px edge takes a 3 x 16 slice, matching
/// JE's `door_bottom_left` (`west`/`east` = `[0,0,16,16]`, `north`/`south` = `[3,0,0,16]`).
/// Dividing by 32 (a mistaken 16 x 32 assumption) squashed the art horizontally and stretched it
/// vertically.
fn door_face_uv(direction: CardinalDirection) -> [(BlockFace, [[f32; 2]; 4]); 6] {
    let wide = uv_rect(0.0, 0.0, 16.0, 16.0);
    let edge = uv_rect(0.0, 0.0, 3.0, 16.0);
    // A north/south door is a 16 x 16 x 3 px slab, so its **north and south** faces are the
    // 16 x 16 wide ones and east/west are the 3 px edges. East/west doors are the mirror case.
    let (north, south, east, west) = match direction {
        CardinalDirection::North | CardinalDirection::South => (wide, wide, edge, edge),
        CardinalDirection::East | CardinalDirection::West => (edge, edge, wide, wide),
    };
    [
        (BlockFace::North, north),
        (BlockFace::South, south),
        (BlockFace::East, east),
        (BlockFace::West, west),
        (BlockFace::Up, uv_rect(0.0, 0.0, 3.0, 3.0)),
        (BlockFace::Down, uv_rect(0.0, 0.0, 3.0, 3.0)),
    ]
}

/// One rect of a door tile (16 x 16), emitted bottom-left first.
fn uv_rect(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}
