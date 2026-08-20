use crate::material::BlockFace;
use crate::model_family::ModelFamily;
use crate::model_family::direction::{state_bool, state_string};
use crate::model_family::shape::{ModelCuboid, ModelShape, detail_cuboid_with_local_uv};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("slab")
        || name.starts_with("double_")
        || name.starts_with("waxed_double_")
        || matches!(name, "stone_slab" | "wooden_slab")
    {
        Some(ModelFamily::Slab)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, state: &BlockStateQuery) -> Option<ModelShape> {
    if is_double_slab_block(name) {
        return Some(ModelShape::from_cuboids([detail_cuboid_with_local_uv(
            ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::Side, "side"),
        )]));
    }
    let top = state_bool(state, "top_slot_bit")
        .or_else(|| state_string(state, "minecraft:vertical_half").map(is_top_half))
        .or_else(|| state_string(state, "slab_slot").map(|value| value == "top"))
        .unwrap_or(false);
    let (min_y, max_y) = if top { (0.5, 1.0) } else { (0.0, 0.5) };
    Some(ModelShape::from_cuboids([detail_cuboid_with_local_uv(
        ModelCuboid::new([0.0, min_y, 0.0], [1.0, max_y, 1.0])
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Side, "side"),
    )]))
}

fn is_double_slab_block(name: &str) -> bool {
    name.contains("double_") || name.ends_with("_double_slab")
}

fn is_top_half(value: &str) -> bool {
    value == "top" || value == "upper"
}
