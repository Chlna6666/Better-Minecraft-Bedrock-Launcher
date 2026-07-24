use super::*;

impl NovaRenderer {
    pub(super) fn prepare_draw_steps(&mut self, partial_scissor: Option<ScissorRect>) {
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let custom_mesh_3d_pipelines = &self.custom_mesh_3d_pipelines;
        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;
        let backdrop_blur_resource_set = self
            .backdrop_blur_targets
            .as_ref()
            .and_then(|targets| {
                targets
                    .target_resource_sets
                    .get(frame_resource_index)
                    .copied()
            })
            .unwrap_or_else(|| ResourceSetId::new(0));
        let steps = &mut self.draw_step_scratch.draw_steps;
        draw_steps_for_upload_into(
            &self.frame_upload,
            &self.pipelines,
            blend_pipelines,
            self.quad_resource_set,
            self.shadow_resource_set,
            self.path_resource_set,
            |texture_id| sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index),
            |shader_id| custom_mesh_3d_pipelines.get(&shader_id).copied(),
            |mesh_id, generation| {
                custom_mesh_cache_entry(custom_mesh_3d_mesh_cache, mesh_id, generation)
            },
            self.underline_resource_set,
            backdrop_blur_resource_set,
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            NovaDrawStepMode::Present,
            steps,
        );
        if let Some(scissor) = partial_scissor {
            apply_scissor_to_steps(steps, scissor);
        }
    }

    pub(super) fn prepare_present_copy_steps(&mut self, enabled: bool) {
        let steps = &mut self.draw_step_scratch.present_copy_steps;
        steps.clear();
        if !enabled {
            return;
        }
        steps.push(RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: self.pipelines.present_copy,
            resource_sets: resource_set_list([self.present_cache_resource_set]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        }));
    }

    pub(super) fn prepare_backdrop_blur_source_steps(&mut self, enabled: bool) {
        self.draw_step_scratch.backdrop_blur_source_steps.clear();
        if !enabled {
            return;
        }
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let custom_mesh_3d_pipelines = &self.custom_mesh_3d_pipelines;
        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;
        let backdrop_blur_resource_set = self
            .backdrop_blur_targets
            .as_ref()
            .and_then(|targets| {
                targets
                    .target_resource_sets
                    .get(frame_resource_index)
                    .copied()
            })
            .unwrap_or_else(|| ResourceSetId::new(0));
        let steps = &mut self.draw_step_scratch.backdrop_blur_source_steps;
        draw_steps_for_upload_into(
            &self.frame_upload,
            &self.pipelines,
            blend_pipelines,
            self.quad_resource_set,
            self.shadow_resource_set,
            self.path_resource_set,
            |texture_id| sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index),
            |shader_id| custom_mesh_3d_pipelines.get(&shader_id).copied(),
            |mesh_id, generation| {
                custom_mesh_cache_entry(custom_mesh_3d_mesh_cache, mesh_id, generation)
            },
            self.underline_resource_set,
            backdrop_blur_resource_set,
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            NovaDrawStepMode::BackdropSource,
            steps,
        );
    }

    pub(super) fn backdrop_blur_render_passes(&self) -> Vec<NovaBackdropBlurRenderPass> {
        let Some(targets) = self.backdrop_blur_targets.as_ref() else {
            return Vec::new();
        };
        backdrop_blur_render_passes_for_targets(
            &self.pipelines,
            targets,
            self.current_frame_resource_index,
            self.frame_upload.backdrop_blur_levels(),
        )
    }

    pub(super) fn has_backdrop_blurs(&self) -> bool {
        !self.frame_upload.backdrop_blurs.is_empty()
    }

    fn current_blend_pipelines(&self) -> NovaBlendPipelines {
        if self.surface_alpha.outputs_premultiplied_alpha() {
            self.pipelines.premultiplied
        } else {
            self.pipelines.alpha
        }
    }

    pub(super) fn prepare_path_mask_draw_steps(&mut self) {
        path_mask_draw_steps_for_upload_into(
            &self.frame_upload,
            &self.pipelines,
            self.path_rasterization_resource_set,
            &mut self.draw_step_scratch.path_mask_steps,
        );
    }
}

fn sprite_resource_set(
    gpu_atlas_textures: &FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    texture_id: AtlasTextureId,
    frame_resource_index: usize,
) -> Option<ResourceSetId> {
    gpu_atlas_textures.get(&texture_id).and_then(|texture| {
        let resource_sets = match texture_id.kind {
            AtlasTextureKind::Monochrome | AtlasTextureKind::Subpixel => {
                &texture.mono_resource_sets
            }
            AtlasTextureKind::Bgra | AtlasTextureKind::Rgba => &texture.poly_resource_sets,
        };
        resource_sets.get(frame_resource_index).copied()
    })
}

fn custom_mesh_cache_entry(
    custom_mesh_3d_mesh_cache: &FxHashMap<GpuMesh3dId, NovaMeshCacheEntry>,
    mesh_id: GpuMesh3dId,
    generation: u64,
) -> Option<NovaMeshCacheEntry> {
    custom_mesh_3d_mesh_cache
        .get(&mesh_id)
        .copied()
        .filter(|entry| entry.generation == generation)
}
