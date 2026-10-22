//! Backend capability traits for nova-gfx.
//!
//! These traits are the public contract implemented by concrete nova-gfx
//! backends. They keep backend users generic over Direct3D 11/12, Vulkan, OpenGL,
//! and Metal while preserving static dispatch for hot rendering paths.

use std::time::Duration;

use crate::{
    AsyncCapabilities, BackendKind, BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor,
    BindGroupLayoutId, BufferDescriptor, BufferId, BufferUploadStats, BufferWrite,
    CommandEncoderDescriptor, CommandEncoderId, CompositeAlphaMode, DeviceMemoryBudget,
    DrawDescriptor, Error, Extent2d, MemoryArchitecture, PipelineLayoutDescriptor,
    PipelineLayoutId, PresentFrameDescriptor, RenderPassCommands, RenderPassDescriptor,
    RenderPassId, RenderPipelineDescriptor, RenderPipelineId, ResourceStats, Result,
    SamplerDescriptor, SamplerId, ShaderModuleDescriptor, ShaderModuleId, SubmissionId,
    SubmissionStatus, SurfaceCapabilities, SurfaceConfig, SurfaceDescriptor, SurfaceId,
    SwapchainId, TextureDescriptor, TextureId, TextureReadback, TextureViewDescriptor,
    TextureViewId, TextureWrite, TextureWriteDescriptor,
};

/// Identifies the graphics API implemented by a backend type.
///
/// Implementors should use this associated constant for diagnostics, adapter
/// selection, and logs. It must describe the concrete backend used by the
/// implementing type.
pub trait Backend {
    /// Graphics API exposed by this backend implementation.
    const BACKEND_KIND: BackendKind;

    /// Returns the adapter name selected when this device was created.
    fn adapter_name(&self) -> &str;
}

/// Complete device contract for a nova-gfx backend.
///
/// This is a convenience trait for call sites that need the full backend API.
/// Prefer narrower traits such as [`ResourceDevice`] or [`PipelineDevice`]
/// on helper functions that only need part of the device surface.
pub trait Device:
    Backend
    + SurfaceDevice
    + ResourceDevice
    + PipelineDevice
    + CommandDevice
    + SubmissionDevice
    + PresentationDevice
    + DiagnosticsDevice
{
}

impl<T> Device for T where
    T: Backend
        + SurfaceDevice
        + ResourceDevice
        + PipelineDevice
        + CommandDevice
        + SubmissionDevice
        + PresentationDevice
        + DiagnosticsDevice
{
}

/// Creates and destroys native window surfaces and swapchains.
///
/// Surface and swapchain handles are owned by the device that created them.
/// Passing a handle to another device, or reusing it after destruction, must
/// return [`Error::InvalidInput`].
pub trait SurfaceDevice {
    /// Backend-defined native presentation target.
    ///
    /// `gfx-core` deliberately does not define the window-handle ABI. Backend
    /// crates or platform adapters choose the concrete target type they support.
    type SurfaceTarget: ?Sized;

    /// Creates a backend surface for a native window.
    ///
    /// The target must be a valid native presentation target for the backend.
    /// The surface does not own that target; callers must keep it alive until
    /// all swapchains and the surface are destroyed.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the target handles are invalid, the platform is
    /// unsupported, or the backend cannot create a presentable surface.
    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId>;

    /// Creates a swapchain for an existing surface.
    ///
    /// The `surface` handle must have been created by this device and must not
    /// already be destroyed. The returned swapchain is tied to that surface.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the surface handle is invalid, the configuration
    /// is unsupported, or the backend cannot allocate swapchain images.
    fn create_swapchain(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId>;

    /// Polls a retirement point for this swapchain's current native buffers.
    ///
    /// The first call may enqueue a fence/query after earlier rendering and Present work;
    /// subsequent calls reuse it. Returns `false` while that work is pending, without waiting
    /// on the CPU. Later work for another swapchain must not move this retirement point.
    /// Rendering or presenting to this swapchain invalidates the preparation. A successful
    /// resize consumes it; destruction also releases it. This does not resize buffers, publish
    /// geometry, or establish physical scanout completion. Callers must still use matching
    /// scene geometry and serialize native resize/submit/destroy operations on the device owner.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unavailable`] when nonblocking resize preparation is unsupported,
    /// [`Error::InvalidInput`] for a stale/foreign handle, or a backend error on query failure.
    fn prepare_swapchain_resize(&mut self, _swapchain: SwapchainId) -> Result<bool> {
        Err(Error::Unavailable(
            "nonblocking swapchain resize preparation is unsupported".into(),
        ))
    }

    /// Resizes a swapchain in place, preserving its format, present mode, and alpha mode.
    ///
    /// The new size must be nonzero and within the backend's surface limits. Backends that
    /// cannot resize in place return [`Error::Unavailable`]; callers then recreate the swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is stale or the native resize fails.
    fn resize_swapchain(&mut self, _swapchain: SwapchainId, _size: Extent2d) -> Result<()> {
        Err(Error::Unavailable(
            "in-place swapchain resize is not supported by this backend".into(),
        ))
    }

    /// Recreates a swapchain with a new configuration.
    ///
    /// Returns the handle that owns the new native images. Backends that reconfigure in place
    /// return `swapchain`; backends that replace the native swapchain return a new handle and
    /// retire the old one. The default resizes the native images and keeps the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is stale or the configuration is unsupported.
    fn recreate_swapchain(
        &mut self,
        swapchain: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        self.resize_swapchain(swapchain, config.size)?;
        Ok(swapchain)
    }

    /// Returns the current native swapchain extent.
    ///
    /// This can differ from the configured extent when the platform window system resolves a
    /// different physical size. Backends without a native extent query return [`Error::Unavailable`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is stale or the query fails.
    fn swapchain_extent(&self, _swapchain: SwapchainId) -> Result<Extent2d> {
        Err(Error::Unavailable(
            "native swapchain extent query is not supported by this backend".into(),
        ))
    }

    /// Resolves a requested composite alpha mode for this surface.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the surface is stale or capability querying fails.
    fn resolve_composite_alpha(
        &self,
        _surface: SurfaceId,
        requested: CompositeAlphaMode,
    ) -> Result<CompositeAlphaMode> {
        Ok(requested)
    }

    /// Selects the active render viewport and top-left source rectangle used by the next
    /// swapchain presentation.
    ///
    /// This lets a backend retain a larger allocation during live resize while presenting the
    /// current client-sized region at native pixel scale. Backends must render into this active
    /// extent even when their attachment allocation is larger. The extent must be nonzero and no
    /// larger than the configured swapchain extent. Backends that naturally present the active
    /// native surface region may keep the default implementation.
    ///
    /// The selected extent is presented one-to-one when the surface matches the client area. A
    /// native client area that grew ahead of this extent may transiently be covered by the
    /// platform compositor scaling the last presented frame; that is a surface fallback, not a
    /// change to the scene geometry.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is stale, the extent is outside its allocation, or the
    /// backend cannot select the requested source region.
    fn set_swapchain_source_extent(
        &mut self,
        _swapchain: SwapchainId,
        _extent: Extent2d,
    ) -> Result<()> {
        Ok(())
    }

    /// Destroys a swapchain created by this device.
    ///
    /// After this call succeeds, the handle must not be used again. Destroying a
    /// surface before its swapchain is backend-invalid and should be rejected.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale, invalid, or still in use by
    /// backend work that cannot be retired.
    fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()>;

    /// Destroys a surface created by this device.
    ///
    /// After this call succeeds, the handle must not be used again.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale, invalid, or still owns live
    /// swapchain resources.
    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()>;
}

/// Creates, updates, and destroys GPU resource objects.
///
/// All handles passed to these methods must belong to the same device. Backends
/// should validate descriptors before creating native resources and report bad
/// inputs with [`Error::InvalidInput`].
pub trait ResourceDevice {
    /// Queues GPU pixel copies, ordered before subsequent draws on this device.
    /// Source/destination lifetimes must cover completion; destruction obeys normal submission
    /// retirement. This method does not imply GPU completion and never reads pixels to the CPU.
    /// All rectangles are validated before recording the batch.
    ///
    /// # Errors
    /// Returns InvalidInput for invalid handles/rectangles, Backend for recording failure, or
    /// Unavailable when native texture copying is not implemented.
    fn copy_texture_batch(&mut self, copies: &[crate::TextureCopy]) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        Err(Error::Unavailable(
            "GPU texture copy is not implemented".into(),
        ))
    }
    /// Creates a buffer resource.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or native allocation
    /// fails.
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId>;

    /// Writes bytes into a buffer.
    ///
    /// `buffer` must identify a live CPU-visible or upload-compatible buffer
    /// created by this device. `offset + data.len()` must fit inside the buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is invalid, the write is out of bounds,
    /// or the backend cannot map or stage the upload.
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()>;

    /// Writes borrowed buffer updates in order on the device's owning thread.
    ///
    /// Empty writes are ignored. Before returning successfully, source bytes must have been
    /// consumed or copied into backend-owned staging, and uploads must be ordered before
    /// subsequent draws using those destinations. This does not mean GPU work has completed:
    /// destination/staging retirement remains governed by the backend's submission fences.
    /// It does not permit overwriting a buffer still read by an earlier submission; callers
    /// must use the existing resource-slot/fence rules. The default uses the single-write path;
    /// backends may batch native work and must account for any expanded destination writes.
    ///
    /// # Errors
    ///
    /// Returns the first validation or upload error. The operation is not transactional:
    /// earlier writes may have taken effect, so a failed plan must not be marked resident.
    fn write_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        crate::buffer_upload::execute_buffer_batch(writes, |write| {
            self.write_buffer(write.descriptor.buffer, write.descriptor.offset, write.data)?;
            Ok(BufferUploadStats {
                calls: 1,
                bytes: write.data.len() as u64,
            })
        })
    }

    /// Creates a texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or native allocation
    /// fails.
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId>;

    /// Writes pixel bytes into a texture.
    ///
    /// `desc.texture` must identify a live texture created by this device. The
    /// data layout and source byte slice must cover the requested upload region.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is invalid, the layout is invalid, the
    /// data slice is too short, or the backend cannot stage the upload.
    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()>;

    /// Writes a batch of texture uploads in order.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error`] reported by [`Self::write_texture`], or a
    /// backend-specific batch upload error. This operation is not transactional; earlier writes
    /// may have taken effect when a later write fails.
    fn write_texture_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = TextureWrite<'a>>,
    ) -> Result<()> {
        for write in writes {
            self.write_texture(write.descriptor, write.data)?;
        }
        Ok(())
    }

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the source texture handle is invalid or the view
    /// descriptor is incompatible with the texture.
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId>;

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the sampler descriptor is unsupported by the
    /// backend.
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId>;

    /// Creates a bind group layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout descriptor is invalid or unsupported.
    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId>;

    /// Creates a bind group from live resources.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout or any bound resource handle is invalid,
    /// or if bindings do not match the layout.
    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId>;

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()>;

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_texture(&mut self, texture: TextureId) -> Result<()>;

    /// Destroys a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()>;

    /// Destroys a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()>;

    /// Destroys a bind group layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()>;

    /// Destroys a bind group.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()>;
}

/// Synchronizes and inspects native texture transfers.
///
/// This capability is separate from [`ResourceDevice`]: rendering can submit uploads without
/// requiring synchronous readback or timestamp support from every backend.
pub trait TextureTransferDevice: ResourceDevice {
    /// Returns whether native GPU timestamp queries are available for texture transfers.
    #[must_use]
    fn texture_transfer_timestamps_supported(&self) -> bool;

    /// Waits until texture transfers submitted before this call have completed.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend wait or completion processing fails.
    fn wait_texture_transfers(&mut self) -> Result<()>;

    /// Returns the most recently completed native texture-transfer GPU duration.
    #[must_use]
    fn last_texture_transfer_time(&self) -> Option<Duration>;

    /// Copies mip level zero into tightly packed CPU memory.
    ///
    /// The texture must have [`crate::TextureUsage::COPY_SRC`]. This method waits for earlier
    /// writes before recording the readback copy.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the texture is invalid, lacks copy-source usage, synchronization
    /// fails, or the backend cannot map the readback allocation.
    fn read_texture(&mut self, texture: TextureId) -> Result<TextureReadback>;
}

/// Creates and destroys shader and pipeline objects.
///
/// Pipeline handles and layout handles are device-local. Callers must keep
/// dependent shader modules, render passes, and layouts alive while creating
/// pipelines that reference them.
pub trait PipelineDevice {
    /// Creates a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or references stale
    /// bind group layouts.
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId>;

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if shader code is empty or not compatible with the
    /// backend.
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId>;

    /// Creates a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the render pass descriptor is unsupported.
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId>;

    /// Creates a render pipeline.
    ///
    /// `viewport_extent` is the size used for fixed-function viewport and
    /// scissor state in backends that bake it into the pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if any referenced handle is invalid, shader stages do
    /// not match, or the backend cannot create the native pipeline.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: crate::Extent2d,
    ) -> Result<RenderPipelineId>;

    /// Destroys a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()>;

    /// Destroys a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()>;

    /// Destroys a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()>;

    /// Destroys a render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()>;
}

/// Records and submits explicit command encoder work.
pub trait CommandDevice {
    /// Creates a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the backend cannot allocate command recording
    /// resources.
    fn create_command_encoder(
        &mut self,
        desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId>;

    /// Records one draw pass into a command encoder.
    ///
    /// All handles referenced by `draw` must be live and belong to this device.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the encoder or any referenced resource is invalid,
    /// or if the backend rejects the draw state.
    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDescriptor) -> Result<()>;

    /// Submits a command encoder for execution.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the encoder handle is invalid or queue submission
    /// fails.
    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()>;

    /// Destroys a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()>;
}

/// Tracks deferred GPU submissions.
pub trait SubmissionDevice {
    /// Returns async and threading capabilities for this device.
    #[must_use]
    fn async_capabilities(&self) -> AsyncCapabilities {
        AsyncCapabilities::default()
    }

    /// Submits a command encoder without waiting for GPU completion.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the encoder is invalid, the backend cannot submit
    /// it, or the backend cannot track a deferred submission.
    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId>
    where
        Self: CommandDevice,
    {
        self.submit(encoder)?;
        Ok(SubmissionId::from_parts(0, 0))
    }

    /// Polls a previously returned submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when `submission` is not known to this device.
    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        if submission.raw() == 0 {
            Ok(SubmissionStatus::Complete)
        } else {
            Err(Error::InvalidInput(format!(
                "unknown submission {}",
                submission.raw()
            )))
        }
    }

    /// Blocks until a submission has completed.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when waiting fails or the backend reports a failed
    /// submission.
    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        match self.poll_submission(submission)? {
            SubmissionStatus::Complete => Ok(()),
            SubmissionStatus::Pending => Err(Error::Unavailable(
                "submission wait is not implemented by this backend".to_string(),
            )),
            SubmissionStatus::Failed(error) => Err(Error::Backend(error)),
        }
    }
}

/// Host-side time spent in backend presentation stages.
///
/// Absent or uninstrumented stages (for example, explicit DXGI image acquisition) remain zero.
/// These measurements describe host calls, not GPU execution time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentTimings {
    /// Time waiting for the image's previous in-flight fence.
    pub acquire_fence_wait: Duration,
    /// Time waiting for a swapchain image from `acquire_next_image`.
    pub image_acquire: Duration,
    /// Host time spent allocating the per-frame command pool and buffer.
    pub command_encoder_create: Duration,
    /// Host time spent translating render commands into backend command buffers.
    pub command_record: Duration,
    /// Host time spent resetting the swapchain image fence before submission.
    pub fence_reset: Duration,
    /// Host time spent submitting the graphics queue work.
    pub queue_submit: Duration,
    /// Time waiting for the submitted frame when deferred presentation is disabled.
    pub submission_wait: Duration,
    /// Host time spent in the native presentation queue call.
    pub queue_present: Duration,
    /// Host time spent retiring the encoder and polling deferred resource cleanup.
    pub post_present_cleanup: Duration,
}

/// A result from a presentation attempt that may have been deferred by the surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentedFrame {
    /// GPU submission to track, if this backend uses deferred submission.
    pub submission: Option<SubmissionId>,
    /// Backend-specific host timings when available.
    pub timings: Option<PresentTimings>,
}

/// Provides swapchain presentation and offscreen render helpers.
///
/// These are the normalized frame APIs. Backend-specific acquire/present
/// synchronization stays inside each backend crate.
pub trait PresentationDevice {
    /// Registers a one-shot notification that this swapchain can accept a frame.
    ///
    /// `Ok(true)` means registration was accepted. The callback may run before this
    /// method returns and runs at most once; replacement, resize or destruction may
    /// cancel it. A new registration replaces an earlier pending registration for
    /// the same swapchain. `Ok(false)` means unsupported and does not invoke it.
    /// The callback must only enqueue work: it must not call the device or wait for
    /// its owner, because cancellation may drain it while the owner holds the device.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale swapchain or failed native registration. An
    /// unsuccessful registration does not invoke the supplied callback.
    fn request_frame_ready(
        &mut self,
        _swapchain: SwapchainId,
        _callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        Ok(false)
    }

    /// Reports swapchain presentation facilities, separate from device submission capabilities.
    ///
    /// Backends reporting partial presentation must keep every rotating back buffer coherent, restrict rendering
    /// to the effective changed region, and pass that same region to the native presentation API.
    ///
    /// Unsupported facilities and stale swapchain IDs return default capabilities.
    fn surface_capabilities(&self, _swapchain: SwapchainId) -> SurfaceCapabilities {
        SurfaceCapabilities::default()
    }

    /// Returns whether the swapchain can accept another frame without blocking the caller.
    ///
    /// Backends whose native presentation would park until a frame slot becomes available
    /// return `false` so the caller can defer the frame instead of blocking its owner thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the swapchain is stale or the native readiness query fails.
    fn can_present_now(&mut self, _swapchain: SwapchainId) -> Result<bool> {
        Ok(true)
    }

    /// Stretches the swapchain's composited content over a surface larger than its buffers.
    ///
    /// `scale` holds the new surface size divided by the current buffer size; `None` resets to
    /// identity. Compositor-backed swapchains use this between a native surface resize and the
    /// next buffer resize so the previous frame keeps covering the surface. Backends without
    /// such a visual ignore the request.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend fails to apply or reset the stretch.
    fn set_content_stretch(
        &mut self,
        _swapchain: SwapchainId,
        _scale: Option<[f32; 2]>,
    ) -> Result<()> {
        Ok(())
    }

    /// Presents the retained scene target over a new native extent without rendering.
    ///
    /// A native client area can grow ahead of the next buffer resize. Backends whose
    /// compositor already scales the last presented frame over that strip keep the
    /// default `Unavailable` result. A capable backend copies its retained scene target
    /// one-to-one and extends its edges into the newly exposed strip, so the window never
    /// exposes undefined black pixels and no text is resampled.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unavailable`] when the backend cannot retain the last frame, or a
    /// native error when the presentation fails.
    fn present_retained_frame(&mut self, _swapchain: SwapchainId, _target: Extent2d) -> Result<()> {
        Err(Error::Unavailable(
            "retained-frame presentation is not supported by this backend".into(),
        ))
    }

    /// Records commands into a swapchain image and presents it.
    ///
    /// A backend may skip presentation, for example when the swapchain has no drawable
    /// image, and return `Ok(None)`.
    ///
    /// A returned [`PresentedFrame::submission`] tracks GPU completion when the backend
    /// supports deferred submission. Callers must wait for or track it before reusing frame
    /// resources.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when image acquisition, command recording, submission, or
    /// presentation fails.
    fn render_and_present(
        &mut self,
        frame: &PresentFrameDescriptor<'_>,
    ) -> Result<Option<PresentedFrame>>;

    /// Records one offscreen render pass.
    ///
    /// Backends that support deferred offscreen completion may override
    /// [`Self::render_to_textures`] instead. The default returns [`Error::Unavailable`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the pass cannot be recorded or submitted.
    fn render_to_texture(&mut self, _pass: &RenderPassCommands<'_>) -> Result<()> {
        Err(Error::Unavailable(
            "offscreen rendering is not supported by this backend".into(),
        ))
    }

    /// Records ordered offscreen render passes.
    ///
    /// The default submits each pass separately and returns `None`. Backends may batch the
    /// passes into one submission while preserving their order and load behavior.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when any pass cannot be recorded or submitted.
    fn render_to_textures(
        &mut self,
        passes: &[RenderPassCommands<'_>],
    ) -> Result<Option<SubmissionId>> {
        for pass in passes {
            self.render_to_texture(pass)?;
        }
        Ok(None)
    }
}

/// Provides backend resource diagnostics.
pub trait DiagnosticsDevice {
    /// Relocates live resources during explicit owner-thread memory maintenance, retaining public
    /// resource IDs and contents. Implementations may wait at this maintenance boundary; callers
    /// must not run this on the ordinary draw hot path. Unsubmitted encoders prohibit relocation.
    /// Driver-managed backends return an empty report: they cannot relocate driver heaps.
    ///
    /// # Errors
    /// Returns a backend error for allocation, GPU copy or synchronization failure. Implementations
    /// must preserve old resources when preparing replacement allocations fails.
    fn compact_memory(&mut self) -> Result<crate::MemoryCompactReport> {
        Ok(crate::MemoryCompactReport::default())
    }
    /// Returns the current live resource counts known to the backend.
    #[must_use]
    fn resource_stats(&self) -> ResourceStats;

    /// Queries the physical memory architecture on the device owner thread.
    ///
    /// Unsupported queries return `Unknown`. This does not describe the heap
    /// selected for a buffer or establish a preferred placement policy.
    ///
    /// # Errors
    /// Returns a backend error when a supported native query fails.
    fn memory_architecture(&self) -> Result<MemoryArchitecture> {
        Ok(MemoryArchitecture::Unknown)
    }

    /// Queries current driver usage and budget on the device owner thread.
    ///
    /// `None` means unsupported. This does not wait for GPU work, enforce a
    /// budget, or describe only Nova-owned resources. Callers should sample it
    /// periodically during existing owner work rather than on every draw.
    ///
    /// # Errors
    /// Returns a backend error when a supported driver query fails.
    fn memory_budget(&self) -> Result<Option<DeviceMemoryBudget>> {
        Ok(None)
    }
}
