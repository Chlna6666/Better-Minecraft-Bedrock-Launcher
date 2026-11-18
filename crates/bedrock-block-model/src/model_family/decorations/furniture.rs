use crate::material::BlockFace;
use crate::banner_art::CLOTH_PANELS;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{CardinalDirection, cardinal_direction};
use crate::model_family::shape::{
    ModelCuboid, ModelShape, rect_uv, rotate_shape_cardinal, rotate_shape_y_degrees,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.ends_with("_standing_sign")
        || name.ends_with("_wall_sign")
        || name.ends_with("_hanging_sign")
        || matches!(name, "standing_sign" | "wall_sign" | "hanging_sign")
    {
        return Some(ModelFamily::Sign);
    }
    if name.ends_with("_bed") || name == "bed" {
        return Some(ModelFamily::Bed);
    }
    if name.ends_with("_banner") || matches!(name, "banner" | "standing_banner" | "wall_banner") {
        return Some(ModelFamily::Banner);
    }
    if name == "bell" {
        return Some(ModelFamily::Bell);
    }
    None
}

pub(crate) fn sign_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.ends_with("_wall_sign") || name == "wall_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([sign_board_uv(wall_sign_board_cuboid(direction))]);
    }

    if name.ends_with("_wall_hanging_sign") || name == "wall_hanging_sign" {
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([hanging_sign_board_uv(wall_hanging_sign_board_cuboid(
            direction,
        ))]);
    }

    if name.ends_with("_hanging_sign") || name == "hanging_sign" {
        let mut cuboids = vec![hanging_sign_board_uv(ModelCuboid::new(
            [px(1.0), 0.0, px(7.0)],
            [px(15.0), px(10.0), px(9.0)],
        ))];
        if crate::model_family::direction::state_bool(state, "attached_bit").unwrap_or(false) {
            cuboids.push(
                hanging_sign_chain_uv(ModelCuboid::new(
                    [0.0, px(10.0), px(7.5)],
                    [1.0, 1.0, px(8.5)],
                ))
                .with_face_uv(BlockFace::South, sign_uv(12.0, 6.0, 16.0, 6.0))
                .with_face_uv(BlockFace::North, sign_uv(12.0, 6.0, 16.0, 6.0)),
            );
        } else {
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(1.5), px(10.0), px(7.5)],
                [px(4.5), 1.0, px(8.5)],
            )));
            cuboids.push(hanging_sign_chain_uv(ModelCuboid::new(
                [px(11.5), px(10.0), px(7.5)],
                [px(14.5), 1.0, px(8.5)],
            )));
        }
        if !crate::model_family::direction::state_bool(state, "hanging").unwrap_or(false) {
            cuboids.push(hanging_sign_bar_uv(ModelCuboid::new(
                [0.0, px(14.0), px(6.0)],
                [1.0, 1.0, px(10.0)],
            )));
        }
        return ModelShape::from_cuboids(cuboids);
    }

    // Standing signs have sixteen 22.5 degree orientations. Value 0 faces south and the value
    // grows clockwise seen from above, so the authored south-facing board just turns.
    let degrees = ground_sign_degrees(state);
    rotate_shape_y_degrees(
        ModelShape::from_cuboids([
            sign_board_uv(ModelCuboid::new(
                [px(0.25), px(8.25), px(7.25)],
                [px(15.75), 1.0, px(8.75)],
            )),
            sign_post_uv(ModelCuboid::new(
                [px(7.25), 0.0, px(7.25)],
                [px(8.75), px(8.25), px(8.75)],
            )),
        ]),
        degrees,
    )
}

/// `ground_sign_direction` (0..15) as degrees clockwise from the south-facing authored model.
fn ground_sign_degrees(state: &BlockStateQuery) -> f32 {
    let value = crate::model_family::direction::state_i64(state, "ground_sign_direction")
        .unwrap_or(0)
        .rem_euclid(16);
    value as f32 * 22.5
}

fn sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 2.0, 24.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 2.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 0.0, 24.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 0.0, 24.0, 2.0))
}

fn sign_post_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::Side, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::East, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::West, sign_uv(2.0, 16.0, 2.0, 12.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 16.0, 2.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(2.0, 16.0, 2.0, 2.0))
}

fn hanging_sign_board_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_face_material_slot(BlockFace::North, "front")
        .with_face_material_slot(BlockFace::South, "front")
        .with_face_material_slot(BlockFace::East, "side")
        .with_face_material_slot(BlockFace::West, "side")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down")
        .with_face_uv(BlockFace::North, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::South, sign_uv(2.0, 14.0, 14.0, 10.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 14.0, 2.0, 10.0))
        .with_face_uv(BlockFace::Up, sign_uv(2.0, 12.0, 14.0, 2.0))
        .with_face_uv(BlockFace::Down, sign_uv(16.0, 12.0, 14.0, 2.0))
}

fn hanging_sign_chain_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("chain")
        .with_face_uv(BlockFace::South, sign_uv(0.0, 6.0, 6.0, 6.0))
        .with_face_uv(BlockFace::North, sign_uv(6.0, 6.0, 6.0, 6.0))
}

fn hanging_sign_bar_uv(cuboid: ModelCuboid) -> ModelCuboid {
    cuboid
        .with_material_slot("bar")
        .with_face_uv(BlockFace::North, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::South, sign_uv(4.0, 4.0, 16.0, 2.0))
        .with_face_uv(BlockFace::East, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::West, sign_uv(0.0, 4.0, 4.0, 2.0))
        .with_face_uv(BlockFace::Up, sign_uv(4.0, 0.0, 16.0, 4.0))
        .with_face_uv(BlockFace::Down, sign_uv(20.0, 0.0, 16.0, 4.0))
}

fn wall_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(0.25), px(4.25), 0.0], [px(15.75), px(12.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(0.25), px(4.25), px(14.5)], [px(15.75), px(12.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), px(4.25), px(0.25)], [1.0, px(12.0), px(15.75)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, px(4.25), px(0.25)], [px(1.5), px(12.0), px(15.75)])
        }
    }
}

fn wall_hanging_sign_board_cuboid(direction: CardinalDirection) -> ModelCuboid {
    match direction {
        CardinalDirection::North => {
            ModelCuboid::new([px(1.0), 0.0, 0.0], [px(15.0), px(10.0), px(1.5)])
        }
        CardinalDirection::South => {
            ModelCuboid::new([px(1.0), 0.0, px(14.5)], [px(15.0), px(10.0), 1.0])
        }
        CardinalDirection::East => {
            ModelCuboid::new([px(14.5), 0.0, px(1.0)], [1.0, px(10.0), px(15.0)])
        }
        CardinalDirection::West => {
            ModelCuboid::new([0.0, 0.0, px(1.0)], [px(1.5), px(10.0), px(15.0)])
        }
    }
}

fn sign_uv(u: f32, v: f32, width: f32, height: f32) -> [[f32; 2]; 4] {
    rect_uv(u / 64.0, v / 32.0, (u + width) / 64.0, (v + height) / 32.0)
}

const fn px(value: f32) -> f32 {
    value / 16.0
}

/// Banners are ONE block each in Bedrock (`standing_banner` / `wall_banner`; there is no plain
/// `banner` block): the visual simply spills out of the cell. A standing banner's pole spans its
/// cell and the full 20 x 40 px cloth hangs from the pole top into the cell below — the
/// two-block-tall look. When that cell is occupied (`banner_compressed`, resolved from the
/// neighbour) the cloth squeezes back into the banner's own cell. A vertically attached wall
/// banner (`facing_direction` 0/1) hangs under the block above with the same compression rule
/// (`hanging_compressed`); wall-mounted ones (2..5) keep the cloth flat against the mount face.
pub(crate) fn banner_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name.ends_with("wall_banner") {
        let facing = crate::model_family::direction::state_i64(state, "facing_direction");
        if matches!(facing, Some(0) | Some(1)) {
            let compressed = crate::model_family::direction::state_bool(
                state,
                "hanging_compressed",
            )
            .unwrap_or(false);
            if !compressed {
                return ModelShape::from_cuboids([
                    banner_cloth(hanging_banner_cloth(), true, true),
                    banner_rod(hanging_banner_rod()),
                ]);
            }
            return ModelShape::from_cuboids([
                banner_cloth(hanging_banner_cloth_compressed(), true, true),
                banner_rod(hanging_banner_rod()),
            ]);
        }
        let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
        return ModelShape::from_cuboids([
            banner_cloth(wall_banner_cloth(direction), true, true),
            banner_rod(wall_banner_rod(direction)),
        ]);
    }
    // Standing banner (and the legacy `banner` spelling): the pole spans the cell, the cloth
    // hangs from the pole top — y 16 down to y −24 px, i.e. 2.5 cells — and the rod rides the
    // cloth's bottom edge. `ground_sign_direction` 0 = south, 22.5 degrees per step clockwise.
    let compressed = crate::model_family::direction::state_bool(state, "banner_compressed")
        .unwrap_or(false);
    let cloth = if compressed {
        banner_cloth_cuboid()
    } else {
        ModelCuboid::new([-0.125, -1.5, 0.5625], [1.125, 1.0, 0.625])
    };
    let rod = if compressed {
        banner_rod_cuboid()
    } else {
        ModelCuboid::new([-0.125, -1.5, 0.5625], [1.125, -1.4375, 0.625])
    };
    let mut cuboids = vec![
        banner_cloth(cloth, true, true),
        banner_pole_cuboid(),
        banner_rod(rod),
    ];
    rotate_shape_y_degrees(ModelShape::from_cuboids(cuboids), ground_sign_degrees(state))
}

pub(crate) fn bell_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let attachment = crate::model_family::direction::state_string(state, "attachment")
        .unwrap_or("standing")
        .trim();
    // `bell_side` is one unfolded piece: a 6 x 7 px body column at `(1, 0)..(7, 7)` over an
    // 8 x 2 px skirt at `(0, 7)..(8, 9)`, while `bell_top` / `bell_bottom` are 8 x 8 squares in
    // the tile corner. Sampling the whole 16 x 16 tile would shrink the bell to a quarter of
    // each face and open transparent gutters around it.
    let body_side = bell_uv(1.0, 0.0, 7.0, 7.0);
    let skirt_side = bell_uv(0.0, 7.0, 8.0, 9.0);
    let cap = bell_uv(0.0, 0.0, 8.0, 8.0);
    let body = ModelCuboid::new([px(5.0), px(6.0), px(5.0)], [px(11.0), px(13.0), px(11.0)])
        .with_face_uv(BlockFace::North, body_side)
        .with_face_uv(BlockFace::South, body_side)
        .with_face_uv(BlockFace::East, body_side)
        .with_face_uv(BlockFace::West, body_side)
        .with_face_uv(BlockFace::Up, bell_uv(1.0, 1.0, 7.0, 7.0))
        .with_face_uv(BlockFace::Down, cap)
        // `blocks.json` only maps north/south to `bell_side`: east/west are the wooden bar and
        // the stone posts, so the metal sides have to name their slot explicitly.
        .with_face_material_slot(BlockFace::East, "north")
        .with_face_material_slot(BlockFace::West, "north")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down");
    let skirt = ModelCuboid::new([px(4.0), px(4.0), px(4.0)], [px(12.0), px(6.0), px(12.0)])
        .with_face_uv(BlockFace::North, skirt_side)
        .with_face_uv(BlockFace::South, skirt_side)
        .with_face_uv(BlockFace::East, skirt_side)
        .with_face_uv(BlockFace::West, skirt_side)
        .with_face_uv(BlockFace::Up, cap)
        .with_face_uv(BlockFace::Down, cap)
        .with_face_material_slot(BlockFace::North, "north")
        .with_face_material_slot(BlockFace::South, "north")
        .with_face_material_slot(BlockFace::East, "north")
        .with_face_material_slot(BlockFace::West, "north")
        .with_face_material_slot(BlockFace::Up, "up")
        .with_face_material_slot(BlockFace::Down, "down");

    // Frames follow the Java bell models: bar `[2,13,7]..[14,15,9]` on dark oak planks, posts
    // `[0/14,0,6]..[2/16,16,10]` on the stone slot, a stub for ceilings and one bracket for
    // walls (floor posts double as the between-walls frame).
    let bar = |min: [f32; 3], max: [f32; 3]| ModelCuboid::new(min, max).with_material_slot("east");
    let post = |min: [f32; 3], max: [f32; 3]| ModelCuboid::new(min, max).with_material_slot("west");
    let mut cuboids = vec![body, skirt];
    match attachment {
        "hanging" => cuboids.push(bar([px(7.0), px(13.0), px(7.0)], [px(9.0), 1.0, px(9.0)])),
        "side" => cuboids.push(bar([px(3.0), px(13.0), px(7.0)], [1.0, px(15.0), px(9.0)])),
        "multiple" => cuboids.push(bar([0.0, px(13.0), px(7.0)], [1.0, px(15.0), px(9.0)])),
        _ => {
            cuboids.push(bar(
                [px(2.0), px(13.0), px(7.0)],
                [px(14.0), px(15.0), px(9.0)],
            ));
            cuboids.push(post([0.0, 0.0, px(6.0)], [px(2.0), 1.0, px(10.0)]));
            cuboids.push(post([px(14.0), 0.0, px(6.0)], [1.0, 1.0, px(10.0)]));
        }
    }
    rotate_shape_cardinal(ModelShape::from_cuboids(cuboids), direction)
}

/// One rect of a bell sheet: the art sits in the top-left corner of a 16 x 16 tile.
fn bell_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    // Authored rows measure V from the texture top; face corner 0 is the bottom-left one.
    [
        [u0 / 16.0, v1 / 16.0],
        [u1 / 16.0, v1 / 16.0],
        [u1 / 16.0, v0 / 16.0],
        [u0 / 16.0, v0 / 16.0],
    ]
}

/// The banner cloth box: 20 px wide, 1 px thick, standing in front of the pole at z 6..7.
fn banner_cloth_cuboid() -> ModelCuboid {
    ModelCuboid::new([-0.125, 0.0, 0.5625], [1.125, 1.0, 0.625])
}

/// `banner_base` holds the 20 x 40 px cloth twice: the front panel at `(1, 1)..(21, 41)` and a
/// mirrored back panel at `(22, 1)..(42, 41)`. `lower` selects the bottom half of that cloth,
/// `full` squeezes the whole panel onto a single-block wall banner.
fn banner_cloth(cuboid: ModelCuboid, lower: bool, full: bool) -> ModelCuboid {
    let [_, front_y0, _, front_y1] = CLOTH_PANELS[0];
    let (v0, v1) = if full {
        (front_y0 as f32, front_y1 as f32)
    } else if lower {
        (front_y0 as f32 + 20.0, front_y1 as f32)
    } else {
        (front_y0 as f32, front_y0 as f32 + 20.0)
    };
    let [front_x0, _, front_x1, _] = CLOTH_PANELS[0];
    let [back_x0, _, back_x1, _] = CLOTH_PANELS[1];
    let front = banner_sheet(front_x0 as f32, v0, front_x1 as f32, v1);
    // The back panel is the same art mirrored, so its U order is reversed.
    let back = banner_sheet(back_x1 as f32, v0, back_x0 as f32, v1);
    cuboid
        .with_material_slot("front")
        .with_face_uv(BlockFace::North, front)
        .with_face_uv(BlockFace::East, front)
        .with_face_uv(BlockFace::West, front)
        .with_face_uv(BlockFace::Up, front)
        .with_face_uv(BlockFace::Down, front)
        .with_face_uv(BlockFace::South, back)
}

/// The 2 px pole that runs up both banner blocks, sampling the sheet's pole strip.
fn banner_pole_cuboid() -> ModelCuboid {
    // The pole strip is the 2 px column at x 44..45, painted for the whole sheet height.
    let pole = banner_sheet(44.0, 2.0, 46.0, 44.0);
    let mut cuboid =
        ModelCuboid::new([0.4375, 0.0, 0.4375], [0.5625, 1.0, 0.5625]).with_material_slot("front");
    for face in [
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
        BlockFace::Up,
        BlockFace::Down,
    ] {
        cuboid = cuboid.with_face_uv(face, pole);
    }
    cuboid
}

/// The 20 px rod under the cloth, drawn from the sheet's bottom bar.
fn banner_rod_cuboid() -> ModelCuboid {
    ModelCuboid::new([-0.125, 0.0, 0.5625], [1.125, 0.0625, 0.625])
}

fn banner_rod(cuboid: ModelCuboid) -> ModelCuboid {
    // The rod is the 40 x 2 px band under the cloth panels, `(2, 42)..(42, 44)`.
    let rod = banner_sheet(2.0, 42.0, 42.0, 44.0);
    let mut rod_cuboid = cuboid.with_material_slot("front");
    for face in [
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
        BlockFace::Up,
        BlockFace::Down,
    ] {
        rod_cuboid = rod_cuboid.with_face_uv(face, rod);
    }
    rod_cuboid
}

/// One rect of the 64 x 64 `banner_base` sheet, emitted bottom-left first (the authored rect
/// measures V from the texture top).
fn banner_sheet(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [
        [u0 / 64.0, v1 / 64.0],
        [u1 / 64.0, v1 / 64.0],
        [u1 / 64.0, v0 / 64.0],
        [u0 / 64.0, v0 / 64.0],
    ]
}

/// Wall banners keep the cloth in one block, flat against the mount face.
fn wall_banner_cloth(direction: CardinalDirection) -> ModelCuboid {
    // `facing_direction` names the side the art faces, so the cloth hangs on the opposite edge,
    // flat against the wall it is mounted on.
    match direction.opposite() {
        CardinalDirection::North => ModelCuboid::new([-0.125, 0.0, 0.0], [1.125, 0.9375, 0.0625]),
        CardinalDirection::South => ModelCuboid::new([-0.125, 0.0, 0.9375], [1.125, 0.9375, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.9375, 0.0, -0.125], [1.0, 0.9375, 1.125]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.0, -0.125], [0.0625, 0.9375, 1.125]),
    }
}

/// A hanging banner hangs under the block above it: the rod hugs the mount and the full
/// 40 px cloth panel drops below the cell (2.5 blocks total).
fn hanging_banner_cloth() -> ModelCuboid {
    ModelCuboid::new([-0.125, -1.5625, 0.5625], [1.125, 0.9375, 0.625])
}

/// The squeezed hanging cloth: the whole panel compressed back into one cell because the
/// block below the mount is occupied.
fn hanging_banner_cloth_compressed() -> ModelCuboid {
    ModelCuboid::new([-0.125, 0.0, 0.5625], [1.125, 0.9375, 0.625])
}

fn hanging_banner_rod() -> ModelCuboid {
    ModelCuboid::new([-0.125, 0.9375, 0.5625], [1.125, 1.0, 0.625])
}

fn wall_banner_rod(direction: CardinalDirection) -> ModelCuboid {
    match direction.opposite() {
        CardinalDirection::North => ModelCuboid::new([-0.125, 0.9375, 0.0], [1.125, 1.0, 0.0625]),
        CardinalDirection::South => ModelCuboid::new([-0.125, 0.9375, 0.9375], [1.125, 1.0, 1.0]),
        CardinalDirection::East => ModelCuboid::new([0.9375, 0.9375, -0.125], [1.0, 1.0, 1.125]),
        CardinalDirection::West => ModelCuboid::new([0.0, 0.9375, -0.125], [0.0625, 1.0, 1.125]),
    }
}
