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
        self.path_geometry_hash_memo.clear();
        self.path_paint_key_scratch = Vec::new();
    }

    pub(in crate::platform::nova) fn backdrop_blur_downsample(&self) -> NovaBackdropBlurConfigSet {
        self.backdrop_blur_config_set()
    }

    pub(in crate::platform::nova) fn backdrop_blur_levels(&self) -> usize {
        usize::from(self.backdrop_blur_levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS))
    }

    /// Atlas textures that can contribute pixels to at least one backdrop source.
    ///
    /// Keep this as a hash set all the way through invalidation: converting it to a temporary Vec
    /// would add another allocation and turn pending-upload lookup into an O(uploads * textures)
    /// nested scan.
    pub(in crate::platform::nova) fn backdrop_source_atlas_texture_ids(
        &self,
    ) -> FxHashSet<AtlasTextureId> {
        let Some(last_blur_batch) = self
            .batches
            .iter()
            .rposition(|batch| matches!(batch, NovaUploadedBatch::BackdropBlurs { .. }))
        else {
            return FxHashSet::default();
        };

        let mut textures = FxHashSet::default();
        for batch in &self.batches[..last_blur_batch] {
            match *batch {
                NovaUploadedBatch::MonoSprites { texture_id, .. }
                | NovaUploadedBatch::PolySprites { texture_id, .. } => {
                    textures.insert(texture_id);
                }
                NovaUploadedBatch::SolidQuads { .. }
                | NovaUploadedBatch::Quads { .. }
                | NovaUploadedBatch::Shadows { .. }
                | NovaUploadedBatch::PathRasterization { .. }
                | NovaUploadedBatch::Paths { .. }
                | NovaUploadedBatch::Underlines { .. }
                | NovaUploadedBatch::BackdropBlurs { .. }
                | NovaUploadedBatch::CustomMesh3d { .. } => {}
            }
        }
        textures
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

    pub(in crate::platform::nova) fn mapped_upload_bytes(&self, has_backdrop_blurs: bool) -> usize {
        let mut bytes = self.uploaded_bytes();
        if !has_backdrop_blurs {
            bytes = bytes
                .saturating_sub(self.backdrop_blur_passes.len())
                .saturating_sub(self.backdrop_blurs.len());
        }
        bytes
    }

    pub(in crate::platform::nova) fn upload_breakdown(
        &self,
    ) -> crate::diagnostics::performance_metrics::FrameUploadBreakdown {
        crate::diagnostics::performance_metrics::FrameUploadBreakdown {
            encoded_primitives: self.encoded_primitive_count(),
            encoded_batches: self.batches.len(),
            quad_bytes: self.quads.len(),
            shadow_bytes: self.shadows.len(),
            path_bytes: self
                .path_rasterization_vertices
                .len()
                .saturating_add(self.path_sprites.len()),
            mono_sprite_bytes: self.mono_sprites.len(),
            poly_sprite_bytes: self.poly_sprites.len(),
            underline_bytes: self.underlines.len(),
            backdrop_blur_bytes: self
                .backdrop_blur_passes
                .len()
                .saturating_add(self.backdrop_blurs.len()),
            animation_bytes: self
                .animation_bindings
                .len()
                .saturating_add(self.animation_values.len()),
            custom_mesh_parameter_bytes: self.custom_mesh_3d_parameters.len(),
        }
    }

    fn encoded_primitive_count(&self) -> usize {
        [
            packed_item_count(&self.quads, PACKED_QUAD_BYTES),
            packed_item_count(&self.shadows, PACKED_SHADOW_BYTES),
            packed_item_count(&self.path_sprites, PACKED_PATH_SPRITE_BYTES),
            packed_item_count(&self.mono_sprites, PACKED_MONO_SPRITE_BYTES),
            packed_item_count(&self.poly_sprites, PACKED_POLY_SPRITE_BYTES),
            packed_item_count(&self.underlines, PACKED_UNDERLINE_BYTES),
            packed_item_count(&self.backdrop_blurs, PACKED_BACKDROP_BLUR_BYTES),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

fn packed_item_count(bytes: &[u8], stride: usize) -> usize {
    bytes.len().checked_div(stride).unwrap_or(0)
}

fn trim_upload_vec<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}
