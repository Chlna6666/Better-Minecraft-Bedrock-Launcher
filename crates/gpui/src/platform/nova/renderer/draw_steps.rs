use super::*;

pub(super) struct PreparedBackdropBlurGroup {
    /// Scene segment that advances the shared backdrop source from the previous blur barrier to this
    /// group's exact source state. The previous blur batch itself is included in the next segment,
    /// so its filtered result is composited once into the accumulated scene color.
    pub(super) source_steps: Vec<RenderStepDescriptor>,
    pub(super) filter_passes: Vec<BackdropBlurRenderPass>,
    /// Partial refreshes preserve pixels outside every filter-pass scissor. Full invalidations
    /// clear the retained targets before recomputing them.
    pub(super) preserve_filtered_pixels: bool,
}

pub(super) struct PreparedElementBlurLayer {
    pub(super) index: u32,
    pub(super) source_texture_view: TextureViewId,
    pub(super) source_groups: Vec<PreparedBackdropBlurGroup>,
    pub(super) filter_passes: Vec<BackdropBlurRenderPass>,
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
            |config| backdrop_blur_targets?.resource_set_for_config(config, frame_resource_index),
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            DrawStepMode::Present,
            steps,
        );
    }

    /// Builds the root backdrop compositor plan while preserving clean filtered targets.
    ///
    /// GPUI owns backdrop caching automatically. A normal partial frame only refreshes blur
    /// configs whose Gaussian sampling footprint intersects this frame's damage. For a dirty
    /// full-window filter, source capture and both separable passes are further clipped to the
    /// convolution footprint affected by the damage, while pixels outside that footprint remain in
    /// the retained ping/final targets. Non-spatial invalidations still rebuild every target.
    pub(super) fn prepare_backdrop_blur_groups(
        &self,
        enabled: bool,
    ) -> Vec<PreparedBackdropBlurGroup> {
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

        let blur_groups: Vec<_> =
            direct_backdrop_barriers(&self.frame_upload, 0, self.frame_upload.batches.len())
                .into_iter()
                .filter_map(|batch_index| {
                    let UploadedBatch::BackdropBlurs { first, count } =
                        self.frame_upload.batches[batch_index]
                    else {
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

        let force_full = self.draw_step_scratch.force_full_backdrop_blur_refresh;
        let damage = &self.draw_step_scratch.backdrop_blur_damage_region;
        let dirty_configs: Vec<Vec<BackdropBlurConfig>> = blur_groups
            .iter()
            .map(|(_, configs)| {
                if force_full {
                    configs.clone()
                } else {
                    configs
                        .iter()
                        .copied()
                        .filter(|config| {
                            blur_damage_scissors(*config, self.current_size, damage).is_some()
                        })
                        .collect()
                }
            })
            .collect();

        let Some(last_dirty_group) = dirty_configs
            .iter()
            .rposition(|configs| !configs.is_empty())
        else {
            // A coarse scene invalidation can still arrive for an animation above or far away from
            // every root filter. Keep all cached Gaussian results and submit no offscreen work.
            return Vec::new();
        };

        // The scene-color source is scratch, not retained. Reconstruct only the dependency halo
        // required by the dirty Gaussian outputs. Every sequential source segment uses the same
        // union so later dirty barriers can safely composite earlier cached filters in draw order.
        let source_scissor = dirty_configs[..=last_dirty_group]
            .iter()
            .flat_map(|configs| configs.iter().copied())
            .filter_map(|config| {
                if force_full {
                    blur_full_source_scissor(config, self.current_size)
                } else {
                    blur_damage_scissors(config, self.current_size, damage)
                        .map(|damage| damage.source_capture)
                }
            })
            .reduce(union_scissor_rects);

        let mut groups = Vec::with_capacity(last_dirty_group.saturating_add(1));
        let mut batch_start = 0usize;
        for (group_index, (batch_end, _configs)) in blur_groups
            .into_iter()
            .enumerate()
            .take(last_dirty_group.saturating_add(1))
        {
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
                DrawStepMode::BackdropSegment {
                    batch_start,
                    batch_end,
                },
                &mut source_steps,
            );
            if let Some(scissor) = source_scissor {
                apply_scissor_to_steps(&mut source_steps, scissor);
            }

            let configs = &dirty_configs[group_index];
            let mut filter_passes = Vec::new();
            if !configs.is_empty() {
                backdrop_blur_render_passes_for_configs_into(
                    &self.pipelines,
                    targets,
                    frame_resource_index,
                    configs,
                    &mut filter_passes,
                );
                if force_full {
                    apply_filter_pass_scissors(configs, self.current_size, &mut filter_passes);
                } else {
                    apply_filter_pass_damage_scissors(
                        configs,
                        self.current_size,
                        damage,
                        &mut filter_passes,
                    );
                }
            }

            groups.push(PreparedBackdropBlurGroup {
                source_steps,
                filter_passes,
                preserve_filtered_pixels: !force_full,
            });
            // Include this group's backdrop batch in the next segment. If this group was clean, the
            // draw samples its retained filtered texture rather than recomputing the Gaussian pass.
            batch_start = batch_end;
        }
        groups
    }

    pub(super) fn prepare_backdrop_blur_passes(&mut self, enabled: bool) {
        let passes = &mut self.draw_step_scratch.backdrop_blur_passes;
        passes.clear();
        if !enabled {
            return;
        }
        let Some(targets) = self.backdrop_blur_targets.as_ref() else {
            return;
        };
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

    pub(super) fn prepare_element_blur_layers(
        &self,
        enabled: bool,
    ) -> Vec<PreparedElementBlurLayer> {
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
        let mut layers = Vec::new();

        for range in self.frame_upload.blur_content_ranges() {
            let Some(config) = self
                .frame_upload
                .backdrop_blur_config_for_index(range.index)
            else {
                continue;
            };
            let Some(source_texture_view) = targets.isolated_source_texture_view(range.index)
            else {
                continue;
            };
            let Some(source_resource_set) =
                targets.isolated_source_resource_set(range.index, frame_resource_index)
            else {
                continue;
            };

            let mut source_groups = Vec::new();
            let mut segment_start = range.content_start;
            for batch_index in
                direct_backdrop_barriers(&self.frame_upload, range.content_start, range.content_end)
            {
                let UploadedBatch::BackdropBlurs { first, count } =
                    self.frame_upload.batches[batch_index]
                else {
                    continue;
                };
                let configs = self
                    .frame_upload
                    .backdrop_blur_configs_for_range(first, count);
                if configs.is_empty() {
                    continue;
                }
                source_groups.push(self.prepare_element_blur_group(
                    segment_start,
                    batch_index,
                    &configs,
                    source_resource_set,
                    targets,
                ));
                segment_start = batch_index;
            }

            let mut final_source_steps = Vec::new();
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
                |blur_config| targets.resource_set_for_config(blur_config, frame_resource_index),
                self.custom_mesh_3d_resource_set,
                self.custom_mesh_3d_indices_buffer,
                DrawStepMode::BlurContent {
                    batch_start: segment_start,
                    batch_end: range.content_end,
                },
                &mut final_source_steps,
            );
            if let Some(scissor) = blur_full_source_scissor(config, self.current_size) {
                apply_scissor_to_steps(&mut final_source_steps, scissor);
            }

            let mut filter_passes = Vec::new();
            backdrop_blur_render_passes_for_configs_with_source_into(
                &self.pipelines,
                targets,
                frame_resource_index,
                std::slice::from_ref(&config),
                source_resource_set,
                &mut filter_passes,
            );
            apply_filter_pass_scissors(
                std::slice::from_ref(&config),
                self.current_size,
                &mut filter_passes,
            );
            source_groups.push(PreparedBackdropBlurGroup {
                source_steps: final_source_steps,
                filter_passes: Vec::new(),
                preserve_filtered_pixels: false,
            });
            layers.push(PreparedElementBlurLayer {
                index: range.index,
                source_texture_view,
                source_groups,
                filter_passes,
            });
        }
        layers
    }

    fn prepare_element_blur_group(
        &self,
        batch_start: usize,
        batch_end: usize,
        configs: &[BackdropBlurConfig],
        source_resource_set: ResourceSetId,
        targets: &BackdropBlurTargets,
    ) -> PreparedBackdropBlurGroup {
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let custom_mesh_3d_pipelines = &self.custom_mesh_3d_pipelines;
        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;
        let mut source_steps = Vec::new();
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
            |blur_config| targets.resource_set_for_config(blur_config, frame_resource_index),
            self.custom_mesh_3d_resource_set,
            self.custom_mesh_3d_indices_buffer,
            DrawStepMode::BlurContent {
                batch_start,
                batch_end,
            },
            &mut source_steps,
        );

        let mut filter_passes = Vec::new();
        backdrop_blur_render_passes_for_configs_with_source_into(
            &self.pipelines,
            targets,
            frame_resource_index,
            configs,
            source_resource_set,
            &mut filter_passes,
        );
        if let Some(scissor) = configs
            .iter()
            .copied()
            .filter_map(|config| blur_full_source_scissor(config, self.current_size))
            .reduce(union_scissor_rects)
        {
            apply_scissor_to_steps(&mut source_steps, scissor);
        }
        apply_filter_pass_scissors(configs, self.current_size, &mut filter_passes);
        PreparedBackdropBlurGroup {
            source_steps,
            filter_passes,
            preserve_filtered_pixels: false,
        }
    }

    pub(super) fn has_backdrop_blurs(&self) -> bool {
        !self.frame_upload.backdrop_blurs.is_empty()
    }

    fn current_blend_pipelines(&self) -> BlendPipelines {
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
    custom_mesh_3d_mesh_cache: &FxHashMap<GpuMesh3dId, MeshCacheEntry>,
    mesh_id: GpuMesh3dId,
    generation: u64,
) -> Option<MeshCacheEntry> {
    custom_mesh_3d_mesh_cache
        .get(&mesh_id)
        .copied()
        .filter(|entry| entry.generation == generation)
}

fn apply_filter_pass_scissors(
    configs: &[BackdropBlurConfig],
    drawable_size: DrawableSize,
    passes: &mut [BackdropBlurRenderPass],
) {
    for (config, pass_pair) in configs.iter().zip(passes.chunks_mut(2)) {
        let [horizontal, vertical] = pass_pair else {
            continue;
        };
        let Some(source_scissor) = blur_full_source_scissor(*config, drawable_size) else {
            continue;
        };
        let horizontal_scissor =
            downsample_x_scissor(source_scissor, config.downsample(), drawable_size);
        let final_scissor = downsample_scissor(source_scissor, config.downsample(), drawable_size);
        horizontal.step.scissor = Some(clip_scissor(horizontal.step.scissor, horizontal_scissor));
        vertical.step.scissor = Some(clip_scissor(vertical.step.scissor, final_scissor));
    }
}

fn apply_filter_pass_damage_scissors(
    configs: &[BackdropBlurConfig],
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
    passes: &mut [BackdropBlurRenderPass],
) {
    for (config, pass_pair) in configs.iter().zip(passes.chunks_mut(2)) {
        let [horizontal, vertical] = pass_pair else {
            continue;
        };
        let Some(damage) = blur_damage_scissors(*config, drawable_size, dirty_region) else {
            continue;
        };
        let horizontal_scissor =
            downsample_x_scissor(damage.horizontal_output, config.downsample(), drawable_size);
        let final_scissor =
            downsample_scissor(damage.final_output, config.downsample(), drawable_size);
        horizontal.step.scissor = Some(clip_scissor(horizontal.step.scissor, horizontal_scissor));
        vertical.step.scissor = Some(clip_scissor(vertical.step.scissor, final_scissor));
    }
}

fn direct_backdrop_barriers(upload: &FrameUpload, start: usize, end: usize) -> Vec<usize> {
    let mut barriers = Vec::new();
    let mut depth = 0usize;
    let end = end.min(upload.batches.len());
    for batch_index in start.min(end)..end {
        match upload.batches[batch_index] {
            UploadedBatch::BeginBlur { .. } => {
                depth = depth.saturating_add(1);
            }
            UploadedBatch::EndBlur { .. } => {
                depth = depth.saturating_sub(1);
            }
            UploadedBatch::BackdropBlurs { .. } if depth == 0 => {
                barriers.push(batch_index);
            }
            UploadedBatch::SolidQuads { .. }
            | UploadedBatch::Quads { .. }
            | UploadedBatch::Shadows { .. }
            | UploadedBatch::PathRasterization { .. }
            | UploadedBatch::Paths { .. }
            | UploadedBatch::MonoSprites { .. }
            | UploadedBatch::PolySprites { .. }
            | UploadedBatch::Underlines { .. }
            | UploadedBatch::BackdropBlurs { .. }
            | UploadedBatch::CompositeBlur { .. }
            | UploadedBatch::CustomMesh3d { .. } => {}
        }
    }
    barriers
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

fn scissor_intersects_dirty_region(scissor: ScissorRect, dirty_region: &DirtyRegion) -> bool {
    if dirty_region.is_full() {
        return true;
    }
    if dirty_region.is_empty() || scissor.is_empty() {
        return false;
    }
    let source_bounds = crate::Bounds::new(
        crate::Point {
            x: crate::ScaledPixels(scissor.x as f32),
            y: crate::ScaledPixels(scissor.y as f32),
        },
        crate::Size {
            width: crate::ScaledPixels(scissor.width as f32),
            height: crate::ScaledPixels(scissor.height as f32),
        },
    );
    dirty_region
        .rects()
        .iter()
        .any(|rect| rect.bounds.intersects(&source_bounds))
}

fn downsample_x_scissor(
    source: ScissorRect,
    downsample: u8,
    drawable_size: DrawableSize,
) -> ScissorRect {
    let factor = u32::from(downsample.max(1));
    let target_width = drawable_size.width.div_ceil(factor).max(1);
    let right = source.x.saturating_add(source.width);
    let x = (source.x / factor).min(target_width);
    let scaled_right = right.div_ceil(factor).min(target_width);
    ScissorRect {
        x,
        y: source.y.min(drawable_size.height),
        width: scaled_right.saturating_sub(x),
        height: source.height.min(
            drawable_size
                .height
                .saturating_sub(source.y.min(drawable_size.height)),
        ),
    }
}

fn downsample_scissor(
    source: ScissorRect,
    downsample: u8,
    drawable_size: DrawableSize,
) -> ScissorRect {
    let factor = u32::from(downsample.max(1));
    let target_width = drawable_size.width.div_ceil(factor).max(1);
    let target_height = drawable_size.height.div_ceil(factor).max(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dirty_region(x: f32, y: f32, width: f32, height: f32) -> DirtyRegion {
        let mut region = DirtyRegion::empty();
        region.push(crate::Bounds::new(
            crate::Point {
                x: crate::ScaledPixels(x),
                y: crate::ScaledPixels(y),
            },
            crate::Size {
                width: crate::ScaledPixels(width),
                height: crate::ScaledPixels(height),
            },
        ));
        region
    }

    #[test]
    fn backdrop_damage_rejects_disjoint_sampling_region() {
        let damage = dirty_region(20.0, 20.0, 30.0, 30.0);
        assert!(!scissor_intersects_dirty_region(
            ScissorRect {
                x: 400,
                y: 300,
                width: 120,
                height: 80,
            },
            &damage,
        ));
    }

    #[test]
    fn backdrop_damage_accepts_sampling_overlap() {
        let damage = dirty_region(430.0, 330.0, 20.0, 20.0);
        assert!(scissor_intersects_dirty_region(
            ScissorRect {
                x: 400,
                y: 300,
                width: 120,
                height: 80,
            },
            &damage,
        ));
    }
}
