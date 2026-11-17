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
    let mut cuboids = vec![ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.125, 1.0])];

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
        cuboids.push(torch_bar(px(4.0), px(7.9), px(10.0), px(14.0)));
        cuboids.push(torch_bar(px(10.0), px(7.9), px(10.0), px(14.0)));
        if is_subtract {
            cuboids.push(torch_bar(px(7.0), px(5.9), px(1.0), px(5.0)));
        } else {
            cuboids.push(torch_bar_wide(px(6.0), px(4.0), px(4.9), px(1.0), px(5.0)));
        }
    } else {
        let delay = state_i64(state, "repeater_delay")
            .or_else(|| state_i64(state, "delay"))
            .unwrap_or(0)
            .clamp(0, 3);

        // The fixed input bar sits at the front; the second bar slides 2 px per delay step away
        // from it (capture: north frame bar z 7..11 -> 5..9 -> 3..7 -> 1..5 as delay climbs,
        // mirrored into this frame).
        let shift = px(2.0 * delay as f32);
        cuboids.push(torch_bar(px(7.0), px(7.9), px(1.0), px(5.0)));
        cuboids.push(torch_bar(px(7.0), px(7.9), px(5.0) + shift, px(9.0) + shift));
    }

    // The torches above are authored with the output torch on the north side; the whole
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
/// block's own texture map.
fn torch_bar(x0: f32, y_max: f32, z0: f32, z1: f32) -> ModelCuboid {
    torch_bar_wide(x0, px(2.0), y_max, z0, z1)
}

/// The general bar form, for the comparator's wider compare-mode output bar.
fn torch_bar_wide(x0: f32, width: f32, y_max: f32, z0: f32, z1: f32) -> ModelCuboid {
    ModelCuboid::new([x0, 0.125, z0], [x0 + width, y_max, z1]).with_material_slot("torch")
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

/// Resolves the side the repeater drives power towards.
///
/// Bedrock's modern `minecraft:cardinal_direction` names the direction *from the output side
/// towards the input side*, so the output is the opposite face. The deprecated `direction`
/// integer names the output side directly (0=north, 1=east, 2=south, 3=west). Worlds may carry
/// both spellings, and the modern one wins because it describes the current state.
fn repeater_output_direction(state: &BlockStateQuery) -> CardinalDirection {
    if let Some(direction) = state_string(state, "minecraft:cardinal_direction")
        .or_else(|| state_string(state, "cardinal_direction"))
        .and_then(crate::model_family::direction::from_string)
    {
        return direction.opposite();
    }
    if let Some(value) = state_i64(state, "direction") {
        return match value.rem_euclid(4) {
            0 => CardinalDirection::North,
            1 => CardinalDirection::East,
            2 => CardinalDirection::South,
            _ => CardinalDirection::West,
        };
    }
    cardinal_direction(state)
        .map(CardinalDirection::opposite)
        .unwrap_or(CardinalDirection::North)
}
