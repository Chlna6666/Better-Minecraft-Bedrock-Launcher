//! Bedrock resource-pack block model resolution.
//!
//! The resolver keeps resource-pack parsing separate from mesh generation. It reads block,
//! terrain texture, material instance, permutation, and geometry JSON data, then returns a
//! `ResolvedBlockModel` suitable for a renderer or OBJ exporter to consume.

mod error;
mod geometry;
mod java;
mod java_bake;
mod java_db;
mod java_runtime;
mod json;
mod material;
mod model_family;
mod obj;
mod pack;
mod permutation;
mod resolver;
mod state;
mod texture;

pub use error::{BlockModelError, Result};
pub use geometry::{BlockGeometry, GeometryBone, GeometryCube, GeometryLibrary, GeometryUv};
pub use java::{
    JavaBakedModel, JavaModelRepository, java_block_id_for_bedrock_state,
    java_properties_for_bedrock_state,
};
pub use java_bake::{JAVA_MODEL_DB_SCHEMA, JavaModelBakeStats, bake_java_model_database};
pub use java_db::{
    JavaModelApplication, JavaModelAxis, JavaModelDatabase, JavaModelId, JavaPackedElement,
    JavaPackedElementIter, JavaPackedFace, JavaPackedFaceIter, JavaPackedModel, JavaPropertySource,
    vanilla_java_model_database,
};
pub use java_runtime::java_model_shape_for_bedrock_state;
pub use material::{
    BlockComponents, BlockFace, BlockTransformation, MaterialInstance, TextureReference, TextureSet,
};
pub use model_family::{
    ModelCuboid, ModelFamily, ModelPlane, ModelShape, canonical_block_name_for_state,
    detail_material_block_name_for_state, is_full_opaque_block, model_family_for_block_name,
    model_family_has_detail_shape,
};
pub use obj::{
    FALLBACK_MATERIAL_NAME, MATERIAL_SLOT_SEPARATOR, NamedObjMaterial, ObjExport,
    ObjExportMaterial, ObjExportTarget, ObjExportWriteSummary, ObjFace, ObjMaterial,
    ObjMaterialSample, ObjMeshFace, ObjMeshFaceSource, ObjResolvedTexture, ObjTextureCopy,
    ObjTextureResolver, alpha_mask_obj_texture_image, block_export_material_name_for_block,
    block_export_material_name_for_face, block_export_material_name_for_plane,
    block_export_material_name_for_slot, block_face_for_normal,
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
    texture_candidate_relatives, tint_obj_texture_image, vanilla_resource_pack_roots,
    vanilla_resource_pack_roots_from_packages, world_resource_pack_ids, world_resource_pack_paths,
    write_obj_export_files, write_obj_texture_copy,
};
pub use pack::{BlockDefinition, BlockModelRepository};
pub use permutation::{BlockPermutation, ConditionEvaluation};
pub use resolver::{ModelWarning, ResolvedBlockModel, ResolvedMaterialInstance, ResolvedTexture};
pub use state::{BlockStateQuery, BlockStateValue};
pub use texture::{TerrainTexture, TerrainTextureAtlas};

/// Resolves a renderer-neutral model for a Bedrock block state.
///
/// Vanilla detail families prefer the baked Java Edition geometry database because it contains
/// the authoritative vanilla element layout and blockstate rotations. Bedrock-specific/custom
/// states that do not map to Java, plus full blocks, keep the existing family implementation as a
/// fallback. Resource-pack `minecraft:geometry` remains a higher-level resolver concern and is
/// therefore still authoritative in BMCBL before this function is called.
#[must_use]
pub fn model_shape_for_block_state(state: &BlockStateQuery) -> Option<ModelShape> {
    if model_family::model_family_has_detail_shape(&state.name) {
        if let Some(shape) = java_runtime::java_model_shape_for_bedrock_state(state, 0) {
            return Some(shape);
        }
    }
    model_family::model_shape_for_block_state(state)
}
