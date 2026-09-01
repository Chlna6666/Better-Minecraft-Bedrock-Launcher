use super::*;

#[derive(Clone, Copy, Default)]
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
    /// Parsed BeginBlur/EndBlur topology. Batches are static for a retained upload, so compute this
    /// once when blur configs refresh and let target planning/present/draw-step code borrow it.
    pub(in crate::platform::nova) blur_content_ranges_cache: Vec<BlurContentRange>,
    /// Atlas textures sampled before the earliest blur barrier. This is derived only from the
    /// static retained batch stream and shared by renderer cache planning and present-time damage
    /// checks, so keep it behind Arc for cheap same-frame reuse without cloning hash buckets.
    pub(in crate::platform::nova) backdrop_source_atlas_texture_ids_cache:
        Arc<FxHashSet<AtlasTextureId>>,
    /// Animated backdrop indices whose current composite geometry fits entirely inside the
    /// unanimated filter footprint. These use the base blur geometry for filter planning while
    /// the GPU primitive buffer still receives the sampled composite geometry/opacity.
    pub(in crate::platform::nova) backdrop_blur_use_base_filter_indices: FxHashSet<u32>,
    /// Animated backdrop indices that sampled outside their base filter footprint on the previous
    /// frame. The transition back into the base footprint performs one restoring filter refresh.
    pub(in crate::platform::nova) backdrop_blur_filter_dirty_indices: FxHashSet<u32>,
    /// Reusable set used to build the next frame's filter-dirty state without allocating a fresh
    /// hash table every animation frame.
    pub(in crate::platform::nova) backdrop_blur_filter_dirty_scratch: FxHashSet<u32>,
    /// Reusable union/difference scratch for one-shot restoring filter refreshes.
    pub(in crate::platform::nova) backdrop_blur_filter_refresh_scratch: FxHashSet<u32>,
    /// Composite-only backdrop animations that may ignore the Scene-level self-animation
    /// `mark_full` damage entry for this frame.
    pub(in crate::platform::nova) backdrop_blur_ignore_animation_damage_indices: FxHashSet<u32>,
    /// Whether animated backdrop state changed Gaussian pass/config data this frame.
    pub(in crate::platform::nova) backdrop_blur_passes_dirty_this_frame: bool,
    /// True when the static encoded display list was retained for the current frame. Composite-only
    /// blur animation is only allowed to suppress self damage in this mode.
    pub(in crate::platform::nova) retained_static_reused: bool,
    /// Animation ids that were sampled on the previous frame. Keeping one frame of history makes
    /// source-animation completion conservative instead of accidentally treating it as idle.
    pub(in crate::platform::nova) backdrop_blur_previous_animation_ids:
        FxHashSet<crate::SceneAnimationId>,
    /// Reusable current-frame animation-id set. At frame end it is swapped with the previous set,
    /// so both hash-table allocations stay hot across animation frames.
    pub(in crate::platform::nova) backdrop_blur_current_animation_ids_scratch:
        FxHashSet<crate::SceneAnimationId>,
    pub(in crate::platform::nova) animation_bindings: Vec<u8>,
    pub(in crate::platform::nova) animation_values: Vec<u8>,
    pub(in crate::platform::nova) animated_primitives: Vec<AnimatedUpload>,
    pub(in crate::platform::nova) sampled_animation_values: Vec<crate::SceneAnimationValue>,
    /// One shared serialization scratch for all animated primitives. The largest current record is
    /// 136 bytes, so this replaces thousands of per-primitive tiny Vec allocations in glyph-heavy
    /// retained animations while keeping the packed GPU buffers contiguous.
    pub(in crate::platform::nova) animated_primitive_staging: Vec<u8>,
    /// Reused sampled visual bounds used by backdrop damage dependency checks.
    pub(in crate::platform::nova) animated_visual_bounds_scratch:
        Vec<crate::Bounds<crate::ScaledPixels>>,
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
