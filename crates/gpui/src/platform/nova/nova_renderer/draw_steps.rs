use super::*;

pub(super) struct NovaPreparedBackdropBlurGroup {
    /// Incremental scene segment that advances the shared backdrop source from the previous blur
    /// barrier to this group's exact draw-order prefix. The first group starts from a clear target;
    /// later groups load the existing source and append only the delta.
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

    /// Builds one sequential source segment for every backdrop draw-order barrier.
    ///
    /// `BackdropSourceThrough` still gives us the exact semantic prefix for a group, but that
    /// prefix is used only as CPU-side planning input. We subtract the previous prefix and submit
    /// only the newly exposed segment to the shared source texture. Earlier backdrop groups are
    /// therefore composited once and become part of the source for later groups instead of causing
    /// the renderer to replay the whole scene prefix for every blur.
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

        let blur_groups: Vec<_> = self
            .frame_upload
            .batches
            .iter()
            .enumerate()
            .filter_map(|(batch_index, batch)| {
                let NovaUploadedBatch::BackdropBlurs { first, count } = *batch else {
                    return None;
                };
                let configs = self
                    .frame_upload
                    .backdrop_blur_configs_for_range(first, count);
                (!configs.is_empty()).then_some((batch_index, configs))
            })
            .collect();
        if blur_groups.is_empty() {
            return Vec::new();
        }

        // Every incremental segment uses the same source footprint. This is essential because a
        // later blur may sample pixels that an earlier blur did not need; rendering each segment
        // with a different local scissor would leave holes in the accumulated source texture.
        let source_scissor = blur_groups
            .iter()
            .flat_map(|(_, configs)| configs.iter().copied())
            .filter_map(|config| blur_source_scissor(config, self.current_size))
            .reduce(union_scissor_rects);

        let mut groups = Vec::with_capacity(blur_groups.len());
        let mut previous_prefix = Vec::<RenderStepDescriptor>::new();

        for (batch_index, configs) in blur_groups {
            let mut exact_prefix = Vec::new();
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
                &mut exact_prefix,
            );

            if let Some(scissor) = source_scissor {
                apply_scissor_to_steps(&mut exact_prefix, scissor);
            }
            let source_steps = incremental_source_steps(&previous_prefix, &exact_prefix);
            previous_prefix = exact_prefix;

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
        // GPU targets have stable identities and deliberately keep their allocation across animated
        // bounds changes. Always derive pass geometry/scissors from the current frame instead of
        // reading the bounds stored when the target texture was originally allocated.
        let configs = self.frame_upload.backdrop_blur_configs();
        backdrop_blur_render_passes_for_configs_into(
            &self.pipelines,
            targets,
            self.current_frame_resource_index,
            &configs,
            passes,
        );
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

/// Returns the GPU work needed to advance `previous_prefix` to `current_prefix`.
///
/// Draw packing may merge two adjacent batches into one instanced draw. In that case the current
/// prefix is not a literal vector prefix of the previous one, so the final merged draw is split at
/// the old instance boundary. Any unexpected structural mismatch falls back to the exact current
/// prefix to preserve rendering correctness.
fn incremental_source_steps(
    previous_prefix: &[RenderStepDescriptor],
    current_prefix: &[RenderStepDescriptor],
) -> Vec<RenderStepDescriptor> {
    if previous_prefix.is_empty() {
        return current_prefix.to_vec();
    }
    if is_only_noop_draw(previous_prefix) {
        return current_prefix.to_vec();
    }

    let common = previous_prefix
        .iter()
        .zip(current_prefix)
        .take_while(|(previous, current)| previous == current)
        .count();
    if common == previous_prefix.len() {
        return current_prefix[common..].to_vec();
    }

    if common + 1 == previous_prefix.len()
        && let (
            Some(RenderStepDescriptor::Draw(previous)),
            Some(RenderStepDescriptor::Draw(current)),
        ) = (previous_prefix.get(common), current_prefix.get(common))
        && draw_step_extends(previous, current)
    {
        let mut result = Vec::with_capacity(current_prefix.len().saturating_sub(common));
        let added_instances = current.instance_count.saturating_sub(previous.instance_count);
        if added_instances != 0
            && let Some(first_instance) = previous
                .first_instance
                .checked_add(previous.instance_count)
        {
            let mut delta = current.clone();
            delta.first_instance = first_instance;
            delta.instance_count = added_instances;
            result.push(RenderStepDescriptor::Draw(delta));
        }
        result.extend_from_slice(&current_prefix[common + 1..]);
        return result;
    }

    log::debug!(
        "nova backdrop incremental source prefix mismatch: previous_steps={} current_steps={}; falling back to exact prefix",
        previous_prefix.len(),
        current_prefix.len()
    );
    current_prefix.to_vec()
}

fn is_only_noop_draw(steps: &[RenderStepDescriptor]) -> bool {
    matches!(
        steps,
        [RenderStepDescriptor::Draw(step)] if step.vertex_count == 0 || step.instance_count == 0
    )
}

fn draw_step_extends(previous: &DrawStepDescriptor, current: &DrawStepDescriptor) -> bool {
    previous.pipeline == current.pipeline
        && previous.resource_sets == current.resource_sets
        && previous.vertex_count == current.vertex_count
        && previous.first_vertex == current.first_vertex
        && previous.first_instance == current.first_instance
        && previous.scissor == current.scissor
        && current.instance_count >= previous.instance_count
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
