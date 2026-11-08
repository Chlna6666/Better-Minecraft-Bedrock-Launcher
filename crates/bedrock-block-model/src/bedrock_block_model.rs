//! Bedrock resource-pack block model resolution.
//!
//! The resolver keeps resource-pack parsing separate from mesh generation. It reads block,
//! terrain texture, material instance, permutation, and geometry JSON data, then returns a
//! `ResolvedBlockModel` suitable for a renderer or OBJ exporter to consume.

mod error;
mod geometry;
mod geometry_shape;
mod item_icons;
mod json;
mod lighting;
mod material;
pub use lighting::BlockLightProperties;
mod model_family;
mod neighbor;
mod obj;
mod pack;
mod permutation;
mod resolver;
mod resource;
mod runtime_capture;
mod state;
mod texture;
mod voxel_shape;

pub use error::{BlockModelError, Result};
pub use geometry::{BlockGeometry, GeometryBone, GeometryCube, GeometryLibrary, GeometryUv};
pub use geometry_shape::shape_from_geometry;
pub use item_icons::{ItemEntry, item_icon_count, item_icon_entry, item_icon_name};
pub use material::{
    BlockComponents, BlockFace, BlockTransformation, MaterialInstance, TextureReference, TextureSet,
};
pub use model_family::{
    ModelCuboid, ModelFamily, ModelPlane, ModelShape, canonical_block_name_for_state,
    detail_cuboid_with_local_uv, detail_material_block_name_for_state, is_full_opaque_block,
    model_family_for_block_name, model_family_has_detail_shape,
};
pub use neighbor::{
    DerivedModelVariant, HorizontalDirection, HorizontalMask, NeighborBlockDescriptor,
    NeighborModelKind, RedstoneConnection, StairShape, WallConnection, derive_model_variant,
};
pub use obj::{
    FALLBACK_MATERIAL_NAME, LocalLightDefinition, MATERIAL_SLOT_SEPARATOR, NamedObjMaterial,
    ObjExport, ObjExportMaterial, ObjExportTarget, ObjExportWriteSummary, ObjFace, ObjMaterial,
    ObjMaterialSample, ObjMeshFace, ObjMeshFaceSource, ObjResolvedPbrTextureSet,
    ObjResolvedTexture, ObjTextureCopy, ObjTextureResolver, PbrTextureLayer,
    alpha_mask_obj_texture_image, block_export_material_name_for_block,
    block_export_material_name_for_face, block_export_material_name_for_plane,
    block_export_material_name_for_slot, block_face_for_normal, decode_texture_image,
    default_block_face_uvs_from_corners, export_obj_from_face_sources_with_package_roots,
    find_texture_file, obj_alpha_texture_path, obj_block_face_for_normal, obj_block_identifier,
    obj_block_texture_name, obj_canonical_block_lookup_name, obj_cull_hidden_mesh_faces,
    obj_default_face_uvs_from_corners, obj_document_string,
    obj_export_from_face_sources_with_package_roots, obj_export_from_mesh_face_groups,
    obj_export_from_mesh_face_groups_with_progress, obj_export_from_parts, obj_export_materials,
    obj_face_normal_from_triangle, obj_face_texture_slot_suffix, obj_faces_string,
    obj_material_library_from_export_materials, obj_material_library_string,
    obj_material_name_for_block, obj_material_name_for_face, obj_material_name_for_slot,
    obj_material_needs_biome_tinted_texture, obj_material_slot_candidates,
    obj_material_slot_component, obj_material_slots_for_block_face, obj_material_texture_name,
    obj_material_texture_tint, obj_material_uses_preview_tint, obj_material_uses_texture_alpha,
    obj_mesh_face_materials, obj_mesh_faces_from_source, obj_mesh_faces_string,
    obj_normal_for_block_face, obj_normalize_texture_key, obj_path_extension_eq,
    obj_texture_copies, obj_texture_faces_for_block_face, obj_texture_key_from_value,
    obj_vertex_offsets, path_extension_eq, path_starts_with_directory, prefixed_relative_path,
    push_unique_resource_pack_path, read_obj_texture_copy_image, relative_path_string,
    replace_path_extension, resource_pack_manifest_uuid, resource_pack_roots_for_world,
    vanilla_resource_pack_roots, vanilla_resource_pack_roots_from_packages,
    world_resource_pack_ids, world_resource_pack_paths, write_obj_export_files,
    write_obj_texture_copy,
};
pub use pack::{BlockDefinition, BlockModelRepository};
pub use permutation::{BlockPermutation, ConditionEvaluation};
pub use resolver::{ModelWarning, ResolvedBlockModel, ResolvedMaterialInstance, ResolvedTexture};
pub use resource::ResourceFile;
pub use runtime_capture::{
    RuntimeCaptureCatalog, capture_texture_slot, decode_capture_texture_slot,
};
pub use state::{BlockStateQuery, BlockStateValue};
pub use texture::{TerrainTexture, TerrainTextureAtlas};
pub use voxel_shape::{
    VoxelBox, VoxelFaceCoverage, VoxelShape, VoxelShapeLibrary, vanilla_voxel_shapes,
};

/// Bedrock engine-rendered block entities that need a procedural geometry fallback when
/// an exact native capture or resource-pack model is unavailable.
pub fn is_engine_block_entity_model_block(name: &str) -> bool {
    // Beds use Bedrock model-family geometry; signs, hanging signs, bells, and lecterns
    // use Bedrock-layout shapes and locally resolved art.
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "bed" || name.ends_with("_bed")
}

/// Procedural Bedrock-only fallback for states without resource-pack/captured geometry.
/// These shapes are not represented as authoritative native exports. Exact per-state
/// geometry is resolved by BlockModelRepository::captured_shape or block pack geometry.
#[must_use]
pub fn model_shape_for_block_state(state: &BlockStateQuery) -> Option<ModelShape> {
    let mut shape = model_family::model_shape_for_block_state(state)?;
    apply_double_plant_material_slots(state, &mut shape);
    Some(shape)
}

/// Preserve Bedrock double-plant face texture selection by block-state half.
fn apply_double_plant_material_slots(state: &BlockStateQuery, shape: &mut ModelShape) {
    let canonical = canonical_block_name_for_state(state);
    let normalized = canonical.strip_prefix("minecraft:").unwrap_or(&canonical);
    if !matches!(
        normalized,
        "tall_grass"
            | "tallgrass"
            | "large_fern"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "pitcher_plant"
            | "double_plant"
    ) {
        return;
    }
    let upper = state
        .state("upper_block_bit")
        .is_some_and(|value| value.is_truthy());
    let slot = if upper { "up" } else { "down" };
    for plane in &mut shape.planes {
        plane.material_slot = Some(slot.to_owned());
    }
}


#[cfg(test)]
mod native_fallback_tests {
    use super::*;

    #[test]
    fn double_plant_slots_follow_bedrock_half_state() {
        for (upper, expected) in [(false, "down"), (true, "up")] {
            let state = BlockStateQuery::new("minecraft:sunflower")
                .with_state("upper_block_bit", upper);
            let mut shape = ModelShape::default().with_planes([ModelPlane::new(
                [[0.0, 0.0, 0.0]; 4],
                [0, 1, 0],
            )]);
            apply_double_plant_material_slots(&state, &mut shape);
            assert_eq!(shape.planes[0].material_slot.as_deref(), Some(expected));
        }
    }

    #[test]
    fn non_plant_blocks_keep_original_material_slots() {
        let state = BlockStateQuery::new("minecraft:stone");
        let mut shape = ModelShape::default().with_planes([ModelPlane::new(
            [[0.0, 0.0, 0.0]; 4],
            [0, 1, 0],
        ).with_material_slot("original")]);
        apply_double_plant_material_slots(&state, &mut shape);
        assert_eq!(shape.planes[0].material_slot.as_deref(), Some("original"));
    }
}
