use super::super::blur_damage::{bounds_to_scissor, intersect_scissor_rects};
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
    /// Gaussian element filters retain ping/final targets across partial refreshes. Zero-filter
    /// compositor layers render source pixels directly into the final retained target and submit
    /// no filter passes at all.
    pub(super) preserve_filtered_pixels: bool,
    /// First source pass loads previously completed root color instead of clearing it.
    pub(super) preserve_retained_source: bool,
}

impl NovaRenderer {
    pub(super) fn prepare_draw_steps(&mut self, scene_revision: u64) {
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let cache_key = DrawStepCacheKey {
            scene_revision,
            dynamic_frame_id: (!self.frame_upload.renderer_extensions.is_empty())
                .then_some(self.frame_upload.renderer_extension_frame_id),
            size: self.current_size,
            frame_resource_index,
            atlas_texture_generation: self.synced_atlas_texture_generation,
            atlas_texture_count: self.gpu_atlas_textures.len(),
            premultiplied_alpha: self.surface_alpha.outputs_premultiplied_alpha(),
        };
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let backdrop_blur_targets = self.filters.targets.as_ref();
        self.draw_step_scratch
            .prepare_steps(cache_key, self.frame_resources.len(), |steps| {
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
                    self.underline_resource_set,
                    |config| {
                        backdrop_blur_targets?.resource_set_for_config(config, frame_resource_index)
                    },
                    DrawStepMode::Present,
                    steps,
                );
            });
    }

    pub(super) fn prepare_renderer_extensions(&mut self, frame_time: Instant) -> Result<()> {
        let extension_count = self.frame_upload.renderer_extensions.len();
        self.frame_upload
            .renderer_extension_steps
            .resize_with(extension_count, Vec::new);
        self.frame_upload
            .renderer_extension_steps
            .truncate(extension_count);
        let viewport = Extent2d::new(self.current_size.width, self.current_size.height)?;
        let active_types = self.active_renderer_extension_types();
        self.release_inactive_renderer_extensions(&active_types);
        for index in 0..extension_count {
            self.prepare_renderer_extension(index, viewport, frame_time)?;
        }
        Ok(())
    }

    fn active_renderer_extension_types(&self) -> SmallVec<[std::any::TypeId; 4]> {
        let mut active_types = SmallVec::new();
        for extension in &self.frame_upload.renderer_extensions {
            let type_id = extension.extension.renderer_type();
            if !active_types.contains(&type_id) {
                active_types.push(type_id);
            }
        }
        active_types
    }

    fn release_inactive_renderer_extensions(&mut self, active_types: &[std::any::TypeId]) {
        if !self.renderer_registry.has_inactive(active_types) {
            return;
        }
        let registry = &mut self.renderer_registry;
        if let Err(error) = lock_backend(&self.backend).with_extension_device(|device| {
            registry.retain(active_types, device);
            Ok(())
        }) {
            log::debug!("failed to access nova-gfx device to release extensions: {error}");
        }
    }

    fn prepare_renderer_extension(
        &mut self,
        index: usize,
        viewport: Extent2d,
        frame_time: Instant,
    ) -> Result<()> {
        let extension = self.frame_upload.renderer_extensions[index].clone();
        let Some(bounds_scissor) = bounds_to_scissor(extension.bounds, self.current_size) else {
            return Ok(());
        };
        let Some(mask_scissor) =
            bounds_to_scissor(extension.content_mask.bounds, self.current_size)
        else {
            return Ok(());
        };
        let scissor = intersect_scissor_rects(bounds_scissor, mask_scissor);
        if scissor.is_empty() {
            return Ok(());
        }

        let context = crate::RendererExtensionContext::new(
            self.backend_info.kind()?,
            self.render_pass,
            self.surface_config.format,
            viewport,
            extension.bounds,
            extension.content_mask,
            scissor,
            frame_time,
        );
        let mut backend = lock_backend(&self.backend);
        let (registry, steps) = (
            &mut self.renderer_registry,
            &mut self.frame_upload.renderer_extension_steps[index],
        );
        steps.clear();
        backend.with_extension_device(|device| {
            registry.prepare(extension.extension.as_ref(), device, context, steps)
        })?;
        apply_scissor_to_steps(steps, scissor);
        Ok(())
    }

    pub(super) fn destroy_renderer_extensions(&mut self) {
        if self.renderer_registry.is_empty() {
            return;
        }
        let registry = &mut self.renderer_registry;
        let result = lock_backend(&self.backend).with_extension_device(|device| {
            registry.destroy(device);
            Ok(())
        });
        if let Err(error) = result {
            log::debug!("failed to access nova-gfx device to destroy extensions: {error}");
            self.renderer_registry = extensions::RendererRegistry::default();
        }
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
        let Some(targets) = self.filters.targets.as_ref() else {
            return Vec::new();
        };
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;

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
        let group_damage: Vec<_> = blur_groups
            .iter()
            .map(|(_, configs)| {
                backdrop_damage_for_configs(
                    &self.draw_step_scratch.backdrop_blur_damage_plan,
                    configs,
                )
            })
            .collect();
        let dirty_configs: Vec<Vec<BackdropBlurConfig>> = blur_groups
            .iter()
            .zip(&group_damage)
            .map(|((_, configs), (group_full_refresh, damage))| {
                blur_configs_for_refresh(
                    configs,
                    self.current_size,
                    damage,
                    force_full || *group_full_refresh,
                )
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
            .zip(&group_damage[..=last_dirty_group])
            .flat_map(|(configs, damage)| {
                configs.iter().copied().map(move |config| (config, damage))
            })
            .filter_map(|(config, (group_full_refresh, damage))| {
                blur_source_scissor_for_refresh(
                    config,
                    self.current_size,
                    damage,
                    force_full || *group_full_refresh,
                )
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
            draw_steps_for_upload_into_clipped(
                &self.frame_upload,
                &self.pipelines,
                blend_pipelines,
                self.quad_resource_set,
                self.shadow_resource_set,
                self.path_resource_set,
                |texture_id| {
                    sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index)
                },
                self.underline_resource_set,
                |config| targets.resource_set_for_config(config, frame_resource_index),
                DrawStepMode::BackdropSegment {
                    batch_start,
                    batch_end,
                },
                source_scissor,
                &mut source_steps,
            );
            if let Some(scissor) = source_scissor {
                apply_scissor_to_steps(&mut source_steps, scissor);
            }

            let configs = &dirty_configs[group_index];
            let (group_full_refresh, damage) = &group_damage[group_index];
            let group_force_full = force_full || *group_full_refresh;
            let mut filter_passes = Vec::new();
            if !configs.is_empty() {
                backdrop_blur_render_passes_for_configs_into(
                    &self.pipelines,
                    targets,
                    frame_resource_index,
                    configs,
                    &mut filter_passes,
                );
                apply_filter_refresh_scissors(
                    configs,
                    self.current_size,
                    damage,
                    group_force_full,
                    &mut filter_passes,
                );
            }

            groups.push(PreparedBackdropBlurGroup {
                source_steps,
                filter_passes,
                preserve_filtered_pixels: !group_force_full,
            });
            // Include this group's backdrop batch in the next segment. If this group was clean, the
            // draw samples its retained filtered texture rather than recomputing the Gaussian pass.
            batch_start = batch_end;
        }
        // Cached filter outputs leave gaps with no GPU filtering work.
        // Adjacent source segments always target the same color attachment
        // and can be submitted as one render pass when there is no filter
        // pass between them. Preserve painter order and depth contents.
        coalesce_source_groups(&mut groups);
        groups
    }

    pub(super) fn prepare_backdrop_blur_passes(&mut self, enabled: bool) {
        let passes = &mut self.draw_step_scratch.backdrop_blur_passes;
        passes.clear();
        if !enabled {
            return;
        }
        let Some(targets) = self.filters.targets.as_ref() else {
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

    /// Builds isolated element-filter work. Gaussian layers reconstruct their source scratch and
    /// refresh retained ping/final targets. A zero-radius compositor layer instead renders the
    /// complete dirty subtree directly into its retained final target, so it has no H/V passes and
    /// no texture-copy stage. Composite-only animation is skipped above this path entirely.
    pub(super) fn prepare_element_blur_layers(
        &self,
        dirty_indices: &[u32],
    ) -> Vec<PreparedElementBlurLayer> {
        if dirty_indices.is_empty() {
            return Vec::new();
        }
        let Some(targets) = self.filters.targets.as_ref() else {
            return Vec::new();
        };
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let force_full = self.draw_step_scratch.force_full_backdrop_blur_refresh;
        let damage = &self.draw_step_scratch.backdrop_blur_damage_region;
        let composite_only = if force_full {
            FxHashSet::default()
        } else {
            self.frame_upload.composite_only_element_blur_indices()
        };
        let mut layers = Vec::new();

        for range in self.frame_upload.blur_content_ranges() {
            // The input-dependency check already selected exactly which filter
            // outputs need rebuilding. Skip clean layers BEFORE creating their
            // source draw steps, resource-set bindings and Gaussian pass lists.
            // This matters with nested filters and high-Hz animation frames.
            if !dirty_indices.contains(&range.index) {
                continue;
            }
            // A retained child scene whose only changing state is the promoted final composite has
            // identical source pixels and Gaussian output. Keep the cached target and submit no
            // offscreen work. Non-spatial cache/atlas/quality invalidation sets `force_full`, which
            // deliberately disables this fast path.
            if composite_only.contains(&range.index) {
                continue;
            }
            let Some(config) = self
                .frame_upload
                .backdrop_blur_config_for_index(range.index)
            else {
                continue;
            };

            let direct_composite_target =
                targets.direct_composite_target(config, frame_resource_index);
            let direct_composite = direct_composite_target.is_some();
            // Ordinary element filters keep their established clear/full-layer path.
            // The synthetic ROOT compositor is special: keep its previous color and
            // reconstruct only the dirty source rectangle from the full painter list.
            let is_retained_root = self.frame_upload.retained_root_blur == Some(range.index);
            // Chromium-style paint invalidation: keep spatially disconnected
            // damage disconnected all the way to the retained GPU color target.
            // Combining a left-hand label and a right-hand cursor into one
            // bounding scissor can turn two tiny changes into a full-window
            // redraw. Multiple rects need separate Load/clear/repaint passes.
            //
            // A normal element filter or a root with nested painter barriers
            // still follows the established one-pass path; its captured source
            // is not a single independently preserved scene-color layer.
            let can_split_root = is_retained_root
                && !force_full
                && !damage.is_full()
                // A changed child filter can expand its visible output beyond
                // the original element damage; keep the union/halo path then.
                && dirty_indices.iter().all(|index| *index == range.index)
                // Painter-ordered root backdrop filters may change their
                // source between segment barriers. Do not split across them.
                && direct_backdrop_barriers(
                    &self.frame_upload,
                    range.content_start,
                    range.content_end,
                )
                .is_empty();
            // Unchanged nested element filters are safe: their retained GPU
            // outputs are sampled by the root just like cached images, with
            // no Gaussian pass or source recapture for either dirty patch.
            let damage_patches = retained_source_damage_patches(damage, can_split_root);
            for damage in &damage_patches {
            let preserve_root = is_retained_root && !force_full && !damage.is_full();
            let outer_source_scissor = if direct_composite && !is_retained_root {
                blur_full_source_scissor(config, self.current_size)
            } else {
                blur_source_scissor_for_refresh(config, self.current_size, damage, force_full)
            };
            let Some(outer_source_scissor) = outer_source_scissor else {
                // The caller can conservatively select a layer whose effect bounds intersect a
                // coarse dirty region. If the actual dependency footprint itself is clean, the
                // retained result is already valid and no offscreen work is needed.
                continue;
            };

            let (source_texture_view, source_resource_set) = if let Some(ref target) =
                direct_composite_target
            {
                (target.texture_view, target.source_resource_set)
            } else {
                let Some(source_texture_view) = targets.isolated_source_texture_view(range.index)
                else {
                    continue;
                };
                let Some(source_resource_set) =
                    targets.isolated_source_resource_set(range.index, frame_resource_index)
                else {
                    continue;
                };
                (source_texture_view, source_resource_set)
            };

            let barrier_groups: Vec<_> = direct_backdrop_barriers(
                &self.frame_upload,
                range.content_start,
                range.content_end,
            )
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
            let dirty_barrier_configs: Vec<Vec<BackdropBlurConfig>> = barrier_groups
                .iter()
                .map(|(_, configs)| {
                    blur_configs_for_refresh(configs, self.current_size, damage, force_full)
                })
                .collect();

            // Every segment contributes to one accumulated scene-color source. For a normal
            // element filter this is isolated scratch storage; for a zero-filter compositor it is
            // the retained final target itself. Zero-filter refreshes use the full tight layer
            // scissor, while nested dirty filters can expand the source halo further if required.
            let source_scissor = dirty_barrier_configs
                .iter()
                .flat_map(|configs| configs.iter().copied())
                .filter_map(|nested_config| {
                    blur_source_scissor_for_refresh(
                        nested_config,
                        self.current_size,
                        damage,
                        force_full,
                    )
                })
                .fold(outer_source_scissor, union_scissor_rects);

            let mut source_groups = Vec::with_capacity(barrier_groups.len().saturating_add(1));
            let mut segment_start = range.content_start;
            for (group_index, (batch_index, _configs)) in barrier_groups.into_iter().enumerate() {
                source_groups.push(self.prepare_element_blur_group(
                    segment_start,
                    batch_index,
                    &dirty_barrier_configs[group_index],
                    source_resource_set,
                    targets,
                    source_scissor,
                    damage,
                    force_full,
                ));
                segment_start = batch_index;
            }

            let mut final_source_steps = Vec::new();
            draw_steps_for_upload_into_clipped(
                &self.frame_upload,
                &self.pipelines,
                blend_pipelines,
                self.quad_resource_set,
                self.shadow_resource_set,
                self.path_resource_set,
                |texture_id| {
                    sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index)
                },
                self.underline_resource_set,
                |blur_config| targets.resource_set_for_config(blur_config, frame_resource_index),
                DrawStepMode::BlurContent {
                    batch_start: segment_start,
                    batch_end: range.content_end,
                },
                Some(source_scissor),
                &mut final_source_steps,
            );
            apply_scissor_to_steps(&mut final_source_steps, source_scissor);

            let mut filter_passes = Vec::new();
            if !direct_composite {
                backdrop_blur_render_passes_for_configs_with_source_into(
                    &self.pipelines,
                    targets,
                    frame_resource_index,
                    std::slice::from_ref(&config),
                    source_resource_set,
                    &mut filter_passes,
                );
                apply_filter_refresh_scissors(
                    std::slice::from_ref(&config),
                    self.current_size,
                    damage,
                    force_full,
                    &mut filter_passes,
                );
            }
            source_groups.push(PreparedBackdropBlurGroup {
                source_steps: final_source_steps,
                filter_passes: Vec::new(),
                preserve_filtered_pixels: !force_full,
            });
            if preserve_root {
                if let Some(clear_index) = self.frame_upload.retained_root_clear_quad {
                    // Replace (not alpha blend) with transparent black before
                    // replaying ALL intersecting primitives in painter order.
                    // This prevents transparent shadows from accumulating.
                    source_groups[0].source_steps.insert(
                        0,
                        RenderStepDescriptor::Draw(DrawStepDescriptor {
                            pipeline: self.pipelines.retained_clear,
                            resource_sets: resource_set_list([self.quad_resource_set]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: 1,
                            first_instance: clear_index,
                            scissor: Some(source_scissor),
                        }),
                    );
                }
            }
            // Multiple nested backdrop barriers may all sample already
            // retained Gaussian results. Splitting each barrier into a
            // separate Load/Store render pass is unnecessary (and particularly
            // expensive on Vulkan). Merge contiguous source-only segments
            // without crossing a real filter execution barrier.
            coalesce_source_groups(&mut source_groups);
            layers.push(PreparedElementBlurLayer {
                index: range.index,
                source_texture_view,
                source_groups,
                filter_passes,
                preserve_filtered_pixels: !force_full,
                preserve_retained_source: preserve_root,
            });
            } // independent GPU dirty rectangles for this retained color layer
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
        source_scissor: ScissorRect,
        dirty_region: &DirtyRegion,
        force_full: bool,
    ) -> PreparedBackdropBlurGroup {
        let blend_pipelines = self.current_blend_pipelines();
        let frame_resource_index = self.current_frame_resource_index;
        let gpu_atlas_textures = &self.gpu_atlas_textures;
        let mut source_steps = Vec::new();
        draw_steps_for_upload_into_clipped(
            &self.frame_upload,
            &self.pipelines,
            blend_pipelines,
            self.quad_resource_set,
            self.shadow_resource_set,
            self.path_resource_set,
            |texture_id| sprite_resource_set(gpu_atlas_textures, texture_id, frame_resource_index),
            self.underline_resource_set,
            |blur_config| targets.resource_set_for_config(blur_config, frame_resource_index),
            DrawStepMode::BlurContent {
                batch_start,
                batch_end,
            },
            Some(source_scissor),
            &mut source_steps,
        );
        apply_scissor_to_steps(&mut source_steps, source_scissor);

        let mut filter_passes = Vec::new();
        if !configs.is_empty() {
            backdrop_blur_render_passes_for_configs_with_source_into(
                &self.pipelines,
                targets,
                frame_resource_index,
                configs,
                source_resource_set,
                &mut filter_passes,
            );
            apply_filter_refresh_scissors(
                configs,
                self.current_size,
                dirty_region,
                force_full,
                &mut filter_passes,
            );
        }
        PreparedBackdropBlurGroup {
            source_steps,
            filter_passes,
            preserve_filtered_pixels: !force_full,
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

    pub(super) fn prepare_path_mask_draw_steps(&mut self, scene_revision: u64) {
        let cache_key = PathMaskCacheKey {
            scene_revision,
            path_rasterization_resource_set: self.path_rasterization_resource_set,
        };
        self.draw_step_scratch.prepare_path_steps(
            cache_key,
            self.current_frame_resource_index,
            self.frame_resources.len(),
            |steps| {
                path_mask_draw_steps_for_upload_into(
                    &self.frame_upload,
                    &self.pipelines,
                    self.path_rasterization_resource_set,
                    steps,
                );
            },
        );
    }

    pub(super) fn invalidate_draw_step_cache(&mut self) {
        self.draw_step_scratch.invalidate_draw_steps();
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

/// Fuse source-only segments between real filter executions. The source
/// texture, render pass and depth attachment are identical for all groups of
/// one layer. No Gaussian output is read or written at an empty boundary, so
/// preserving draw order is sufficient. The final group's filter passes stay
/// AFTER all accumulated source steps.
fn coalesce_source_groups(groups: &mut Vec<PreparedBackdropBlurGroup>) {
    if groups.len() < 2 {
        return;
    }
    let original = std::mem::take(groups);
    groups.reserve(original.len());
    for mut next in original {
        if let Some(last) = groups.last_mut()
            && last.filter_passes.is_empty()
        {
            last.source_steps.append(&mut next.source_steps);
            last.filter_passes.append(&mut next.filter_passes);
            last.preserve_filtered_pixels = next.preserve_filtered_pixels;
        } else {
            groups.push(next);
        }
    }
}

fn backdrop_damage_for_configs(
    plan: &crate::BackdropBlurDamagePlan,
    configs: &[BackdropBlurConfig],
) -> (bool, DirtyRegion) {
    let Some(first_order) = configs
        .iter()
        .map(|config| *config.order_range().start())
        .min()
    else {
        return (false, DirtyRegion::empty());
    };
    let last_order = configs
        .iter()
        .map(|config| *config.order_range().end())
        .max()
        .unwrap_or(first_order);
    let (full_refresh, damage) = plan.source_damage_for_orders(first_order, last_order);
    let mut region = DirtyRegion::empty();
    for bounds in damage {
        region.push(bounds);
    }
    (full_refresh, region)
}

/// Preserve exact disjoint dirty rectangles instead of submitting the
/// bounding box as one large GPU raster region. Cap the number of passes:
/// command overhead can outweigh saved fill when a layer is fragmented.
fn retained_source_damage_patches(
    damage: &DirtyRegion,
    can_split: bool,
) -> Vec<DirtyRegion> {
    if !can_split || damage.rect_count() < 2 || damage.rect_count() > 8 {
        return vec![damage.clone()];
    }
    damage
        .rects()
        .iter()
        .map(|rect| {
            let mut patch = DirtyRegion::empty();
            patch.push(rect.bounds);
            patch
        })
        .collect()
}

fn blur_configs_for_refresh(
    configs: &[BackdropBlurConfig],
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
    force_full: bool,
) -> Vec<BackdropBlurConfig> {
    if force_full {
        return configs.to_vec();
    }
    configs
        .iter()
        .copied()
        .filter(|config| blur_damage_scissors(*config, drawable_size, dirty_region).is_some())
        .collect()
}

fn blur_source_scissor_for_refresh(
    config: BackdropBlurConfig,
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
    force_full: bool,
) -> Option<ScissorRect> {
    if force_full {
        blur_full_source_scissor(config, drawable_size)
    } else {
        blur_damage_scissors(config, drawable_size, dirty_region)
            .map(|damage| damage.source_capture)
    }
}

fn apply_filter_refresh_scissors(
    configs: &[BackdropBlurConfig],
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
    force_full: bool,
    passes: &mut [BackdropBlurRenderPass],
) {
    if force_full {
        apply_filter_pass_scissors(configs, drawable_size, passes);
    } else {
        apply_filter_pass_damage_scissors(configs, drawable_size, dirty_region, passes);
    }
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
            | UploadedBatch::RendererExtensions { .. } => {}
        }
    }
    barriers
}

fn apply_scissor_to_steps(steps: &mut Vec<RenderStepDescriptor>, scissor: ScissorRect) {
    for step in steps.iter_mut() {
        match step {
            RenderStepDescriptor::Draw(step) => {
                step.scissor = Some(clip_scissor(step.scissor, scissor));
            }
            RenderStepDescriptor::DrawIndexed(step) => {
                step.scissor = Some(clip_scissor(step.scissor, scissor));
            }
        }
    }
    // Vulkan and D3D may still incur descriptor/pipeline bookkeeping for
    // a submitted zero-area draw even though the rasterizer produces no
    // fragments. A retained layer with tiny damage should not replay those
    // entirely clipped commands at 240 Hz.
    steps.retain(|step| match step {
        RenderStepDescriptor::Draw(step) => {
            step.scissor.is_none_or(|scissor| !scissor.is_empty())
        }
        RenderStepDescriptor::DrawIndexed(step) => {
            step.scissor.is_none_or(|scissor| !scissor.is_empty())
        }
    });
}

fn clip_scissor(previous: Option<ScissorRect>, scissor: ScissorRect) -> ScissorRect {
    previous.map_or(scissor, |previous| {
        intersect_scissor_rects(previous, scissor)
    })
}

#[cfg(test)]
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
    #[test]
    fn cached_filter_barriers_do_not_force_empty_source_render_passes() {
        let mut groups = (0..4)
            .map(|_| PreparedBackdropBlurGroup {
                source_steps: Vec::new(),
                filter_passes: Vec::new(),
                preserve_filtered_pixels: true,
            })
            .collect::<Vec<_>>();
        coalesce_source_groups(&mut groups);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].filter_passes.is_empty());
    }

    #[test]
    fn disconnected_damage_remains_independent_when_nested_effect_is_cached() {
        let mut damage = DirtyRegion::empty();
        damage.push(crate::bounds(
            crate::point(crate::ScaledPixels(20.0), crate::ScaledPixels(30.0)),
            crate::size(crate::ScaledPixels(12.0), crate::ScaledPixels(14.0)),
        ));
        damage.push(crate::bounds(
            crate::point(crate::ScaledPixels(680.0), crate::ScaledPixels(440.0)),
            crate::size(crate::ScaledPixels(18.0), crate::ScaledPixels(12.0)),
        ));
        // Eligibility is checked separately using the actual source barrier
        // and dirty filter lists; the patcher itself preserves exact regions.
        let parts = retained_source_damage_patches(&damage, true);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].rect_count(), 1);
        assert_eq!(parts[1].rect_count(), 1);
        assert_eq!(retained_source_damage_patches(&damage, false).len(), 1);
    }

    #[test]
    fn retained_gpu_color_patches_keep_disconnected_damage_independent() {
        let mut damage = DirtyRegion::empty();
        let a = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
        );
        let b = crate::bounds(
            crate::point(crate::ScaledPixels(900.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
        );
        damage.push(a);
        damage.push(b);
        let patches = retained_source_damage_patches(&damage, true);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches.iter().map(DirtyRegion::area).sum::<f32>(), 200.0);
        assert_eq!(retained_source_damage_patches(&damage, false).len(), 1);
    }


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

    #[test]
    fn partial_blur_source_scissor_is_smaller_than_full_refresh() {
        let config = test_backdrop_blur_config(2, 1);
        let drawable_size = DrawableSize {
            width: 800,
            height: 600,
        };
        let damage = dirty_region(24.0, 12.0, 2.0, 2.0);
        let full = blur_source_scissor_for_refresh(config, drawable_size, &damage, true)
            .expect("full blur source scissor");
        let partial = blur_source_scissor_for_refresh(config, drawable_size, &damage, false)
            .expect("partial blur source scissor");
        assert!(partial.width <= full.width);
        assert!(partial.height <= full.height);
        assert!(partial.width < full.width || partial.height < full.height);
    }
}
