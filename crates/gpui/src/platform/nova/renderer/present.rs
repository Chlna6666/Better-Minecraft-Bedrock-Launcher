use super::draw_steps::{PreparedBackdropBlurGroup, PreparedElementBlurLayer};
use super::*;

#[derive(Clone, Copy)]
struct FrameBufferTargets {
    global: BufferId,
    text_raster: BufferId,
    quad: BufferId,
    shadow: BufferId,
    path_rasterization_vertex: BufferId,
    path_sprite: BufferId,
    mono_sprite: BufferId,
    poly_sprite: BufferId,
    underline: BufferId,
    backdrop_blur_pass: BufferId,
    backdrop_blur: BufferId,
    custom_mesh_3d_parameters: BufferId,
}

impl NovaRenderer {
    fn frame_buffer_targets(&self) -> FrameBufferTargets {
        FrameBufferTargets {
            global: self.global_buffer,
            text_raster: self.text_raster_buffer,
            quad: self.quad_buffer,
            shadow: self.shadow_buffer,
            path_rasterization_vertex: self.path_rasterization_vertex_buffer,
            path_sprite: self.path_sprite_buffer,
            mono_sprite: self.mono_sprite_buffer,
            poly_sprite: self.poly_sprite_buffer,
            underline: self.underline_buffer,
            backdrop_blur_pass: self.backdrop_blur_pass_buffer,
            backdrop_blur: self.backdrop_blur_buffer,
            custom_mesh_3d_parameters: self.custom_mesh_3d_parameters_buffer,
        }
    }
}

fn upload_frame_buffers<D>(
    device: &mut D,
    buffers: FrameBufferTargets,
    frame_upload: &FrameUpload,
    has_backdrop_blurs: bool,
    upload_static: bool,
) -> Result<()>
where
    D: BackendResources,
{
    if !upload_static {
        return upload_animated_buffers(device, buffers, frame_upload);
    }
    device.write_buffer(buffers.global, 0, &frame_upload.globals)?;
    device.write_buffer(buffers.text_raster, 0, &frame_upload.text_raster_params)?;
    if !frame_upload.quads.is_empty() {
        device.write_buffer(buffers.quad, 0, &frame_upload.quads)?;
    }
    if !frame_upload.shadows.is_empty() {
        device.write_buffer(buffers.shadow, 0, &frame_upload.shadows)?;
    }
    if !frame_upload.path_rasterization_vertices.is_empty() {
        device.write_buffer(
            buffers.path_rasterization_vertex,
            0,
            &frame_upload.path_rasterization_vertices,
        )?;
    }
    if !frame_upload.path_sprites.is_empty() {
        device.write_buffer(buffers.path_sprite, 0, &frame_upload.path_sprites)?;
    }
    if !frame_upload.mono_sprites.is_empty() {
        device.write_buffer(buffers.mono_sprite, 0, &frame_upload.mono_sprites)?;
    }
    if !frame_upload.poly_sprites.is_empty() {
        device.write_buffer(buffers.poly_sprite, 0, &frame_upload.poly_sprites)?;
    }
    if !frame_upload.underlines.is_empty() {
        device.write_buffer(buffers.underline, 0, &frame_upload.underlines)?;
    }
    if has_backdrop_blurs {
        device.write_buffer(
            buffers.backdrop_blur_pass,
            0,
            &frame_upload.backdrop_blur_passes,
        )?;
        device.write_buffer(buffers.backdrop_blur, 0, &frame_upload.backdrop_blurs)?;
    }
    if !frame_upload.custom_mesh_3d_parameters.is_empty() {
        device.write_buffer(
            buffers.custom_mesh_3d_parameters,
            0,
            &frame_upload.custom_mesh_3d_parameters,
        )?;
    }
    Ok(())
}

fn upload_animated_buffers<D: BackendResources>(
    device: &mut D,
    buffers: FrameBufferTargets,
    frame_upload: &FrameUpload,
) -> Result<()> {
    // Animation metadata is CPU input to materialization. Current Nova shaders
    // only consume the patched primitive buffers; uploading metadata does no work.
    if frame_upload.has_animated_backdrop_blurs() {
        device.write_buffer(
            buffers.backdrop_blur_pass,
            0,
            &frame_upload.backdrop_blur_passes,
        )?;
    }
    for primitive in &frame_upload.animated_primitives {
        let buffer = match primitive.kind {
            AnimatedPrimitiveKind::Quad => buffers.quad,
            AnimatedPrimitiveKind::Shadow => buffers.shadow,
            AnimatedPrimitiveKind::MonochromeSprite => buffers.mono_sprite,
            AnimatedPrimitiveKind::PolychromeSprite => buffers.poly_sprite,
            AnimatedPrimitiveKind::BackdropBlur => buffers.backdrop_blur,
        };
        device.write_buffer(buffer, primitive.offset(), &primitive.bytes)?;
    }
    Ok(())
}

struct MainPresentDescriptor<'a> {
    submission_mode: GpuSubmissionMode,
    async_capabilities: BackendAsyncCapabilities,
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
) -> Result<()>
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

/// Advances one shared scene-color source through each backdrop draw-order barrier.
///
/// The first segment clears scene color and depth. Later segments load both, preserving 3D
/// occlusion as well as color across backdrop barriers. Gaussian filter pipelines have depth
/// disabled, but still bind the shared depth view with `LoadOp::Load` because the renderer's
/// render-pass contract requires a depth attachment whenever the pass declares a depth format.
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
        device.render_steps_to_texture(
            source_texture_view,
            render_pass,
            &group.source_steps,
            source_load_op,
            Some(source_depth_attachment),
        )?;
        let filter_depth_attachment = RenderPassDepthAttachment {
            target: depth_attachment.target,
            depth_load_op: LoadOp::Load,
        };
        for pass in &group.filter_passes {
            device.render_step_list_to_texture(
                pass.target_texture_view,
                render_pass,
                RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                LoadOp::Clear(clear_color()),
                Some(filter_depth_attachment),
            )?;
        }
    }
    Ok(())
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
            device.render_steps_to_texture(
                layer.source_texture_view,
                render_pass,
                &group.source_steps,
                if group_index == 0 {
                    LoadOp::Clear(clear_color())
                } else {
                    LoadOp::Load
                },
                Some(source_depth_attachment),
            )?;
            for pass in &group.filter_passes {
                device.render_step_list_to_texture(
                    pass.target_texture_view,
                    render_pass,
                    RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                    LoadOp::Clear(clear_color()),
                    Some(filter_depth_attachment),
                )?;
            }
        }
        for pass in &layer.filter_passes {
            device.render_step_list_to_texture(
                pass.target_texture_view,
                render_pass,
                RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                LoadOp::Clear(clear_color()),
                Some(filter_depth_attachment),
            )?;
        }
    }
    Ok(())
}

/// Returns whether the flattened upload contains a backdrop filter outside an element-blur
/// capture. Nested backdrop filters belong to their element layer and must not make the root
/// backdrop cache miss every time that layer exists.
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
            | UploadedBatch::CustomMesh3d { .. } => {}
        }
    }
    false
}

/// Determines which isolated element-blur targets actually depend on this frame's damage.
///
/// `PaintBlur.bounds` is packed as the effect bounds (content plus Gaussian support), so testing
/// against it keeps unrelated animation outside the filtered subtree from invalidating the layer.
/// A shared target/layout invalidation forces every element layer to rebuild once.
fn dirty_element_blur_indices(
    frame_upload: &FrameUpload,
    dirty_region: &crate::DirtyRegion,
    force_all: bool,
) -> Vec<u32> {
    let ranges = frame_upload.blur_content_ranges();
    if ranges.is_empty() {
        return Vec::new();
    }
    if force_all || dirty_region.is_full() {
        return ranges.into_iter().map(|range| range.index).collect();
    }
    if dirty_region.is_empty() {
        return Vec::new();
    }

    let mut dirty = Vec::new();
    for range in ranges {
        let Some(config) = frame_upload.backdrop_blur_config_for_index(range.index) else {
            // Missing metadata is not a safe cache hit. Re-render this layer and let the normal
            // preparation path decide whether a renderable target exists.
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

impl NovaRenderer {
    fn drawable_pixels(&self) -> usize {
        (self.current_size.width as usize).saturating_mul(self.current_size.height as usize)
    }

    fn backdrop_blur_pixel_metrics(
        &self,
        enabled: bool,
        source_group_count: usize,
    ) -> (usize, [usize; 6]) {
        if !enabled {
            return (0, [0; 6]);
        }
        let source_pixels = if source_group_count == 0 {
            0
        } else {
            self.drawable_pixels()
        };
        let mut level_pixels = [0usize; 6];
        let source_width = self.current_size.width as usize;
        let source_height = self.current_size.height as usize;
        for &config in self.frame_upload.backdrop_blur_configs() {
            let factor = usize::from(config.downsample().max(1));
            let filtered_width = source_width.div_ceil(factor).max(1);
            let filtered_height = source_height.div_ceil(factor).max(1);
            level_pixels[0] =
                level_pixels[0].saturating_add(filtered_width.saturating_mul(source_height));
            level_pixels[1] =
                level_pixels[1].saturating_add(filtered_width.saturating_mul(filtered_height));
        }
        (source_pixels, level_pixels)
    }

    pub(super) fn draw_present(
        &mut self,
        upload: FrameUploadSummary,
        render_plan: FrameRenderPlan<'_>,
        backdrop_blur_quality: BackdropBlurQuality,
    ) -> Result<()> {
        self.prepare_for_frame_submission()?;
        if self.atlas.has_pending_removals() {
            self.wait_for_pending_submissions()?;
            self.atlas.apply_pending_removals();
        }
        self.sync_atlas_textures_for_current_backend()?;
        self.ensure_custom_mesh_3d_cache_for_current_backend()?;
        let frame_started = Instant::now();
        let backend_label = self.backend.label();
        let async_capabilities = self.backend.async_capabilities();
        let native_partial_presentation =
            self.backend.supports_partial_presentation(self.swapchain);
        let submission_mode = self.presentation_submission_mode();
        let has_backdrop_blurs = self.has_backdrop_blurs();
        let has_root_backdrop_blurs = has_root_backdrop_blurs(&self.frame_upload);
        let has_element_blurs = self.frame_upload.has_element_blurs();

        let backdrop_source_atlas_textures = if has_backdrop_blurs {
            self.frame_upload.backdrop_source_atlas_texture_ids()
        } else {
            Default::default()
        };
        let atlas_content_generation = self.atlas.content_generation();
        let backdrop_source_atlas_dirty = has_backdrop_blurs
            && (self.backdrop_blur_cache_atlas_generation != atlas_content_generation
                || self
                    .atlas
                    .pending_uploads_touch_any(&backdrop_source_atlas_textures));
        // Target allocation/quality and source-atlas changes invalidate both cache families. Their
        // ordinary frame-to-frame dirtiness is independent below: an element blur existing is no
        // longer sufficient to rebuild the root backdrop source.
        let shared_blur_cache_invalid = has_backdrop_blurs
            && (!self.backdrop_blur_cache_valid
                || backdrop_source_atlas_dirty
                || self.backdrop_blur_cache_quality != Some(backdrop_blur_quality));
        let backdrop_blur_refresh_required = has_root_backdrop_blurs
            && (render_plan.backdrop_blur_refresh_required || shared_blur_cache_invalid);
        let dirty_element_indices = dirty_element_blur_indices(
            &self.frame_upload,
            render_plan.dirty_region,
            shared_blur_cache_invalid,
        );
        let element_blur_refresh_required = has_element_blurs && !dirty_element_indices.is_empty();
        if backdrop_blur_refresh_required || element_blur_refresh_required {
            self.backdrop_blur_cache_valid = false;
        }
        let present_damage = (native_partial_presentation
            && upload.unsupported_batches.total() == 0)
            .then(|| partial_scissor_for_plan(render_plan, self.current_size))
            .flatten();
        // Native dirty-rect presentation makes the OS preserve every pixel outside the
        // dirty rects with copies, so large damage regions turn each present into a
        // full-surface composition copy. Fall back to a full present once the damaged
        // area stops being a minority of the surface, which is what typically happens
        // during window-level animations and live resizes.
        let present_damage = present_damage.filter(|damage| {
            let drawable_pixels = self.drawable_pixels();
            drawable_pixels == 0
                || u64::from(damage.width)
                    * u64::from(damage.height)
                    * PARTIAL_PRESENT_MAX_DAMAGE_AREA_RECIPROCAL
                    <= drawable_pixels as u64
        });
        if present_damage.is_some() {
            crate::diagnostics::performance_metrics::record_partial_redraw();
        } else if render_plan.partial_present_mode == PartialPresentMode::Partial {
            crate::diagnostics::performance_metrics::record_full_redraw_fallback();
        }

        self.prepare_draw_steps();
        self.prepare_path_mask_draw_steps();
        self.prepare_backdrop_blur_passes(has_backdrop_blurs);
        let backdrop_blur_groups = if backdrop_blur_refresh_required {
            self.prepare_backdrop_blur_groups(true)
        } else {
            Vec::new()
        };
        let mut element_blur_layers = if element_blur_refresh_required {
            self.prepare_element_blur_layers(true)
        } else {
            Vec::new()
        };
        if element_blur_refresh_required && !shared_blur_cache_invalid {
            element_blur_layers.retain(|layer| dirty_element_indices.contains(&layer.index));
        }
        let draw_step_count = self.draw_step_scratch.draw_steps.len();
        let path_mask_step_count = self.draw_step_scratch.path_mask_steps.len();
        let mask_pass_count = usize::from(path_mask_step_count != 0);
        let main_pass_count = 1;
        let backdrop_blur_refreshed;
        let element_blur_refreshed;
        let blur_group_pass_count = backdrop_blur_groups.iter().fold(0usize, |total, group| {
            total.saturating_add(1usize.saturating_add(group.filter_passes.len()))
        });
        let element_blur_pass_count = element_blur_layers.iter().fold(0usize, |total, layer| {
            let source_passes = layer.source_groups.iter().fold(0usize, |total, group| {
                total.saturating_add(1usize.saturating_add(group.filter_passes.len()))
            });
            total
                .saturating_add(source_passes)
                .saturating_add(layer.filter_passes.len())
        });
        let composite_pass_count =
            blur_group_pass_count.saturating_add(element_blur_pass_count);
        crate::diagnostics::performance_metrics::record_gpu_pass_metrics(
            mask_pass_count,
            main_pass_count,
            composite_pass_count,
        );

        let unsupported = upload.unsupported_batches;
        let upload_static = self
            .retained_upload
            .needs_static_upload(self.current_frame_resource_index);
        let animated_upload_bytes = self.frame_upload.animated_upload_bytes();
        let mapped_upload_bytes = if upload_static {
            self.frame_upload.mapped_upload_bytes(has_backdrop_blurs)
        } else {
            animated_upload_bytes
        };
        let uploaded_bytes = mapped_upload_bytes;
        let breakdown = if upload_static {
            let mut breakdown = self.frame_upload.upload_breakdown();
            // Metadata is CPU-only and animated primitives are already counted in
            // their full static buffers on the first upload for each frame slot.
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
                    "gpu_meshes_3d={} set GPUI_NOVA_RENDER_DIAGNOSTICS=1 for every-frame details"
                ),
                backend_label,
                unsupported.paths,
                unsupported.surfaces,
                unsupported.backdrop_blurs,
                unsupported.backdrop_blur_tint_fallbacks,
                unsupported.gpu_meshes_3d,
            );
        }
        if self.diagnostics.enabled {
            log::warn!(
                concat!(
                    "nova-gfx frame diagnostics: backend={} alpha_swapchain={:?} ",
                    "alpha_output={:?} premultiplied={} quads={} shadows={} paths={} ",
                    "path_vertices={} mono_sprites={} poly_sprites={} underlines={} ",
                    "draw_steps={} path_mask_steps={} gpu_passes={} upload_bytes={} ",
                    "async_submission={} async_wait={} async_presentation={} ",
                    "async_partial_presentation={} native_partial_presentation={} ",
                    "present_damage={:?} dirty_mode={:?} dirty_full={} dirty_rects={} ",
                    "dirty_area={} backdrop_blur_refresh={} element_blur_refresh={} ",
                    "element_blur_dirty_layers={} blur_source_atlas_dirty={} ",
                    "blur_source_atlas_textures={} blur_groups={} animation_bindings={} ",
                    "animation_values={} threading={:?}"
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
                path_mask_step_count,
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
                uploaded_bytes,
                async_capabilities.async_submission,
                async_capabilities.async_wait,
                async_capabilities.async_presentation,
                async_capabilities.partial_presentation,
                native_partial_presentation,
                present_damage,
                render_plan.partial_present_mode,
                render_plan.dirty_region.is_full(),
                render_plan.dirty_region.rect_count(),
                render_plan.dirty_region.area(),
                backdrop_blur_refresh_required,
                element_blur_refresh_required,
                dirty_element_indices.len(),
                backdrop_source_atlas_dirty,
                backdrop_source_atlas_textures.len(),
                backdrop_blur_groups.len(),
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
                self.backdrop_blur_targets
                    .as_ref()
                    .context("missing nova backdrop blur targets")?
                    .source
                    .texture_view,
            )
        } else {
            None
        };
        let mesh_upload_bytes = self.custom_mesh_3d_uploaded_bytes_this_frame;
        let mesh_retained_bytes = self.custom_mesh_3d_retained_bytes();
        let mesh_buffer_count = self.custom_mesh_3d_buffer_count();
        let atlas_texture_region_count: usize;
        let atlas_texture_upload_bytes: usize;

        let render_result: Result<()> = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let upload_started = Instant::now();
                upload_frame_buffers(
                    device,
                    frame_buffers,
                    &self.frame_upload,
                    has_backdrop_blurs,
                    upload_static,
                )?;
                let buffer_upload_elapsed_ms = upload_started.elapsed().as_millis();
                let atlas_started = Instant::now();
                let atlas_stats = upload_pending_atlas(&self.atlas, device, |atlas_id| {
                    self.gpu_atlas_textures
                        .get(&atlas_id)
                        .map(|texture| texture.texture)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "missing nova atlas texture {:?}/{}",
                                atlas_id.kind,
                                atlas_id.index
                            )
                        })
                })?;
                let atlas_upload_elapsed_ms = atlas_started.elapsed().as_millis();
                atlas_texture_region_count = atlas_stats.upload_count;
                atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                record_nova_upload_metrics(
                    (mapped_upload_bytes, self.frame_upload.uploaded_bytes()),
                    mesh_upload_bytes,
                    mesh_retained_bytes,
                    mesh_buffer_count,
                    atlas_stats,
                );
                let offscreen_started = Instant::now();
                if path_mask_step_count != 0 {
                    device.render_step_list_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        RenderStepList::from_draw_steps(&self.draw_step_scratch.path_mask_steps),
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                }
                backdrop_blur_refreshed = backdrop_blur_refresh_required;
                element_blur_refreshed = element_blur_refresh_required;
                if element_blur_refresh_required {
                    render_element_blur_layers(
                        device,
                        self.render_pass,
                        depth_attachment,
                        &element_blur_layers,
                    )?;
                }
                if backdrop_blur_refresh_required
                    && let Some(source_texture_view) = backdrop_blur_source_texture_view
                {
                    render_backdrop_blur_groups(
                        device,
                        source_texture_view,
                        self.render_pass,
                        depth_attachment,
                        &backdrop_blur_groups,
                    )?;
                }
                let offscreen_elapsed_ms = offscreen_started.elapsed().as_millis();
                let present_started = Instant::now();
                render_main_and_present(
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
                    &self.draw_step_scratch.draw_steps,
                )?;
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
                Ok(())
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                upload_frame_buffers(
                    device,
                    frame_buffers,
                    &self.frame_upload,
                    has_backdrop_blurs,
                    upload_static,
                )?;
                let atlas_stats = upload_pending_atlas(&self.atlas, device, |atlas_id| {
                    self.gpu_atlas_textures
                        .get(&atlas_id)
                        .map(|texture| texture.texture)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "missing nova atlas texture {:?}/{}",
                                atlas_id.kind,
                                atlas_id.index
                            )
                        })
                })?;
                atlas_texture_region_count = atlas_stats.upload_count;
                atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                record_nova_upload_metrics(
                    (mapped_upload_bytes, self.frame_upload.uploaded_bytes()),
                    mesh_upload_bytes,
                    mesh_retained_bytes,
                    mesh_buffer_count,
                    atlas_stats,
                );
                if path_mask_step_count != 0 {
                    device.render_step_list_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        RenderStepList::from_draw_steps(&self.draw_step_scratch.path_mask_steps),
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                }
                backdrop_blur_refreshed = backdrop_blur_refresh_required;
                element_blur_refreshed = element_blur_refresh_required;
                if element_blur_refresh_required {
                    render_element_blur_layers(
                        device,
                        self.render_pass,
                        depth_attachment,
                        &element_blur_layers,
                    )?;
                }
                if backdrop_blur_refresh_required
                    && let Some(source_texture_view) = backdrop_blur_source_texture_view
                {
                    render_backdrop_blur_groups(
                        device,
                        source_texture_view,
                        self.render_pass,
                        depth_attachment,
                        &backdrop_blur_groups,
                    )?;
                }
                render_main_and_present(
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
                    &self.draw_step_scratch.draw_steps,
                )?;
                Ok(())
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                let upload_started = Instant::now();
                upload_frame_buffers(
                    device,
                    frame_buffers,
                    &self.frame_upload,
                    has_backdrop_blurs,
                    upload_static,
                )?;
                let buffer_upload_elapsed_ms = upload_started.elapsed().as_millis();
                let atlas_started = Instant::now();
                let atlas_stats = upload_pending_atlas(&self.atlas, device, |atlas_id| {
                    self.gpu_atlas_textures
                        .get(&atlas_id)
                        .map(|texture| texture.texture)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "missing nova atlas texture {:?}/{}",
                                atlas_id.kind,
                                atlas_id.index
                            )
                        })
                })?;
                let atlas_upload_elapsed_ms = atlas_started.elapsed().as_millis();
                atlas_texture_region_count = atlas_stats.upload_count;
                atlas_texture_upload_bytes = atlas_stats.uploaded_bytes;
                record_nova_upload_metrics(
                    (mapped_upload_bytes, self.frame_upload.uploaded_bytes()),
                    mesh_upload_bytes,
                    mesh_retained_bytes,
                    mesh_buffer_count,
                    atlas_stats,
                );
                let offscreen_started = Instant::now();
                if path_mask_step_count != 0 {
                    device.render_step_list_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        RenderStepList::from_draw_steps(&self.draw_step_scratch.path_mask_steps),
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                }
                backdrop_blur_refreshed = backdrop_blur_refresh_required;
                element_blur_refreshed = element_blur_refresh_required;
                if element_blur_refresh_required {
                    render_element_blur_layers(
                        device,
                        self.render_pass,
                        depth_attachment,
                        &element_blur_layers,
                    )?;
                }
                if backdrop_blur_refresh_required
                    && let Some(source_texture_view) = backdrop_blur_source_texture_view
                {
                    render_backdrop_blur_groups(
                        device,
                        source_texture_view,
                        self.render_pass,
                        depth_attachment,
                        &backdrop_blur_groups,
                    )?;
                }
                let offscreen_elapsed_ms = offscreen_started.elapsed().as_millis();
                let present_started = Instant::now();
                render_main_and_present(
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
                    &self.draw_step_scratch.draw_steps,
                )?;
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
                Ok(())
            }
            #[cfg(not(any(
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
        if let Err(error) = &render_result {
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
        render_result?;
        self.retained_upload
            .mark_uploaded(self.current_frame_resource_index);
        if has_backdrop_blurs {
            self.backdrop_blur_cache_valid = true;
            self.backdrop_blur_cache_quality = Some(backdrop_blur_quality);
            self.backdrop_blur_cache_atlas_generation = atlas_content_generation;
        } else {
            self.invalidate_backdrop_blur_cache();
        }
        self.swapchain_warmup_frames = self.swapchain_warmup_frames.saturating_sub(1);
        crate::diagnostics::performance_metrics::record_direct_present();
        let blur_refreshed = backdrop_blur_refreshed || element_blur_refreshed;
        let (blur_source_pixels, blur_level_pixels) = self.backdrop_blur_pixel_metrics(
            blur_refreshed,
            backdrop_blur_groups
                .len()
                .saturating_add(element_blur_layers.len()),
        );
        crate::diagnostics::performance_metrics::record_backdrop_blur_frame(
            blur_source_pixels,
            blur_level_pixels,
        );
        crate::diagnostics::performance_metrics::record_present();
        if self.diagnostics.should_log_frame_details() {
            let blur_render_passes =
                blur_group_pass_count.saturating_add(element_blur_pass_count);
            log::warn!(
                concat!(
                    "nova-gfx copy attribution: backend={} frame={} ",
                    "explicit_copy_source=atlas_texture_upload atlas_texture_regions={} ",
                    "atlas_texture_bytes={} mapped_frame_upload_bytes={} ",
                    "mapped_frame_upload_is_gpu_copy=false retained_present_copy_regions={} ",
                    "path_mask_render_passes={} blur_render_passes={} blur_groups={} ",
                    "element_blur_layers={} backdrop_blur_refresh={} element_blur_refresh={} ",
                    "blur_source_atlas_dirty={} blur_source_atlas_textures={} ",
                    "blur_source_mode=sequential-segments main_render_passes=1 present_damage={:?} ",
                    "dirty_mode={:?} dirty_full={} dirty_rects={} dirty_area={}"
                ),
                backend_label,
                self.submitted_frames.saturating_add(1),
                atlas_texture_region_count,
                atlas_texture_upload_bytes,
                mapped_upload_bytes.saturating_add(mesh_upload_bytes),
                usize::from(present_damage.is_some()),
                mask_pass_count,
                blur_render_passes,
                backdrop_blur_groups.len(),
                element_blur_layers.len(),
                backdrop_blur_refreshed,
                element_blur_refreshed,
                backdrop_source_atlas_dirty,
                backdrop_source_atlas_textures.len(),
                present_damage,
                render_plan.partial_present_mode,
                render_plan.dirty_region.is_full(),
                render_plan.dirty_region.rect_count(),
                render_plan.dirty_region.area(),
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
                "GPUI nova-gfx first frame: renderer_path=nova-gfx phase=path-offscreen first_frame_time_ms={} submitted_frames={} quads={} paths={} mono_sprites={}",
                self.metrics_started_at.elapsed().as_millis(),
                self.submitted_frames,
                upload.quad_count,
                upload.path_sprite_count,
                upload.mono_sprite_count
            );
        }
        let _ = (self.surface, self.atlas_sampler, self.path_texture);
        Ok(())
    }
}
