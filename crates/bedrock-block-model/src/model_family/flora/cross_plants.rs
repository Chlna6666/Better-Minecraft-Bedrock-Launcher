use crate::model_family::ModelFamily;
use crate::model_family::direction::state_string;
use crate::model_family::shape::{ModelCuboid, ModelPlane, ModelShape, full_texture_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if is_cross_plant_family(name) {
        Some(ModelFamily::CrossPlant)
    } else {
        None
    }
}

pub(crate) fn shape_for_cross_plant(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if !is_cross_plant_family(name) {
        return None;
    }
    if is_row_plant_family(name) {
        return Some(row_plant_shape(row_plant_material_slot(name, state)));
    }
    Some(cross_plane_shape(0.0, 1.0))
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
            let plane = ModelPlane::new(corners, normal).with_uv(full_texture_uv());
            if let Some(material_slot) = material_slot {
                plane.with_material_slot(material_slot)
            } else {
                plane
            }
        }),
    )
}

fn cross_plane_shape(thickness: f32, height: f32) -> ModelShape {
    let inset = thickness.min(0.2);
    let (offset_x, offset_z) = (0.0625, -0.0625);
    let mut shape = ModelShape::default().with_planes([
        ModelPlane {
            corners: [
                [offset_x, 0.0, offset_z],
                [1.0 + offset_x, 0.0, 1.0 + offset_z],
                [1.0 + offset_x, height, 1.0 + offset_z],
                [offset_x, height, offset_z],
            ],
            normal: [-1, 0, 1],
            material_slot: None,
            uv: None,
        },
        ModelPlane {
            corners: [
                [1.0 + offset_x, 0.0, offset_z],
                [offset_x, 0.0, 1.0 + offset_z],
                [offset_x, height, 1.0 + offset_z],
                [1.0 + offset_x, height, offset_z],
            ],
            normal: [1, 0, 1],
            material_slot: None,
            uv: None,
        },
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
