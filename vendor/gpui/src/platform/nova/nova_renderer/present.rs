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
    animation_binding: BufferId,
    animation_value: BufferId,
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
            animation_binding: self.animation_binding_buffer,
            animation_value: self.animation_value_buffer,
            custom_mesh_3d_parameters: self.custom_mesh_3d_parameters_buffer,
        }
    }
}

fn upload_frame_buffers<D>(
    device: &mut D,
    buffers: FrameBufferTargets,
    frame_upload: &NovaFrameUpload,
    has_backdrop_blurs: bool,
) -> Result<()>
where
    D: BackendResources,
{
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
    if !frame_upload.animation_bindings.is_empty() {
        device.write_buffer(
            buffers.animation_binding,
            0,
            &frame_upload.animation_bindings,
        )?;
    }
    if !frame_upload.animation_values.is_empty() {
        device.write_buffer(buffers.animation_value, 0, &frame_upload.animation_values)?;
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

fn partial_present_scissor(
    render_plan: FrameRenderPlan<'_>,
    current_size: DrawableSize,
    unsupported_batches: UnsupportedBatchSummary,
    has_backdrop_blurs: bool,
) -> Option<ScissorRect> {
    if unsupported_batches.total() != 0 || has_backdrop_blurs {
        return None;
    }
    partial_scissor_for_plan(render_plan, current_size)
}

fn present_cache_load_op(partial_scissor: Option<ScissorRect>) -> LoadOp<ClearColor> {
    if partial_scissor.is_some() {
        LoadOp::Load
    } else {
        LoadOp::Clear(clear_color())
    }
}

fn write_present_copy_sprite_buffer<D>(
    device: &mut D,
    buffer: BufferId,
    current_size: DrawableSize,
) -> Result<()>
where
    D: BackendResources,
{
    let mut bytes = Vec::with_capacity(PACKED_POLY_SPRITE_BYTES);
    write_polychrome_sprite(&mut bytes, &present_copy_sprite(current_size));
    device.write_buffer(buffer, 0, &bytes)?;
    Ok(())
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "drawable size comes from a validated swapchain extent and is clamped by gpui viewport limits"
)]
fn present_copy_sprite(current_size: DrawableSize) -> PolychromeSprite {
    let width = current_size.width as i32;
    let height = current_size.height as i32;
    let bounds = Bounds {
        origin: Point {
            x: crate::ScaledPixels(0.0),
            y: crate::ScaledPixels(0.0),
        },
        size: Size {
            width: crate::ScaledPixels(current_size.width as f32),
            height: crate::ScaledPixels(current_size.height as f32),
        },
    };
    PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false,
        opacity: 1.0,
        animation_id: None,
        bounds,
        content_mask: crate::ContentMask { bounds },
        corner_radii: Default::default(),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Bgra,
            },
            tile_id: crate::TileId(0),
            padding: 0,
            bounds: Bounds {
                origin: Point {
                    x: DevicePixels(0),
                    y: DevicePixels(0),
                },
                size: Size {
                    width: DevicePixels(width),
                    height: DevicePixels(height),
                },
            },
        },
    }
}

struct MainPresentDescriptor<'a> {
    submission_mode: GpuSubmissionMode,
    async_capabilities: BackendAsyncCapabilities,
    pending_submissions: &'a mut Vec<SubmissionId>,
    swapchain: SwapchainId,
    render_pass: RenderPassId,
    present_cache_texture_view: TextureViewId,
    present_copy_sprite_buffer: BufferId,
    current_size: DrawableSize,
    depth_attachment: RenderPassDepthAttachment,
    use_retained_present: bool,
    present_cache_load_op: LoadOp<ClearColor>,
}

fn render_main_and_present<D>(
    device: &mut D,
    descriptor: MainPresentDescriptor<'_>,
    draw_steps: &[RenderStepDescriptor],
    present_copy_steps: &[RenderStepDescriptor],
) -> Result<()>
where
    D: BackendPresentationCompat + BackendQueue + BackendResources,
{
    if descriptor.use_retained_present {
        device.render_steps_to_texture(
            descriptor.present_cache_texture_view,
            descriptor.render_pass,
            draw_steps,
            descriptor.present_cache_load_op,
            Some(descriptor.depth_attachment),
        )?;
        write_present_copy_sprite_buffer(
            device,
            descriptor.present_copy_sprite_buffer,
            descriptor.current_size,
        )?;
        NovaRenderer::submit_present_frame(
            descriptor.submission_mode,
            descriptor.async_capabilities,
            descriptor.pending_submissions,
            device,
            descriptor.swapchain,
            descriptor.render_pass,
            present_copy_steps,
            clear_color(),
            Some(descriptor.depth_attachment),
        )?;
    } else {
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
        )?;
    }
    Ok(())
}

impl NovaRenderer {
    pub(super) fn draw_present(
        &mut self,
        upload: FrameUploadSummary,
        render_plan: FrameRenderPlan<'_>,
    ) -> Result<()> {
        self.prepare_for_frame_submission()?;
        self.sync_atlas_textures_for_current_backend()?;
        self.ensure_custom_mesh_3d_cache_for_current_backend()?;
        let frame_started = Instant::now();
        let backend_label = self.backend.label();
        let async_capabilities = self.backend.async_capabilities();
        let submission_mode = self.presentation_submission_mode();
        let has_backdrop_blurs = self.has_backdrop_blurs();
        let requested_partial_scissor = partial_present_scissor(
            render_plan,
            self.current_size,
            upload.unsupported_batches,
            has_backdrop_blurs,
        );
        let use_retained_present = requested_partial_scissor.is_some();
        let partial_scissor = if self.present_cache_valid {
            requested_partial_scissor
        } else {
            None
        };
        self.prepare_draw_steps(partial_scissor);
        self.prepare_present_copy_steps(use_retained_present);
        self.prepare_path_mask_draw_steps();
        self.prepare_backdrop_blur_source_steps(has_backdrop_blurs);
        let backdrop_blur_passes = if has_backdrop_blurs {
            self.backdrop_blur_render_passes()
        } else {
            Vec::new()
        };
        let draw_step_count = self.draw_step_scratch.draw_steps.len();
        let path_mask_step_count = self.draw_step_scratch.path_mask_steps.len();
        let mask_pass_count = usize::from(path_mask_step_count != 0);
        let main_pass_count = 1 + usize::from(use_retained_present);
        let composite_pass_count =
            usize::from(has_backdrop_blurs).saturating_add(backdrop_blur_passes.len());
        crate::diagnostics::performance_metrics::record_gpu_pass_metrics(
            mask_pass_count,
            main_pass_count,
            composite_pass_count,
        );
        let unsupported = upload.unsupported_batches;
        let uploaded_bytes = self.frame_upload.uploaded_bytes();
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
                    "partial_presentation={} retained_partial_present={} ",
                    "present_cache_valid={} threading={:?}"
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
                use_retained_present,
                self.present_cache_valid,
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
        let mut backdrop_blur_elapsed = None;
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
        let retained_cache_load_op = present_cache_load_op(partial_scissor);
        let render_result: Result<()> = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let upload_started = Instant::now();
                upload_frame_buffers(
                    device,
                    frame_buffers,
                    &self.frame_upload,
                    has_backdrop_blurs,
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
                record_nova_upload_metrics(
                    self.frame_upload.uploaded_bytes(),
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
                if let Some(source_texture_view) = backdrop_blur_source_texture_view {
                    let backdrop_blur_started = Instant::now();
                    device.render_steps_to_texture(
                        source_texture_view,
                        self.render_pass,
                        &self.draw_step_scratch.backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.render_step_list_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                            LoadOp::Clear(clear_color()),
                            Some(depth_attachment),
                        )?;
                    }
                    backdrop_blur_elapsed = Some(backdrop_blur_started.elapsed());
                }
                let offscreen_elapsed_ms = offscreen_started.elapsed().as_millis();
                let present_started = Instant::now();
                render_main_and_present(
                    device,
                    MainPresentDescriptor {
                        submission_mode,
                        async_capabilities,
                        pending_submissions: &mut self.pending_submissions,
                        swapchain: self.swapchain,
                        render_pass: self.render_pass,
                        present_cache_texture_view: self.present_cache_texture_view,
                        present_copy_sprite_buffer: self.present_copy_sprite_buffer,
                        current_size: self.current_size,
                        depth_attachment,
                        use_retained_present,
                        present_cache_load_op: retained_cache_load_op,
                    },
                    &self.draw_step_scratch.draw_steps,
                    &self.draw_step_scratch.present_copy_steps,
                )?;
                let present_elapsed_ms = present_started.elapsed().as_millis();
                let total_elapsed_ms = frame_started.elapsed().as_millis();
                if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                    log::warn!(
                        concat!(
                            "nova-gfx frame stages: backend={} total_ms={} ",
                            "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                            "present_ms={} submission_mode={:?} atlas_uploads={} ",
                            "atlas_bytes={}"
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
                record_nova_upload_metrics(
                    self.frame_upload.uploaded_bytes(),
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
                if let Some(source_texture_view) = backdrop_blur_source_texture_view {
                    let backdrop_blur_started = Instant::now();
                    device.render_steps_to_texture(
                        source_texture_view,
                        self.render_pass,
                        &self.draw_step_scratch.backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.render_step_list_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                            LoadOp::Clear(clear_color()),
                            Some(depth_attachment),
                        )?;
                    }
                    backdrop_blur_elapsed = Some(backdrop_blur_started.elapsed());
                }
                render_main_and_present(
                    device,
                    MainPresentDescriptor {
                        submission_mode,
                        async_capabilities,
                        pending_submissions: &mut self.pending_submissions,
                        swapchain: self.swapchain,
                        render_pass: self.render_pass,
                        present_cache_texture_view: self.present_cache_texture_view,
                        present_copy_sprite_buffer: self.present_copy_sprite_buffer,
                        current_size: self.current_size,
                        depth_attachment,
                        use_retained_present,
                        present_cache_load_op: retained_cache_load_op,
                    },
                    &self.draw_step_scratch.draw_steps,
                    &self.draw_step_scratch.present_copy_steps,
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
                record_nova_upload_metrics(
                    self.frame_upload.uploaded_bytes(),
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
                if let Some(source_texture_view) = backdrop_blur_source_texture_view {
                    let backdrop_blur_started = Instant::now();
                    device.render_steps_to_texture(
                        source_texture_view,
                        self.render_pass,
                        &self.draw_step_scratch.backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                        Some(depth_attachment),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.render_step_list_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            RenderStepList::from_draw_steps(std::slice::from_ref(&pass.step)),
                            LoadOp::Clear(clear_color()),
                            Some(depth_attachment),
                        )?;
                    }
                    backdrop_blur_elapsed = Some(backdrop_blur_started.elapsed());
                }
                let offscreen_elapsed_ms = offscreen_started.elapsed().as_millis();
                let present_started = Instant::now();
                render_main_and_present(
                    device,
                    MainPresentDescriptor {
                        submission_mode,
                        async_capabilities,
                        pending_submissions: &mut self.pending_submissions,
                        swapchain: self.swapchain,
                        render_pass: self.render_pass,
                        present_cache_texture_view: self.present_cache_texture_view,
                        present_copy_sprite_buffer: self.present_copy_sprite_buffer,
                        current_size: self.current_size,
                        depth_attachment,
                        use_retained_present,
                        present_cache_load_op: retained_cache_load_op,
                    },
                    &self.draw_step_scratch.draw_steps,
                    &self.draw_step_scratch.present_copy_steps,
                )?;
                let present_elapsed_ms = present_started.elapsed().as_millis();
                let total_elapsed_ms = frame_started.elapsed().as_millis();
                if self.diagnostics.should_warn_slow_frame(total_elapsed_ms) {
                    log::warn!(
                        concat!(
                            "nova-gfx frame stages: backend={} total_ms={} ",
                            "buffer_upload_ms={} atlas_upload_ms={} offscreen_ms={} ",
                            "present_ms={} submission_mode={:?} atlas_uploads={} ",
                            "atlas_bytes={}"
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
        self.present_cache_valid = use_retained_present;
        if let Some(elapsed) = backdrop_blur_elapsed {
            self.record_backdrop_blur_frame_time(elapsed);
        }
        crate::diagnostics::performance_metrics::record_present();
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
