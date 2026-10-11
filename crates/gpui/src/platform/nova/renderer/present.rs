use super::buffer_upload::{FrameBufferUpload, upload_frame_buffers};
use super::draw_steps::{PreparedBackdropBlurGroup, PreparedElementBlurLayer};
use super::filters::FilterRegistry;
use super::frame_graph::{FrameGraphPlan, Pass as GraphPass};
use super::*;
use std::time::Duration;

struct MainPresentDescriptor<'a> {
    submission_mode: GpuSubmissionMode,
    async_capabilities: AsyncCapabilities,
    pending_submissions: &'a mut Vec<PendingSubmission>,
    frame_resource_index: usize,
    swapchain: SwapchainId,
    render_pass: RenderPassId,
    depth_attachment: RenderPassDepthAttachment,
    damage: Option<ScissorRect>,
}

fn render_main_and_present<D>(
    device: &mut D,
    descriptor: MainPresentDescriptor<'_>,
    draw_steps: &[RenderStepDescriptor],
) -> Result<Option<gfx_core::PresentationFrame>>
where
    D: BackendPresentationCompat + BackendQueue + BackendResources,
{
    NovaRenderer::submit_present_frame(
        descriptor.submission_mode,
        descriptor.async_capabilities,
        descriptor.pending_submissions,
        device,
        descriptor.swapchain,
        descriptor.render_pass,
        draw_steps,
        clear_color(),
        Some(descriptor.depth_attachment),
        descriptor.frame_resource_index,
        descriptor.damage,
    )
}

fn render_backdrop_blur_groups<D>(
    device: &mut D,
    source_texture_view: TextureViewId,
    render_pass: RenderPassId,
    depth_attachment: RenderPassDepthAttachment,
    groups: &[PreparedBackdropBlurGroup],
) -> Result<()>
where
    D: BackendPresentationCompat,
{
    let pass_count = groups
        .iter()
        .enumerate()
        .map(|(index, group)| usize::from(index == 0 || !group.source_steps.is_empty())
            + group.filter_passes.len())
        .sum();
    let mut passes = Vec::with_capacity(pass_count);
    for (group_index, group) in groups.iter().enumerate() {
        let first_group = group_index == 0;
        let source_load_op = if first_group {
            LoadOp::Clear(clear_color())
        } else {
            LoadOp::Load
        };
        let source_depth_attachment = RenderPassDepthAttachment {
            target: depth_attachment.target,
            depth_load_op: if first_group {
                LoadOp::Clear(1.0)
            } else {
                LoadOp::Load
            },
        };
        // An empty continuation with Load/Load changes no pixels. Do not
        // submit a native offscreen render pass (and its layout transitions)
        // just to preserve the existing source. The first pass must still
        // clear the source even if it contains no geometry.
        if first_group || !group.source_steps.is_empty() {
            passes.push(gfx_core::TextureRenderStepList {
                texture_view: source_texture_view,
                render_pass,
                steps: RenderStepList::from_render_steps(&group.source_steps),
                color_load_op: source_load_op,
                clear_region: if first_group { group.source_clear_region } else { None },
                depth_attachment: Some(source_depth_attachment),
            });
        }
        let filter_depth_attachment = RenderPassDepthAttachment {
            target: depth_attachment.target,
            depth_load_op: LoadOp::Load,
        };
        let filter_load_op = if group.preserve_filtered_pixels {
            LoadOp::Load
        } else {
            LoadOp::Clear(clear_color())
        };
        for pass in &group.filter_passes {
            passes.push(gfx_core::TextureRenderStepList {
                texture_view: pass.target_texture_view,
                render_pass,
                steps: RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                color_load_op: filter_load_op,
                clear_region: None,
                depth_attachment: Some(filter_depth_attachment),
            });
        }
    }
    Ok(device.render_step_lists_to_textures_compat(&passes)?)
}

/// A preserved source with no draw steps has no render attachment work.
/// Ordinary isolated sources must still clear their first pass.
fn source_pass_required(first: bool, preserved: bool, has_steps: bool) -> bool {
    has_steps || (first && !preserved)
}

fn render_element_blur_layers<D>(
    device: &mut D,
    render_pass: RenderPassId,
    depth_attachment: RenderPassDepthAttachment,
    layers: &[PreparedElementBlurLayer],
) -> Result<()>
where
    D: BackendPresentationCompat,
{
    let pass_count = layers
        .iter()
        .map(|layer| {
            layer
                .source_groups
                .iter()
                .enumerate()
                .map(|(index, group)| {
                    usize::from(source_pass_required(
                        index == 0,
                        layer.preserve_retained_source,
                        !group.source_steps.is_empty(),
                    )) + group.filter_passes.len()
                })
                .sum::<usize>()
                + layer.filter_passes.len()
        })
        .sum();
    let mut passes = Vec::with_capacity(pass_count);
    for layer in layers {
        let filter_depth_attachment = RenderPassDepthAttachment {
            target: depth_attachment.target,
            depth_load_op: LoadOp::Load,
        };
        for (group_index, group) in layer.source_groups.iter().enumerate() {
            let source_depth_attachment = RenderPassDepthAttachment {
                target: depth_attachment.target,
                depth_load_op: if group_index == 0 {
                    LoadOp::Clear(1.0)
                } else {
                    LoadOp::Load
                },
            };
            if source_pass_required(
                group_index == 0,
                layer.preserve_retained_source,
                !group.source_steps.is_empty(),
            ) {
                passes.push(gfx_core::TextureRenderStepList {
                    texture_view: layer.source_texture_view,
                    render_pass,
                    steps: RenderStepList::from_render_steps(&group.source_steps),
                    color_load_op: if group_index == 0 && !layer.preserve_retained_source {
                        LoadOp::Clear(clear_color())
                    } else {
                        LoadOp::Load
                    },
                    clear_region: if group_index == 0 && !layer.preserve_retained_source {
                        group.source_clear_region
                    } else {
                        None
                    },
                    depth_attachment: Some(source_depth_attachment),
                });
            }
            let filter_load_op = if group.preserve_filtered_pixels {
                LoadOp::Load
            } else {
                LoadOp::Clear(clear_color())
            };
            for pass in &group.filter_passes {
                passes.push(gfx_core::TextureRenderStepList {
                    texture_view: pass.target_texture_view,
                    render_pass,
                    steps: RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                    color_load_op: filter_load_op,
                    clear_region: None,
                    depth_attachment: Some(filter_depth_attachment),
                });
            }
        }
        let filter_load_op = if layer.preserve_filtered_pixels {
            LoadOp::Load
        } else {
            LoadOp::Clear(clear_color())
        };
        for pass in &layer.filter_passes {
            passes.push(gfx_core::TextureRenderStepList {
                texture_view: pass.target_texture_view,
                render_pass,
                steps: RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                color_load_op: filter_load_op,
                clear_region: None,
                depth_attachment: Some(filter_depth_attachment),
            });
        }
    }
    Ok(device.render_step_lists_to_textures_compat(&passes)?)
}

fn execute_frame_graph_offscreen<D: BackendPresentationCompat>(
    device: &mut D,
    graph: &FrameGraphPlan,
    render_pass: RenderPassId,
    depth_attachment: RenderPassDepthAttachment,
    path_texture_view: TextureViewId,
    path_steps: &[DrawStepDescriptor],
    path_mask_cpu_elapsed: &mut Duration,
    element_layers: &[PreparedElementBlurLayer],
    backdrop_source: Option<TextureViewId>,
    backdrop_groups: &[PreparedBackdropBlurGroup],
) -> Result<()> {
    for pass in graph.offscreen_passes() {
        match pass {
            GraphPass::PathMask => path_mask::render(
                device,
                path_mask::Pass {
                    texture_view: path_texture_view,
                    render_pass,
                    steps: path_steps,
                    depth_attachment,
                },
                path_mask_cpu_elapsed,
            )?,
            GraphPass::ElementLayers => render_element_blur_layers(
                device,
                render_pass,
                depth_attachment,
                element_layers,
            )?,
            GraphPass::BackdropBlur => {
                if let Some(source_texture_view) = backdrop_source {
                    render_backdrop_blur_groups(
                        device,
                        source_texture_view,
                        render_pass,
                        depth_attachment,
                        backdrop_groups,
                    )?;
                }
            }
            GraphPass::MainPresent => unreachable!("swapchain Present is the graph sink"),
        }
    }
    Ok(())
}

fn has_root_backdrop_blurs(frame_upload: &FrameUpload) -> bool {
    let mut element_depth = 0usize;
    for batch in &frame_upload.batches {
        match batch {
            UploadedBatch::BeginBlur { .. } => {
                element_depth = element_depth.saturating_add(1);
            }
            UploadedBatch::EndBlur { .. } => {
                element_depth = element_depth.saturating_sub(1);
            }
            UploadedBatch::BackdropBlurs { .. } if element_depth == 0 => return true,
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
    false
}

fn dirty_element_blur_indices(
    frame_upload: &FrameUpload,
    dirty_region: &crate::DirtyRegion,
    filters: &FilterRegistry,
    animation_values: &[crate::SceneAnimationValue],
    force_all: bool,
) -> Vec<u32> {
    let ranges = frame_upload.blur_content_ranges();
    if ranges.is_empty() {
        return Vec::new();
    }
    if force_all {
        return ranges.iter().map(|range| range.index).collect();
    }

    let mut dirty = Vec::new();
    for range in ranges {
        // Each captured Layer owns its dirty decision: the source may have a
        // different stable identity, content revision, clip, radius or animated
        // pixel values even when the outer window reports no spatial damage.
        // A composite-only transform leaves the captured source unchanged.
        if let Some((_, source)) = frame_upload
            .element_blur_inputs
            .iter()
            .find(|(index, _)| *index == range.index)
        {
            if !filters.source_unchanged(range.index, source, animation_values) {
                dirty.push(range.index);
            }
            continue;
        }
        // The synthetic retained-root target has no isolated Scene source:
        // it must still be driven by window-level dirty pixels.
        if dirty_region.is_empty() {
            continue;
        }
        if dirty_region.is_full() {
            dirty.push(range.index);
            continue;
        }
        let Some(config) = frame_upload.backdrop_blur_config_for_index(range.index) else {
            dirty.push(range.index);
            continue;
        };
        let [x, y, width, height] = config.bounds();
        if ![x, y, width, height].into_iter().all(f32::is_finite) {
            dirty.push(range.index);
            continue;
        }
        let effect_bounds = Bounds {
            origin: Point {
                x: crate::ScaledPixels(x),
                y: crate::ScaledPixels(y),
            },
            size: Size {
                width: crate::ScaledPixels(width.max(0.0)),
                height: crate::ScaledPixels(height.max(0.0)),
            },
        };
        if dirty_region
            .rects()
            .iter()
            .any(|rect| rect.bounds.intersects(&effect_bounds))
        {
            dirty.push(range.index);
        }
    }
    dirty
}

fn scissor_pixel_area(scissor: ScissorRect) -> usize {
    (scissor.width as usize).saturating_mul(scissor.height as usize)
}

fn render_step_scissor(step: &RenderStepDescriptor) -> Option<ScissorRect> {
    match step {
        RenderStepDescriptor::Draw(step) => step.scissor,
        RenderStepDescriptor::DrawIndexed(step) => step.scissor,
    }
}

impl NovaRenderer {
    fn drawable_pixels(&self) -> usize {
        (self.current_size.width as usize).saturating_mul(self.current_size.height as usize)
    }

    fn backdrop_blur_pixel_metrics(
        &self,
        backdrop_groups: &[PreparedBackdropBlurGroup],
        element_layers: &[PreparedElementBlurLayer],
    ) -> (usize, [usize; 6]) {
        let drawable_pixels = self.drawable_pixels();
        let mut source_pixels = 0usize;
        let mut level_pixels = [0usize; 6];

        let mut record_group = |group: &PreparedBackdropBlurGroup| {
            if !group.source_steps.is_empty() {
                let mut source_scissor = None::<ScissorRect>;
                let mut unbounded = false;
                for step in &group.source_steps {
                    let Some(scissor) = render_step_scissor(step) else {
                        unbounded = true;
                        break;
                    };
                    if scissor.is_empty() {
                        continue;
                    }
                    source_scissor = Some(match source_scissor {
                        Some(current) => union_scissor_rects(current, scissor),
                        None => scissor,
                    });
                }
                source_pixels = source_pixels.saturating_add(if unbounded {
                    drawable_pixels
                } else {
                    source_scissor.map_or(0, scissor_pixel_area)
                });
            }
            for (pass_index, pass) in group.filter_passes.iter().enumerate() {
                let pixels = pass
                    .step
                    .scissor
                    .map_or(drawable_pixels, scissor_pixel_area);
                let level = pass_index & 1;
                level_pixels[level] = level_pixels[level].saturating_add(pixels);
            }
        };

        for group in backdrop_groups {
            record_group(group);
        }
        for layer in element_layers {
            for group in &layer.source_groups {
                record_group(group);
            }
        }
        drop(record_group);
        for layer in element_layers {
            for (pass_index, pass) in layer.filter_passes.iter().enumerate() {
                let pixels = pass
                    .step
                    .scissor
                    .map_or(drawable_pixels, scissor_pixel_area);
                let level = pass_index & 1;
                level_pixels[level] = level_pixels[level].saturating_add(pixels);
            }
        }

        (source_pixels, level_pixels)
    }

    fn backdrop_blur_full_target_pixels(&self) -> usize {
        let source_width = self.current_size.width as usize;
        let source_height = self.current_size.height as usize;
        self.frame_upload
            .backdrop_blur_configs()
            .iter()
            .fold(0usize, |total, config| {
                let factor = usize::from(config.downsample().max(1));
                let filtered_width = source_width.div_ceil(factor).max(1);
                let filtered_height = source_height.div_ceil(factor).max(1);
                total
                    .saturating_add(filtered_width.saturating_mul(source_height))
                    .saturating_add(filtered_width.saturating_mul(filtered_height))
            })
    }

    pub(super) fn draw_present(
        &mut self,
        upload: FrameUploadSummary,
        packet: &mut PresentationPacket,
        backdrop_blur_quality: BackdropBlurQuality,
        presentation_timing: &mut Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<bool> {
        let result =
            self.draw_present_inner(upload, packet, backdrop_blur_quality, presentation_timing);
        // Failed/declined presentation can follow a partial write into a uniquely owned version.
        // Do not advertise its previous content token to another slot on the next scene switch.
        if !matches!(result, Ok(true)) {
            self.retained_upload
                .invalidate_slot(self.current_frame_resource_index);
        } else if let Err(error) = self.coalesce_idle_static_buffers() {
            // This frame is already submitted; a failed rebind retains the old idle version.
            log::debug!("failed to coalesce idle nova static buffers: {error}");
        }
        result
    }

    fn draw_present_inner(
        &mut self,
        upload: FrameUploadSummary,
        packet: &mut PresentationPacket,
        backdrop_blur_quality: BackdropBlurQuality,
        presentation_timing: &mut Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<bool> {
        if let Some(timing) = presentation_timing.as_mut() {
            timing.renderer_scene_prepare =
                Instant::now().saturating_duration_since(timing.frame_started_at);
        }
        let submission_prepare_started = Instant::now();
        self.prepare_for_frame_submission(packet.frame_time)?;
        if let Some(timing) = presentation_timing.as_mut() {
            timing.submission_prepare = submission_prepare_started.elapsed();
        }
        let retained_resource_prepare_started = Instant::now();
        if self.atlas.has_pending_removals() {
            // A retained Scene stores AtlasTile identities directly. Asset/cache eviction can queue
            // a removal after that Scene was built but before this submission starts, so never
            // deallocate a tile that the Scene about to be encoded still samples.
            let mut live_scene_tiles = FxHashSet::default();
            packet
                .scene
                .collect_atlas_tile_ids_into(&mut live_scene_tiles);
            if self.atlas.has_retirable_pending_removals(&live_scene_tiles) {
                self.wait_for_pending_submissions()?;
                self.atlas.apply_pending_removals_except(&live_scene_tiles);
            }
        }
        self.sync_atlas_textures_for_current_backend()?;
        self.ensure_quad_capacity()?;
        self.ensure_path_rasterization_capacity()?;
        if let Some(timing) = presentation_timing.as_mut() {
            timing.retained_resource_prepare = retained_resource_prepare_started.elapsed();
        }
        let frame_started = Instant::now();
        let backend_label = self.backend_info.label();
        let async_capabilities = lock_backend(&self.backend).async_capabilities();
        let native_partial_presentation = lock_backend(&self.backend)
            .presentation_capabilities(self.swapchain)
            .partial_presentation;
        let submission_mode = self.presentation_submission_mode();
        let has_backdrop_blurs = self.has_backdrop_blurs();
        let has_root_backdrop_blurs = has_root_backdrop_blurs(&self.frame_upload);
        let has_element_blurs = self.frame_upload.has_element_blurs();

        let backdrop_source_atlas_textures = if has_backdrop_blurs {
            self.frame_upload.backdrop_source_atlas_texture_ids()
        } else {
            Default::default()
        };
        let backdrop_source_atlas_tiles = if has_backdrop_blurs {
            self.frame_upload.backdrop_source_atlas_tiles()
        } else {
            Default::default()
        };
        let atlas_content_generation = self.atlas.content_generation();
        let atlas_generation_changed = self.filters.atlas_generation != atlas_content_generation;
        let backdrop_source_atlas_dirty = has_backdrop_blurs
            && self
                .atlas
                .pending_uploads_touch_source_tiles(
                    &backdrop_source_atlas_textures,
                    &backdrop_source_atlas_tiles,
                );
        let shared_blur_cache_invalid = has_backdrop_blurs
            && self
                .filters
                .refresh_required(backdrop_blur_quality, backdrop_source_atlas_dirty);
        let backdrop_blur_refresh_required = has_root_backdrop_blurs
            && (packet.force_full_backdrop_blur_refresh
                || packet.backdrop_blur_damage_plan.refresh_required()
                || shared_blur_cache_invalid);
        let dirty_element_indices = dirty_element_blur_indices(
            &self.frame_upload,
            &packet.dirty_region,
            &self.filters,
            &packet.presentation_animation_values,
            shared_blur_cache_invalid,
        );
        let element_blur_refresh_required = has_element_blurs && !dirty_element_indices.is_empty();
        if backdrop_blur_refresh_required || element_blur_refresh_required {
            self.filters.begin_refresh();
        }
        let present_damage = (native_partial_presentation
            && upload.unsupported_batches.total() == 0)
            .then(|| partial_scissor_for_packet(packet, self.current_size))
            .flatten();
        let present_damage = present_damage.filter(|damage| {
            let drawable_pixels = self.drawable_pixels();
            drawable_pixels == 0
                || u64::from(damage.width)
                    * u64::from(damage.height)
                    * PARTIAL_PRESENT_MAX_DAMAGE_AREA_RECIPROCAL
                    <= drawable_pixels as u64
        });
        // Native dirty-rect presentation and retained scene-color damage are
        // independent. Vulkan cannot trust rotating backbuffer contents, but
        // it can still scissor the retained offscreen scene before a full present.
        let retained_partial = self.frame_upload.retained_root_blur.is_some()
            && !packet.dirty_region.is_full()
            && !packet.dirty_region.is_empty();
        if present_damage.is_some() || retained_partial {
            crate::diagnostics::performance_metrics::record_partial_redraw();
        } else if packet.partial_present_mode == PartialPresentMode::Partial {
            crate::diagnostics::performance_metrics::record_full_redraw_fallback();
        }

        self.prepare_draw_steps(packet.scene.revision);
        self.prepare_path_mask_draw_steps(packet.scene.revision);
        self.prepare_backdrop_blur_passes(has_backdrop_blurs);
        let backdrop_blur_groups = if backdrop_blur_refresh_required {
            self.prepare_backdrop_blur_groups(true)
        } else {
            Vec::new()
        };
        let element_blur_layers = self.prepare_element_blur_layers(&dirty_element_indices);
        let root_scene_color_cached = self.frame_upload.retained_root_blur.is_some();
        let root_scene_color_refreshes = element_blur_layers
            .iter()
            .filter(|layer| Some(layer.index) == self.frame_upload.retained_root_blur)
            .count();
        // Unlike the main swapchain pass count, this measures actual root
        // source geometry replay after batch-level culling. Useful for
        // verifying that a small animation no longer replays static chrome.
        let retained_root_draw_steps: usize = element_blur_layers
            .iter()
            .filter(|layer| Some(layer.index) == self.frame_upload.retained_root_blur)
            .flat_map(|layer| &layer.source_groups)
            .map(|group| group.source_steps.len())
            .sum();
        let filter_source_draw_steps: usize = backdrop_blur_groups
            .iter()
            .map(|group| group.source_steps.len())
            .sum::<usize>()
            .saturating_add(
                element_blur_layers
                    .iter()
                    .filter(|layer| Some(layer.index) != self.frame_upload.retained_root_blur)
                    .flat_map(|layer| &layer.source_groups)
                    .map(|group| group.source_steps.len())
                    .sum::<usize>(),
            );
        let (blur_source_pixels, blur_level_pixels) =
            self.backdrop_blur_pixel_metrics(&backdrop_blur_groups, &element_blur_layers);
        let blur_target_pixels = blur_level_pixels.iter().copied().sum::<usize>();
        let blur_full_target_pixels = self.backdrop_blur_full_target_pixels();
        let draw_step_count = self.draw_step_scratch.steps().len();
        let draw_step_cache_hit = self.draw_step_scratch.draw_step_cache_hit;
        let path_mask_step_count = self.draw_step_scratch.path_steps().len();
        let path_mask_cache_hit = self.draw_step_scratch.path_mask_cache_hit;
        let single_full_path_step = matches!(
            self.draw_step_scratch.path_steps(),
            [step] if step.first_vertex == 0
                && step.vertex_count == upload.path_vertex_count
                && usize::try_from(step.vertex_count)
                    .ok()
                    .and_then(|count| count.checked_mul(PACKED_PATH_RASTERIZATION_VERTEX_BYTES))
                    == Some(self.frame_upload.path_rasterization_vertices.len())
        );
        let path_mask_key = path_mask::Key {
            // Packed path bytes include ordered geometry, paint and clip. Multiple draws
            // need an additional command-order identity before their pixels can be reused.
            content: single_full_path_step
                .then(|| self.retained_upload.path_mask_token())
                .flatten(),
            texture_view: self.path_texture_view,
            target_size: self.path_texture_size,
            viewport: self.current_size,
            format: self.surface_format,
            pipeline: self.pipelines.path_rasterization,
        };
        let render_path_mask =
            path_mask_step_count != 0 && self.path_mask_residency.begin(path_mask_key);
        let mut path_mask_cpu_elapsed = Duration::ZERO;
        let graph = FrameGraphPlan::compile(
            render_path_mask,
            element_blur_layers.len(),
            backdrop_blur_groups.len(),
        );
        let mask_pass_count = usize::from(graph.requires(GraphPass::PathMask));
        let main_pass_count = usize::from(graph.requires(GraphPass::MainPresent));
        if self.diagnostics.enabled {
            let versioned_layers = dirty_element_indices.iter().filter(|index| {
                self.filters.layer_content_version(**index).is_some()
            }).count();
            log::debug!(
                "nova render graph: nodes={} refreshed_layer_targets={} previously_versioned_layers={} path_mask={} element_layers={} backdrop={} main_present={}",
                graph.node_count(),
                element_blur_layers.len(),
                versioned_layers,
                graph.requires(GraphPass::PathMask),
                graph.requires(GraphPass::ElementLayers),
                graph.requires(GraphPass::BackdropBlur),
                graph.requires(GraphPass::MainPresent),
            );
        }
        let backdrop_blur_refreshed: bool;
        let element_blur_refreshed: bool;
        let blur_group_pass_count = backdrop_blur_groups.iter().enumerate().fold(
            0usize,
            |total, (index, group)| {
                total.saturating_add(
                    usize::from(index == 0 || !group.source_steps.is_empty())
                        .saturating_add(group.filter_passes.len()),
                )
            },
        );
        let element_blur_pass_count = element_blur_layers.iter().fold(0usize, |total, layer| {
            let source_passes = layer.source_groups.iter().enumerate().fold(
                0usize,
                |total, (index, group)| {
                    total.saturating_add(
                        usize::from(source_pass_required(
                            index == 0,
                            layer.preserve_retained_source,
                            !group.source_steps.is_empty(),
                        ))
                        .saturating_add(group.filter_passes.len()),
                    )
                },
            );
            total
                .saturating_add(source_passes)
                .saturating_add(layer.filter_passes.len())
        });
        let composite_pass_count = blur_group_pass_count.saturating_add(element_blur_pass_count);
        crate::diagnostics::performance_metrics::record_gpu_pass_metrics(
            mask_pass_count,
            main_pass_count,
            composite_pass_count,
        );

        let unsupported = upload.unsupported_batches;
        let static_uploads = self
            .retained_upload
            .static_upload_mask(self.current_frame_resource_index);
        let quad_upload_plan = self.retained_upload.quad_upload_plan(
            self.current_frame_resource_index,
            self.frame_upload.quads.len(),
        );
        let upload_static = !static_uploads.is_empty();
        let animated_upload_bytes = self.frame_upload.animated_upload_bytes();
        let mut mapped_upload_bytes = if upload_static {
            static_uploads.mapped_upload_bytes(&self.frame_upload, has_backdrop_blurs)
        } else {
            animated_upload_bytes
        };
        if static_uploads.quad {
            mapped_upload_bytes = mapped_upload_bytes
                .saturating_sub(self.frame_upload.quads.len())
                .saturating_add(quad_upload_plan.uploaded_bytes(self.frame_upload.quads.len()));
        }
        let uploaded_bytes = mapped_upload_bytes;
        let breakdown = if upload_static {
            let mut breakdown = self.frame_upload.upload_breakdown();
            breakdown.quad_bytes = quad_upload_plan.uploaded_bytes(self.frame_upload.quads.len());
            breakdown.animation_bytes = 0;
            breakdown
        } else {
            crate::diagnostics::performance_metrics::FrameUploadBreakdown {
                animation_bytes: mapped_upload_bytes,
                ..Default::default()
            }
        };
        crate::diagnostics::performance_metrics::record_frame_upload_breakdown(breakdown);
        crate::diagnostics::performance_metrics::record_backdrop_blur_primitive_count(
            upload.backdrop_blur_count as usize,
        );
        if self.diagnostics.should_warn_unsupported(unsupported) {
            log::warn!(
                concat!(
                    "nova-gfx unsupported or fallback batches: backend={} ",
                    "paths={} surfaces={} backdrop_blurs={} backdrop_blur_tint_fallbacks={} ",
                    "set GPUI_NOVA_RENDER_DIAGNOSTICS=1 for every-frame details"
                ),
                backend_label,
                unsupported.paths,
                unsupported.surfaces,
                unsupported.backdrop_blurs,
                unsupported.backdrop_blur_tint_fallbacks,
            );
        }
        if self.diagnostics.enabled {
            log::warn!(
                concat!(
                    "nova-gfx frame diagnostics: backend={} alpha_swapchain={:?} ",
                    "alpha_output={:?} premultiplied={} quads={} shadows={} paths={} ",
                    "path_vertices={} mono_sprites={} poly_sprites={} underlines={} ",
                    "draw_steps={} draw_step_cache_hit={} path_mask_steps={} path_mask_cache_hit={} gpu_passes={} upload_bytes={} ",
                    "async_submission={} async_wait={} async_presentation={} ",
                    "native_partial_presentation={} retained_root={} ",
                    "retained_root_refreshes={} main_swapchain_steps={} ",
                    "retained_root_draw_steps={} filter_source_draw_steps={} ",
                    "present_damage={:?} dirty_mode={:?} dirty_full={} dirty_rects={} ",
                    "dirty_area={} backdrop_blur_refresh={} element_blur_refresh={} ",
                    "element_blur_dirty_layers={} blur_source_atlas_dirty={} ",
                    "blur_atlas_generation_changed={} blur_source_atlas_textures={} blur_groups={} ",
                    "blur_source_pixels={} blur_horizontal_pixels={} blur_final_pixels={} ",
                    "blur_target_pixels={} blur_full_target_pixels={} ",
                    "animation_bindings={} animation_values={} threading={:?}"
                ),
                backend_label,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                self.surface_alpha.outputs_premultiplied_alpha(),
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.path_vertex_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_step_count,
                draw_step_cache_hit,
                path_mask_step_count,
                path_mask_cache_hit,
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
                uploaded_bytes,
                async_capabilities.async_submission,
                async_capabilities.async_wait,
                async_capabilities.async_presentation,
                native_partial_presentation,
                root_scene_color_cached,
                root_scene_color_refreshes,
                draw_step_count,
                retained_root_draw_steps,
                filter_source_draw_steps,
                present_damage,
                packet.partial_present_mode,
                packet.dirty_region.is_full(),
                packet.dirty_region.rect_count(),
                packet.dirty_region.area(),
                backdrop_blur_refresh_required,
                element_blur_refresh_required,
                dirty_element_indices.len(),
                backdrop_source_atlas_dirty,
                atlas_generation_changed,
                backdrop_source_atlas_textures.len(),
                backdrop_blur_groups.len(),
                blur_source_pixels,
                blur_level_pixels[0],
                blur_level_pixels[1],
                blur_target_pixels,
                blur_full_target_pixels,
                upload.animation_binding_count,
                upload.animation_value_count,
                async_capabilities.threading_mode,
            );
        } else {
            log::trace!(
                concat!(
                    "nova-gfx frame upload: alpha_swapchain={:?} alpha_output={:?} ",
                    "quads={} shadows={} paths={} mono_sprites={} poly_sprites={} ",
                    "underlines={} draw_steps={} path_mask_steps={} gpu_passes={}"
                ),
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_step_count,
                path_mask_step_count,
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
            );
        }

        let depth_attachment = self.depth_attachment();
        let frame_buffers = self.frame_buffer_targets();
        let backdrop_blur_source_texture_view = if has_backdrop_blurs {
            Some(
                self.filters
                    .targets
                    .as_ref()
                    .context("missing nova backdrop blur targets")?
                    .source
                    .texture_view,
            )
        } else {
            None
        };
        let atlas_texture_region_count: usize;
        let atlas_texture_upload_bytes: usize;
        let backend_work_started = Instant::now();
        let atlas_resource_descriptor = self.atlas_resource_descriptor();

        let render_result: Result<(bool, Option<gfx_core::PresentationTimings>)> =
            match &mut *lock_backend(&self.backend) {
                #[cfg(all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ))]
                NovaBackend::OpenGl(device) => {
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.draw_step_prepare =
                            backend_work_started.saturating_duration_since(frame_started);
                    }
                    let upload_started = Instant::now();
                    upload_frame_buffers(
                        device,
                        FrameBufferUpload {
                            buffers: frame_buffers,
                            source: &self.frame_upload,
                            has_backdrop_blurs,
                            static_uploads,
                            quad_upload_plan: &quad_upload_plan,
                        },
                    )?;
                    let buffer_upload_elapsed = upload_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.buffer_upload = buffer_upload_elapsed;
                    }
                    let buffer_upload_elapsed_ms = buffer_upload_elapsed.as_millis();
                    let atlas_started = Instant::now();
                    let atlas_stats = upload_pending_atlas(
                        &self.atlas,
                        device,
                        &mut self.gpu_atlas_textures,
                        backend_label,
                        &atlas_resource_descriptor,
                    )?;
                    let atlas_upload_elapsed = atlas_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.atlas_upload = atlas_upload_elapsed;
                    }
                    let atlas_upload_elapsed_ms = atlas_upload_elapsed.as_millis();
                    atlas_texture_region_count = atlas_stats.upload_count;
                    atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                    record_nova_upload_metrics(mapped_upload_bytes, atlas_stats);
                    let offscreen_started = Instant::now();
                    execute_frame_graph_offscreen(
                        device,
                        &graph,
                        self.render_pass,
                        depth_attachment,
                        self.path_texture_view,
                        self.draw_step_scratch.path_steps(),
                        &mut path_mask_cpu_elapsed,
                        &element_blur_layers,
                        backdrop_blur_source_texture_view,
                        &backdrop_blur_groups,
                    )?;
                    backdrop_blur_refreshed = graph.requires(GraphPass::BackdropBlur);
                    element_blur_refreshed = graph.requires(GraphPass::ElementLayers);
                    let offscreen_elapsed = offscreen_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.offscreen_render = offscreen_elapsed;
                    }
                    let offscreen_elapsed_ms = offscreen_elapsed.as_millis();
                    let present_started = Instant::now();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.frame_prepare_upload =
                            present_started.saturating_duration_since(frame_started);
                    }
                    let presentation_frame = render_main_and_present(
                        device,
                        MainPresentDescriptor {
                            submission_mode,
                            async_capabilities,
                            pending_submissions: &mut self.pending_submissions,
                            frame_resource_index: self.current_frame_resource_index,
                            swapchain: self.swapchain,
                            render_pass: self.render_pass,
                            depth_attachment,
                            damage: present_damage,
                        },
                        self.draw_step_scratch.steps(),
                    )?;
                    let did_present = presentation_frame.is_some();
                    let backend_presentation_timings =
                        presentation_frame.and_then(|frame| frame.timings);
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.backend_present = present_started.elapsed();
                    }
                    let present_elapsed_ms = present_started.elapsed().as_millis();
                    let total_elapsed_ms = frame_started.elapsed().as_millis();
                    if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                        log::warn!(
                            concat!(
                                "nova-gfx frame stages: backend={} total_ms={} ",
                                "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                                "present_ms={} submission_mode={:?} atlas_uploads={} ",
                                "atlas_bytes={} blur_groups={} element_blur_layers={}"
                            ),
                            backend_label,
                            total_elapsed_ms,
                            buffer_upload_elapsed_ms,
                            atlas_upload_elapsed_ms,
                            offscreen_elapsed_ms,
                            present_elapsed_ms,
                            submission_mode,
                            atlas_stats.upload_count,
                            atlas_stats.uploaded_bytes,
                            backdrop_blur_groups.len(),
                            element_blur_layers.len(),
                        );
                    }
                    Ok((did_present, backend_presentation_timings))
                }
                #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
                NovaBackend::Dx11(device) => {
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.draw_step_prepare =
                            backend_work_started.saturating_duration_since(frame_started);
                    }
                    let upload_started = Instant::now();
                    upload_frame_buffers(
                        device,
                        FrameBufferUpload {
                            buffers: frame_buffers,
                            source: &self.frame_upload,
                            has_backdrop_blurs,
                            static_uploads,
                            quad_upload_plan: &quad_upload_plan,
                        },
                    )?;
                    let buffer_upload_elapsed = upload_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.buffer_upload = buffer_upload_elapsed;
                    }
                    let buffer_upload_elapsed_ms = buffer_upload_elapsed.as_millis();
                    let atlas_started = Instant::now();
                    let atlas_stats = upload_pending_atlas(
                        &self.atlas,
                        device,
                        &mut self.gpu_atlas_textures,
                        backend_label,
                        &atlas_resource_descriptor,
                    )?;
                    let atlas_upload_elapsed = atlas_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.atlas_upload = atlas_upload_elapsed;
                    }
                    let atlas_upload_elapsed_ms = atlas_upload_elapsed.as_millis();
                    atlas_texture_region_count = atlas_stats.upload_count;
                    atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                    record_nova_upload_metrics(mapped_upload_bytes, atlas_stats);
                    let offscreen_started = Instant::now();
                    execute_frame_graph_offscreen(
                        device,
                        &graph,
                        self.render_pass,
                        depth_attachment,
                        self.path_texture_view,
                        self.draw_step_scratch.path_steps(),
                        &mut path_mask_cpu_elapsed,
                        &element_blur_layers,
                        backdrop_blur_source_texture_view,
                        &backdrop_blur_groups,
                    )?;
                    backdrop_blur_refreshed = graph.requires(GraphPass::BackdropBlur);
                    element_blur_refreshed = graph.requires(GraphPass::ElementLayers);
                    let offscreen_elapsed = offscreen_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.offscreen_render = offscreen_elapsed;
                    }
                    let offscreen_elapsed_ms = offscreen_elapsed.as_millis();
                    let present_started = Instant::now();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.frame_prepare_upload =
                            present_started.saturating_duration_since(frame_started);
                    }
                    let presentation_frame = render_main_and_present(
                        device,
                        MainPresentDescriptor {
                            submission_mode,
                            async_capabilities,
                            pending_submissions: &mut self.pending_submissions,
                            frame_resource_index: self.current_frame_resource_index,
                            swapchain: self.swapchain,
                            render_pass: self.render_pass,
                            depth_attachment,
                            damage: present_damage,
                        },
                        self.draw_step_scratch.steps(),
                    )?;
                    let did_present = presentation_frame.is_some();
                    let backend_presentation_timings =
                        presentation_frame.and_then(|frame| frame.timings);
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.backend_present = present_started.elapsed();
                    }
                    let present_elapsed_ms = present_started.elapsed().as_millis();
                    let total_elapsed_ms = frame_started.elapsed().as_millis();
                    if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                        log::warn!(
                            concat!(
                                "nova-gfx frame stages: backend={} total_ms={} ",
                                "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                                "present_ms={} submission_mode={:?} atlas_uploads={} ",
                                "atlas_bytes={} blur_groups={} element_blur_layers={}"
                            ),
                            backend_label,
                            total_elapsed_ms,
                            buffer_upload_elapsed_ms,
                            atlas_upload_elapsed_ms,
                            offscreen_elapsed_ms,
                            present_elapsed_ms,
                            submission_mode,
                            atlas_stats.upload_count,
                            atlas_stats.uploaded_bytes,
                            backdrop_blur_groups.len(),
                            element_blur_layers.len(),
                        );
                    }
                    Ok((did_present, backend_presentation_timings))
                }
                #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
                NovaBackend::Dx12(device) => {
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.draw_step_prepare =
                            backend_work_started.saturating_duration_since(frame_started);
                    }
                    let upload_started = Instant::now();
                    upload_frame_buffers(
                        device,
                        FrameBufferUpload {
                            buffers: frame_buffers,
                            source: &self.frame_upload,
                            has_backdrop_blurs,
                            static_uploads,
                            quad_upload_plan: &quad_upload_plan,
                        },
                    )?;
                    let buffer_upload_elapsed = upload_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.buffer_upload = buffer_upload_elapsed;
                    }
                    let buffer_upload_elapsed_ms = buffer_upload_elapsed.as_millis();
                    let atlas_started = Instant::now();
                    let atlas_stats = upload_pending_atlas(
                        &self.atlas,
                        device,
                        &mut self.gpu_atlas_textures,
                        backend_label,
                        &atlas_resource_descriptor,
                    )?;
                    let atlas_upload_elapsed = atlas_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.atlas_upload = atlas_upload_elapsed;
                    }
                    let atlas_upload_elapsed_ms = atlas_upload_elapsed.as_millis();
                    atlas_texture_region_count = atlas_stats.upload_count;
                    atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                    record_nova_upload_metrics(mapped_upload_bytes, atlas_stats);
                    let offscreen_started = Instant::now();
                    execute_frame_graph_offscreen(
                        device,
                        &graph,
                        self.render_pass,
                        depth_attachment,
                        self.path_texture_view,
                        self.draw_step_scratch.path_steps(),
                        &mut path_mask_cpu_elapsed,
                        &element_blur_layers,
                        backdrop_blur_source_texture_view,
                        &backdrop_blur_groups,
                    )?;
                    backdrop_blur_refreshed = graph.requires(GraphPass::BackdropBlur);
                    element_blur_refreshed = graph.requires(GraphPass::ElementLayers);
                    let offscreen_elapsed = offscreen_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.offscreen_render = offscreen_elapsed;
                    }
                    let offscreen_elapsed_ms = offscreen_elapsed.as_millis();
                    let present_started = Instant::now();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.frame_prepare_upload =
                            present_started.saturating_duration_since(frame_started);
                    }
                    let presentation_frame = render_main_and_present(
                        device,
                        MainPresentDescriptor {
                            submission_mode,
                            async_capabilities,
                            pending_submissions: &mut self.pending_submissions,
                            frame_resource_index: self.current_frame_resource_index,
                            swapchain: self.swapchain,
                            render_pass: self.render_pass,
                            depth_attachment,
                            damage: present_damage,
                        },
                        self.draw_step_scratch.steps(),
                    )?;
                    let did_present = presentation_frame.is_some();
                    let backend_presentation_timings =
                        presentation_frame.and_then(|frame| frame.timings);
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.backend_present = present_started.elapsed();
                    }
                    let present_elapsed_ms = present_started.elapsed().as_millis();
                    let total_elapsed_ms = frame_started.elapsed().as_millis();
                    if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                        log::warn!(
                            concat!(
                                "nova-gfx frame stages: backend={} total_ms={} ",
                                "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                                "present_ms={} submission_mode={:?} atlas_uploads={} ",
                                "atlas_bytes={} blur_groups={} element_blur_layers={}"
                            ),
                            backend_label,
                            total_elapsed_ms,
                            buffer_upload_elapsed_ms,
                            atlas_upload_elapsed_ms,
                            offscreen_elapsed_ms,
                            present_elapsed_ms,
                            submission_mode,
                            atlas_stats.upload_count,
                            atlas_stats.uploaded_bytes,
                            backdrop_blur_groups.len(),
                            element_blur_layers.len(),
                        );
                    }
                    Ok((did_present, backend_presentation_timings))
                }
                #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
                NovaBackend::Metal(device) => {
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.draw_step_prepare =
                            backend_work_started.saturating_duration_since(frame_started);
                    }
                    upload_frame_buffers(
                        device,
                        FrameBufferUpload {
                            buffers: frame_buffers,
                            source: &self.frame_upload,
                            has_backdrop_blurs,
                            static_uploads,
                            quad_upload_plan: &quad_upload_plan,
                        },
                    )?;
                    let atlas_stats = upload_pending_atlas(
                        &self.atlas,
                        device,
                        &mut self.gpu_atlas_textures,
                        backend_label,
                        &atlas_resource_descriptor,
                    )?;
                    atlas_texture_region_count = atlas_stats.upload_count;
                    atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                    record_nova_upload_metrics(mapped_upload_bytes, atlas_stats);
                    execute_frame_graph_offscreen(
                        device,
                        &graph,
                        self.render_pass,
                        depth_attachment,
                        self.path_texture_view,
                        self.draw_step_scratch.path_steps(),
                        &mut path_mask_cpu_elapsed,
                        &element_blur_layers,
                        backdrop_blur_source_texture_view,
                        &backdrop_blur_groups,
                    )?;
                    backdrop_blur_refreshed = graph.requires(GraphPass::BackdropBlur);
                    element_blur_refreshed = graph.requires(GraphPass::ElementLayers);
                    let present_started = Instant::now();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.frame_prepare_upload =
                            present_started.saturating_duration_since(frame_started);
                    }
                    let presentation_frame = render_main_and_present(
                        device,
                        MainPresentDescriptor {
                            submission_mode,
                            async_capabilities,
                            pending_submissions: &mut self.pending_submissions,
                            frame_resource_index: self.current_frame_resource_index,
                            swapchain: self.swapchain,
                            render_pass: self.render_pass,
                            depth_attachment,
                            damage: present_damage,
                        },
                        self.draw_step_scratch.steps(),
                    )?;
                    let did_present = presentation_frame.is_some();
                    let backend_presentation_timings =
                        presentation_frame.and_then(|frame| frame.timings);
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.backend_present = present_started.elapsed();
                    }
                    Ok((did_present, backend_presentation_timings))
                }
                #[cfg(all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                ))]
                NovaBackend::Vulkan(device) => {
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.draw_step_prepare =
                            backend_work_started.saturating_duration_since(frame_started);
                    }
                    let upload_started = Instant::now();
                    upload_frame_buffers(
                        device,
                        FrameBufferUpload {
                            buffers: frame_buffers,
                            source: &self.frame_upload,
                            has_backdrop_blurs,
                            static_uploads,
                            quad_upload_plan: &quad_upload_plan,
                        },
                    )?;
                    let buffer_upload_elapsed = upload_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.buffer_upload = buffer_upload_elapsed;
                    }
                    let buffer_upload_elapsed_ms = buffer_upload_elapsed.as_millis();
                    let atlas_started = Instant::now();
                    let atlas_stats = upload_pending_atlas(
                        &self.atlas,
                        device,
                        &mut self.gpu_atlas_textures,
                        backend_label,
                        &atlas_resource_descriptor,
                    )?;
                    let atlas_upload_elapsed = atlas_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.atlas_upload = atlas_upload_elapsed;
                    }
                    let atlas_upload_elapsed_ms = atlas_upload_elapsed.as_millis();
                    atlas_texture_region_count = atlas_stats.upload_count;
                    atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                    record_nova_upload_metrics(mapped_upload_bytes, atlas_stats);
                    let offscreen_started = Instant::now();
                    execute_frame_graph_offscreen(
                        device,
                        &graph,
                        self.render_pass,
                        depth_attachment,
                        self.path_texture_view,
                        self.draw_step_scratch.path_steps(),
                        &mut path_mask_cpu_elapsed,
                        &element_blur_layers,
                        backdrop_blur_source_texture_view,
                        &backdrop_blur_groups,
                    )?;
                    backdrop_blur_refreshed = graph.requires(GraphPass::BackdropBlur);
                    element_blur_refreshed = graph.requires(GraphPass::ElementLayers);
                    let offscreen_elapsed = offscreen_started.elapsed();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.offscreen_render = offscreen_elapsed;
                    }
                    let offscreen_elapsed_ms = offscreen_elapsed.as_millis();
                    let present_started = Instant::now();
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.frame_prepare_upload =
                            present_started.saturating_duration_since(frame_started);
                    }
                    let presentation_frame = render_main_and_present(
                        device,
                        MainPresentDescriptor {
                            submission_mode,
                            async_capabilities,
                            pending_submissions: &mut self.pending_submissions,
                            frame_resource_index: self.current_frame_resource_index,
                            swapchain: self.swapchain,
                            render_pass: self.render_pass,
                            depth_attachment,
                            damage: present_damage,
                        },
                        self.draw_step_scratch.steps(),
                    )?;
                    let did_present = presentation_frame.is_some();
                    let backend_presentation_timings =
                        presentation_frame.and_then(|frame| frame.timings);
                    if let Some(timing) = presentation_timing.as_mut() {
                        timing.backend_present = present_started.elapsed();
                    }
                    let present_elapsed_ms = present_started.elapsed().as_millis();
                    let total_elapsed_ms = frame_started.elapsed().as_millis();
                    if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                        log::warn!(
                            concat!(
                                "nova-gfx frame stages: backend={} total_ms={} ",
                                "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                                "present_ms={} submission_mode={:?} atlas_uploads={} ",
                                "atlas_bytes={} blur_groups={} element_blur_layers={}"
                            ),
                            backend_label,
                            total_elapsed_ms,
                            buffer_upload_elapsed_ms,
                            atlas_upload_elapsed_ms,
                            offscreen_elapsed_ms,
                            present_elapsed_ms,
                            submission_mode,
                            atlas_stats.upload_count,
                            atlas_stats.uploaded_bytes,
                            backdrop_blur_groups.len(),
                            element_blur_layers.len(),
                        );
                    }
                    Ok((did_present, backend_presentation_timings))
                }
                #[cfg(not(any(
                    all(
                        feature = "nova-gfx-opengl",
                        any(target_os = "windows", target_os = "linux")
                    ),
                    all(feature = "nova-gfx-dx11", target_os = "windows"),
                    all(feature = "nova-gfx-dx12", target_os = "windows"),
                    all(feature = "nova-gfx-metal", target_os = "macos"),
                    all(
                        feature = "nova-gfx-vulkan",
                        any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                    )
                )))]
                NovaBackend::Unavailable => {
                    anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
                }
            };

        let frame_elapsed_ms = frame_started.elapsed().as_millis();
        // OUT_OF_DATE is a normal swapchain transition, not a failed render.
        // The backend already rebuilds its native swapchain on this path.
        // Preserve unsent damage and let the owner schedule the next frame.
        if render_result.as_ref().err().is_some_and(|error| {
            matches!(
                error.downcast_ref::<gfx_core::Error>(),
                Some(gfx_core::Error::SurfaceOutdated)
            )
        }) {
            self.path_mask_residency.invalidate();
            return Ok(false);
        }
        // Resource ownership is observable after the attempt even when a draw
        // fails or presentation is skipped. The backend guard is released here.
        if let Err(error) = &render_result {
            self.path_mask_residency.invalidate();
            log::error!(
                concat!(
                    "nova-gfx frame render failed: backend={} alpha_swapchain={:?} ",
                    "alpha_output={:?} quads={} shadows={} paths={} mono_sprites={} ",
                    "poly_sprites={} underlines={} draw_steps={} path_mask_steps={} ",
                    "upload_bytes={} elapsed_ms={} error={:#}"
                ),
                backend_label,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_step_count,
                path_mask_step_count,
                uploaded_bytes,
                frame_elapsed_ms,
                error,
            );
        }
        let (did_present, backend_presentation_timings) = render_result?;
        if !did_present {
            self.path_mask_residency.invalidate();
            return Ok(false);
        }
        if path_mask_step_count != 0 {
            self.path_mask_residency.commit(path_mask_key);
            crate::diagnostics::performance_metrics::record_window_path_mask(
                packet.window_id,
                render_path_mask,
                path_mask_cpu_elapsed,
            );
        }
        let presentation_finished_at = Instant::now();
        // A composition swapchain may still be stretching the last old-size frame while its
        // buffers are rebuilt. Only publish the identity transform after the first new-size
        // frame has been successfully presented; resetting it during ResizeBuffers exposes
        // undefined backbuffer contents as black client-area margins.
        self.reset_live_resize_stretch();
        self.retained_upload
            .mark_uploaded(self.current_frame_resource_index);
        if has_backdrop_blurs {
            self.filters.last_gpu_animation_values.clear();
            self.filters
                .last_gpu_animation_values
                .extend_from_slice(&self.frame_upload.sampled_animation_values);
            self.filters.record_element_blur_inputs(
                &self.frame_upload.element_blur_inputs,
                &packet.presentation_animation_values,
                element_blur_layers.iter().map(|layer| layer.index),
            );
            self.filters
                .record_submission(backdrop_blur_quality, atlas_content_generation);
        } else {
            self.invalidate_backdrop_blur_cache();
        }
        self.swapchain_warmup_frames = self.swapchain_warmup_frames.saturating_sub(1);
        if self.frame_upload.retained_root_blur.is_some() {
            let pixels = self.drawable_pixels();
            crate::diagnostics::performance_metrics::record_retained_present(
                pixels,
                pixels.saturating_mul(self.surface_format.bytes_per_pixel() as usize),
            );
        } else {
            crate::diagnostics::performance_metrics::record_direct_present();
        }
        crate::diagnostics::performance_metrics::record_backdrop_blur_frame(
            blur_source_pixels,
            blur_level_pixels,
        );
        crate::diagnostics::performance_metrics::record_present();
        if self.diagnostics.should_log_frame_details() {
            let blur_render_passes = blur_group_pass_count.saturating_add(element_blur_pass_count);
            log::warn!(
                concat!(
                    "nova-gfx copy attribution: backend={} frame={} ",
                    "explicit_copy_source=atlas_texture_upload atlas_texture_regions={} ",
                    "atlas_texture_bytes={} mapped_frame_upload_bytes={} ",
                    "mapped_frame_upload_is_gpu_copy=false retained_present_copy_regions={} ",
                    "path_mask_render_passes={} blur_render_passes={} blur_groups={} ",
                    "element_blur_layers={} backdrop_blur_refresh={} element_blur_refresh={} ",
                    "blur_source_atlas_dirty={} blur_atlas_generation_changed={} ",
                    "blur_source_atlas_textures={} blur_source_pixels={} ",
                    "blur_horizontal_pixels={} blur_final_pixels={} blur_target_pixels={} ",
                    "blur_full_target_pixels={} ",
                    "blur_source_mode=damage-local-retained-filter main_render_passes=1 present_damage={:?} ",
                    "dirty_mode={:?} dirty_full={} dirty_rects={} dirty_area={}"
                ),
                backend_label,
                self.submitted_frames.saturating_add(1),
                atlas_texture_region_count,
                atlas_texture_upload_bytes,
                mapped_upload_bytes,
                usize::from(present_damage.is_some()),
                mask_pass_count,
                blur_render_passes,
                backdrop_blur_groups.len(),
                element_blur_layers.len(),
                backdrop_blur_refreshed,
                element_blur_refreshed,
                backdrop_source_atlas_dirty,
                atlas_generation_changed,
                backdrop_source_atlas_textures.len(),
                blur_source_pixels,
                blur_level_pixels[0],
                blur_level_pixels[1],
                blur_target_pixels,
                blur_full_target_pixels,
                present_damage,
                packet.partial_present_mode,
                packet.dirty_region.is_full(),
                packet.dirty_region.rect_count(),
                packet.dirty_region.area(),
            );
        }
        if self.diagnostics.should_warn_slow_frame(frame_elapsed_ms) {
            log::warn!(
                concat!(
                    "nova-gfx frame completed: backend={} elapsed_ms={} ",
                    "alpha_swapchain={:?} alpha_output={:?} quads={} shadows={} paths={} ",
                    "mono_sprites={} poly_sprites={} underlines={} draw_steps={} ",
                    "path_mask_steps={} gpu_passes={} upload_bytes={}"
                ),
                backend_label,
                frame_elapsed_ms,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_step_count,
                path_mask_step_count,
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
                uploaded_bytes,
            );
        }
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        if !self.first_frame_reported {
            self.first_frame_reported = true;
            log::info!(
                "GPUI nova-gfx first frame: renderer_path=nova-gfx phase=path-offscreen first_frame_time_ms={} submitted_frames={} quads={} paths={} mono_sprites={} thread={:?}",
                self.metrics_started_at.elapsed().as_millis(),
                self.submitted_frames,
                upload.quad_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                std::thread::current().id()
            );
        }
        if packet.sampled_active_presentation_animation {
            if let Some(timing) = presentation_timing.as_mut() {
                timing.renderer_post_present = presentation_finished_at.elapsed();
            }
            crate::diagnostics::performance_metrics::record_presentation_animation_sample(
                packet.window_id,
                &packet.presentation_animation_values,
                *presentation_timing,
                backend_presentation_timings,
            );
        }
        let _ = (self.surface, self.atlas_sampler, self.path_texture);
        packet.consume_submitted_damage();
        Ok(true)
    }
}

#[cfg(test)]
mod per_layer_damage_tests {
    use super::*;

    #[test]
    fn isolated_source_damage_does_not_depend_on_window_damage() {
        let mut frame = FrameUpload::default();
        frame.blur_content_ranges_cache.push(BlurContentRange {
            index: 7,
            depth: 0,
            content_start: 0,
            content_end: 0,
        });
        let source = crate::PaintBlur {
            order: 0,
            layer_id: None,
            animation_id: None,
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(4.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        frame.element_blur_inputs.push((7, source.clone()));
        let empty = crate::DirtyRegion::empty();
        let mut filters = FilterRegistry::new(None);
        assert_eq!(
            dirty_element_blur_indices(&frame, &empty, &filters, &[], false),
            vec![7],
            "uninitialized layer must render even when window spatial dirty is empty"
        );
        filters.record_element_blur_inputs(&frame.element_blur_inputs, &[], [7]);
        assert!(dirty_element_blur_indices(&frame, &empty, &filters, &[], false).is_empty());
        frame.element_blur_inputs[0].1.radius = crate::ScaledPixels(5.0);
        assert_eq!(
            dirty_element_blur_indices(&frame, &empty, &filters, &[], false),
            vec![7],
            "changed source invalidates only its layer, independently of window dirty"
        );
    }
}

#[cfg(test)]
mod root_cached_source_pass_tests {
    use super::source_pass_required;

    #[test]
    fn retained_root_without_new_pixels_does_not_submit_an_empty_load_pass() {
        assert!(!source_pass_required(true, true, false));
        assert!(!source_pass_required(false, true, false));
        assert!(source_pass_required(true, true, true));
        assert!(source_pass_required(true, false, false));
    }
}
