use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_bool, state_i64, state_string,
};
use crate::model_family::shape::{ModelCuboid, ModelShape, rotate_cuboid_cardinal};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("repeater") || name.contains("comparator") {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let output = repeater_output_direction(state);

    // The redstone torches are drawn from the two-frame `repeater_torch`/`comparator_torch`
    // terrain arrays [off, on]. Repeater activation lives in the block name (powered_ vs
    // unpowered_); comparator activation in `output_lit_bit`. Both can also arrive as a bool
    // state, so check every spelling before falling back to off.
    let lit = name.contains("powered")
        && !name.contains("unpowered")
        || state_bool(state, "powered_bit").unwrap_or(false)
        || state_bool(state, "powered").unwrap_or(false)
        || state_bool(state, "output_lit_bit").unwrap_or(false);
    let torch_slot = if lit { "torch_on" } else { "torch_off" };

    // Base slab (JE `repeater_1tick` / `comparator`): the TOP carries the mechanism art
    // (`repeater_up` / `comparator_up`, a two-frame [off, on] array), while the sides show the
    // bottom 2 px band of the stone slab and the underside the whole slab tile. The previous
    // base had no slot at all, so every face fell back to the block's own texture — the
    // stone base read as a second copy of the mechanism art with the grooves pointing the
    // wrong way.
    let top_slot = if lit { "top_on" } else { "top_off" };
    let floor_slot = if name.contains("comparator") {
        "comparator_floor"
    } else {
        "repeater_floor"
    };
    let slab_band = flipped_uv(0.0, 14.0, 16.0, 16.0);
    let mut cuboids = vec![
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])
            .with_face_uv(BlockFace::Up, flipped_uv(0.0, 0.0, 16.0, 16.0))
            .with_face_uv(BlockFace::Down, flipped_uv(0.0, 0.0, 16.0, 16.0))
            .with_face_uv(BlockFace::North, slab_band)
            .with_face_uv(BlockFace::South, slab_band)
            .with_face_uv(BlockFace::East, slab_band)
            .with_face_uv(BlockFace::West, slab_band)
            .with_face_material_slot(BlockFace::Up, top_slot)
            .with_face_material_slot(BlockFace::Down, floor_slot)
            .with_face_material_slot(BlockFace::North, floor_slot)
            .with_face_material_slot(BlockFace::South, floor_slot)
            .with_face_material_slot(BlockFace::East, floor_slot)
            .with_face_material_slot(BlockFace::West, floor_slot),
    ];

    // Bedrock draws the redstone "torches" as flat slab bars 2..7.9 px tall (capture refs: north
    // repeater 560/564/568/570, south comparators 274 compare / 278 subtract). The authored frame
    // below is the capture frame after the rotation each family already applies, so the bars
    // land where the client puts them per facing.
    if name.contains("comparator") {
        let is_subtract = state_bool(state, "output_subtract_bit")
            .or_else(|| state_string(state, "comparator_mode").map(|s| s == "subtract" || s == "1"))
            .unwrap_or(false);

        // Two input bars flanking the centre at the back, and the output bar at the front. The
        // compare-mode output bar is wider and shorter (x 6..10, y to 4.9); subtract mode keeps
        // it narrow but taller (x 7..9, y to 5.9).
        cuboids.push(torch_bar(px(4.0), px(7.9), px(10.0), px(14.0), torch_slot));
        cuboids.push(torch_bar(px(10.0), px(7.9), px(10.0), px(14.0), torch_slot));
        if is_subtract {
            cuboids.push(torch_bar(px(7.0), px(5.9), px(1.0), px(5.0), torch_slot));
        } else {
            cuboids.push(torch_bar_wide(px(6.0), px(4.0), px(4.9), px(1.0), px(5.0), torch_slot));
        }
    } else {
        let delay = state_i64(state, "repeater_delay")
            .or_else(|| state_i64(state, "delay"))
            .unwrap_or(0)
            .clamp(0, 3);

        // Capture (refs 558-570, north frame): both sticks sit centred at x 7..9. The input
        // torch is fixed at z 11..15; the delay torch starts touching it at z 7..11 and slides
        // toward the output (north) 2 px per delay step, reaching z 1..5 at delay 3. Each torch
        // carries a 2 px cap at y 6.9..7.9 and a pair of 4 px-wide flat art cards across its
        // cap, which is what gives the torch its wider head silhouette.
        let shift = px(2.0 * delay as f32);
        cuboids.push(torch_bar(px(7.0), px(7.9), px(11.0), px(15.0), torch_slot));
        cuboids.push(torch_cap(px(7.0), px(12.0), px(14.0), torch_slot));
        cuboids.push(torch_bar(px(7.0), px(7.9), px(7.0) - shift, px(11.0) - shift, torch_slot));
        cuboids.push(torch_cap(px(7.0), px(10.0) - shift, px(12.0) - shift, torch_slot));
    }

    // The torches above are authored with the input torch on the south side; the whole
    // assembly turns with the block.
    ModelShape::from_cuboids(
        cuboids
            .iter()
            .map(|cuboid| rotate_cuboid_cardinal(cuboid, output))
            .collect::<Vec<_>>(),
    )
}

/// One redstone torch bar: a 2 px wide slab spanning `z0..z1` at `x0..x0+2`, standing on the base
/// slab. Samples the shared `repeater_torch` / `comparator_torch` art, which is not part of the
/// block's own texture map. The art is a 16 x 16 standing-torch sprite whose 2 px stick runs at
/// x 7..9 from y 6 down — a full-tile UV squeezes the whole sprite (mostly transparent) onto the
/// 2 px face, so every face samples the stick column slice instead.
fn torch_bar(x0: f32, y_max: f32, z0: f32, z1: f32, slot: &'static str) -> ModelCuboid {
    torch_bar_wide(x0, px(2.0), y_max, z0, z1, slot)
}

/// The torch head cap: a 2 x 2 x 1 px block sitting on the stick at `z0..z1`, sampling the
/// sprite's glowing head rows.
fn torch_cap(x0: f32, z0: f32, z1: f32, slot: &'static str) -> ModelCuboid {
    let mut cap = ModelCuboid::new([x0, px(6.9), z0], [x0 + px(2.0), px(7.9), z1])
        .with_material_slot(slot);
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        cap = cap.with_face_uv(face, torch_slice_uv(6.0, 6.0, 10.0, 8.0));
    }
    cap
}

/// The general bar form, for the comparator's wider compare-mode output bar.
fn torch_bar_wide(
    x0: f32,
    width: f32,
    y_max: f32,
    z0: f32,
    z1: f32,
    slot: &'static str,
) -> ModelCuboid {
    let mut bar = ModelCuboid::new([x0, px(2.0), z0], [x0 + width, y_max, z1])
        .with_material_slot(slot);
    // JE `repeater_1tick` binds the torch art window exactly: sides uv [7,6,9,11] and the cap
    // [7,6,9,8] on the 2 px stick, so the flame reads as the sprite's own head instead of a
    // stretched slice of the column.
    let side = torch_slice_uv(7.0, 6.0, 9.0, 11.0);
    let head = torch_slice_uv(7.0, 6.0, 9.0, 8.0);
    for (face, uv) in [
        (BlockFace::North, side),
        (BlockFace::South, side),
        (BlockFace::East, side),
        (BlockFace::West, side),
        (BlockFace::Up, head),
        (BlockFace::Down, side),
    ] {
        bar = bar.with_face_uv(face, uv);
    }
    bar
}

/// A flipped sprite slice: bottom-left corner first, matching the renderer's face order while
/// the authored rect measures V downwards.
fn torch_slice_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

fn head_slice_uv() -> [[f32; 2]; 4] {
    torch_slice_uv(7.0, 6.0, 9.0, 8.0)
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

/// Resolves the side the repeater drives power towards.
///
/// The capture's north-facing frame (ref 560) carries the fixed input torch at the south end
/// with the delay torch sliding north, so `minecraft:cardinal_direction` names the OUTPUT side
/// directly. The deprecated `direction` integer does the same (0=north, 1=east, 2=south,
/// 3=west).
fn repeater_output_direction(state: &BlockStateQuery) -> CardinalDirection {
    if let Some(direction) = state_string(state, "minecraft:cardinal_direction")
        .or_else(|| state_string(state, "cardinal_direction"))
        .and_then(crate::model_family::direction::from_string)
    {
        return direction;
    }
    if let Some(value) = state_i64(state, "direction") {
        return match value.rem_euclid(4) {
            0 => CardinalDirection::North,
            1 => CardinalDirection::East,
            2 => CardinalDirection::South,
            _ => CardinalDirection::West,
        };
    }
    cardinal_direction(state).unwrap_or(CardinalDirection::North)
}

/// One rect of a 16 x 16 sheet, emitted bottom-left first (the authored rect measures V from
/// the texture top).
fn flipped_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}
