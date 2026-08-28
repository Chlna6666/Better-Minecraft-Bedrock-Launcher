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

impl FrameUploadSummary {
    pub(in crate::platform::nova) fn accumulate(&mut self, other: Self) {
        self.quad_count = self.quad_count.saturating_add(other.quad_count);
        self.shadow_count = self.shadow_count.saturating_add(other.shadow_count);
        self.animation_binding_count = self
            .animation_binding_count
            .saturating_add(other.animation_binding_count);
        self.animation_value_count = self
            .animation_value_count
            .saturating_add(other.animation_value_count);
        self.path_vertex_count = self
            .path_vertex_count
            .saturating_add(other.path_vertex_count);
        self.path_sprite_count = self
            .path_sprite_count
            .saturating_add(other.path_sprite_count);
        self.mono_sprite_count = self
            .mono_sprite_count
            .saturating_add(other.mono_sprite_count);
        self.poly_sprite_count = self
            .poly_sprite_count
            .saturating_add(other.poly_sprite_count);
        self.underline_count = self.underline_count.saturating_add(other.underline_count);
        self.backdrop_blur_count = self
            .backdrop_blur_count
            .saturating_add(other.backdrop_blur_count);
        self.unsupported_batches.paths = self
            .unsupported_batches
            .paths
            .saturating_add(other.unsupported_batches.paths);
        self.unsupported_batches.surfaces = self
            .unsupported_batches
            .surfaces
            .saturating_add(other.unsupported_batches.surfaces);
        self.unsupported_batches.backdrop_blurs = self
            .unsupported_batches
            .backdrop_blurs
            .saturating_add(other.unsupported_batches.backdrop_blurs);
        self.unsupported_batches.backdrop_blur_tint_fallbacks = self
            .unsupported_batches
            .backdrop_blur_tint_fallbacks
            .saturating_add(other.unsupported_batches.backdrop_blur_tint_fallbacks);
        self.unsupported_batches.gpu_meshes_3d = self
            .unsupported_batches
            .gpu_meshes_3d
            .saturating_add(other.unsupported_batches.gpu_meshes_3d);
    }
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
