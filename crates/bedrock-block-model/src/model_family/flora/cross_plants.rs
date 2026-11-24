use crate::model_family::ModelFamily;
use crate::model_family::direction::state_string;
use crate::model_family::direction::state_i64;
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, full_texture_uv, hanging_full_texture_uv, rotate_shape_x,
    rotate_shape_y_degrees, upright_full_texture_uv,
};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if is_cross_plant_family(name) {
        Some(ModelFamily::CrossPlant)
    } else {
        None
    }
}

pub(crate) fn cross_plant_shape(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if !is_cross_plant_family(name) {
        return None;
    }
    if is_row_plant_family(name) {
        return Some(row_plant_shape(row_plant_material_slot(name, state)));
    }
    if let Some(half) = double_plant_half(name, state) {
        return Some(half);
    }
    // Amethyst clusters and buds carry a `minecraft:block_face` state with **per-facing captured
    // geometry** (up = refs 201/520, down = 466, north = 251/252 …), so a floor cluster and a wall
    // cluster are genuinely different shapes. The previous shape ignored the state entirely and
    // rendered every wall cluster as a floor cross.
    if is_amethyst_name(name) {
        return Some(amethyst_shape(state));
    }
    // Floor coral fans are not a plant cross: the capture (ref 204, tube) is four tilted sheets
    // that overhang the cell by 4-8 px, turning with `coral_fan_direction` 0/1.
    if name.ends_with("_coral_fan") {
        return Some(coral_fan_shape(state));
    }
    // Solid coral blocks (tube/brain/bubble/fire/horn, alive and dead) share one captured shape
    // (ref 149): two FULL-CELL diagonal sheets corner-to-corner, not the generic 45° mini-cross
    // (which left them hollow). Use the exact captured corners.
    if name.ends_with("_coral") {
        return Some(coral_block_shape());
    }
    if hangs_from_above(name) {
        return Some(cross_plane_shape_with_uv(
            0.0,
            1.0,
            hanging_full_texture_uv(),
        ));
    }
    Some(cross_plane_shape(0.0, 1.0))
}

/// A solid coral block: two full-cell diagonal sheets crossing corner to corner, from the exact
/// capture geometry (ref 149, shared by every live/dead `*coral` block). The generic plant cross
/// used inset 45° cards (`0.5 ± 0.354`), leaving the block hollow; these span 0..1 corner to
/// corner. Each sheet is emitted double-sided so it reads from any angle.
fn coral_block_shape() -> ModelShape {
    let uv = upright_full_texture_uv();
    let sheet = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]| {
        [
            ModelPlane::new([a, b, c, d], [0, 1, 0]).with_uv(uv),
            ModelPlane::new([d, c, b, a], [0, -1, 0]).with_uv(uv),
        ]
    };
    ModelShape::default().with_planes(
        // sheet 1: (0,0,1)-(1,1,0)
        sheet(
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
        )
        .into_iter()
        // sheet 2: (0,0,0)-(1,1,1)
        .chain(sheet(
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ))
        .collect::<Vec<_>>(),
    )
}

/// A floor coral fan is not four flat quads: the game tessellates a folded fan
/// (official BedrockSceneCapture ref 358 / dead ref 202). Four quads form a pleated
/// sheet that spreads along X (x -2.93..22.67 px) with two crests at y 8.8 px and
/// two rims dipping to y 5.6, and folds out along Z (z -6.13..19.47). Every quad
/// binds the coral's own art full-sheet. `coral_fan_direction` (0..3) yaw-turns the
/// whole fan by 90 degrees. The wall fan (`*_coral_wall_fan`) uses the upright
/// path and is handled elsewhere.
fn coral_fan_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = state_i64(state, "coral_fan_direction")
        .or_else(|| state_i64(state, "minecraft:coral_fan_direction"))
        .unwrap_or(0)
        .rem_euclid(4);
    let at = |x: f32, y: f32, z: f32| [x / 16.0, y / 16.0, z / 16.0];
    let quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], n: [i32; 3], u: [[f32; 2]; 4]| {
        ModelPlane::new([at(a[0],a[1],a[2]), at(b[0],b[1],b[2]), at(c[0],c[1],c[2]), at(d[0],d[1],d[2])], n)
            .with_uv(u)
    };
    // Exact captured corners (px) and face UVs from ref 358.
    let shape = ModelShape::default().with_planes([
        // Left rim: from the crest (-2.93, 5.6) down to the central valley (9.87, 0).
        quad([-2.93,5.6,-1.33], [9.87,0.0,-1.33], [9.87,0.0,14.67], [-2.93,5.6,14.67],
             [0,-1,0], [[1.0,0.0],[1.0,1.0],[0.0,1.0],[0.0,0.0]]),
        // Upper-middle crest: (1.87,8.8,-6.13) folding down to the valley.
        quad([1.87,8.8,-6.13], [1.87,0.0,6.67], [17.87,0.0,6.67], [17.87,8.8,-6.13],
             [0,1,1], [[0.0,0.0],[0.0,1.0],[1.0,1.0],[1.0,0.0]]),
        // Lower-middle crest: folds down from the far Z edge.
        quad([1.87,8.8,19.47], [1.87,0.0,6.67], [17.87,0.0,6.67], [17.87,8.8,19.47],
             [0,-1,1], [[1.0,0.0],[1.0,1.0],[0.0,1.0],[0.0,0.0]]),
        // Right rim: valley (9.87,0) up to the far crest (22.67,5.6).
        quad([22.67,5.6,-1.33], [9.87,0.0,-1.33], [9.87,0.0,14.67], [22.67,5.6,14.67],
             [0,1,0], [[0.0,0.0],[0.0,1.0],[1.0,1.0],[1.0,0.0]]),
    ]);
    rotate_shape_y_degrees(shape, direction as f32 * 90.0)
}

/// Two-cell plants draw three crossed cards per cell, not one pair.
///
/// Measured from the capture: `tall_grass` is refs 706/707/708 in the lower cell and
/// 709/707/708 in the upper one, and `large_fern` is 545/713/714 and 715/713/714. The shared
/// refs are why the two halves look alike, but the ref unique to each half is a **third card slid
/// sideways**, so a two-card cross cannot reproduce either half. Each card is a 0.9-wide diagonal
/// pair of quads spanning the whole cell height.
///
/// The offsets below are the captured card origins in `(x, z)`, keyed by plant and half.
fn double_plant_half(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    let offsets: &[[f32; 2]] = match name {
        "tall_grass" | "tallgrass" => {
            if upper_half(state) {
                &[[0.0999, 0.2666], [0.2332, -0.0668], [0.0332, 0.2666]]
            } else {
                &[[0.1666, -0.1666], [0.2332, -0.0668], [0.0332, 0.2666]]
            }
        }
        "large_fern" => {
            if upper_half(state) {
                &[[0.2666, 0.0666], [0.1999, 0.1999], [-0.1001, 0.0332]]
            } else {
                &[[-0.1001, -0.1001], [0.1999, 0.1999], [-0.1001, 0.0332]]
            }
        }
        _ => return None,
    };
    let mut planes = Vec::with_capacity(offsets.len() * 2);
    for [x0, z0] in offsets {
        planes.extend(double_plant_card(*x0, *z0));
    }
    Some(ModelShape::default().with_planes(planes))
}

/// One double-plant card: two diagonal quads of a 0.9 square, from the floor to the ceiling.
fn double_plant_card(x0: f32, z0: f32) -> [ModelPlane; 2] {
    let (x1, z1) = (x0 + 0.9, z0 + 0.9);
    [
        ModelPlane::new(
            [
                [x0, 0.0, z1],
                [x1, 0.0, z0],
                [x1, 1.0, z0],
                [x0, 1.0, z1],
            ],
            [-1, 0, 1],
        )
        .with_uv(upright_full_texture_uv()),
        ModelPlane::new(
            [
                [x1, 0.0, z1],
                [x0, 0.0, z0],
                [x0, 1.0, z0],
                [x1, 1.0, z1],
            ],
            [1, 0, 1],
        )
        .with_uv(upright_full_texture_uv()),
    ]
}

fn upper_half(state: &BlockStateQuery) -> bool {
    state
        .state("upper_block_bit")
        .is_some_and(|value| value.is_truthy())
}

fn is_cross_plant_family(name: &str) -> bool {
    matches!(
        name,
        "bamboo_sapling"
            | "beetroot"
            | "bush"
            | "cactus_flower"
            | "carrots"
            | "cave_vines"
            | "cave_vines_plant"
            | "cave_vines_body_with_berries"
            | "cave_vines_head_with_berries"
            | "closed_eyeblossom"
            | "cobweb"
            | "small_amethyst_bud"
            | "medium_amethyst_bud"
            | "large_amethyst_bud"
            | "amethyst_cluster"
            | "allium"
            | "azure_bluet"
            | "blue_orchid"
            | "cornflower"
            | "dandelion"
            | "deadbush"
            | "double_plant"
            | "fern"
            | "firefly_bush"
            | "golden_dandelion"
            | "kelp"
            | "large_fern"
            | "lilac"
            | "lily_of_the_valley"
            | "nether_wart"
            | "pale_hanging_moss"
            | "open_eyeblossom"
            | "orange_tulip"
            | "oxeye_daisy"
            | "peony"
            | "pink_tulip"
            | "pitcher_plant"
            | "poppy"
            | "rose_bush"
            | "melon_stem"
            | "nether_sprouts"
            | "potatoes"
            | "pumpkin_stem"
            | "reeds"
            | "red_flower"
            | "red_tulip"
            | "sapling"
            | "seagrass"
            | "short_dry_grass"
            | "short_grass"
            | "sunflower"
            | "sweet_berry_bush"
            | "tall_dry_grass"
            | "tall_grass"
            | "tallgrass"
            | "torchflower"
            | "torchflower_crop"
            | "waterlily"
            | "web"
            | "wheat"
            | "white_tulip"
            | "wildflowers"
            | "wither_rose"
            | "yellow_flower"
    ) || name.ends_with("_sapling")
        || name.ends_with("_flower")
        || name.ends_with("_mushroom")
        || name.ends_with("_fungus")
        || name.ends_with("_roots")
        || name.ends_with("_coral")
        || name.ends_with("_coral_fan")
        || name.ends_with("_crop")
        || name.ends_with("_vines")
        // A vine's `_plant` segment is the same cross model as the tip; without this the body
        // blocks fell through to the full-block fallback and rendered as opaque cubes.
        || name.ends_with("_vines_plant")
}

fn is_row_plant_family(name: &str) -> bool {
    matches!(
        name,
        "beetroot"
            | "carrots"
            | "nether_wart"
            | "potatoes"
            | "seagrass"
            | "tall_seagrass"
            | "wheat"
    )
}

fn row_plant_material_slot<'a>(name: &str, state: &'a BlockStateQuery) -> Option<&'a str> {
    if name != "seagrass" && name != "tall_seagrass" {
        return None;
    }
    match state_string(state, "sea_grass_type") {
        Some("double_bot") | Some("double_bottom") | Some("bottom") => Some("down"),
        Some("double_top") | Some("top") => Some("east"),
        _ => Some("up"),
    }
}

fn row_plant_shape(material_slot: Option<&str>) -> ModelShape {
    ModelShape::default().with_planes(
        [
            (
                [
                    [0.0, 0.0, 0.25],
                    [1.0, 0.0, 0.25],
                    [1.0, 1.0, 0.25],
                    [0.0, 1.0, 0.25],
                ],
                [0, 0, 1],
            ),
            (
                [
                    [0.0, 0.0, 0.75],
                    [1.0, 0.0, 0.75],
                    [1.0, 1.0, 0.75],
                    [0.0, 1.0, 0.75],
                ],
                [0, 0, 1],
            ),
            (
                [
                    [0.25, 0.0, 0.0],
                    [0.25, 0.0, 1.0],
                    [0.25, 1.0, 1.0],
                    [0.25, 1.0, 0.0],
                ],
                [1, 0, 0],
            ),
            (
                [
                    [0.75, 0.0, 0.0],
                    [0.75, 0.0, 1.0],
                    [0.75, 1.0, 1.0],
                    [0.75, 1.0, 0.0],
                ],
                [1, 0, 0],
            ),
        ]
        .map(|(corners, normal)| {
            let plane = ModelPlane::new(corners, normal).with_uv(upright_full_texture_uv());
            if let Some(material_slot) = material_slot {
                plane.with_material_slot(material_slot)
            } else {
                plane
            }
        }),
    )
}

/// Plants whose art hangs from the block above instead of standing on its floor.
fn hangs_from_above(name: &str) -> bool {
    matches!(
        name,
        "pale_hanging_moss"
            | "hanging_roots"
            | "cave_vines"
            | "cave_vines_plant"
            | "cave_vines_body_with_berries"
            | "cave_vines_head_with_berries"
            | "weeping_vines"
            | "weeping_vines_plant"
            | "twisting_vines"
            | "twisting_vines_plant"
    )
}

pub(super) fn cross_plane_shape(thickness: f32, height: f32) -> ModelShape {
    cross_plane_shape_with_uv(thickness, height, upright_full_texture_uv())
}

fn cross_plane_shape_with_uv(thickness: f32, height: f32, uv: [[f32; 2]; 4]) -> ModelShape {
    let inset = thickness.min(0.2);
    // Two full-size planes rotated 45 degrees about the block centre. Their horizontal extent is
    // 0.5 +/- 0.5*sin(45), matching the vanilla/holoprint cross model instead of sticking out of
    // the cell.
    let reach = 0.5_f32 * std::f32::consts::FRAC_1_SQRT_2;
    let (lo, hi) = (0.5 - reach, 0.5 + reach);
    let mut shape = ModelShape::default().with_planes([
        ModelPlane::new(
            [
                [lo, 0.0, lo],
                [hi, 0.0, hi],
                [hi, height, hi],
                [lo, height, lo],
            ],
            [-1, 0, 1],
        )
        .with_uv(uv),
        ModelPlane::new(
            [
                [hi, 0.0, lo],
                [lo, 0.0, hi],
                [lo, height, hi],
                [hi, height, lo],
            ],
            [1, 0, 1],
        )
        .with_uv(uv),
    ]);
    if inset > 0.0 {
        shape.cuboids.push(ModelCuboid::new(
            [0.5 - inset, 0.0, 0.5 - inset],
            [0.5 + inset, height, 0.5 + inset],
        ));
    }
    shape
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_grass_id_is_not_a_cross_plant() {
        assert_eq!(family_for("grass"), None);
        assert_eq!(family_for("short_grass"), Some(ModelFamily::CrossPlant));
        assert_eq!(family_for("tallgrass"), Some(ModelFamily::CrossPlant));
    }
}

/// Whether this block is one of the amethyst growth stages.
fn is_amethyst_name(name: &str) -> bool {
    matches!(
        name,
        "small_amethyst_bud" | "medium_amethyst_bud" | "large_amethyst_bud" | "amethyst_cluster"
    )
}

/// Amethyst clusters attach by `minecraft:block_face` and **change geometry per facing**.
///
/// Measured from the capture: a floor cluster (refs 201/520) is the standard crossed pair of
/// diagonal sheets spanning the full cell height; a ceiling cluster (466) spans the same extents;
/// and a wall cluster (251/252) spans the **full depth** on one horizontal axis while the other
/// two axes clip to `0.05..0.95`. So the cross is genuinely rotated per facing, not just
/// re-textured.
fn amethyst_shape(state: &BlockStateQuery) -> ModelShape {
    let cross = cross_plane_shape(0.0, 1.0);
    match state_string(state, "minecraft:block_face").unwrap_or("up") {
        "up" => cross,
        // A ceiling cluster keeps the same silhouette: the capture's extents match the floor one.
        "down" => cross,
        // Wall clusters rotate the cross into the vertical plane facing outward.
        "north" => rotate_shape_x(cross, 1),
        "south" => rotate_shape_x(cross, 3),
        "west" => rotate_shape_x(cross, 1),
        "east" => rotate_shape_x(cross, 3),
        _ => cross,
    }
}
