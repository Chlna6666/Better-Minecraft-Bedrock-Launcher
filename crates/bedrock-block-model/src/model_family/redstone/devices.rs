use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{BlockFacing, block_facing};
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
    let facing = block_facing(state).unwrap_or(BlockFacing::North);
    ModelShape::from_cuboids([device_cube(name, facing)])
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
