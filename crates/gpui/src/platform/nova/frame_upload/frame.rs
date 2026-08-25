use super::*;

#[derive(Default)]
pub(in crate::platform::nova) struct FrameUploadSummary {
    pub(in crate::platform::nova) quad_count: u32,
    pub(in crate::platform::nova) shadow_count: u32,
    pub(in crate::platform::nova) animation_binding_count: u32,
    pub(in crate::platform::nova) animation_value_count: u32,
    pub(in crate::platform::nova) path_vertex_count: u32,
    pub(in crate::platform::nova) path_sprite_count: u32,
    pub(in crate::platform::nova) mono_sprite_count: u32,
    pub(in crate::platform::nova) poly_sprite_count: u32,
    pub(in crate::platform::nova) underline_count: u32,
    pub(in crate::platform::nova) backdrop_blur_count: u32,
    pub(in crate::platform::nova) unsupported_batches: UnsupportedBatchSummary,
}

#[derive(Default)]
pub(in crate::platform::nova) struct FrameUpload {
    pub(in crate::platform::nova) globals: Vec<u8>,
    pub(in crate::platform::nova) text_raster_params: Vec<u8>,
    pub(in crate::platform::nova) quads: Vec<u8>,
    pub(in crate::platform::nova) shadows: Vec<u8>,
    pub(in crate::platform::nova) path_rasterization_vertices: Vec<u8>,
    pub(in crate::platform::nova) path_sprites: Vec<u8>,
    pub(in crate::platform::nova) mono_sprites: Vec<u8>,
    pub(in crate::platform::nova) poly_sprites: Vec<u8>,
    pub(in crate::platform::nova) underlines: Vec<u8>,
    pub(in crate::platform::nova) backdrop_blur_passes: Vec<u8>,
    pub(in crate::platform::nova) backdrop_blurs: Vec<u8>,
    pub(in crate::platform::nova) backdrop_blur_configs: Vec<BackdropBlurConfig>,
    pub(in crate::platform::nova) animation_bindings: Vec<u8>,
    pub(in crate::platform::nova) animation_values: Vec<u8>,
    pub(in crate::platform::nova) custom_mesh_3d_parameters: Vec<u8>,
    pub(in crate::platform::nova) custom_mesh_3d_meshes: Vec<Arc<GpuMesh3d>>,
    pub(in crate::platform::nova) custom_mesh_3d_shaders: Vec<Arc<GpuMesh3dShader>>,
    pub(in crate::platform::nova) custom_mesh_3d_ids: FxHashSet<GpuMesh3dId>,
    pub(in crate::platform::nova) custom_mesh_3d_shader_ids: FxHashSet<GpuMesh3dShaderId>,
    pub(in crate::platform::nova) batches: Vec<UploadedBatch>,
    pub(in crate::platform::nova) path_rasterization_cache:
        FxHashMap<PathRasterizationCacheKey, PathRasterizationCacheEntry>,
    pub(in crate::platform::nova) path_rasterization_cache_hits: u64,
    pub(in crate::platform::nova) path_rasterization_cache_misses: u64,
    pub(in crate::platform::nova) path_geometry_hash_memo:
        FxHashMap<crate::PathCacheId, PathGeometryHashMemo>,
    pub(in crate::platform::nova) path_paint_key_scratch: Vec<u8>,
}
