use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{BlockFacing, block_facing, state_bool};
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if matches!(name, "dispenser" | "dropper" | "observer" | "target")
        || name.starts_with("daylight_detector")
    {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.starts_with("daylight_detector") {
        return ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.375, 1.0])]);
    }
    if name.ends_with("observer") {
        return observer_shape(state);
    }
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    ModelShape::from_cuboids([device_cube(name, facing)])
}

/// The observer is a rigidly rotated model, not a role-mapped cube.
///
/// The official blockstate turns one north-authored model exactly like the piston's:
/// `east y90, south y180, west y270, up x270, down x90`. Only the front face carries the
/// silver eye art, the back the red-dot art, and the top texture is directional, so the
/// side/top/back art must travel with the turn — keeping the authored roles in place
/// (the previous approach) left the top arrow pointing sideways on east/west facings.
///
/// Authored facing north, the per-face windows follow `template_observer`:
/// front/back/sides take the full tile while the **up face reads the tile V-flipped**
/// (`uv [0,16,16,0]`) so its diagonal arrow points at the observed side.
fn observer_shape(state: &BlockStateQuery) -> ModelShape {
    use crate::model_family::direction::CardinalDirection;
    use crate::model_family::shape::{rotate_shape_cardinal, rotate_shape_x};
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    // `observer_south` is the two-frame terrain array `[observer_back, observer_back_lit]`;
    // a powered observer's red dot lights up, which is the frame-1 slot suffix.
    let back_slot = if state_bool(state, "powered_bit")
        .or_else(|| state_bool(state, "powered"))
        .unwrap_or(false)
    {
        "south_on"
    } else {
        "south"
    };
    let full = full_tile();
    let authored = ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_uv(BlockFace::North, full)
        .with_face_material_slot(BlockFace::South, back_slot)
        .with_face_uv(BlockFace::South, full)
        .with_face_material_slot(BlockFace::East, "east")
        .with_face_material_slot(BlockFace::West, "west")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_uv(BlockFace::Up, up_face_window())
        .with_face_material_slot(BlockFace::Down, "down")]);
    match facing {
        BlockFacing::North => authored,
        BlockFacing::South => rotate_shape_cardinal(authored, CardinalDirection::South),
        BlockFacing::East => rotate_shape_cardinal(authored, CardinalDirection::East),
        BlockFacing::West => rotate_shape_cardinal(authored, CardinalDirection::West),
        BlockFacing::Up => rotate_shape_x(authored, 1),
        BlockFacing::Down => rotate_shape_x(authored, 3),
    }
}

/// A full cube whose faces take their art by **role**, not by rotating model slots.
///
/// `blocks.json` does not describe a rotatable model here; it names one slot per role:
///
/// ```text
/// dispenser: north/south/west = dispenser_side, south = front_horizontal,
///            east = front_vertical, up/down = dispenser_top
/// observer:  north = observer_front, south = observer_back,
///            east/west = observer_side, up/down = observer_top/bottom
/// ```
///
/// So the face the state points at always shows the front art — using the *vertical* variant
/// when it points up or down — and the rest keep their role. Rotating the model's slots instead
/// (the previous approach) moved `dispenser_side` onto the front and `front_horizontal` onto a
/// side, which is why the faces came out scrambled.
fn device_cube(name: &str, facing: BlockFacing) -> ModelCuboid {
    let front = facing_face(facing);
    let observer = name.ends_with("observer");
    let mut cuboid = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        let slot = if face == front {
            if observer {
                "north"
            } else if matches!(facing, BlockFacing::Up | BlockFacing::Down) {
                // The vertical front art lives in the `east` slot.
                "east"
            } else {
                "south"
            }
        } else if observer && face == opposite_face(front) {
            "south"
        } else if matches!(face, BlockFace::Up | BlockFace::Down) {
            if observer {
                if face == BlockFace::Up { "up" } else { "down" }
            } else {
                "up"
            }
        } else if observer {
            // Both observer sides share `observer_side`.
            if face == BlockFace::East { "east" } else { "west" }
        } else {
            // Every non-front side of a dispenser/dropper is `dispenser_side`.
            "north"
        };
        cuboid = cuboid.with_face_material_slot(face, slot);
    }
    // The front art is drawn with an orientation (the dispenser nozzle, the observer's face), so
    // it has to rotate **in the face plane** as the block turns. Leaving the UV fixed while the
    // slot moved kept the art upright on every side, which is the "side is pinned" look.
    if let Some(turns) = front_uv_turns(facing) {
        cuboid = cuboid.with_face_uv(front, crate::model_family::shape::rotate_uv_quarter(
            full_tile_uv(),
            turns,
        ));
    }
    cuboid
}

/// Full 0..1 tile in the renderer's bottom-left-first order.
fn full_tile_uv() -> [[f32; 2]; 4] {
    [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
}

/// Full-tile quad in the family UV order (matches `shape::full_texture_uv`).
fn full_tile() -> [[f32; 2]; 4] {
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
}

/// The observer's up-face window: the full tile read V-flipped (JE `uv [0,16,16,0]`),
/// so the top texture's diagonal arrow points at the observed side.
fn up_face_window() -> [[f32; 2]; 4] {
    [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
}

/// Quarter turns that keep the front art upright for a given facing.
///
/// The front texture is authored pointing at the viewer when the block faces south, so a block
/// facing north needs a half turn, and the east/west facings need a quarter turn either way.
fn front_uv_turns(facing: BlockFacing) -> Option<i32> {
    match facing {
        BlockFacing::South => None,
        BlockFacing::North => Some(2),
        BlockFacing::East => Some(1),
        BlockFacing::West => Some(3),
        // A vertical front uses the dedicated vertical art, which needs no in-face rotation.
        BlockFacing::Up | BlockFacing::Down => None,
    }
}

fn opposite_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::Up => BlockFace::Down,
        BlockFace::Down => BlockFace::Up,
        BlockFace::North => BlockFace::South,
        BlockFace::South => BlockFace::North,
        BlockFace::East => BlockFace::West,
        BlockFace::West => BlockFace::East,
        other => other,
    }
}

fn facing_face(facing: BlockFacing) -> BlockFace {
    match facing {
        BlockFacing::North => BlockFace::North,
        BlockFacing::South => BlockFace::South,
        BlockFacing::East => BlockFace::East,
        BlockFacing::West => BlockFace::West,
        BlockFacing::Up => BlockFace::Up,
        BlockFacing::Down => BlockFace::Down,
    }
}
