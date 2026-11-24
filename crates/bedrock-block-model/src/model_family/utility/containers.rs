use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    BlockFacing, CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, box_unwrap_face_uvs, detail_cuboid_with_local_uv, full_texture_uv,
    rect_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_chest")
        || matches!(name, "chest" | "trapped_chest" | "ender_chest" | "barrel")
    {
        return Some(ModelFamily::Container);
    }
    if name.ends_with("_shulker_box") || matches!(name, "shulker_box" | "undyed_shulker_box") {
        return Some(ModelFamily::ShulkerBox);
    }
    if name == "chiseled_bookshelf" {
        return Some(ModelFamily::ChiseledBookshelf);
    }
    None
}

pub(crate) fn container_shape(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if name == "barrel" {
        return Some(barrel_shape(state));
    }
    if name == "chest" || name.ends_with("_chest") {
        return Some(chest_shape(state));
    }
    Some(ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    )
    .with_face_material_slot(BlockFace::Up, "up")
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "side")
    .with_face_material_slot(BlockFace::South, "side")
    .with_face_material_slot(BlockFace::East, "side")
    .with_face_material_slot(BlockFace::West, "side")]))
}

fn barrel_shape(state: &BlockStateQuery) -> ModelShape {
    let is_open = crate::model_family::direction::state_bool(state, "open_bit").unwrap_or(false);
    // `blocks.json` draws the barrel with its lid on the north face; `facing_direction` names
    // the side the lid points at, so the whole slot map turns with it (0=down, 1=up, 2..5).
    let facing = crate::model_family::direction::block_facing(state).unwrap_or(BlockFacing::North);
    let lid_slot = if is_open { "north_open" } else { "north" };
    let mut cuboid = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    for (model_face, slot) in [
        (BlockFace::Up, "side"),
        (BlockFace::Down, "side"),
        (BlockFace::North, lid_slot),
        (BlockFace::South, "south"),
        (BlockFace::East, "side"),
        (BlockFace::West, "side"),
    ] {
        cuboid = cuboid.with_face_material_slot(
            crate::model_family::direction::turn_model_face(model_face, facing),
            slot,
        );
    }
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(cuboid)])
}

pub(crate) fn shulker_box_shape() -> ModelShape {
    // Vanilla `geometry.shulker.v1.8`: the base starts at uv (0, 28) and the lid at (0, 0) of
    // the 64x64 entity skin. The top face is the only one that uses the separate 16x16
    // `shulker_top_<color>` tile, so it samples that tile whole.
    ModelShape::from_cuboids([
        box_uv_cuboid(
            ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.5, 1.0])
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Side, "side"),
            0.0,
            28.0,
        ),
        box_uv_cuboid(
            ModelCuboid::new([0.0, 0.25, 0.0], [1.0, 1.0, 1.0])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Side, "side"),
            0.0,
            0.0,
        ),
    ])
}

/// Unwraps an axis-aligned box onto the 64x64 entity skin with the standard box layout.
///
/// The top face samples the separate 16x16 `shulker_top_<color>` tile, so it keeps whole-tile UVs
/// while the sides and bottom read their own regions of the 64x64 skin.
fn box_uv_cuboid(cuboid: ModelCuboid, origin_u: f32, origin_v: f32) -> ModelCuboid {
    let min = cuboid.min.map(|value| value * 16.0);
    let max = cuboid.max.map(|value| value * 16.0);
    let mut textured = cuboid;
    for (face, uv) in box_unwrap_face_uvs(min, max, [origin_u, origin_v], [64.0, 64.0]) {
        textured = textured.with_face_uv(face, uv);
    }
    textured.with_face_uv(BlockFace::Up, rect_uv(0.0, 0.0, 1.0, 1.0))
}

pub(crate) fn chiseled_bookshelf_shape(state: &BlockStateQuery) -> ModelShape {
    let books = state_i64(state, "books_stored").unwrap_or(63);
    let mut cuboids = vec![detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, 0.0, px(1.0)], [1.0, 1.0, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side")
            .with_face_material_slot(BlockFace::North, "front"),
    )];

    for slot in 0..6 {
        if books & (1 << slot) == 0 {
            continue;
        }
        let row = slot / 3;
        let column = slot % 3;
        let (x0, x1) = match column {
            0 => (0.0, 5.0),
            1 => (5.0, 11.0),
            _ => (11.0, 16.0),
        };
        let (y0, y1) = if row == 0 { (8.0, 16.0) } else { (0.0, 8.0) };
        cuboids.push(
            detail_cuboid_with_local_uv(ModelCuboid::new(
                [px(x0), px(y0), 0.0],
                [px(x1), px(y1), px(1.0)],
            ))
            .with_material_slot("front")
            // The shelf top is wood; book tops must not show through when seen from above.
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down"),
        );
    }

    ModelShape::from_cuboids(cuboids)
}

fn chest_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let pair_direction = state_string(state, "pair_direction").and_then(cardinal_from_string);
    let (body_min, body_max) = chest_pair_bounds(pair_direction);
    // Bedrock's chest art is six independent 16x16 tiles (`chest_side`/`chest_front`/
    // `chest_top`), each carrying its own dark metal rim, wood grain and latch — NOT the
    // Java 64x64 entity skin that needs a box unwrap. Sampling these tiles through a 64x64
    // unwrap cropped away the rims and left plain wood on the sides (the user's screenshot).
    // Every face therefore reads its whole tile; the material slots route the front tile
    // onto the face the block turns toward the player.
    let full = full_texture_uv();
    let body = ModelCuboid::new(body_min, [body_max[0], px(10.0), body_max[2]])
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_material_slot(BlockFace::North, "side")
        .with_face_material_slot(BlockFace::South, "side")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side");
    let lid = ModelCuboid::new(
        [body_min[0], px(9.0), body_min[2]],
        [body_max[0], px(14.0), body_max[2]],
    )
    .with_face_material_slot(BlockFace::Up, "up")
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "side")
    .with_face_material_slot(BlockFace::South, "side")
    .with_face_material_slot(BlockFace::East, "side")
    .with_face_material_slot(BlockFace::West, "side");
    let mut shape = ModelShape::from_cuboids([body, lid]);
    for cuboid in &mut shape.cuboids {
        for face in [
            BlockFace::Up, BlockFace::Down, BlockFace::North, BlockFace::South,
            BlockFace::East, BlockFace::West,
        ] {
            *cuboid = cuboid.clone().with_face_uv(face, full);
        }
    }
    // The front tile (with its baked-in latch) goes on the face named by `cardinal_direction`.
    let front_face = match direction {
        CardinalDirection::North => BlockFace::North,
        CardinalDirection::South => BlockFace::South,
        CardinalDirection::East => BlockFace::East,
        CardinalDirection::West => BlockFace::West,
    };
    for cuboid in &mut shape.cuboids {
        *cuboid = cuboid.clone().with_face_material_slot(front_face, "front");
    }
    shape
}

fn chest_pair_bounds(pair_direction: Option<CardinalDirection>) -> ([f32; 3], [f32; 3]) {
    let mut min = [0.0625, 0.0, 0.0625];
    let mut max = [0.9375, 1.0, 0.9375];
    match pair_direction {
        Some(CardinalDirection::North) => min[2] = 0.0,
        Some(CardinalDirection::South) => max[2] = 1.0,
        Some(CardinalDirection::East) => max[0] = 1.0,
        Some(CardinalDirection::West) => min[0] = 0.0,
        None => {}
    }
    (min, max)
}

fn cardinal_from_string(value: &str) -> Option<CardinalDirection> {
    match value
        .trim()
        .strip_prefix("minecraft:")
        .unwrap_or(value.trim())
    {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shulker_sides_use_their_own_skin_regions() {
        // `geometry.shulker.v1.8`: base 16x8x16 at uv (0,28), lid 16x12x16 at uv (0,0), 64x64.
        // The standard box unwrap puts west at u0..u0+d, north at u0+d..u0+d+w, east after
        // north and south after east; the four sides share the band below the top cap.
        let shape = shulker_box_shape();
        let base = &shape.cuboids[0];
        let region = |face: BlockFace| {
            let uv = base.face_uvs[&face];
            [
                (uv[0][0] * 64.0).round(),
                (uv[0][1] * 64.0).round(),
                (uv[2][0] * 64.0).round(),
                (uv[2][1] * 64.0).round(),
            ]
        };
        assert_eq!(region(BlockFace::West), [0.0, 52.0, 16.0, 44.0]);
        assert_eq!(region(BlockFace::North), [16.0, 52.0, 32.0, 44.0]);
        assert_eq!(region(BlockFace::East), [32.0, 52.0, 48.0, 44.0]);
        assert_eq!(region(BlockFace::South), [48.0, 52.0, 64.0, 44.0]);
    }

    #[test]
    fn chest_boxes_use_whole_16px_tiles_per_face() {
        // Bedrock ships six independent 16x16 tiles with the rims/latch baked in; every
        // face must sample its whole tile, never a 64x64 box-unwrap region.
        let shape = chest_shape(
            &crate::state::BlockStateQuery::new("minecraft:chest")
                .with_state("minecraft:cardinal_direction", "north"),
        );
        assert_eq!(shape.cuboids.len(), 2); // body + lid; latch is baked into the front tile
        let full = rect_uv(0.0, 0.0, 1.0, 1.0);
        for cuboid in &shape.cuboids {
            for face in [
                BlockFace::Up, BlockFace::Down, BlockFace::North, BlockFace::South,
                BlockFace::East, BlockFace::West,
            ] {
                assert_eq!(cuboid.face_uvs[&face], full, "face {face:?} must use the whole tile");
            }
        }
    }

    #[test]
    fn chest_front_tile_follows_the_block_direction() {
        let front_faces = |direction: &str| {
            let shape = chest_shape(
                &crate::state::BlockStateQuery::new("minecraft:chest")
                    .with_state("minecraft:cardinal_direction", direction),
            );
            shape.cuboids[0].face_material_slots.iter()
                .filter(|(_, slot)| slot.as_str() == "front")
                .map(|(face, _)| *face)
                .collect::<Vec<_>>()
        };
        assert_eq!(front_faces("north"), vec![BlockFace::North]);
        assert_eq!(front_faces("south"), vec![BlockFace::South]);
        assert_eq!(front_faces("east"), vec![BlockFace::East]);
        assert_eq!(front_faces("west"), vec![BlockFace::West]);
    }

    #[test]
    fn shulker_top_face_samples_the_separate_tile() {
        // The top face's material is `shulker_top_<color>`, a 16x16 tile, not the skin.
        let shape = shulker_box_shape();
        let uv = shape.cuboids[0].face_uvs[&BlockFace::Up];
        assert_eq!(uv, rect_uv(0.0, 0.0, 1.0, 1.0));
    }
}
