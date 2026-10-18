use super::*;
use crate::platform::frame::ActivePresentationFrame;
use smallvec::SmallVec;

mod buffer_capacity;
mod buffer_upload;
mod chunk_upload;
pub(super) mod compact;
mod destroy;
mod draw_step_scratch;
mod draw_steps;
mod drawable;
mod extensions;
mod filters;
mod root_color;
mod frame_graph;
#[cfg(target_os = "windows")]
use drawable::native_windows_hwnd;
use drawable::resolve_initial_drawable_size;

mod init;
mod init_state;
mod memory;
mod path_mask;
mod pending_offscreen;
mod pending_surface;
#[cfg(target_os = "windows")]
mod preparation;
use init_state::InitializedRenderer;
mod present;
mod retained_upload;
mod shared_buffers;
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

fn packet_drawable_size(packet: &PresentationPacket, fallback: DrawableSize) -> DrawableSize {
    let (width, height) = packet
        .paint
        .geometry
        .layout_extent
        .unwrap_or((fallback.width, fallback.height));
    DrawableSize {
        width: width.max(1),
        height: height.max(1),
    }
}


fn scene_matches_committed_drawable(
    geometry: &crate::window::PaintGeometry,
    committed: DrawableSize,
) -> bool {
    geometry.layout_extent.is_none_or(|(width, height)| {
        width.max(1) == committed.width && height.max(1) == committed.height
    })
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
            if cfg!(target_os = "windows")
                && matches!(
                    backend,
                    RendererBackend::NovaOpenGl
                        | RendererBackend::NovaDx11
                        | RendererBackend::NovaDx12
                ) =>
        {
            gfx_core::PresentMode::Mailbox
        }
        PresentModePreference::AutoVsync => gfx_core::PresentMode::Fifo,
        PresentModePreference::Mailbox => gfx_core::PresentMode::Mailbox,
        PresentModePreference::Immediate => gfx_core::PresentMode::Immediate,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DrawableSize {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Copy)]
pub(super) struct DrawableSizeRequest {
    pub(super) size: Size<DevicePixels>,
    pub(super) epoch: Option<u64>,
}

impl DrawableSizeRequest {
    pub(super) fn applies_to(self, scene_epoch: Option<u64>) -> bool {
        self.epoch.is_none_or(|epoch| scene_epoch == Some(epoch))
    }

    /// A native generation alone cannot certify the physical Scene extent.
    pub(super) fn matches_scene(
        self,
        scene_epoch: Option<u64>,
        scene_extent: Option<(u32, u32)>,
    ) -> bool {
        self.applies_to(scene_epoch) && scene_extent.is_none_or(|(width, height)| {
            self.size.width.0.max(1) as u32 == width.max(1)
                && self.size.height.0.max(1) as u32 == height.max(1)
        })
    }
}

#[derive(Clone)]
pub(crate) struct NovaRendererAtlas(Arc<NovaAtlas>);

/// The owner must distinguish native pacing notifications from resource-retirement retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentReadiness {
    Ready,
    NativeFrame,
    Resources,
}

impl NovaRendererAtlas {
    pub(crate) fn new() -> Self {
        Self(Arc::new(NovaAtlas::new()))
    }

    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.clone()
    }
}

pub(crate) struct NovaRenderer {
    memory_profile: crate::diagnostics::MemoryProfileRegistration,
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
    shadow_resource_set_layout: ResourceSetLayoutId,
    underline_resource_set_layout: ResourceSetLayoutId,
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
    root_color: root_color::RootColor,
    /// At most one unsubmitted independent Root/Filter generation.
    /// Never sampled by Active before the matching resize/present succeeds.
    pending_offscreen_targets: Option<pending_offscreen::PendingOffscreenTargets>,
    pending_surface_attachments: Option<pending_surface::PendingSurfaceAttachments>,
    /// Physical Filter allocations still referenced by specific outstanding GPU submissions.
    retired_offscreen_targets: Vec<pending_offscreen::RetiredOffscreenTargets>,
    atlas_sampler: SamplerId,
    path_texture: TextureId,
    path_texture_view: TextureViewId,
    path_texture_size: Extent2d,
    frame_upload: FrameUpload,
    renderer_registry: extensions::RendererRegistry,
    retained_upload: retained_upload::RetainedUpload,
    draw_step_scratch: DrawStepScratch,
    path_mask_residency: path_mask::Residency,
    active_size: DrawableSize,
    current_size: DrawableSize,
    pending_drawable_size: Option<DrawableSizeRequest>,
    atlas: Arc<NovaAtlas>,
    rendering_parameters: RenderingParameters,
    diagnostics: NovaRenderDiagnostics,
    submission_mode: GpuSubmissionMode,
    pending_submissions: Vec<PendingSubmission>,
    submission_retry_pending: bool,
    metrics_started_at: Instant,
    first_frame_reported: bool,
    submitted_frames: u64,
    swapchain_warmup_frames: u8,
    live_resize_active: bool,
    /// The last successfully submitted scene/animation state. A candidate
    /// must never replace this until its matching geometry reaches the GPU.
    active_presentation_packet: Option<PresentationPacket>,
    /// The newest scene whose GPU submission was deferred. It is separate
    /// from Active and owns all accumulated unsubmitted damage.
    pending_presentation_packet: Option<PresentationPacket>,
    pending_animation_completions: SmallVec<[crate::SceneAnimationCompletion; 4]>,
    /// Completion events from a candidate which has not reached the GPU.
    deferred_animation_completions: SmallVec<[crate::SceneAnimationCompletion; 4]>,
    destroyed: bool,
}

/// Publish a candidate only after native GPU submission succeeded. Failed or
/// backpressured submissions remain Pending, leaving the last successfully
/// displayed scene intact. The newest candidate supersedes older pending
/// work, but callers merge the old unsubmitted damage before replacement.
fn publish_presentation_candidate<T>(
    active: &mut Option<T>,
    pending: &mut Option<T>,
    candidate: T,
    submitted: bool,
) {
    if submitted {
        *active = Some(candidate);
        *pending = None;
    } else {
        *pending = Some(candidate);
    }
}

/// Stage a compositor tick without moving the successfully submitted Active
/// packet. Pending already has exclusive ownership and needs no clone; an
/// Active tick forks only when a new sample is actually due. A failed GPU
/// submission must not advance Active's geometry, animation samples or damage.
fn take_presentation_candidate<T: Clone>(
    active: &Option<T>,
    pending: &mut Option<T>,
) -> Option<(T, bool)> {
    if let Some(candidate) = pending.take() {
        Some((candidate, true))
    } else {
        active.as_ref().cloned().map(|candidate| (candidate, false))
    }
}

/// Defer endpoint notifications until their scene is really submitted.
fn record_animation_completion_sample(
    published: &mut SmallVec<[crate::SceneAnimationCompletion; 4]>,
    deferred: &mut SmallVec<[crate::SceneAnimationCompletion; 4]>,
    sampled: SmallVec<[crate::SceneAnimationCompletion; 4]>,
    submitted: bool,
    new_candidate: bool,
) {
    if new_candidate {
        deferred.clear();
    }
    if submitted {
        published.extend(deferred.drain(..));
        published.extend(sampled);
    } else {
        deferred.extend(sampled);
    }
}

/// Successfully submitted physical content, waiting on the native GPU fence.
/// A submitted version may be consumed by an ordered GPU queue but MUST NOT
/// be mistaken for a completed version or retired before its fence completes.
#[derive(Default)]
struct SubmittedResourceVersions {
    geometry_epoch: u64,
    root: Option<((TextureId, TextureViewId), u64)>,
    filters: SmallVec<[(u32, filters::TextureContentVersion); 4]>,
}

impl SubmittedResourceVersions {
    /// A GPU submission still owns its FrameSlot/Fence even when it does not
    /// write a retained texture. Such frames must not create empty version
    /// publications or trigger no-op fence completion diagnostics.
    fn new_if_written(
        geometry_epoch: u64,
        root: Option<((TextureId, TextureViewId), u64)>,
        filters: impl IntoIterator<Item = (u32, filters::TextureContentVersion)>,
    ) -> Option<Self> {
        let filters: SmallVec<[_; 4]> = filters.into_iter().collect();
        if root.is_none() && filters.is_empty() {
            return None;
        }
        Some(Self { geometry_epoch, root, filters })
    }
}

struct PendingSubmission {
    submission: SubmissionId,
    frame_resource_index: usize,
    publication: Option<SubmittedResourceVersions>,
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
    buffer: Option<BufferId>,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_resource_set(resource_set) {
        log::debug!("failed to retire {label} old quad resource set: {error}");
    }
    if let Some(buffer) = buffer {
        if let Err(error) = device.destroy_buffer(buffer) {
            log::debug!("failed to retire {label} old quad buffer: {error}");
        }
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
                log::debug!("failed to roll back {label} grown path buffer: {destroy_error}");
            }
            Err(error)
        }
    }
}

fn retire_replaced_path_rasterization_resources<D>(
    device: &mut D,
    label: &str,
    resource_set: ResourceSetId,
    buffer: Option<BufferId>,
) where
    D: BackendResources,
{
    if let Err(error) = device.destroy_resource_set(resource_set) {
        log::debug!("failed to retire {label} old path resource set: {error}");
    }
    if let Some(buffer) = buffer {
        if let Err(error) = device.destroy_buffer(buffer) {
            log::debug!("failed to retire {label} old path buffer: {error}");
        }
    }
}

impl NovaRenderer {
    pub(crate) fn set_live_resize_active(&mut self, active: bool) {
        // The pending DrawableSizeRequest is already authoritative, regardless
        // of gesture state. Do not overwrite a newer size/epoch with an old
        // "release capacity" request on WM_EXITSIZEMOVE.
        self.live_resize_active = active;
    }

    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    /// A replaced native surface needs one successful full present before its Scene can yield
    /// to newer queued work. Only a Scene in the committed physical coordinate space qualifies.
    pub(crate) fn needs_initial_present(&self, scene_extent: Option<(u32, u32)>) -> bool {
        self.swapchain_warmup_frames != 0
            && scene_extent.is_some_and(|(width, height)| {
                width.max(1) == self.current_size.width
                    && height.max(1) == self.current_size.height
            })
    }

    /// Checks native pacing before replacing displayed buffers, then resource/geometry readiness.
    /// Native pacing is resumed by a backend notification; resource retirement uses owner retries.
    pub(crate) fn presentation_readiness(
        &mut self,
        scene_epoch: Option<u64>,
        scene_extent: Option<(u32, u32)>,
    ) -> Result<PresentReadiness> {
        // Do not retire the displayed buffers while DXGI/WSI cannot accept the first
        // replacement frame. Resource preparation may retry, but native resize must
        // not create an uninitialized surface merely to wait for presentation credit.
        if !lock_backend(&self.backend).can_present_without_wait(self.swapchain)? {
            return Ok(PresentReadiness::NativeFrame);
        }
        if !self.apply_pending_drawable_size(scene_epoch, scene_extent)? {
            return Ok(PresentReadiness::Resources);
        }
        if !scene_extent.is_none_or(|(width, height)| {
            width.max(1) == self.current_size.width
                && height.max(1) == self.current_size.height
        }) {
            return Ok(PresentReadiness::Resources);
        }
        Ok(if lock_backend(&self.backend).can_present_without_wait(self.swapchain)? {
            PresentReadiness::Ready
        } else {
            PresentReadiness::NativeFrame
        })
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
        if let Some(previous) = self.pending_presentation_packet.as_ref()
            .or(self.active_presentation_packet.as_ref())
        {
            packet.merge_pending_damage_from(previous);
        }
        let sampled_completions = packet.sample_animations(packet.frame_time);
        let started_at = Instant::now();
        let mut presentation_timing = None;
        let result = self.draw_frame(&mut packet, &mut presentation_timing);
        let submitted = result.as_ref().is_ok_and(|submitted| *submitted);
        if submitted {
            packet.record_presentation(Instant::now());
        }
        record_animation_completion_sample(
            &mut self.pending_animation_completions,
            &mut self.deferred_animation_completions,
            sampled_completions,
            submitted,
            true,
        );
        publish_presentation_candidate(
            &mut self.active_presentation_packet,
            &mut self.pending_presentation_packet,
            packet,
            result.as_ref().is_ok_and(|submitted| *submitted),
        );
        let elapsed = started_at.elapsed();
        crate::diagnostics::performance_metrics::record_frame_backend_draw_time(elapsed);
        crate::diagnostics::performance_metrics::record_first_frame_backend_draw_time(elapsed);
        self.sample_memory(
            self.submitted_frames == 1 && result.as_ref().is_ok_and(|submitted| *submitted),
            result.as_ref().err(),
        );
        result
    }

    fn draw_frame(
        &mut self,
        packet: &mut PresentationPacket,
        presentation_timing: &mut Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<bool> {
        // The drawable extent is applied only for the Scene generation that
        // requested it. An older Scene keeps drawing into the current drawable,
        // and a newer request stays pending for its own Scene.
        if !self.apply_pending_drawable_size(
            Some(packet.paint.geometry.layout_epoch),
            packet.paint.geometry.layout_extent,
        )? {
            return Ok(false);
        }
        // The native size event can overtake an immutable Scene while another
        // window owns the GPU thread. A successful epoch check does not prove
        // that this packet's physical layout matches the CURRENT swapchain.
        // Do not run an offscreen Filter/Scissor/Present at another extent.
        if !scene_matches_committed_drawable(&packet.paint.geometry, self.current_size) {
            log::trace!(
                target: "gpui::diagnostics",
                "nova deferred mismatched scene geometry: window={} scene_epoch={} scene_px={:?} committed_px={}x{} pending_resize={}",
                packet.window_id,
                packet.paint.geometry.layout_epoch,
                packet.paint.geometry.layout_extent,
                self.current_size.width,
                self.current_size.height,
                self.pending_drawable_size.is_some(),
            );
            return Ok(false);
        }
        let drawable_size = packet_drawable_size(packet, self.current_size);
        self.active_size = drawable_size;
        self.observe_presentation_packet(&packet);
        let supports_partial = self.swapchain_warmup_frames == 0
            && surface_alpha_allows_partial_presentation(self.surface_alpha)
            && lock_backend(&self.backend)
                .presentation_capabilities(self.swapchain)
                .partial_presentation;
        resolve_surface_packet(packet, !supports_partial);
        if self.swapchain_warmup_frames != 0 {
            // The backbuffer/retained target dimensions changed. A full Present
            // mode alone does not expand Scene/GPU Layer scissors from old dirty
            // rectangles; the first successful frame must repaint the new extent.
            force_full_drawable_damage_after_resize(packet, self.active_size);
        }
        let backdrop_blur_quality = self.backdrop_blur_quality(packet);
        let upload = self.pack_scene(
            &packet.paint.scene,
            &packet.paint.retained_paint,
            packet.presentation_animation_values.as_slice(),
            drawable_size,
            backdrop_blur_quality,
        );
        self.reconcile_gpu_layer_animation_damage(packet);
        // A native full Present is not a full Filter Source invalidation.
        // Track both independently to catch regressions in ClipReveal/Root.
        log::trace!(
            target: "gpui::diagnostics",
            "GPUI surface damage gate: source=active frame={} live_resize={} swapchain_warmup={} retained_root={} element_sources={} gpu_animation={} timelines={} dirty_full={} dirty_area={} force_filter_refresh={}",
            self.submitted_frames,
            self.live_resize_active,
            self.swapchain_warmup_frames,
            self.frame_upload.root_color.is_some(),
            self.frame_upload.element_blur_inputs.len(),
            packet.sampled_active_presentation_animation,
            packet.presentation_animation_timelines.len(),
            packet.dirty_region.is_full(),
            packet.dirty_region.area(),
            packet.force_full_backdrop_blur_refresh,
        );
        if requires_full_drawable_damage(
            self.live_resize_active,
            self.swapchain_warmup_frames != 0,
        ) {
            force_full_drawable_damage_after_resize(packet, self.active_size);
            self.observe_presentation_packet(packet);
        }
        if !self.frame_submission_is_ready(&packet.paint.scene)? {
            return Ok(false);
        }
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

        // Promotion destroys the old 1x1 path mask texture and rebinds all
        // frame-slot descriptor sets. An earlier frame may still sample its
        // image on the GPU even though the owner thread has moved on.
        // Retire tracked submissions before any view/resource-set replacement.
        if !self.pending_submissions.is_empty() {
            self.wait_for_pending_submissions()?;
        }
        let descriptor = self.path_mask_target_descriptor(target_size);
        let old_target = self.current_path_mask_target();
        let next_target: PathMaskTarget = match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => {
                create_path_mask_target(device, "gpui nova opengl", descriptor)?
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => {
                create_path_mask_target(device, "gpui nova dx11", descriptor)?
            }
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

        self.update_path_mask_resource_sets(&next_target.resource_sets)?;
        self.path_texture = next_target.texture;
        self.path_texture_view = next_target.texture_view;
        self.path_texture_size = target_size;
        self.activate_frame_resources(self.current_frame_resource_index)?;

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => destroy_path_mask_target(device, old_target, "OpenGL"),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => destroy_path_mask_target(device, old_target, "DX11"),
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
            NovaBackend::Unavailable => {}
        }

        log::debug!(
            "nova path mask target promoted from startup placeholder: {}x{}",
            target_size.width(),
            target_size.height()
        );
        Ok(())
    }

    /// Zero-radius sources may retain pixels only after their physical views
    /// and stable source identities survive a Capture topology rebind.
    fn identity_capture_layout_reuse_eligible(&self) -> bool {
        self.filters.is_valid()
            && !present::has_root_backdrop_blurs(&self.frame_upload)
            && self.frame_upload.backdrop_blur_configs().iter()
                .all(|config| config.radius() == 0.0)
    }

    /// AutoStatic exists independently of optional window-color retention.
    /// Only validated zero-filter target chains can keep their content versions
    /// when the popup changes packed Capture order or active target count.
    fn auto_static_layout_reuse_eligible(&self) -> bool {
        self.filters.is_valid()
            && self.frame_upload.root_color.is_none()
            && !present::has_root_backdrop_blurs(&self.frame_upload)
            && self.frame_upload.backdrop_blur_configs().iter()
                .all(|config| config.radius() == 0.0)
            && self.filters.targets.as_ref().is_some_and(|targets| {
                targets.source.is_none()
                    && targets.isolated_sources.is_empty()
                    && targets.variants.iter().take(targets.active_variant_count)
                        .all(|variant| variant.config.radius() == 0.0)
            })
    }

    /// Capture GPU view identities only on layout changes; these are not persistent caches.
    fn identity_capture_views(&self, previous: bool) -> Vec<(u32, TextureViewId)> {
        let Some(targets) = self.filters.targets.as_ref() else { return Vec::new() };
        if previous {
            self.filters.element_blur_inputs.iter().filter_map(|source| {
                targets.exclusive_zero_radius_capture_view(source.index)
                    .map(|view| (source.index, view))
            }).collect()
        } else {
            self.frame_upload.element_blur_inputs.iter().filter_map(|(index, _)| {
                targets.exclusive_zero_radius_capture_view(*index)
                    .map(|view| (*index, view))
            }).collect()
        }
    }

    fn preserve_rebound_element_sources(&mut self, previous: &[(u32, TextureViewId)]) {
        let current = self.identity_capture_views(false);
        let retained = self.filters.remap_identity_capture_sources(
            previous, &current, &self.frame_upload.element_blur_inputs,
        );
        log::trace!(target: "gpui::diagnostics",
            "GPUI identity Capture target rebind: retained_sources={} prior_views={} current_views={}",
            retained, previous.len(), current.len());
    }

    fn ensure_backdrop_blur_targets(&mut self) -> Result<()> {
        self.ensure_root_color()?;
        if self.frame_upload.backdrop_blur_configs().is_empty() && self.filters.targets.is_none() {
            return Ok(());
        }
        if self.filters.targets.as_ref().is_some_and(|targets| {
            targets.is_layout_compatible(
                self.frame_upload.backdrop_blur_configs(),
                self.frame_upload.isolated_blur_source_indices(),
                present::has_root_backdrop_blurs(&self.frame_upload),
            )
        }) {
            return Ok(());
        }
        // An in-flight GPU submission still owns its Filter view/descriptors.
        // Never mutate those allocations through reuse_layout while the GPU
        // can sample them; allocate independent replacements instead, then
        // retire the old physical targets after their exact native fences.
        let has_inflight_readers = !self.pending_submissions.is_empty();
        let capture_reuse_eligible = self.identity_capture_layout_reuse_eligible();
        let auto_static_eligible = self.auto_static_layout_reuse_eligible();
        let auto_static_indices = auto_static_eligible.then(|| {
            self.filters.matching_auto_static_indices(&self.frame_upload.element_blur_inputs)
        }).flatten();
        let previous_capture_views = if capture_reuse_eligible || auto_static_eligible {
            self.identity_capture_views(true)
        } else {
            Vec::new()
        };
        if !has_inflight_readers && self.filters.targets.as_mut().is_some_and(|targets| {
            targets.reuse_layout(
                self.frame_upload.backdrop_blur_configs(),
                self.frame_upload.isolated_blur_source_indices(),
                present::has_root_backdrop_blurs(&self.frame_upload),
                auto_static_indices,
            )
        }) {
            // Root has its own lifetime. Only Filter sources whose physical
            // views and identities survive this rebind retain content versions.
            let preserve_root_color = self.root_color.resident_on(self.root_color.identity());
            if capture_reuse_eligible || auto_static_eligible {
                self.preserve_rebound_element_sources(&previous_capture_views);
            } else {
                self.draw_step_scratch.force_full_backdrop_blur_refresh = true;
                self.invalidate_backdrop_blur_cache();
            }
            self.invalidate_draw_step_cache();
            log::debug!(target: "gpui::diagnostics",
                "GPUI retained target layout reused: root_color_preserved={} auto_static_eligible={} auto_static_relocation={:?} root_content_version={:?} active_variants={} allocated_variants={} active_isolated={} allocated_isolated={}",
                preserve_root_color,
                auto_static_eligible,
                auto_static_indices,
                self.root_color.content_version(),
                self.frame_upload.backdrop_blur_configs().len(),
                self.filters.targets.as_ref().expect("reused targets exist").variants.len(),
                self.filters.targets.as_ref().expect("reused targets exist").active_isolated_source_count,
                self.filters.targets.as_ref().expect("reused targets exist").isolated_sources.len());
            return Ok(());
        }
        log::debug!(
            target: "gpui::diagnostics",
            "GPUI retained target recreation required: requested_variants={} previous_allocated={} previous_active={} previous_root={:?} requested_root={:?} old_root_source={} requested_root_source={} old_isolated={} allocated_isolated={} requested_isolated={} in_flight={}",
            self.frame_upload.backdrop_blur_configs().len(),
            self.filters.targets.as_ref().map_or(0, |targets| targets.variants.len()),
            self.filters.targets.as_ref().map_or(0, |targets| targets.active_variant_count),
            self.root_color.identity(),
            self.frame_upload.root_color.map(|root| root.index),
            self.filters.targets.as_ref().is_some_and(|targets| targets.source.is_some()),
            present::has_root_backdrop_blurs(&self.frame_upload),
            self.filters.targets.as_ref().map_or(0, |targets| targets.active_isolated_source_count),
            self.filters.targets.as_ref().map_or(0, |targets| targets.isolated_sources.len()),
            self.frame_upload.isolated_blur_source_indices().len(),
            self.pending_submissions.len(),
        );
        // Construct all new physical resources first. Old Filter targets
        // remain alive in their own retirement generation until every native
        // submission that could sample them reports Fence::Complete. A failed
        // allocation leaves the Active registry and its GPU views untouched.
        // New target storage has no retained filtered pixels. The first frame using the new chain
        // must therefore rebuild every root backdrop regardless of the current damage footprint.
        self.draw_step_scratch.force_full_backdrop_blur_refresh = true;
        let target_size = Extent2d::new(self.current_size.width, self.current_size.height)?;
        let backdrop_blur_target_descriptor = self.backdrop_blur_target_descriptor(target_size);
        // Record only actual allocation/retirement work, not frame packing,
        // full source rasterization, swapchain ResizeBuffers or GPU shader time.
        // Resize traces previously reported ~310 recreations without any
        // measure of how much of the owner service time they consumed.
        let target_recreation_started = Instant::now();
        let next_backdrop_blur_targets: BackdropBlurTargets = match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova opengl",
                    backdrop_blur_target_descriptor,
                )?;
                targets
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova dx11",
                    backdrop_blur_target_descriptor,
                )?;
                targets
            }
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova dx12",
                    backdrop_blur_target_descriptor,
                )?;
                targets
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                let targets = create_backdrop_blur_target_chain(
                    device,
                    "gpui nova metal",
                    backdrop_blur_target_descriptor,
                )?;
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
                targets
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
        log::debug!(target: "gpui::diagnostics",
            "GPUI retained target layout recreated: active_variants={} active_isolated={} target_bytes={} physical_px={}x{} live_resize={} allocate_retire_us={}",
            next_backdrop_blur_targets.active_variant_count,
            next_backdrop_blur_targets.active_isolated_source_count,
            next_backdrop_blur_targets.byte_size(),
            self.current_size.width,
            self.current_size.height,
            self.live_resize_active,
            target_recreation_started.elapsed().as_micros());
        if let Some(old) = self.filters.targets.replace(next_backdrop_blur_targets) {
            self.retire_offscreen_targets(None, Some(old));
        }
        // Cached steps borrow target resource-set handles. Fence retirement
        // protects submitted work, but these CPU copies also outlive a slot.
        self.invalidate_draw_step_cache();
        self.invalidate_backdrop_blur_cache();
        Ok(())
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        mut packet: PresentationPacket,
    ) -> Result<bool> {
        if let Some(previous) = self.pending_presentation_packet.as_ref()
            .or(self.active_presentation_packet.as_ref())
        {
            packet.merge_pending_damage_from(previous);
        }
        let sampled_completions = packet.sample_animations(packet.frame_time);
        let result = (|| {
            if !self.apply_pending_drawable_size(
                Some(packet.paint.geometry.layout_epoch),
                packet.paint.geometry.layout_extent,
            )? {
                return Ok(false);
            }
            // Framebuffer-only redraws are not allowed to silently render the
            // previous logical size into a different committed native surface.
            if !scene_matches_committed_drawable(&packet.paint.geometry, self.current_size) {
                log::trace!(
                    target: "gpui::diagnostics",
                    "nova deferred mismatched direct scene: window={} scene_epoch={} scene_px={:?} committed_px={}x{}",
                    packet.window_id,
                    packet.paint.geometry.layout_epoch,
                    packet.paint.geometry.layout_extent,
                    self.current_size.width,
                    self.current_size.height,
                );
                return Ok(false);
            }
            let drawable_size = packet_drawable_size(&packet, self.current_size);
            self.active_size = drawable_size;
            self.observe_presentation_packet(&packet);
            let supports_partial = self.swapchain_warmup_frames == 0
                && surface_alpha_allows_partial_presentation(self.surface_alpha)
                && lock_backend(&self.backend)
                    .presentation_capabilities(self.swapchain)
                    .partial_presentation;
            resolve_surface_packet(&mut packet, !supports_partial);
            if self.swapchain_warmup_frames != 0 {
                force_full_drawable_damage_after_resize(&mut packet, self.active_size);
            }
            let backdrop_blur_quality = self.backdrop_blur_quality(&packet);
            let upload = self.pack_scene(
                &packet.paint.scene,
                &packet.paint.retained_paint,
                packet.presentation_animation_values.as_slice(),
                drawable_size,
                backdrop_blur_quality,
            );
            self.reconcile_gpu_layer_animation_damage(&mut packet);
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI surface damage gate: source=direct frame={} live_resize={} swapchain_warmup={} retained_root={} element_sources={} gpu_animation={} timelines={} dirty_full={} dirty_area={} force_filter_refresh={}",
                self.submitted_frames,
                self.live_resize_active,
                self.swapchain_warmup_frames,
                self.frame_upload.root_color.is_some(),
                self.frame_upload.element_blur_inputs.len(),
                packet.sampled_active_presentation_animation,
                packet.presentation_animation_timelines.len(),
                packet.dirty_region.is_full(),
                packet.dirty_region.area(),
                packet.force_full_backdrop_blur_refresh,
            );
            if requires_full_drawable_damage(
                self.live_resize_active,
                self.swapchain_warmup_frames != 0,
            ) {
                force_full_drawable_damage_after_resize(&mut packet, self.active_size);
                self.observe_presentation_packet(&packet);
            }
            if !self.frame_submission_is_ready(&packet.paint.scene)? {
                return Ok(false);
            }
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
        let submitted = result.as_ref().is_ok_and(|submitted| *submitted);
        if submitted {
            packet.record_presentation(Instant::now());
        }
        record_animation_completion_sample(
            &mut self.pending_animation_completions,
            &mut self.deferred_animation_completions,
            sampled_completions,
            submitted,
            true,
        );
        publish_presentation_candidate(
            &mut self.active_presentation_packet,
            &mut self.pending_presentation_packet,
            packet,
            result.as_ref().is_ok_and(|submitted| *submitted),
        );
        self.sample_memory(
            self.submitted_frames == 1 && result.as_ref().is_ok_and(|submitted| *submitted),
            result.as_ref().err(),
        );
        result
    }

    /// Present the active immutable scene using a compositor-owned frame timestamp.
    pub(crate) fn present_active_frame(
        &mut self,
        now: Instant,
        presentation_timing: Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<Option<ActivePresentationFrame>> {
        // Inspect scheduling state by reference first. No packet clone or
        // ownership transfer is needed for idle or not-yet-due ticks.
        let was_pending = self.pending_presentation_packet.is_some();
        let Some(reference) = self.pending_presentation_packet.as_ref()
            .or(self.active_presentation_packet.as_ref())
        else {
            return Ok(None);
        };
        // A Pending new-size Scene must be submitted even without dirty pixels.
        if !was_pending && !reference.has_pending_presentation() {
            return Ok(None);
        }
        if !reference.presentation_is_due(now) {
            let continues = !reference.presentation_animation_timelines.is_empty();
            return Ok((continues || was_pending).then(|| ActivePresentationFrame {
                continues: continues || was_pending,
                completed_animations: SmallVec::new(),
            }));
        }
        // Only Pending may be taken in place. The Active packet remains the
        // exact last submitted generation until the new GPU submission succeeds.
        // Cloning the small packet metadata does not clone its Arc<Paint>.
        let Some((mut packet, _)) = take_presentation_candidate(
            &self.active_presentation_packet,
            &mut self.pending_presentation_packet,
        ) else {
            return Ok(None);
        };
        let sampled_completions = packet.sample_animations(now);
        let continues = !packet.presentation_animation_timelines.is_empty();
        let mut presentation_timing = presentation_timing;
        let draw_result = self.draw_frame(&mut packet, &mut presentation_timing);
        let submitted = draw_result.as_ref().is_ok_and(|submitted| *submitted);
        if submitted {
            packet.record_presentation(Instant::now());
        }
        record_animation_completion_sample(
            &mut self.pending_animation_completions,
            &mut self.deferred_animation_completions,
            sampled_completions,
            submitted,
            false,
        );
        publish_presentation_candidate(
            &mut self.active_presentation_packet,
            &mut self.pending_presentation_packet,
            packet,
            submitted,
        );
        self.sample_memory(
            self.submitted_frames == 1 && submitted,
            draw_result.as_ref().err(),
        );
        if !draw_result? {
            return Ok(None);
        }
        Ok(Some(ActivePresentationFrame {
            continues,
            completed_animations: self.take_animation_completions(),
        }))
    }

    /// Match a deferred Scene against its original native Resize generation.
    /// A GPU Owner tick without a UI Draw must not pass None as the geometry
    /// epoch: it would perpetually defer a versioned swapchain request.
    pub(crate) fn pending_presentation_epoch(&self) -> Option<u64> {
        self.pending_presentation_packet
            .as_ref()
            .map(|packet| packet.paint.geometry.layout_epoch)
    }

    pub(crate) fn pending_presentation_extent(&self) -> Option<(u32, u32)> {
        self.pending_presentation_packet
            .as_ref()
            .and_then(|packet| packet.paint.geometry.layout_extent)
    }

    pub(crate) fn has_active_presentation_animations(&self) -> bool {
        self.pending_presentation_packet.is_some()
            || self.active_presentation_packet
                .as_ref()
                .is_some_and(PresentationPacket::has_pending_presentation)
    }

    pub(crate) fn active_presentation_is_due(&self, now: Instant) -> bool {
        self.pending_presentation_packet.as_ref()
            .or(self.active_presentation_packet.as_ref())
            .is_some_and(|packet| {
                (self.pending_presentation_packet.is_some()
                    || packet.has_pending_presentation())
                    && packet.presentation_is_due(now)
            })
    }

    pub(crate) fn set_frame_interval(&mut self, interval: Option<std::time::Duration>) {
        if let Some(packet) = self.active_presentation_packet.as_mut() {
            packet.set_frame_interval(interval);
        }
        if let Some(packet) = self.pending_presentation_packet.as_mut() {
            packet.set_frame_interval(interval);
        }
    }

    pub(crate) fn presentation_deadline(&self) -> Option<Instant> {
        self.pending_presentation_packet.as_ref()
            .or(self.active_presentation_packet.as_ref())
            .and_then(PresentationPacket::presentation_deadline)
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
        if matches!(level, GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive) {
            // Unsubmitted Pending Root/Filter allocations have no GPU readers:
            // free these first under memory pressure, never the Active versions.
            if let Some(pending) = self.pending_offscreen_targets.take() {
                self.destroy_obsolete_pending_offscreen(pending);
            }
            self.discard_pending_surface_attachments();
        }
        if submissions_drained && matches!(level, GpuiMemoryTrimLevel::Moderate) {
            if let Err(error) = self.compact_atlas_page() {
                log::debug!("failed to compact nova atlas page: {error}");
            }
        }
        self.frame_upload.trim_retained_capacity(level);
        self.retained_upload.quad_chunks.trim(level);
        self.draw_step_scratch.trim_retained_capacity(level);
        // Filter and Root views are still referenced by pending GPU command buffers.
        // A failed fence drain must NEVER convert a memory-pressure hint into
        // early native destruction of textures or descriptor sets.
        if submissions_drained
            && matches!(
                level,
                GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive
            )
            && self.frame_upload.backdrop_blurs.is_empty()
        {
            self.destroy_backdrop_blur_targets();
            self.retire_root_color();
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

        if submissions_drained && matches!(level, GpuiMemoryTrimLevel::Moderate) {
            match lock_backend(&self.backend).compact_memory() {
                Ok(report) if report.moved_resources != 0 => log::debug!(
                    "nova heap compact: moved={} bytes={} reserved={} -> {}",
                    report.moved_resources,
                    report.moved_bytes,
                    report.reserved_before,
                    report.reserved_after
                ),
                Ok(_) => {}
                Err(error) => log::debug!("failed to compact nova backend heap: {error}"),
            }
        }
        if let Err(error) = lock_backend(&self.backend).trim_memory(memory_trim_level(level)) {
            log::debug!("failed to trim nova-gfx backend memory: {error}");
        }
        self.sample_memory(true, None);
    }

    pub(crate) fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.active_presentation_packet = None;
        self.pending_presentation_packet = None;
        self.deferred_animation_completions.clear();
        let destroy_started_at = Instant::now();
        if let Err(error) = self.wait_for_pending_submissions() {
            log::debug!("failed to drain nova-gfx submissions during renderer destroy: {error}");
        }
        let submission_wait = destroy_started_at.elapsed();
        log::debug!(target: "gpui_gpu_shutdown", "renderer submissions drained backend={}", self.backend_info.label());
        self.destroy_renderer_extensions();
        log::debug!(target: "gpui_gpu_shutdown", "renderer extensions destroyed backend={}", self.backend_info.label());
        self.destroy_window_resources();
        self.memory_profile.remove();
        let total = destroy_started_at.elapsed();
        if total >= std::time::Duration::from_millis(16) {
            log::warn!(target: "gpui_gpu_shutdown",
                "slow_renderer_destroy backend={} total_us={} submission_wait_us={} resources_us={}",
                self.backend_info.label(), total.as_micros(), submission_wait.as_micros(),
                total.saturating_sub(submission_wait).as_micros());
        }
    }

    /// The Window can submit a compositor-owned animation without an
    /// accompanying View commit. Repair the root retained-color damage after
    /// the packed animation values are known, and before GPU Pass planning.
    fn reconcile_gpu_layer_animation_damage(&mut self, packet: &mut PresentationPacket) {
        if self.frame_upload.root_color.is_none() || self.root_color.content_version().is_none() {
            return;
        }
        let damage = crate::paint::gpu_sample_damage(
            &packet.paint.scene,
            self.root_color.animation_values(),
            &self.frame_upload.sampled_animation_values,
        );
        if damage.is_empty() {
            return;
        }
        for bounds in damage {
            packet.dirty_region.push(bounds);
        }
        // The same repaired region must control root source clearing, texture
        // draw-step scissors and swapchain partial eligibility.
        self.draw_step_scratch.backdrop_blur_damage_region = packet.dirty_region.clone();
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
        let source_tiles = self.frame_upload.backdrop_source_atlas_tiles();
        if self.atlas.pending_uploads_touch_source_tiles(&source_atlases, &source_tiles) {
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
        if let Some(targets) = self.filters.targets.take() {
            // Reuse the same exact-submission retirement protocol for memory
            // pressure, layout removal and future compositor target switches.
            self.retire_offscreen_targets(None, Some(targets));
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
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => create_grown_quad_resources(
                device,
                "gpui nova opengl grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => create_grown_quad_resources(
                device,
                "gpui nova dx11 grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
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
        let old_buffer = (!self.buffer_is_referenced(old_buffer)).then_some(old_buffer);

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => retire_replaced_quad_resources(
                device,
                "gpui nova opengl",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => retire_replaced_quad_resources(
                device,
                "gpui nova dx11",
                old_resource_set,
                old_buffer,
            ),
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
        let required_vertices = required_bytes.div_ceil(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
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
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => create_grown_path_rasterization_resources(
                device,
                "gpui nova opengl grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => create_grown_path_rasterization_resources(
                device,
                "gpui nova dx11 grown",
                layout,
                current.buffers,
                new_capacity,
            )?,
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
        let old_buffer = (!self.buffer_is_referenced(old_buffer)).then_some(old_buffer);

        match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => retire_replaced_path_rasterization_resources(
                device,
                "gpui nova opengl",
                old_resource_set,
                old_buffer,
            ),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => retire_replaced_path_rasterization_resources(
                device,
                "gpui nova dx11",
                old_resource_set,
                old_buffer,
            ),
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
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => sync_gpu_atlas_textures(
                &self.atlas,
                &mut self.gpu_atlas_textures,
                device,
                "gpui nova opengl",
                descriptor,
            ),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => sync_gpu_atlas_textures(
                &self.atlas,
                &mut self.gpu_atlas_textures,
                device,
                "gpui nova dx11",
                descriptor,
            ),
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
    fn resize_scene_and_committed_surface_must_have_identical_pixel_geometry() {
        let current = DrawableSize { width: 972, height: 600 };
        let mut geometry = crate::window::PaintGeometry {
            layout_extent: Some((972, 600)),
            layout_epoch: 7,
            scale_factor: 1.0,
        };
        assert!(scene_matches_committed_drawable(&geometry, current));
        // Same epoch does not rescue an old layout from a new HWND drawable.
        geometry.layout_extent = Some((971, 600));
        assert!(!scene_matches_committed_drawable(&geometry, current));
        geometry.layout_extent = Some((972, 601));
        assert!(!scene_matches_committed_drawable(&geometry, current));
        geometry.layout_extent = Some((972, 600));
        geometry.layout_epoch += 1;
        assert!(scene_matches_committed_drawable(&geometry, current));
        // Legacy/headless Paint without physical provenance uses the committed
        // surface and must not stall unrelated backend tests.
        geometry.layout_extent = None;
        assert!(scene_matches_committed_drawable(&geometry, current));
    }

    #[test]
    fn empty_spatial_damage_does_not_force_full_blur_refresh() {
        assert!(!force_full_backdrop_blur_refresh(true, false));
    }

    #[test]
    fn invalid_cache_still_forces_full_blur_refresh() {
        assert!(force_full_backdrop_blur_refresh(false, false));
    }

    #[test]
    fn animation_completion_events_follow_successful_presentation() {
        let completion = |generation| crate::SceneAnimationCompletion {
            animation_id: crate::SceneAnimationId(3),
            property: crate::TransitionProperty::Opacity,
            generation,
            view_id: None,
            retained_id: None,
        };
        let mut published = SmallVec::new();
        let mut deferred = SmallVec::new();
        record_animation_completion_sample(
            &mut published, &mut deferred,
            smallvec::smallvec![completion(1)], false, true,
        );
        assert!(published.is_empty());
        assert_eq!(deferred.len(), 1);
        record_animation_completion_sample(
            &mut published, &mut deferred,
            SmallVec::new(), false, false,
        );
        assert_eq!(deferred.len(), 1);
        // Superseding a never-presented Scene drops only its events.
        record_animation_completion_sample(
            &mut published, &mut deferred,
            smallvec::smallvec![completion(2)], false, true,
        );
        assert!(published.is_empty());
        assert_eq!(deferred[0].generation, 2);
        record_animation_completion_sample(
            &mut published, &mut deferred,
            SmallVec::new(), true, false,
        );
        assert!(deferred.is_empty());
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].generation, 2);
    }

    #[test]
    fn empty_resource_publication_skips_fence_version_bookkeeping() {
        assert!(SubmittedResourceVersions::new_if_written(
            12, None, std::iter::empty(),
        ).is_none());
        let root = (
            (TextureId::from_parts(4, 1), TextureViewId::from_parts(4, 1)),
            9u64,
        );
        let root_only = SubmittedResourceVersions::new_if_written(
            13, Some(root), std::iter::empty(),
        ).expect("root refresh");
        assert_eq!(root_only.root, Some(root));
        assert!(root_only.filters.is_empty());
        let filter_only = SubmittedResourceVersions::new_if_written(
            14, None, [(3, filters::TextureContentVersion(9))],
        ).expect("filter refresh");
        assert_eq!(filter_only.geometry_epoch, 14);
        assert_eq!(filter_only.filters.len(), 1);
    }

    #[test]
    fn pending_geometry_does_not_evict_last_submitted_active_scene() {
        let mut active = Some(10u64);
        let mut pending = None;
        publish_presentation_candidate(&mut active, &mut pending, 11, false);
        assert_eq!(active, Some(10));
        assert_eq!(pending, Some(11));
        publish_presentation_candidate(&mut active, &mut pending, 12, false);
        assert_eq!(active, Some(10));
        assert_eq!(pending, Some(12), "latest candidate supersedes stalled geometry");
        publish_presentation_candidate(&mut active, &mut pending, 12, true);
        assert_eq!(active, Some(12));
        assert!(pending.is_none());
    }

    #[test]
    fn compositor_only_tick_failure_preserves_committed_active_animation_sample() {
        // Unlike a new UI scene, an animation tick starts from the committed
        // Active. Mutating the sampled candidate must never overwrite it on
        // either a deferred or failed GPU submission.
        let mut active = Some(vec![10u64, 20]);
        let mut pending = None;
        let (mut candidate, was_pending) =
            take_presentation_candidate(&active, &mut pending).expect("active tick");
        assert!(!was_pending);
        candidate.push(30);
        publish_presentation_candidate(&mut active, &mut pending, candidate, false);
        assert_eq!(active.as_deref(), Some(&[10, 20][..]));
        assert_eq!(pending.as_deref(), Some(&[10, 20, 30][..]));

        // A retry owns Pending directly, so it does not clone the committed
        // scene or accumulate duplicate intermediate animation samples.
        let (mut retry, was_pending) =
            take_presentation_candidate(&active, &mut pending).expect("retry");
        assert!(was_pending);
        assert!(pending.is_none());
        retry.push(40);
        publish_presentation_candidate(&mut active, &mut pending, retry, true);
        assert_eq!(active.as_deref(), Some(&[10, 20, 30, 40][..]));
        assert!(pending.is_none());
    }

    #[test]
    fn retry_failure_must_leave_pending_scene_and_active_scene_unchanged() {
        let mut active = Some(20u64);
        let mut pending = Some(21);
        let candidate = pending.take().unwrap();
        publish_presentation_candidate(&mut active, &mut pending, candidate, false);
        assert_eq!(active, Some(20));
        assert_eq!(pending, Some(21));
        let candidate = pending.take().unwrap();
        publish_presentation_candidate(&mut active, &mut pending, candidate, true);
        assert_eq!(active, Some(21));
        assert!(pending.is_none());
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
