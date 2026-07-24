use super::*;

impl NovaFrameUpload {
    pub(in crate::platform::nova) fn trim_retained_capacity(&mut self, level: GpuiMemoryTrimLevel) {
        let multiplier = match level {
            GpuiMemoryTrimLevel::Light => 16,
            GpuiMemoryTrimLevel::Moderate => 8,
            GpuiMemoryTrimLevel::Aggressive => 1,
        };
        trim_upload_vec(&mut self.globals, GLOBAL_UPLOAD_BYTES, multiplier);
        trim_upload_vec(
            &mut self.text_raster_params,
            TEXT_RASTER_UPLOAD_BYTES,
            multiplier,
        );
        trim_upload_vec(&mut self.quads, 64 * PACKED_QUAD_BYTES, multiplier);
        trim_upload_vec(&mut self.shadows, 64 * PACKED_SHADOW_BYTES, multiplier);
        trim_upload_vec(
            &mut self.path_rasterization_vertices,
            256 * PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.path_sprites,
            64 * PACKED_PATH_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.mono_sprites,
            64 * PACKED_MONO_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.poly_sprites,
            64 * PACKED_POLY_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.underlines,
            64 * PACKED_UNDERLINE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.backdrop_blur_passes,
            BACKDROP_BLUR_PASS_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.backdrop_blurs,
            PACKED_BACKDROP_BLUR_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.animation_bindings,
            64 * PACKED_ANIMATION_BINDING_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.animation_values,
            64 * PACKED_ANIMATION_VALUE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.custom_mesh_3d_parameters,
            16 * PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES,
            multiplier,
        );
        trim_upload_vec(&mut self.custom_mesh_3d_meshes, 8, multiplier);
        trim_upload_vec(&mut self.custom_mesh_3d_shaders, 8, multiplier);
        trim_upload_vec(&mut self.batches, 64, multiplier);
        self.path_rasterization_cache.clear();
    }

    pub(in crate::platform::nova) fn backdrop_blur_downsample(&self) -> u8 {
        self.backdrop_blur_downsample.max(1)
    }

    pub(in crate::platform::nova) fn backdrop_blur_levels(&self) -> usize {
        usize::from(self.backdrop_blur_levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS))
    }

    pub(in crate::platform::nova) fn uploaded_bytes(&self) -> usize {
        self.globals
            .len()
            .saturating_add(self.text_raster_params.len())
            .saturating_add(self.quads.len())
            .saturating_add(self.shadows.len())
            .saturating_add(self.path_rasterization_vertices.len())
            .saturating_add(self.path_sprites.len())
            .saturating_add(self.mono_sprites.len())
            .saturating_add(self.poly_sprites.len())
            .saturating_add(self.underlines.len())
            .saturating_add(self.backdrop_blur_passes.len())
            .saturating_add(self.backdrop_blurs.len())
            .saturating_add(self.animation_bindings.len())
            .saturating_add(self.animation_values.len())
            .saturating_add(self.custom_mesh_3d_parameters.len())
    }
}

fn trim_upload_vec<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}
