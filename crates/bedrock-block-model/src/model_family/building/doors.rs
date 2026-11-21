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
    // BE's canonical bool is `open_bit`; read it before any legacy `open` key — a world
    // that carries both with a stale `open` value must follow the bit.
    let open = state_bool(state, "open_bit")
        .or_else(|| state_bool(state, "open"))
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

    // Which side of the cell the slab sits on, taken straight from the client capture. All 32
    // captured states place both halves identically, so the door is one slab per block and the
    // half only changes the art.
    //
    // The capture also settles the direction question: a door's `cardinal_direction` names the
    // side the **slab sits against**, not the side it swings toward, so a closed south door is a
    // slab at `x 0..0.1825` (the low-X edge) rather than the high-Z one. Opening swaps the thin
    // axis and moves the slab to the hinge's side; `door_hinge_bit` chooses which edge.
    let placement = door_placement(direction, hinge_right, open);
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
    // The slab's thin axis decides which face pair is "wide", so the UVs have to follow the
    // placement rather than the pre-open direction. Using the pre-open direction put the 16 px
    // art on the 3 px edge of an open door, which is why open doors looked stretched.
    let mut cuboid = placement
        .cuboid(0.0, 1.0)
        .with_material_slot(slot.as_str());
    for (face, uv) in door_face_uv(placement) {
        cuboid = cuboid.with_face_uv(face, uv);
    }
    ModelShape::from_cuboids([cuboid])
}

/// Where one door block's slab sits: which axis it is thin on, and which edge it hugs.
#[derive(Clone, Copy)]
enum DoorPlacement {
    /// Thin on X, hugging the low-X edge.
    XLow,
    /// Thin on X, hugging the high-X edge.
    XHigh,
    /// Thin on Z, hugging the low-Z edge.
    ZLow,
    /// Thin on Z, hugging the high-Z edge.
    ZHigh,
}

impl DoorPlacement {
    /// The slab box for this placement, spanning `min_y..max_y` over the whole cell otherwise.
    fn cuboid(self, min_y: f32, max_y: f32) -> ModelCuboid {
        const THICKNESS: f32 = 0.1875;
        match self {
            Self::XLow => {
                ModelCuboid::new([0.0, min_y, 0.0], [THICKNESS, max_y, 1.0])
            }
            Self::XHigh => ModelCuboid::new(
                [1.0 - THICKNESS, min_y, 0.0],
                [1.0, max_y, 1.0],
            ),
            Self::ZLow => ModelCuboid::new([0.0, min_y, 0.0], [1.0, max_y, THICKNESS]),
            Self::ZHigh => ModelCuboid::new(
                [0.0, min_y, 1.0 - THICKNESS],
                [1.0, max_y, 1.0],
            ),
        }
    }
}

/// The captured slab placement for one door state.
///
/// Read from the client: the four closed states are `south → XLow`, `west → ZLow`,
/// `north → XHigh`, `east → ZHigh`; opening swaps the thin axis and puts the slab on the side
/// `door_hinge_bit` selects. Both halves of a door always agree, so the half is not an input.
const fn door_placement(
    direction: CardinalDirection,
    hinge_right: bool,
    open: bool,
) -> DoorPlacement {
    match (direction, hinge_right, open) {
        (CardinalDirection::South, false, false) => DoorPlacement::XLow,
        (CardinalDirection::South, true, false) => DoorPlacement::XLow,
        (CardinalDirection::South, false, true) => DoorPlacement::ZLow,
        (CardinalDirection::South, true, true) => DoorPlacement::ZHigh,
        (CardinalDirection::West, false, false) => DoorPlacement::ZLow,
        (CardinalDirection::West, true, false) => DoorPlacement::ZLow,
        (CardinalDirection::West, false, true) => DoorPlacement::XHigh,
        (CardinalDirection::West, true, true) => DoorPlacement::XLow,
        (CardinalDirection::North, false, false) => DoorPlacement::XHigh,
        (CardinalDirection::North, true, false) => DoorPlacement::XHigh,
        (CardinalDirection::North, false, true) => DoorPlacement::ZHigh,
        (CardinalDirection::North, true, true) => DoorPlacement::ZLow,
        (CardinalDirection::East, false, false) => DoorPlacement::ZHigh,
        (CardinalDirection::East, true, false) => DoorPlacement::ZHigh,
        (CardinalDirection::East, false, true) => DoorPlacement::XLow,
        (CardinalDirection::East, true, true) => DoorPlacement::XHigh,
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
fn door_face_uv(placement: DoorPlacement) -> [(BlockFace, [[f32; 2]; 4]); 6] {
    let wide = uv_rect(0.0, 0.0, 16.0, 16.0);
    let edge = uv_rect(0.0, 0.0, 3.0, 16.0);
    // A slab thin on Z spans the whole 16 px in X, so its **north and south** faces are the wide
    // ones and east/west are the 3 px edges. A slab thin on X is the mirror case.
    let (north, south, east, west) = match placement {
        DoorPlacement::XLow | DoorPlacement::XHigh => (edge, edge, wide, wide),
        DoorPlacement::ZLow | DoorPlacement::ZHigh => (wide, wide, edge, edge),
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
