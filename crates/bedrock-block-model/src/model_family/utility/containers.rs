use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{
    CardinalDirection, cardinal_direction, state_i64, state_string,
};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, box_unwrap_face_uvs, detail_cuboid_with_local_uv, rect_uv,
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
    let top_slot = if is_open { "barrel_top_open" } else { "up" };
    ModelShape::from_cuboids([detail_cuboid_with_local_uv(ModelCuboid::new(
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ))
    .with_face_material_slot(BlockFace::Up, top_slot)
    .with_face_material_slot(BlockFace::Down, "down")
    .with_face_material_slot(BlockFace::North, "side")
    .with_face_material_slot(BlockFace::South, "side")
    .with_face_material_slot(BlockFace::East, "side")
    .with_face_material_slot(BlockFace::West, "side")])
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
    // The chest is the classic 14x14x14 entity box on a 64x64 sheet: the lid unwraps from
    // (0,0) and the body from (0,19). The sheet's own alpha layout proves both origins - the
    // lid fills rows 0..19 and the body rows 19..43 in 56-pixel-wide strips, and the 6x5 latch
    // art sits at (0,0). `blocks.json` only carries the flat `chest_front`/`chest_side` icons.
    ModelShape::from_cuboids([
        chest_uv_cuboid(
            ModelCuboid::new(body_min, [body_max[0], px(10.0), body_max[2]]),
            [0.0, 19.0],
        ),
        // The lid overlaps the body's top pixel exactly like the vanilla model, so the seam
        // stays closed while the lid keeps its own 5-pixel side band.
        chest_uv_cuboid(
            ModelCuboid::new(
                [body_min[0], px(9.0), body_min[2]],
                [body_max[0], px(14.0), body_max[2]],
            ),
            [0.0, 0.0],
        ),
        chest_latch(direction),
    ])
}

/// The latch is a 2x4x1 box drawn at skin (0,0); it sits on the chest's front face.
///
/// It is authored on the north face and then turned with the block, so each face carries the
/// art of the surface it now points at.
fn chest_latch(direction: CardinalDirection) -> ModelCuboid {
    let authored = chest_uv_cuboid(
        ModelCuboid::new([px(7.0), px(8.0), 0.0], [px(9.0), px(12.0), px(1.0)]),
        [0.0, 0.0],
    );
    let turns = match direction {
        CardinalDirection::North => 0,
        CardinalDirection::East => 1,
        CardinalDirection::South => 2,
        CardinalDirection::West => 3,
    };
    (0..turns).fold(authored, |cuboid, _| rotate_cuboid_clockwise(cuboid))
}

/// Turns a cuboid one quarter turn clockwise about the block's Y axis.
///
/// The UV quads stay with their faces: for a quarter turn the renderer's corner order maps to
/// the new face's corner order index for index, so the art turns with the box.
fn rotate_cuboid_clockwise(cuboid: ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [1.0 - cuboid.max[2], cuboid.min[1], cuboid.min[0]],
        [1.0 - cuboid.min[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot;
    for (face, slot) in cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(rotate_face_clockwise(face), slot);
    }
    for (face, uv) in cuboid.face_uvs {
        rotated.face_uvs.insert(rotate_face_clockwise(face), uv);
    }
    rotated
}

fn rotate_face_clockwise(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

/// Applies the chest sheet's box unwrap, normalised to the 64x64 entity skin.
fn chest_uv_cuboid(cuboid: ModelCuboid, origin: [f32; 2]) -> ModelCuboid {
    let min = cuboid.min.map(|value| value * 16.0);
    let max = cuboid.max.map(|value| value * 16.0);
    let mut textured = cuboid;
    for (face, uv) in box_unwrap_face_uvs(min, max, origin, [64.0, 64.0]) {
        textured = textured.with_face_uv(face, uv);
    }
    textured
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
    fn chest_boxes_unwrap_the_64x64_entity_sheet() {
        // The sheet's alpha layout is a 56x19 lid strip over a 56x29 body strip, i.e. boxes of
        // 14x5x14 at (0,0) and 14x10x14 at (0,19) read with the standard unwrap.
        let shape = chest_shape(
            &crate::state::BlockStateQuery::new("minecraft:chest")
                .with_state("minecraft:cardinal_direction", "north"),
        );
        let region = |cuboid: &ModelCuboid, face: BlockFace| {
            let uv = cuboid.face_uvs[&face];
            [
                (uv[0][0] * 64.0).round(),
                (uv[0][1] * 64.0).round(),
                (uv[2][0] * 64.0).round(),
                (uv[2][1] * 64.0).round(),
            ]
        };
        let body = &shape.cuboids[0];
        assert_eq!(region(body, BlockFace::Up), [14.0, 19.0, 28.0, 33.0]);
        assert_eq!(region(body, BlockFace::North), [14.0, 43.0, 28.0, 33.0]);
        assert_eq!(region(body, BlockFace::West), [0.0, 43.0, 14.0, 33.0]);
        assert_eq!(region(body, BlockFace::East), [28.0, 43.0, 42.0, 33.0]);
        assert_eq!(region(body, BlockFace::South), [42.0, 43.0, 56.0, 33.0]);
        assert_eq!(region(body, BlockFace::Down), [28.0, 19.0, 42.0, 33.0]);

        let lid = &shape.cuboids[1];
        assert_eq!(region(lid, BlockFace::Up), [14.0, 0.0, 28.0, 14.0]);
        assert_eq!(region(lid, BlockFace::North), [14.0, 19.0, 28.0, 14.0]);

        let latch = &shape.cuboids[2];
        assert_eq!(region(latch, BlockFace::North), [1.0, 5.0, 3.0, 1.0]);
    }

    #[test]
    fn chest_latch_follows_the_block_direction() {
        let shape = |direction: &str| {
            chest_shape(
                &crate::state::BlockStateQuery::new("minecraft:chest")
                    .with_state("minecraft:cardinal_direction", direction),
            )
        };
        let latch = |direction: &str| shape(direction).cuboids[2].clone();
        assert!(latch("north").min[2] == 0.0);
        assert!(latch("south").max[2] == 1.0);
        assert!(latch("west").min[0] == 0.0);
        assert!(latch("east").max[0] == 1.0);
    }

    #[test]
    fn shulker_top_face_samples_the_separate_tile() {
        // The top face's material is `shulker_top_<color>`, a 16x16 tile, not the skin.
        let shape = shulker_box_shape();
        let uv = shape.cuboids[0].face_uvs[&BlockFace::Up];
        assert_eq!(uv, rect_uv(0.0, 0.0, 1.0, 1.0));
    }
}
