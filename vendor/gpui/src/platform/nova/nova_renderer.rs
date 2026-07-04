use super::*;

mod custom_mesh_pipeline;
mod draw_steps;
mod init;
mod mesh_cache;
mod present;
mod submission;
mod surface_lifecycle;

pub(super) fn nova_present_mode_for_backend(
    backend: RendererBackend,
    renderer_options: &RendererOptions,
) -> gfx_core::PresentMode {
    match renderer_options.present_mode {
        PresentModePreference::AutoVsync
            if matches!(
                backend,
                RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
            ) =>
        {
            gfx_core::PresentMode::Mailbox
        }
        PresentModePreference::AutoVsync => gfx_core::PresentMode::Fifo,
        PresentModePreference::Mailbox => gfx_core::PresentMode::Mailbox,
        PresentModePreference::Immediate => gfx_core::PresentMode::Immediate,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct DrawableSize {
    pub(super) width: u32,
    pub(super) height: u32,
}

const BACKDROP_BLUR_REDUCED_QUALITY_BUDGET: Duration = Duration::from_millis(4);
const BACKDROP_BLUR_REDUCED_QUALITY_DURATION: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NovaMeshCacheEntry {
    pub(super) generation: u64,
    pub(super) vertex_offset: u32,
    pub(super) vertex_count: u32,
    pub(super) index_offset: u32,
    pub(super) index_count: u32,
}

pub(crate) struct NovaRenderer {
    backend: NovaBackend,
    surface: SurfaceId,
    swapchain: SwapchainId,
    surface_config: SurfaceConfig,
    surface_format: Format,
    present_mode: gfx_core::PresentMode,
    surface_alpha: NovaSurfaceAlphaState,
    render_pass: RenderPassId,
    pipelines: NovaPipelines,
    depth_texture: TextureId,
    depth_texture_view: TextureViewId,
    global_buffer: BufferId,
    text_raster_buffer: BufferId,
    quad_buffer: BufferId,
    shadow_buffer: BufferId,
    path_rasterization_vertex_buffer: BufferId,
    path_sprite_buffer: BufferId,
    mono_sprite_buffer: BufferId,
    poly_sprite_buffer: BufferId,
    present_copy_sprite_buffer: BufferId,
    underline_buffer: BufferId,
    backdrop_blur_pass_buffer: BufferId,
    backdrop_blur_buffer: BufferId,
    animation_binding_buffer: BufferId,
    animation_value_buffer: BufferId,
    custom_mesh_3d_parameters_buffer: BufferId,
    custom_mesh_3d_vertices_buffer: BufferId,
    custom_mesh_3d_indices_buffer: BufferId,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_rasterization_resource_set: ResourceSetId,
    path_resource_set_layout: ResourceSetLayoutId,
    path_resource_set: ResourceSetId,
    present_cache_resource_set: ResourceSetId,
    mono_sprite_resource_set_layout: ResourceSetLayoutId,
    poly_sprite_resource_set_layout: ResourceSetLayoutId,
    gpu_atlas_textures: FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    custom_mesh_3d_pipeline_layout: PipelineLayoutId,
    custom_mesh_3d_resource_set: ResourceSetId,
    custom_mesh_3d_mesh_cache: FxHashMap<GpuMesh3dId, NovaMeshCacheEntry>,
    custom_mesh_3d_vertex_cursor: usize,
    custom_mesh_3d_index_cursor: usize,
    custom_mesh_3d_uploaded_bytes_this_frame: usize,
    custom_mesh_3d_vertex_upload_scratch: Vec<u8>,
    custom_mesh_3d_index_upload_scratch: Vec<u8>,
    custom_mesh_3d_pipelines: FxHashMap<GpuMesh3dShaderId, RenderPipelineId>,
    custom_mesh_3d_pipeline_failures: FxHashSet<GpuMesh3dShaderId>,
    backdrop_blur_targets: Option<NovaBackdropBlurTargets>,
    atlas_sampler: SamplerId,
    path_texture: TextureId,
    path_texture_view: TextureViewId,
    present_cache_texture: TextureId,
    present_cache_texture_view: TextureViewId,
    frame_upload: NovaFrameUpload,
    draw_step_scratch: NovaDrawStepScratch,
    current_size: DrawableSize,
    atlas: Arc<NovaAtlas>,
    rendering_parameters: NovaRenderingParameters,
    diagnostics: NovaRenderDiagnostics,
    submission_mode: GpuSubmissionMode,
    pending_submissions: Vec<SubmissionId>,
    metrics_started_at: Instant,
    first_frame_reported: bool,
    submitted_frames: u64,
    needs_full_redraw_after_resize: bool,
    present_cache_valid: bool,
    backdrop_blur_reduced_until: Option<Instant>,
}

#[derive(Default)]
struct NovaDrawStepScratch {
    draw_steps: Vec<RenderStepDescriptor>,
    present_copy_steps: Vec<RenderStepDescriptor>,
    backdrop_blur_source_steps: Vec<RenderStepDescriptor>,
    path_mask_steps: Vec<DrawStepDescriptor>,
}

impl NovaRenderer {
    pub(crate) fn draw(&mut self, render_plan: FrameRenderPlan<'_>) -> Result<()> {
        self.observe_render_plan(render_plan);
        let render_plan =
            resolve_surface_render_plan(render_plan, self.needs_full_redraw_after_resize);
        let backdrop_blur_quality = self.backdrop_blur_quality(render_plan);
        let upload = self.frame_upload.encode(
            render_plan.scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
            backdrop_blur_quality,
        );
        if !self.frame_upload.backdrop_blurs.is_empty() {
            self.ensure_backdrop_blur_targets()?;
        }
        self.ensure_custom_mesh_3d_pipelines_for_current_backend()?;
        self.draw_present(upload, render_plan)?;
        self.needs_full_redraw_after_resize = false;
        Ok(())
    }

    fn ensure_backdrop_blur_targets(&mut self) -> Result<()> {
        let downsample = self.frame_upload.backdrop_blur_downsample();
        if self
            .backdrop_blur_targets
            .as_ref()
            .is_some_and(|targets| targets.downsample == downsample)
        {
            return Ok(());
        }
        let target_size = Extent2d::new(self.current_size.width, self.current_size.height)?;
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(target_size);
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let next_backdrop_blur_targets = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova dx12",
                    backdrop_blur_target_descriptor,
                )?;
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "DX12");
                }
                targets
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova metal",
                    backdrop_blur_target_descriptor,
                )?;
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Metal");
                }
                targets
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova vulkan",
                    backdrop_blur_target_descriptor,
                )?;
                if let Some(old_backdrop_blur_targets) = old_backdrop_blur_targets {
                    destroy_backdrop_blur_target_chain(device, old_backdrop_blur_targets, "Vulkan");
                }
                targets
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
        self.backdrop_blur_targets = Some(next_backdrop_blur_targets);
        Ok(())
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        render_plan: FrameRenderPlan<'_>,
    ) -> Result<()> {
        self.observe_render_plan(render_plan);
        let render_plan =
            resolve_surface_render_plan(render_plan, self.needs_full_redraw_after_resize);
        let render_plan = render_plan.with_full_redraw();
        let backdrop_blur_quality = self.backdrop_blur_quality(render_plan);
        let upload = self.frame_upload.encode(
            render_plan.scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
            backdrop_blur_quality,
        );
        if !self.frame_upload.backdrop_blurs.is_empty() {
            self.ensure_backdrop_blur_targets()?;
        }
        self.ensure_custom_mesh_3d_pipelines_for_current_backend()?;
        self.draw_present(upload, render_plan)?;
        self.needs_full_redraw_after_resize = false;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn draw_scene_for_platform(&mut self, scene: &crate::Scene) -> Result<()> {
        let backdrop_blur_quality = self.backdrop_blur_quality_for_visual_effects(
            FrameVisualEffectQuality::Full,
            Instant::now(),
        );
        let upload = self.frame_upload.encode(
            scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
            backdrop_blur_quality,
        );
        if !self.frame_upload.backdrop_blurs.is_empty() {
            self.ensure_backdrop_blur_targets()?;
        }
        self.ensure_custom_mesh_3d_pipelines_for_current_backend()?;
        let dirty_region = crate::DirtyRegion::default();
        self.draw_present(upload, FrameRenderPlan::full_redraw(scene, &dirty_region))
    }

    pub(crate) fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    pub(crate) fn gpu_specs(&self) -> GpuSpecs {
        let (device_name, driver_name) = match self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => ("nova-gfx DX12", "nova-dx12"),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => ("nova-gfx Metal", "nova-metal"),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(_) => ("nova-gfx Vulkan", "nova-vulkan"),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => ("nova-gfx unavailable", "nova-unavailable"),
        };
        GpuSpecs {
            is_software_emulated: false,
            device_name: device_name.to_string(),
            driver_name: driver_name.to_string(),
            driver_info: "phase2b2-nova-batch-smoke".to_string(),
        }
    }

    pub(crate) fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        if !matches!(level, GpuiMemoryTrimLevel::Light) {
            if let Err(error) = self.wait_for_pending_submissions() {
                log::debug!("failed to drain nova-gfx submissions before memory trim: {error}");
            }
        }
        self.atlas.trim(level);
        self.frame_upload.trim_retained_capacity(level);
        self.draw_step_scratch.trim_retained_capacity(level);
        self.trim_custom_mesh_3d_cache(level);

        if matches!(
            level,
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive
        ) && self.frame_upload.backdrop_blurs.is_empty()
        {
            self.destroy_backdrop_blur_targets();
        }

        if matches!(
            level,
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive
        ) {
            if let Err(error) = self.sync_atlas_textures_for_current_backend() {
                log::debug!("failed to sync nova atlas textures during memory trim: {error}");
            }
        }

        if let Err(error) = self.backend.trim_memory(gfx_memory_trim_level(level)) {
            log::debug!("failed to trim nova-gfx backend memory: {error}");
        }
    }

    pub(crate) fn destroy(&mut self) {
        if let Err(error) = self.wait_for_pending_submissions() {
            log::debug!("failed to drain nova-gfx submissions during renderer destroy: {error}");
        }
    }

    fn observe_render_plan(&mut self, render_plan: FrameRenderPlan<'_>) {
        let _ = (
            render_plan.dirty_region.is_full(),
            render_plan.dirty_region.rect_count(),
            render_plan.partial_present_mode,
            render_plan.trim_policy,
            render_plan.visual_effect_quality,
        );
    }

    fn backdrop_blur_quality(&self, render_plan: FrameRenderPlan<'_>) -> NovaBackdropBlurQuality {
        self.backdrop_blur_quality_for_visual_effects(
            render_plan.visual_effect_quality,
            Instant::now(),
        )
    }

    fn backdrop_blur_quality_for_visual_effects(
        &self,
        visual_effect_quality: FrameVisualEffectQuality,
        now: Instant,
    ) -> NovaBackdropBlurQuality {
        let requested_quality =
            NovaBackdropBlurQuality::from_visual_effect_quality(visual_effect_quality);
        requested_quality.most_reduced(self.dynamic_backdrop_blur_quality(now))
    }

    fn dynamic_backdrop_blur_quality(&self, now: Instant) -> NovaBackdropBlurQuality {
        if self
            .backdrop_blur_reduced_until
            .is_some_and(|deadline| now < deadline)
        {
            NovaBackdropBlurQuality::Reduced
        } else {
            NovaBackdropBlurQuality::Full
        }
    }

    pub(super) fn record_backdrop_blur_frame_time(&mut self, elapsed: Duration) {
        if elapsed < BACKDROP_BLUR_REDUCED_QUALITY_BUDGET {
            return;
        }
        self.backdrop_blur_reduced_until =
            Some(Instant::now() + BACKDROP_BLUR_REDUCED_QUALITY_DURATION);
    }

    fn destroy_backdrop_blur_targets(&mut self) {
        let Some(targets) = self.backdrop_blur_targets.take() else {
            return;
        };
        match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                destroy_backdrop_blur_target_chain(device, targets, "DX12");
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                destroy_backdrop_blur_target_chain(device, targets, "Metal");
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                destroy_backdrop_blur_target_chain(device, targets, "Vulkan");
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {}
        }
    }

    fn depth_attachment(&self) -> RenderPassDepthAttachment {
        RenderPassDepthAttachment {
            target: self.depth_texture_view,
            depth_load_op: LoadOp::Clear(1.0),
        }
    }

    fn atlas_resource_descriptor(&self) -> NovaAtlasResourceDescriptor {
        NovaAtlasResourceDescriptor {
            mono_sprite_resource_set_layout: self.mono_sprite_resource_set_layout,
            poly_sprite_resource_set_layout: self.poly_sprite_resource_set_layout,
            global_buffer: self.global_buffer,
            text_raster_buffer: self.text_raster_buffer,
            mono_sprite_buffer: self.mono_sprite_buffer,
            poly_sprite_buffer: self.poly_sprite_buffer,
            sampler: self.atlas_sampler,
        }
    }

    fn sync_atlas_textures_for_current_backend(&mut self) -> Result<()> {
        let descriptor = self.atlas_resource_descriptor();
        match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => sync_gpu_atlas_textures(
                &self.atlas,
                &mut self.gpu_atlas_textures,
                device,
                "gpui nova dx12",
                descriptor,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => sync_gpu_atlas_textures(
                &self.atlas,
                &mut self.gpu_atlas_textures,
                device,
                "gpui nova metal",
                descriptor,
            ),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => sync_gpu_atlas_textures(
                &self.atlas,
                &mut self.gpu_atlas_textures,
                device,
                "gpui nova vulkan",
                descriptor,
            ),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => Ok(()),
        }
    }
}

impl NovaDrawStepScratch {
    fn trim_retained_capacity(&mut self, level: GpuiMemoryTrimLevel) {
        let multiplier = match level {
            GpuiMemoryTrimLevel::Light => 16,
            GpuiMemoryTrimLevel::Moderate => 8,
            GpuiMemoryTrimLevel::Aggressive => 1,
        };
        trim_vec_capacity(&mut self.draw_steps, 64, multiplier);
        trim_vec_capacity(&mut self.present_copy_steps, 1, multiplier);
        trim_vec_capacity(&mut self.backdrop_blur_source_steps, 64, multiplier);
        trim_vec_capacity(&mut self.path_mask_steps, 32, multiplier);
    }
}

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}

fn gfx_memory_trim_level(level: GpuiMemoryTrimLevel) -> GfxMemoryTrimLevel {
    match level {
        GpuiMemoryTrimLevel::Light => GfxMemoryTrimLevel::Light,
        GpuiMemoryTrimLevel::Moderate => GfxMemoryTrimLevel::Moderate,
        GpuiMemoryTrimLevel::Aggressive => GfxMemoryTrimLevel::Aggressive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_step_scratch_aggressive_trim_shrinks_retained_capacity() {
        let mut scratch = NovaDrawStepScratch::default();
        scratch.draw_steps.reserve(2048);
        scratch.present_copy_steps.reserve(128);
        scratch.backdrop_blur_source_steps.reserve(2048);
        scratch.path_mask_steps.reserve(1024);

        scratch.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);

        assert!(scratch.draw_steps.capacity() <= 64);
        assert!(scratch.present_copy_steps.capacity() <= 1);
        assert!(scratch.backdrop_blur_source_steps.capacity() <= 64);
        assert!(scratch.path_mask_steps.capacity() <= 32);
    }
}
