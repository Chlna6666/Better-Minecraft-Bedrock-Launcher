use super::*;

pub(super) struct NovaPreparedBackdropBlurGroup {
    pub(super) source_steps: Vec<RenderStepDescriptor>,
    pub(super) filter_passes: Vec<NovaBackdropBlurRenderPass>,
}

impl NovaRenderer {
    pub(super) fn prepare_draw_steps(&mut self) {
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let custom_mesh_3d_pipelines = &self.custom_mesh_3d_pipelines;
        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;
        let backdrop_blur_targets = self.backdrop_blur_targets.as_ref();
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
            |config| {
                backdrop_blur_targets?.resource_set_for_config(config, frame_resource_index)
            },
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            NovaDrawStepMode::Present,
            steps,
        );
    }

    /// Builds one exact source-prefix render for every backdrop batch.
    ///
    /// Each group now renders only the source rectangle needed by its Gaussian kernel. The old
    /// implementation replayed every prefix over the full drawable and then ran two full-screen
    /// filter passes, which made a small titlebar/popover blur scale with window resolution.
    pub(super) fn prepare_backdrop_blur_groups(
        &self,
        enabled: bool,
    ) -> Vec<NovaPreparedBackdropBlurGroup> {
        if !enabled {
            return Vec::new();
        }
        let Some(targets) = self.backdrop_blur_targets.as_ref() else {
            return Vec::new();
        };
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let custom_mesh_3d_pipelines = &self.custom_mesh_3d_pipelines;
        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;
        let mut groups = Vec::new();

        for (batch_index, batch) in self.frame_upload.batches.iter().enumerate() {
            let NovaUploadedBatch::BackdropBlurs { first, count } = *batch else {
                continue;
            };
            let configs = self
                .frame_upload
                .backdrop_blur_configs_for_range(first, count);
            if configs.is_empty() {
                continue;
            }

            let mut source_steps = Vec::new();
            draw_steps_for_upload_into(
                &self.frame_upload,
                &self.pipelines,
                blend_pipelines,
                self.quad_resource_set,
                self.shadow_resource_set,
                self.path_resource_set,
                |texture_id| {
                    sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index)
                },
                |shader_id| custom_mesh_3d_pipelines.get(&shader_id).copied(),
                |mesh_id, generation| {
                    custom_mesh_cache_entry(custom_mesh_3d_mesh_cache, mesh_id, generation)
                },
                self.underline_resource_set,
                |config| targets.resource_set_for_config(config, frame_resource_index),
                self.custom_mesh_3d_resource_set,
                self.custom_mesh_3d_indices_buffer,
                NovaDrawStepMode::BackdropSourceThrough {
                    batch_end: batch_index,
                },
                &mut source_steps,
            );

            let group_source_scissor = configs
                .iter()
                .filter_map(|config| blur_source_scissor(*config, self.current_size))
                .reduce(union_scissor_rects);
            if let Some(scissor) = group_source_scissor {
                apply_scissor_to_steps(&mut source_steps, scissor);
            }

            let mut filter_passes = Vec::new();
            backdrop_blur_render_passes_for_configs_into(
                &self.pipelines,
                targets,
                frame_resource_index,
                &configs,
                &mut filter_passes,
            );
            apply_filter_pass_scissors(&configs, self.current_size, &mut filter_passes);

            groups.push(NovaPreparedBackdropBlurGroup {
                source_steps,
                filter_passes,
            });
        }
        groups
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
        let backdrop_blur_targets = self.backdrop_blur_targets.as_ref();
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
            |config| {
                backdrop_blur_targets?.resource_set_for_config(config, frame_resource_index)
            },
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            NovaDrawStepMode::BackdropSource,
            steps,
        );
    }

    pub(super) fn prepare_backdrop_blur_passes(&mut self, enabled: bool) {
        if enabled {
            self.frame_upload
                .rebuild_backdrop_blur_passes_for_current_frame();
        }
        let passes = &mut self.draw_step_scratch.backdrop_blur_passes;
        passes.clear();
        if !enabled {
            return;
        }
        let Some(targets) = self.backdrop_blur_targets.as_ref() else {
            return;
        };
        backdrop_blur_render_passes_for_targets_into(
            &self.pipelines,
            targets,
            self.current_frame_resource_index,
            passes,
        );
        let configs: Vec<_> = targets.variants.iter().map(|variant| variant.config).collect();
        apply_filter_pass_scissors(&configs, self.current_size, passes);
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

fn apply_filter_pass_scissors(
    configs: &[NovaBackdropBlurConfig],
    drawable_size: DrawableSize,
    passes: &mut [NovaBackdropBlurRenderPass],
) {
    for (config, pass_pair) in configs.iter().zip(passes.chunks_mut(2)) {
        let Some(source_scissor) = blur_source_scissor(*config, drawable_size) else {
            continue;
        };
        let target_scissor =
            downsample_scissor(source_scissor, config.downsample(), drawable_size);
        for pass in pass_pair {
            pass.step.scissor = Some(clip_scissor(pass.step.scissor, target_scissor));
        }
    }
}

fn apply_scissor_to_steps(steps: &mut [RenderStepDescriptor], scissor: ScissorRect) {
    for step in steps {
        match step {
            RenderStepDescriptor::Draw(step) => {
                step.scissor = Some(clip_scissor(step.scissor, scissor));
            }
            RenderStepDescriptor::DrawIndexed(step) => {
                step.scissor = Some(clip_scissor(step.scissor, scissor));
            }
        }
    }
}

fn clip_scissor(previous: Option<ScissorRect>, scissor: ScissorRect) -> ScissorRect {
    previous.map_or(scissor, |previous| {
        intersect_scissor_rects(previous, scissor)
    })
}

fn blur_source_scissor(
    config: NovaBackdropBlurConfig,
    drawable_size: DrawableSize,
) -> Option<ScissorRect> {
    let [x, y, width, height] = config.bounds();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    // The Gaussian kernel never samples farther than `radius`. One extra source pixel covers
    // bilinear interpolation at the boundary and avoids a hard seam around rounded glass.
    let support = config.radius().max(0.0) + 1.0;
    let left = floor_clamped_u32(x - support, drawable_size.width);
    let top = floor_clamped_u32(y - support, drawable_size.height);
    let right = ceil_clamped_u32(x + width + support, drawable_size.width);
    let bottom = ceil_clamped_u32(y + height + support, drawable_size.height);
    let scissor = ScissorRect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    };
    (!scissor.is_empty()).then_some(scissor)
}

fn downsample_scissor(
    source: ScissorRect,
    downsample: u8,
    drawable_size: DrawableSize,
) -> ScissorRect {
    let factor = u32::from(downsample.max(1));
    let target_width = (drawable_size.width / factor).max(1);
    let target_height = (drawable_size.height / factor).max(1);
    let right = source.x.saturating_add(source.width);
    let bottom = source.y.saturating_add(source.height);
    let x = (source.x / factor).min(target_width);
    let y = (source.y / factor).min(target_height);
    let scaled_right = right.div_ceil(factor).min(target_width);
    let scaled_bottom = bottom.div_ceil(factor).min(target_height);
    ScissorRect {
        x,
        y,
        width: scaled_right.saturating_sub(x),
        height: scaled_bottom.saturating_sub(y),
    }
}

fn union_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let right_edge = left
        .x
        .saturating_add(left.width)
        .max(right.x.saturating_add(right.width));
    let bottom_edge = left
        .y
        .saturating_add(left.height)
        .max(right.y.saturating_add(right.height));
    ScissorRect {
        x,
        y,
        width: right_edge.saturating_sub(x),
        height: bottom_edge.saturating_sub(y),
    }
}

fn intersect_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
    let x = left.x.max(right.x);
    let y = left.y.max(right.y);
    let right_edge = left
        .x
        .saturating_add(left.width)
        .min(right.x.saturating_add(right.width));
    let bottom_edge = left
        .y
        .saturating_add(left.height)
        .min(right.y.saturating_add(right.height));
    ScissorRect {
        x,
        y,
        width: right_edge.saturating_sub(x),
        height: bottom_edge.saturating_sub(y),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite clamped blur bounds are converted to integer scissor coordinates"
)]
fn floor_clamped_u32(value: f32, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= limit as f32 {
        limit
    } else {
        value.floor() as u32
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite clamped blur bounds are converted to integer scissor coordinates"
)]
fn ceil_clamped_u32(value: f32, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= limit as f32 {
        limit
    } else {
        value.ceil() as u32
    }
}
