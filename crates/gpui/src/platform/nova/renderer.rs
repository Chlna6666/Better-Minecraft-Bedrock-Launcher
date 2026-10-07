use super::*;
use crate::platform::frame::ActivePresentationFrame;
use smallvec::SmallVec;

mod chunk_upload;
mod destroy;
mod draw_step_scratch;
mod draw_steps;
mod extensions;
mod filters;

mod init;
mod present;
mod retained_upload;
mod submission;
mod surface_lifecycle;

use draw_step_scratch::{DrawStepCacheKey, DrawStepScratch, PathMaskCacheKey};

const SWAPCHAIN_WARMUP_FRAME_COUNT: u8 = 1;

/// Damaged-area reciprocal above which native dirty-rect presentation is skipped.
///
/// Dirty-rect `Present1` requires the OS to copy-preserve everything outside the
/// dirty rects, so once damage covers more than `1 / RECIPROCAL` of the surface a
/// full present is cheaper than the resulting per-frame composition copies.
const PARTIAL_PRESENT_MAX_DAMAGE_AREA_RECIPROCAL: u64 = 4;

fn surface_alpha_allows_partial_presentation(surface_alpha: SurfaceAlphaState) -> bool {
    #[cfg(target_os = "windows")]
    {
        // Transparent Windows surfaces are presented through a premultiplied composition
        // swapchain. Until the rotating-backbuffer preservation contract is proven for that path,
        // native dirty-rect presentation is unsafe: a local animation can otherwise expose stale
        // or transparent pixels outside the requested damage after several buffer rotations.
        !surface_alpha.outputs_premultiplied_alpha()
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = surface_alpha;
        true
    }
}

pub(super) fn nova_present_mode_for_backend(
    backend: RendererBackend,
    renderer_options: &RendererOptions,
) -> gfx_core::PresentMode {
    match renderer_options.present_mode {
        // Windows already paces the native owner with DwmFlush. Asking DXGI to wait for another
        // vblank after that wake can halve the display cadence. Mailbox maps to Present(0, 0),
        // without ALLOW_TEARING; DWM remains the pacing authority for the composed surface.
        PresentModePreference::AutoVsync
            if cfg!(target_os = "windows") && backend == RendererBackend::NovaDx12 =>
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

#[derive(Clone)]
pub(crate) struct NovaRendererAtlas(Arc<NovaAtlas>);

impl NovaRendererAtlas {
    pub(crate) fn new() -> Self {
        Self(Arc::new(NovaAtlas::new()))
    }

    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.clone()
    }
}

pub(crate) struct NovaRenderer {
    backend: SharedBackend,
    backend_info: NovaBackendInfo,
    surface: SurfaceId,
    swapchain: SwapchainId,
    surface_config: SurfaceConfig,
    surface_format: Format,
    present_mode: gfx_core::PresentMode,
    surface_alpha: SurfaceAlphaState,
    render_pass: RenderPassId,
    pipelines: Pipelines,
    depth_texture: TextureId,
    depth_texture_view: TextureViewId,
    frame_resources: Vec<FrameResources>,
    current_frame_resource_index: usize,
    global_buffer: BufferId,
    text_raster_buffer: BufferId,
    quad_buffer: BufferId,
    shadow_buffer: BufferId,
    path_rasterization_vertex_buffer: BufferId,
    path_sprite_buffer: BufferId,
    mono_sprite_buffer: BufferId,
    poly_sprite_buffer: BufferId,
    underline_buffer: BufferId,
    backdrop_blur_pass_buffer: BufferId,
    backdrop_blur_buffer: BufferId,
    animation_value_buffer: BufferId,
    quad_resource_set: ResourceSetId,
    quad_resource_set_layout: ResourceSetLayoutId,
    shadow_resource_set: ResourceSetId,
    path_rasterization_resource_set: ResourceSetId,
    path_rasterization_resource_set_layout: ResourceSetLayoutId,
    path_resource_set_layout: ResourceSetLayoutId,
    path_resource_set: ResourceSetId,
    mono_sprite_resource_set_layout: ResourceSetLayoutId,
    poly_sprite_resource_set_layout: ResourceSetLayoutId,
    gpu_atlas_textures: FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    synced_atlas_texture_generation: Option<u64>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    filters: filters::FilterRegistry,
    atlas_sampler: SamplerId,
    path_texture: TextureId,
    path_texture_view: TextureViewId,
    path_texture_size: Extent2d,
    frame_upload: FrameUpload,
    renderer_registry: extensions::RendererRegistry,
    retained_upload: retained_upload::RetainedUpload,
    draw_step_scratch: DrawStepScratch,
    current_size: DrawableSize,
    pending_drawable_size: Option<Size<DevicePixels>>,
    atlas: Arc<NovaAtlas>,
    rendering_parameters: RenderingParameters,
    diagnostics: NovaRenderDiagnostics,
    submission_mode: GpuSubmissionMode,
    pending_submissions: Vec<PendingSubmission>,
    metrics_started_at: Instant,
    first_frame_reported: bool,
    submitted_frames: u64,
    swapchain_warmup_frames: u8,
    active_presentation_packet: Option<PresentationPacket>,
    pending_animation_completions: SmallVec<[crate::SceneAnimationCompletion; 4]>,
    destroyed: bool,
}

#[derive(Clone, Copy)]
struct PendingSubmission {
    submission: SubmissionId,
    frame_resource_index: usize,
}

fn create_grown_quad_resources<D>(
    device: &mut D,
    label: &str,
    layout: ResourceSetLayoutId,
    current_buffers: FrameResourceBuffers,
    new_capacity: usize,
) -> Result<(BufferId, ResourceSetId)>
where
    D: BackendResources,
{
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} quads")),
        size: (new_capacity * PACKED_QUAD_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let mut next_buffers = current_buffers;
    next_buffers.quad_buffer = buffer;
    next_buffers.quad_capacity = new_capacity;
    match create_quad_resource_set(device, label, layout, &next_buffers) {
        Ok(resource_set) => Ok((buffer, resource_set)),
        Err(error) => {
            if let Err(destroy_error) = device.destroy_buffer(buffer) {
                log::debug!("failed to roll back {label} grown quad buffer: {destroy_error}");
            }
            Err(error)
        }
    }
}

fn retire_replaced_quad_resources<D>(
    device: &mut D,
    label: &str,
    resource_set: ResourceSetId,
    buffer: BufferId,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_resource_set(resource_set) {
        log::debug!("failed to retire {label} old quad resource set: {error}");
    }
    if let Err(error) = device.destroy_buffer(buffer) {
        log::debug!("failed to retire {label} old quad buffer: {error}");
    }
}

fn create_grown_path_rasterization_resources<D>(
    device: &mut D,
    label: &str,
    layout: ResourceSetLayoutId,
    current_buffers: FrameResourceBuffers,
    new_capacity: usize,
) -> Result<(BufferId, ResourceSetId)>
where
    D: BackendResources,
{
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(format!("{label} path rasterization vertices")),
        size: (new_capacity * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let mut next_buffers = current_buffers;
    next_buffers.path_rasterization_vertex_buffer = buffer;
    next_buffers.path_rasterization_vertex_capacity = new_capacity;
    match create_path_rasterization_resource_set(device, label, layout, &next_buffers) {
        Ok(resource_set) => Ok((buffer, resource_set)),
        Err(error) => {
            if let Err(destroy_error) = device.destroy_buffer(buffer) {
                log::debug!(
                    "failed to roll back {label} grown path buffer: {destroy_error}"
                );
            }
            Err(error)
        }
    }
}

fn retire_replaced_path_rasterization_resources<D>(
    device: &mut D,
    label: &str,
    resource_set: ResourceSetId,
    buffer: BufferId,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_resource_set(resource_set) {
        log::debug!("failed to retire {label} old path resource set: {error}");
    }
    if let Err(error) = device.destroy_buffer(buffer) {
        log::debug!("failed to retire {label} old path buffer: {error}");
    }
}

impl NovaRenderer {
    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    /// Returns whether the backend can present another frame without parking the caller.
    ///
    /// Windows consumes this as a platform-frame preflight. A saturated DXGI queue is therefore a
    /// deferred presentation rather than synchronous work on GPUI's UI thread.
    pub(crate) fn can_present_without_wait(&mut self) -> Result<bool> {
        if !self.apply_pending_drawable_size()? {
            return Ok(false);
        }
        lock_backend(&self.backend).can_present_without_wait(self.swapchain)
    }

    pub(crate) fn arm_swapchain_frame_ready(
        &mut self,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        lock_backend(&self.backend).arm_swapchain_frame_ready(self.swapchain, callback)
    }

    pub(crate) fn presentation_capabilities(&self) -> gfx_core::PresentationCapabilities {
        lock_backend(&self.backend).presentation_capabilities(self.swapchain)
    }

    pub(crate) fn draw(&mut self, mut packet: PresentationPacket) -> Result<bool> {
        if let Some(previous) = self.active_presentation_packet.as_ref() {
            packet.merge_pending_damage_from(previous);
        }
        self.pending_animation_completions
            .extend(packet.sample_animations(packet.frame_time));
        let started_at = Instant::now();
        let mut presentation_timing = None;
        let result = self.draw_frame(&mut packet, &mut presentation_timing);
        if result.as_ref().is_ok_and(|submitted| *submitted) {
            packet.record_presentation(Instant::now());
        }
        self.active_presentation_packet = Some(packet);
        let elapsed = started_at.elapsed();
        crate::diagnostics::performance_metrics::record_frame_backend_draw_time(elapsed);
        crate::diagnostics::performance_metrics::record_first_frame_backend_draw_time(elapsed);
        result
    }

    fn draw_frame(
        &mut self,
        packet: &mut PresentationPacket,
        presentation_timing: &mut Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<bool> {
        if !self.apply_pending_drawable_size()? {
            return Ok(false);
        }
        self.observe_presentation_packet(&packet);
        let supports_partial = self.swapchain_warmup_frames == 0
            && surface_alpha_allows_partial_presentation(self.surface_alpha)
            && lock_backend(&self.backend)
                .presentation_capabilities(self.swapchain)
                .partial_presentation;
        resolve_surface_packet(packet, !supports_partial);
        let backdrop_blur_quality = self.backdrop_blur_quality(packet);
        let upload = self.pack_scene(
            packet.scene.as_ref(),
            packet.presentation_animation_values.as_slice(),
            backdrop_blur_quality,
        );
        self.ensure_path_mask_target_for_frame()?;
        self.prepare_renderer_extensions(packet.frame_time)?;
        self.update_backdrop_blur_cache_plan(backdrop_blur_quality);
        if !self.frame_upload.backdrop_blurs.is_empty() {
            self.ensure_backdrop_blur_targets()?;
        }
        self.draw_present(upload, packet, backdrop_blur_quality, presentation_timing)
    }

    fn ensure_path_mask_target_for_frame(&mut self) -> Result<()> {
        if self.frame_upload.path_rasterization_vertices.is_empty()
            && self.frame_upload.path_sprites.is_empty()
        {
            return Ok(());
        }

        let target_size = Extent2d::new(self.current_size.width, self.current_size.height)?;
        if self.path_texture_size == target_size {
            return Ok(());
        }

        let descriptor = self.path_mask_target_descriptor(target_size);
        let old_target = self.current_path_mask_target();
        let next_target = match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                create_path_mask_target(device, "gpui nova dx12", descriptor)?
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                create_path_mask_target(device, "gpui nova metal", descriptor)?
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                create_path_mask_target(device, "gpui nova vulkan", descriptor)?
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

        self.update_path_mask_resource_sets(&next_target.resource_sets)?;
        self.path_texture = next_target.texture;
        self.path_texture_view = next_target.texture_view;
        self.path_texture_size = target_size;
        self.activate_frame_resources(self.current_frame_resource_index)?;

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => destroy_path_mask_target(device, old_target, "DX12"),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => destroy_path_mask_target(device, old_target, "Metal"),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => destroy_path_mask_target(device, old_target, "Vulkan"),
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

        log::debug!(
            "nova path mask target promoted from startup placeholder: {}x{}",
            target_size.width(),
            target_size.height()
        );
        Ok(())
    }

    fn ensure_backdrop_blur_targets(&mut self) -> Result<()> {
        if self.filters.targets.as_ref().is_some_and(|targets| {
            targets.is_layout_compatible(
                self.frame_upload.backdrop_blur_configs(),
                self.frame_upload.isolated_blur_source_indices(),
            )
        }) {
            return Ok(());
        }
        // New target storage has no retained filtered pixels. The first frame using the new chain
        // must therefore rebuild every root backdrop regardless of the current damage footprint.
        self.draw_step_scratch.force_full_backdrop_blur_refresh = true;
        let target_size = Extent2d::new(self.current_size.width, self.current_size.height)?;
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(target_size);
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let next_backdrop_blur_targets = match &mut *lock_backend(&self.backend) {
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
        self.filters.targets = Some(next_backdrop_blur_targets);
        self.invalidate_backdrop_blur_cache();
        Ok(())
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        mut packet: PresentationPacket,
    ) -> Result<bool> {
        if let Some(previous) = self.active_presentation_packet.as_ref() {
            packet.merge_pending_damage_from(previous);
        }
        self.pending_animation_completions
            .extend(packet.sample_animations(packet.frame_time));
        let result = (|| {
            if !self.apply_pending_drawable_size()? {
                return Ok(false);
            }
            self.observe_presentation_packet(&packet);
            let supports_partial = self.swapchain_warmup_frames == 0
                && surface_alpha_allows_partial_presentation(self.surface_alpha)
                && lock_backend(&self.backend)
                    .presentation_capabilities(self.swapchain)
                    .partial_presentation;
            resolve_surface_packet(&mut packet, !supports_partial);
            let backdrop_blur_quality = self.backdrop_blur_quality(&packet);
            let upload = self.pack_scene(
                packet.scene.as_ref(),
                packet.presentation_animation_values.as_slice(),
                backdrop_blur_quality,
            );
            self.ensure_path_mask_target_for_frame()?;
            self.prepare_renderer_extensions(packet.frame_time)?;
            self.update_backdrop_blur_cache_plan(backdrop_blur_quality);
            if !self.frame_upload.backdrop_blurs.is_empty() {
                self.ensure_backdrop_blur_targets()?;
            }
            let mut presentation_timing = None;
            self.draw_present(
                upload,
                &mut packet,
                backdrop_blur_quality,
                &mut presentation_timing,
            )
        })();
        if result.as_ref().is_ok_and(|submitted| *submitted) {
            packet.record_presentation(Instant::now());
        }
        self.active_presentation_packet = Some(packet);
        result
    }

    /// Present the active immutable scene using a compositor-owned frame timestamp.
    pub(crate) fn present_active_frame(
        &mut self,
        now: Instant,
        presentation_timing: Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<Option<ActivePresentationFrame>> {
        let Some(mut packet) = self.active_presentation_packet.take() else {
            return Ok(None);
        };
        if !packet.has_pending_presentation() {
            self.active_presentation_packet = Some(packet);
            return Ok(None);
        }
        if !packet.presentation_is_due(now) {
            let continues = !packet.presentation_animation_timelines.is_empty();
            self.active_presentation_packet = Some(packet);
            return Ok(continues.then(|| ActivePresentationFrame {
                continues,
                completed_animations: SmallVec::new(),
            }));
        }
        self.pending_animation_completions
            .extend(packet.sample_animations(now));
        let continues = !packet.presentation_animation_timelines.is_empty();
        let mut presentation_timing = presentation_timing;
        let draw_result = self.draw_frame(&mut packet, &mut presentation_timing);
        if draw_result.as_ref().is_ok_and(|submitted| *submitted) {
            packet.record_presentation(Instant::now());
        }
        self.active_presentation_packet = Some(packet);
        if !draw_result? {
            return Ok(None);
        }
        Ok(Some(ActivePresentationFrame {
            continues,
            completed_animations: self.take_animation_completions(),
        }))
    }

    pub(crate) fn has_active_presentation_animations(&self) -> bool {
        self.active_presentation_packet
            .as_ref()
            .is_some_and(PresentationPacket::has_pending_presentation)
    }

    pub(crate) fn active_presentation_is_due(&self, now: Instant) -> bool {
        self.active_presentation_packet
            .as_ref()
            .is_some_and(|packet| {
                packet.has_pending_presentation() && packet.presentation_is_due(now)
            })
    }

    pub(crate) fn set_frame_interval(&mut self, interval: Option<std::time::Duration>) {
        if let Some(packet) = self.active_presentation_packet.as_mut() {
            packet.set_frame_interval(interval);
        }
    }

    pub(crate) fn take_animation_completions(
        &mut self,
    ) -> SmallVec<[crate::SceneAnimationCompletion; 4]> {
        std::mem::take(&mut self.pending_animation_completions)
    }

    pub(crate) fn gpu_specs(&self) -> GpuSpecs {
        GpuSpecs {
            is_software_emulated: false,
            device_name: self.backend_info.adapter_name().to_string(),
            driver_name: self.backend_info.label().to_string(),
            driver_info: "phase2b2-nova-batch-smoke".to_string(),
        }
    }

    pub(crate) fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        let submissions_drained = if !matches!(level, GpuiMemoryTrimLevel::Light) {
            match self.wait_for_pending_submissions() {
                Ok(()) => true,
                Err(error) => {
                    log::debug!("failed to drain nova-gfx submissions before memory trim: {error}");
                    false
                }
            }
        } else {
            false
        };
        self.atlas.trim(level);
        self.frame_upload.trim_retained_capacity(level);
        self.draw_step_scratch.trim_retained_capacity(level);
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

        if submissions_drained && !self.renderer_registry.is_empty() {
            let renderer_registry = &mut self.renderer_registry;
            if let Err(error) = lock_backend(&self.backend).with_extension_device(|device| {
                renderer_registry.trim(device, memory_trim_level(level));
                Ok(())
            }) {
                log::debug!(
                    "failed to access nova-gfx device to trim renderer extensions: {error}"
                );
            }
        }

        if let Err(error) = lock_backend(&self.backend).trim_memory(memory_trim_level(level)) {
            log::debug!("failed to trim nova-gfx backend memory: {error}");
        }
    }

    pub(crate) fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.active_presentation_packet = None;
        if let Err(error) = self.wait_for_pending_submissions() {
            log::debug!("failed to drain nova-gfx submissions during renderer destroy: {error}");
        }
        self.destroy_renderer_extensions();
        self.destroy_window_resources();
    }

    fn observe_presentation_packet(&mut self, packet: &PresentationPacket) {
        self.draw_step_scratch.backdrop_blur_damage_region = packet.dirty_region.clone();
        self.draw_step_scratch.backdrop_blur_damage_plan = packet.backdrop_blur_damage_plan.clone();
        self.draw_step_scratch.force_full_backdrop_blur_refresh = force_full_backdrop_blur_refresh(
            self.filters.is_valid(),
            packet.force_full_backdrop_blur_refresh,
        );
    }

    fn update_backdrop_blur_cache_plan(&mut self, quality: BackdropBlurQuality) {
        if self.filters.quality != Some(quality) {
            self.draw_step_scratch.force_full_backdrop_blur_refresh = true;
        }
        if self.frame_upload.backdrop_blurs.is_empty() {
            return;
        }

        let source_atlases = self.frame_upload.backdrop_source_atlas_texture_ids();
        if self.atlas.pending_uploads_touch_any(&source_atlases) {
            self.draw_step_scratch.force_full_backdrop_blur_refresh = true;
        }
    }

    fn backdrop_blur_quality(&self, _packet: &PresentationPacket) -> BackdropBlurQuality {
        if self.swapchain_warmup_frames > 0 {
            BackdropBlurQuality::Interactive
        } else {
            BackdropBlurQuality::Full
        }
    }

    fn destroy_backdrop_blur_targets(&mut self) {
        self.invalidate_backdrop_blur_cache();
        let Some(targets) = self.filters.targets.take() else {
            return;
        };
        match &mut *lock_backend(&self.backend) {
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

    fn invalidate_backdrop_blur_cache(&mut self) {
        self.filters.invalidate();
    }

    fn depth_attachment(&self) -> RenderPassDepthAttachment {
        RenderPassDepthAttachment {
            target: self.depth_texture_view,
            depth_load_op: LoadOp::Clear(1.0),
        }
    }

    pub(super) fn ensure_quad_capacity(&mut self) -> Result<()> {
        let required_bytes = self.frame_upload.quads.len();
        if required_bytes == 0 {
            return Ok(());
        }
        let required_quads = required_bytes.div_ceil(PACKED_QUAD_BYTES);
        if required_quads > MAX_QUADS {
            anyhow::bail!(
                "nova quad upload exceeds hard limit: required={} max={}",
                required_quads,
                MAX_QUADS
            );
        }

        let index = self.current_frame_resource_index;
        let current = self
            .frame_resources
            .get(index)
            .copied()
            .context("current nova frame resource slot is unavailable")?;
        let current_capacity = current.buffers.quad_capacity;
        if required_quads <= current_capacity {
            return Ok(());
        }

        let new_capacity = required_quads
            .next_power_of_two()
            .max(current_capacity.saturating_mul(2))
            .min(MAX_QUADS);
        let old_buffer = current.buffers.quad_buffer;
        let old_resource_set = current.resource_sets.quad_resource_set;
        let layout = self.quad_resource_set_layout;

        let (new_buffer, new_resource_set) = match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => create_grown_quad_resources(
                device,
                "gpui nova dx12 grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => create_grown_quad_resources(
                device,
                "gpui nova metal grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => create_grown_quad_resources(
                device,
                "gpui nova vulkan grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova backend is unavailable while growing quad resources")
            }
        };

        if let Some(resources) = self.frame_resources.get_mut(index) {
            resources.buffers.quad_buffer = new_buffer;
            resources.buffers.quad_capacity = new_capacity;
            resources.resource_sets.quad_resource_set = new_resource_set;
        }
        self.quad_buffer = new_buffer;
        self.quad_resource_set = new_resource_set;
        self.retained_upload.invalidate_quad_slot(index);

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => retire_replaced_quad_resources(
                device,
                "gpui nova dx12",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => retire_replaced_quad_resources(
                device,
                "gpui nova metal",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => retire_replaced_quad_resources(
                device,
                "gpui nova vulkan",
                old_resource_set,
                old_buffer,
            ),
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

        log::debug!(
            "nova quad buffer grew: slot={} quads={} -> {}",
            index,
            current_capacity,
            new_capacity
        );
        Ok(())
    }

    pub(super) fn ensure_path_rasterization_capacity(&mut self) -> Result<()> {
        let required_bytes = self.frame_upload.path_rasterization_vertices.len();
        if required_bytes == 0 {
            return Ok(());
        }
        let required_vertices =
            required_bytes.div_ceil(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        if required_vertices > MAX_PATH_VERTICES {
            anyhow::bail!(
                "nova path vertex upload exceeds hard limit: required={} max={}",
                required_vertices,
                MAX_PATH_VERTICES
            );
        }

        let index = self.current_frame_resource_index;
        let current = self
            .frame_resources
            .get(index)
            .copied()
            .context("current nova frame resource slot is unavailable")?;
        let current_capacity = current.buffers.path_rasterization_vertex_capacity;
        if required_vertices <= current_capacity {
            return Ok(());
        }

        let new_capacity = required_vertices
            .next_power_of_two()
            .max(current_capacity.saturating_mul(2))
            .min(MAX_PATH_VERTICES);
        let old_buffer = current.buffers.path_rasterization_vertex_buffer;
        let old_resource_set = current.resource_sets.path_rasterization_resource_set;
        let layout = self.path_rasterization_resource_set_layout;

        let (new_buffer, new_resource_set) = match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => create_grown_path_rasterization_resources(
                device,
                "gpui nova dx12 grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => create_grown_path_rasterization_resources(
                device,
                "gpui nova metal grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => create_grown_path_rasterization_resources(
                device,
                "gpui nova vulkan grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova backend is unavailable while growing path resources")
            }
        };

        if let Some(resources) = self.frame_resources.get_mut(index) {
            resources.buffers.path_rasterization_vertex_buffer = new_buffer;
            resources.buffers.path_rasterization_vertex_capacity = new_capacity;
            resources.resource_sets.path_rasterization_resource_set = new_resource_set;
        }
        self.path_rasterization_vertex_buffer = new_buffer;
        self.path_rasterization_resource_set = new_resource_set;
        self.retained_upload
            .invalidate_path_rasterization_slot(index);

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => retire_replaced_path_rasterization_resources(
                device,
                "gpui nova dx12",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => retire_replaced_path_rasterization_resources(
                device,
                "gpui nova metal",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => retire_replaced_path_rasterization_resources(
                device,
                "gpui nova vulkan",
                old_resource_set,
                old_buffer,
            ),
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

        log::debug!(
            "nova path buffer grew: slot={} vertices={} -> {}",
            index,
            current_capacity,
            new_capacity
        );
        Ok(())
    }

    pub(super) fn activate_frame_resources(&mut self, index: usize) -> Result<()> {
        let Some(resources) = self.frame_resources.get(index).copied() else {
            anyhow::bail!("nova frame resource slot {index} is unavailable");
        };
        self.current_frame_resource_index = index;
        self.global_buffer = resources.buffers.global_buffer;
        self.text_raster_buffer = resources.buffers.text_raster_buffer;
        self.quad_buffer = resources.buffers.quad_buffer;
        self.shadow_buffer = resources.buffers.shadow_buffer;
        self.path_rasterization_vertex_buffer = resources.buffers.path_rasterization_vertex_buffer;
        self.path_sprite_buffer = resources.buffers.path_sprite_buffer;
        self.mono_sprite_buffer = resources.buffers.mono_sprite_buffer;
        self.poly_sprite_buffer = resources.buffers.poly_sprite_buffer;
        self.underline_buffer = resources.buffers.underline_buffer;
        self.backdrop_blur_pass_buffer = resources.buffers.backdrop_blur_pass_buffer;
        self.backdrop_blur_buffer = resources.buffers.backdrop_blur_buffer;
        self.animation_value_buffer = resources.buffers.animation_value_buffer;
        self.quad_resource_set = resources.resource_sets.quad_resource_set;
        self.shadow_resource_set = resources.resource_sets.shadow_resource_set;
        self.path_rasterization_resource_set =
            resources.resource_sets.path_rasterization_resource_set;
        self.path_resource_set = resources.path_resource_set;
        self.underline_resource_set = resources.resource_sets.underline_resource_set;
        Ok(())
    }

    pub(super) fn frame_resource_buffers(&self) -> Vec<FrameResourceBuffers> {
        self.frame_resources
            .iter()
            .map(|resources| resources.buffers)
            .collect()
    }

    pub(super) fn update_path_mask_resource_sets(
        &mut self,
        resource_sets: &[ResourceSetId],
    ) -> Result<()> {
        if resource_sets.len() != self.frame_resources.len() {
            anyhow::bail!("path mask frame resource set count does not match frame resources");
        }
        for (resources, resource_set) in self.frame_resources.iter_mut().zip(resource_sets) {
            resources.path_resource_set = *resource_set;
        }
        Ok(())
    }

    fn atlas_resource_descriptor(&self) -> AtlasResourceDescriptor {
        AtlasResourceDescriptor {
            mono_sprite_resource_set_layout: self.mono_sprite_resource_set_layout,
            poly_sprite_resource_set_layout: self.poly_sprite_resource_set_layout,
            frame_buffers: self
                .frame_resources
                .iter()
                .map(|resources| resources.buffers)
                .collect(),
            sampler: self.atlas_sampler,
        }
    }

    fn sync_atlas_textures_for_current_backend(&mut self) -> Result<()> {
        let texture_set_generation = self.atlas.texture_set_generation();
        if self.synced_atlas_texture_generation == Some(texture_set_generation) {
            return Ok(());
        }
        let descriptor = self.atlas_resource_descriptor();
        let result = match &mut *lock_backend(&self.backend) {
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
        };
        if result.is_ok() {
            self.synced_atlas_texture_generation = Some(texture_set_generation);
        }
        result
    }
}

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}

fn memory_trim_level(level: GpuiMemoryTrimLevel) -> MemoryTrimLevel {
    match level {
        GpuiMemoryTrimLevel::Light => MemoryTrimLevel::Light,
        GpuiMemoryTrimLevel::Moderate => MemoryTrimLevel::Moderate,
        GpuiMemoryTrimLevel::Aggressive => MemoryTrimLevel::Aggressive,
    }
}

fn force_full_backdrop_blur_refresh(cache_valid: bool, explicitly_forced: bool) -> bool {
    !cache_valid || explicitly_forced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_spatial_damage_does_not_force_full_blur_refresh() {
        assert!(!force_full_backdrop_blur_refresh(true, false));
    }

    #[test]
    fn invalid_cache_still_forces_full_blur_refresh() {
        assert!(force_full_backdrop_blur_refresh(false, false));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn transparent_windows_surface_disables_native_partial_presentation() {
        assert!(!surface_alpha_allows_partial_presentation(
            SurfaceAlphaState::for_window_transparency(true)
        ));
        assert!(surface_alpha_allows_partial_presentation(
            SurfaceAlphaState::for_window_transparency(false)
        ));
    }
}
