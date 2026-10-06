//! Backend capability traits for nova-gfx.
//!
//! These traits are the public contract implemented by concrete nova-gfx
//! backends. They keep backend users generic over Vulkan, Direct3D 12, and
//! Metal while preserving static dispatch for hot rendering paths.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    AsyncCapabilities, BackendKind, BoxFuture, BufferDescriptor, BufferId, ClearColor,
    CommandEncoderDescriptor, CommandEncoderId, DrawDescriptor, DrawStepDescriptor, Error, LoadOp,
    MemoryTrimLevel, PipelineLayoutDescriptor, PipelineLayoutId, RenderPassDepthAttachment,
    RenderPassDescriptor, RenderPassId, RenderPipelineDescriptor, RenderPipelineId,
    RenderStepDescriptor, RenderStepList, ResourceSetDescriptor, ResourceSetId,
    ResourceSetLayoutDescriptor, ResourceSetLayoutId, ResourceStats, Result, SamplerDescriptor,
    SamplerId, ScissorRect, ShaderModuleDescriptor, ShaderModuleId, SubmissionId, SubmissionStatus,
    SurfaceConfig, SurfaceDescriptor, SurfaceId, SwapchainId, TextureDescriptor, TextureId,
    TextureReadback, TextureRenderStepList, TextureViewDescriptor, TextureViewId, TextureWrite,
    TextureWriteDescriptor, ThreadingMode, resource_set_list,
};

/// Identifies the graphics API implemented by a backend type.
///
/// Implementors should use this associated constant for diagnostics, adapter
/// selection, and logs. It must describe the concrete backend used by the
/// implementing type.
pub trait Backend {
    /// Graphics API exposed by this backend implementation.
    const BACKEND_KIND: BackendKind;
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

/// Complete async-capable device contract for a nova-gfx backend or proxy.
pub trait AsyncDevice:
    Backend
    + AsyncSurfaceDevice
    + AsyncResourceDevice
    + AsyncPipelineDevice
    + AsyncCommandDevice
    + AsyncPresentationDevice
    + AsyncDiagnosticsDevice
{
}

impl<T> AsyncDevice for T where
    T: Backend
        + AsyncSurfaceDevice
        + AsyncResourceDevice
        + AsyncPipelineDevice
        + AsyncCommandDevice
        + AsyncPresentationDevice
        + AsyncDiagnosticsDevice
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

/// Compatibility name for the surface capability trait.
pub trait BackendSurface: SurfaceDevice {
    /// Creates a backend surface through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SurfaceDevice::create_surface`].
    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        SurfaceDevice::create_surface(self, target, desc)
    }

    /// Creates a swapchain through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SurfaceDevice::create_swapchain`].
    fn create_swapchain(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        SurfaceDevice::create_swapchain(self, surface, config)
    }

    /// Destroys a swapchain through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SurfaceDevice::destroy_swapchain`].
    fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
        SurfaceDevice::destroy_swapchain(self, swapchain)
    }

    /// Destroys a surface through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SurfaceDevice::destroy_surface`].
    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
        SurfaceDevice::destroy_surface(self, surface)
    }
}

impl<T> BackendSurface for T where T: SurfaceDevice {}

/// Creates, updates, and destroys GPU resource objects.
///
/// All handles passed to these methods must belong to the same device. Backends
/// should validate descriptors before creating native resources and report bad
/// inputs with [`Error::InvalidInput`].
pub trait ResourceDevice {
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

    /// Creates a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout descriptor is invalid or unsupported.
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId>;

    /// Creates a resource set from live resources.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout or any bound resource handle is invalid,
    /// or if bindings do not match the layout.
    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId>;

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

    /// Destroys a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()>;

    /// Destroys a resource set.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()>;
}

/// Compatibility name for the resource capability trait.
pub trait BackendResources: ResourceDevice {
    /// Writes a batch of texture uploads in order.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error`] reported by [`ResourceDevice::write_texture`].
    fn write_texture_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = TextureWrite<'a>>,
    ) -> Result<()> {
        ResourceDevice::write_texture_batch(self, writes)
    }
}

impl<T> BackendResources for T where T: ResourceDevice {}

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
    /// resource set layouts.
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

/// Object-safe resource and pipeline access for renderer-owned extensions.
///
/// This deliberately excludes window surfaces, command submission, and presentation. The host
/// renderer retains ownership of those operations while an extension may manage resources and
/// return ordered [`RenderStepDescriptor`] values to the host.
pub trait ExtensionDevice {
    /// Creates a GPU buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or native allocation fails.
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId>;

    /// Writes bytes to a live GPU buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the buffer is invalid, the write is out of bounds, or upload fails.
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()>;

    /// Creates a GPU texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or native allocation fails.
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId>;

    /// Writes pixels to a live GPU texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the texture, region, or data is invalid, or upload fails.
    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()>;

    /// Writes several texture subresources as one backend upload batch.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error`] reported by the backend. This operation is not transactional;
    /// earlier writes may have taken effect when a later write fails.
    fn write_texture_batch(&mut self, writes: &[TextureWrite<'_>]) -> Result<()>;

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the texture is invalid or the view is unsupported.
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId>;

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the sampler descriptor is unsupported.
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId>;

    /// Creates a resource-set layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout is invalid or unsupported.
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId>;

    /// Creates a resource set.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the layout or any bound resource is invalid.
    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId>;

    /// Creates a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is invalid or references stale layouts.
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId>;

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the source is invalid or unsupported by the backend.
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId>;

    /// Creates a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the descriptor is unsupported.
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId>;

    /// Creates a render pipeline for a viewport extent.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if a referenced handle is invalid or pipeline creation fails.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: crate::Extent2d,
    ) -> Result<RenderPipelineId>;

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or cannot be retired safely.
    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()>;

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or cannot be retired safely.
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

    /// Destroys a resource-set layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()>;

    /// Destroys a resource set.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the handle is stale or invalid.
    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()>;

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

impl<T> ExtensionDevice for T
where
    T: ResourceDevice + PipelineDevice,
{
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        ResourceDevice::create_buffer(self, desc)
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        ResourceDevice::write_buffer(self, buffer, offset, data)
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        ResourceDevice::create_texture(self, desc)
    }

    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        ResourceDevice::write_texture(self, desc, data)
    }

    fn write_texture_batch(&mut self, writes: &[TextureWrite<'_>]) -> Result<()> {
        ResourceDevice::write_texture_batch(self, writes.iter().copied())
    }

    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        ResourceDevice::create_texture_view(self, desc)
    }

    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        ResourceDevice::create_sampler(self, desc)
    }

    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId> {
        ResourceDevice::create_resource_set_layout(self, desc)
    }

    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId> {
        ResourceDevice::create_resource_set(self, desc)
    }

    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        PipelineDevice::create_pipeline_layout(self, desc)
    }

    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        PipelineDevice::create_shader_module(self, desc)
    }

    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        PipelineDevice::create_render_pass(self, desc)
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: crate::Extent2d,
    ) -> Result<RenderPipelineId> {
        PipelineDevice::create_render_pipeline(self, desc, viewport_extent)
    }

    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
        ResourceDevice::destroy_buffer(self, buffer)
    }

    fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
        ResourceDevice::destroy_texture(self, texture)
    }

    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
        ResourceDevice::destroy_texture_view(self, view)
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
        ResourceDevice::destroy_sampler(self, sampler)
    }

    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
        ResourceDevice::destroy_resource_set_layout(self, layout)
    }

    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
        ResourceDevice::destroy_resource_set(self, resource_set)
    }

    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
        PipelineDevice::destroy_pipeline_layout(self, layout)
    }

    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
        PipelineDevice::destroy_shader_module(self, shader)
    }

    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
        PipelineDevice::destroy_render_pass(self, render_pass)
    }

    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
        PipelineDevice::destroy_render_pipeline(self, pipeline)
    }
}

/// Compatibility name for the pipeline capability trait.
pub trait BackendPipelines: PipelineDevice {}

impl<T> BackendPipelines for T where T: PipelineDevice {}

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

/// Compatibility name for queue and deferred-submission capabilities.
pub trait BackendQueue: CommandDevice + SubmissionDevice {
    /// Returns async and threading capabilities through the compatibility trait name.
    #[must_use]
    fn async_capabilities(&self) -> AsyncCapabilities {
        SubmissionDevice::async_capabilities(self)
    }

    /// Polls a submission through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SubmissionDevice::poll_submission`].
    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        SubmissionDevice::poll_submission(self, submission)
    }

    /// Waits for a submission through the compatibility trait name.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`SubmissionDevice::wait_submission`].
    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        SubmissionDevice::wait_submission(self, submission)
    }
}

impl<T> BackendQueue for T where T: CommandDevice + SubmissionDevice {}

/// Host-side time spent in backend presentation stages.
///
/// Absent or uninstrumented stages (for example, explicit DXGI image acquisition) remain zero.
/// These measurements describe host calls, not GPU execution time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentationTimings {
    /// Time waiting for the image's previous in-flight fence.
    pub acquire_fence_wait: Duration,
    /// Time waiting for a swapchain image from `acquire_next_image`.
    pub image_acquire: Duration,
    /// Host time spent allocating the per-frame command pool and buffer.
    pub command_encoder_create: Duration,
    /// Host time spent translating draw steps into backend command buffers.
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
pub struct PresentationFrame {
    /// GPU submission to track, if this backend uses deferred submission.
    pub submission: Option<SubmissionId>,
    /// Backend-specific host timings when available.
    pub timings: Option<PresentationTimings>,
}

/// Provides frame presentation and offscreen draw helpers.
///
/// These helpers are the normalized high-level presentation API. Backend-specific
/// acquire/present synchronization details stay inside backend crates.
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
    fn arm_swapchain_frame_ready(
        &mut self,
        _swapchain: SwapchainId,
        _callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        Ok(false)
    }

    /// Returns whether `swapchain` can consume native presentation damage.
    ///
    /// Backends returning `true` must keep every rotating back buffer coherent, restrict rendering
    /// to the effective damaged region, and pass that same region to the native presentation API.
    /// This contract avoids a retained full-surface texture or a previous-buffer copy.
    #[must_use]
    fn supports_partial_presentation(&self, _swapchain: SwapchainId) -> bool {
        false
    }

    /// Stretches the swapchain's composited content over a window larger than its buffers.
    ///
    /// `scale` holds the new client size divided by the current buffer size; `None` resets to
    /// identity. Compositor-backed swapchains use this between a native window resize and the
    /// next `ResizeBuffers` so the previous frame keeps covering the client area instead of
    /// leaving uncomposited margins. Backends without such a visual ignore the request.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend fails to apply or reset the stretch.
    fn set_swapchain_content_stretch(
        &mut self,
        _swapchain: SwapchainId,
        _scale: Option<[f32; 2]>,
    ) -> Result<()> {
        Ok(())
    }

    /// Acquires a swapchain image, records draw steps, submits them, and presents.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when acquire, command recording, submission, or
    /// presentation fails.
    fn draw_steps_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> Result<()>;

    /// Records and submits draw steps into a regular texture view.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when command recording, submission, or render target
    /// validation fails. Backends that do not support offscreen rendering yet
    /// should return [`Error::Unavailable`].
    fn draw_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()>;

    /// Renders compatibility render steps into a swapchain and presents them.
    ///
    /// Backend implementations should override this when they support render
    /// step variants that cannot be represented as [`DrawStepDescriptor`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps or present.
    fn render_steps_and_present_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        let draw_steps = compatible_draw_steps(steps)?;
        self.draw_steps_and_present(swapchain, render_pass, &draw_steps, clear_color)
    }

    /// Renders compatibility render steps into a texture target.
    ///
    /// Backend implementations should override this when they support render
    /// step variants that cannot be represented as [`DrawStepDescriptor`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps.
    fn render_steps_to_texture_compat(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        let draw_steps = compatible_draw_steps(steps)?;
        self.draw_steps_to_texture(texture_view, render_pass, &draw_steps, color_load_op)
    }

    /// Renders borrowed render-step lists into a swapchain and presents them.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing or presentation fails.
    fn render_step_list_and_present_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        match steps {
            RenderStepList::Draw(steps) => {
                self.draw_steps_and_present(swapchain, render_pass, steps, clear_color)
            }
            RenderStepList::Render(steps) => self.render_steps_and_present_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            ),
        }
    }

    /// Renders borrowed render-step lists and supplies the changed region to native incremental
    /// presentation when the backend supports it.
    ///
    /// The default implementation safely ignores `damage` and performs a regular full present.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing or presentation fails.
    fn render_step_list_and_present_with_damage_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        _damage: Option<ScissorRect>,
    ) -> Result<()> {
        self.render_step_list_and_present_compat(
            swapchain,
            render_pass,
            steps,
            clear_color,
            depth_attachment,
        )
    }

    /// Renders borrowed render-step lists into a texture target.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing or render target validation fails.
    fn render_step_list_to_texture_compat(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        match steps {
            RenderStepList::Draw(steps) => {
                self.draw_steps_to_texture(texture_view, render_pass, steps, color_load_op)
            }
            RenderStepList::Render(steps) => self.render_steps_to_texture_compat(
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            ),
        }
    }

    /// Renders offscreen texture passes in the supplied order.
    ///
    /// The default submits each pass separately. A backend may record and submit
    /// the passes together, but must preserve their order and load behavior.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when any pass cannot be recorded or submitted.
    fn render_step_lists_to_textures_compat(
        &mut self,
        passes: &[TextureRenderStepList<'_>],
    ) -> Result<()> {
        for pass in passes {
            self.render_step_list_to_texture_compat(
                pass.texture_view,
                pass.render_pass,
                pass.steps,
                pass.color_load_op,
                pass.depth_attachment,
            )?;
        }
        Ok(())
    }

    /// Draws one non-indexed pipeline with no resource sets and presents it.
    ///
    /// This is a convenience method for simple examples. Production renderers
    /// should usually call [`Self::draw_steps_and_present`] directly.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`Self::draw_steps_and_present`].
    fn draw_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        pipeline: RenderPipelineId,
        clear_color: ClearColor,
    ) -> Result<()> {
        self.draw_resources_and_present(swapchain, render_pass, pipeline, &[], clear_color, 3)
    }

    /// Draws one non-indexed pipeline with resource sets and presents it.
    ///
    /// This is a convenience method for examples and smoke tests.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] from [`Self::draw_steps_and_present`].
    fn draw_resources_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        pipeline: RenderPipelineId,
        resource_sets: &[ResourceSetId],
        clear_color: ClearColor,
        vertex_count: u32,
    ) -> Result<()> {
        self.draw_steps_and_present(
            swapchain,
            render_pass,
            &[DrawStepDescriptor {
                pipeline,
                resource_sets: resource_set_list(resource_sets.iter().copied()),
                vertex_count,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }],
            clear_color,
        )
    }

    /// Records, submits, and presents a frame without waiting for GPU completion
    /// when the backend supports deferred submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing or presentation fails.
    fn draw_steps_and_present_deferred(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        self.draw_steps_and_present(swapchain, render_pass, steps, clear_color)?;
        Ok(SubmissionId::from_parts(0, 0))
    }

    /// Renders and presents compatibility render steps using deferred submission.
    ///
    /// Backend implementations should override this when they support render
    /// step variants that cannot be represented as [`DrawStepDescriptor`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_steps_and_present_deferred_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        let draw_steps = compatible_draw_steps(steps)?;
        self.draw_steps_and_present_deferred(swapchain, render_pass, &draw_steps, clear_color)
    }

    /// Renders and presents borrowed render-step lists using deferred submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_step_list_and_present_deferred_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        match steps {
            RenderStepList::Draw(steps) => {
                self.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)
            }
            RenderStepList::Render(steps) => self.render_steps_and_present_deferred_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            ),
        }
    }

    /// Deferred counterpart of [`Self::render_step_list_and_present_with_damage_compat`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_step_list_and_present_deferred_with_damage_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        _damage: Option<ScissorRect>,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        self.render_step_list_and_present_deferred_compat(
            swapchain,
            render_pass,
            steps,
            clear_color,
            depth_attachment,
        )
    }

    /// Renders and presents a compatibility render-step list while returning any backend timing.
    ///
    /// `None` means the backend did not present a frame, for example because the swapchain has no
    /// drawable image. Backends without stage instrumentation use the default implementation.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_step_list_and_present_deferred_with_damage_measured(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        damage: Option<ScissorRect>,
    ) -> Result<Option<PresentationFrame>>
    where
        Self: SubmissionDevice,
    {
        let submission = self.render_step_list_and_present_deferred_with_damage_compat(
            swapchain,
            render_pass,
            steps,
            clear_color,
            depth_attachment,
            damage,
        )?;
        Ok(Some(PresentationFrame {
            submission: (submission.raw() != 0).then_some(submission),
            timings: None,
        }))
    }
}

/// Compatibility presentation API used by the GPUI nova renderer.
pub trait BackendPresentationCompat: PresentationDevice + SubmissionDevice {
    /// Renders compatibility render steps into a swapchain and presents them.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps or present.
    fn render_steps_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_and_present(
            swapchain,
            render_pass,
            RenderStepList::from_render_steps(steps),
            clear_color,
            depth_attachment,
        )
    }

    /// Renders borrowed render-step lists into a swapchain and presents them.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps or present.
    fn render_step_list_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_and_present_compat(
            swapchain,
            render_pass,
            steps,
            clear_color,
            depth_attachment,
        )
    }

    /// Renders compatibility render steps into a texture target.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps.
    fn render_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_to_texture(
            texture_view,
            render_pass,
            RenderStepList::from_render_steps(steps),
            color_load_op,
            depth_attachment,
        )
    }

    /// Renders borrowed render-step lists into a texture target.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the backend cannot render the steps.
    fn render_step_list_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_to_texture_compat(
            texture_view,
            render_pass,
            steps,
            color_load_op,
            depth_attachment,
        )
    }

    /// Renders and presents compatibility render steps using deferred submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_steps_and_present_deferred(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.render_step_list_and_present_deferred(
            swapchain,
            render_pass,
            RenderStepList::from_render_steps(steps),
            clear_color,
            depth_attachment,
        )
    }

    /// Renders and presents borrowed render-step lists using deferred submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when drawing, presentation, or submission fails.
    fn render_step_list_and_present_deferred(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.render_step_list_and_present_deferred_compat(
            swapchain,
            render_pass,
            steps,
            clear_color,
            depth_attachment,
        )
    }
}

impl<T> BackendPresentationCompat for T where T: PresentationDevice + SubmissionDevice {}

fn compatible_draw_steps(steps: &[RenderStepDescriptor]) -> Result<Vec<DrawStepDescriptor>> {
    if steps
        .iter()
        .any(|step| matches!(step, RenderStepDescriptor::DrawIndexed(_)))
    {
        return Err(Error::Unavailable(
            "indexed render steps are not implemented by this backend compatibility path"
                .to_string(),
        ));
    }
    let mut draw_steps = Vec::with_capacity(steps.len());
    for step in steps {
        if let RenderStepDescriptor::Draw(step) = step {
            draw_steps.push(step.clone());
        }
    }
    Ok(draw_steps)
}

/// Provides backend resource diagnostics.
pub trait DiagnosticsDevice {
    /// Returns the current live resource counts known to the backend.
    #[must_use]
    fn resource_stats(&self) -> ResourceStats;
}

/// Compatibility name for backend diagnostics and memory-pressure hooks.
pub trait BackendDiagnostics: DiagnosticsDevice {
    /// Asks the backend to release caches or transient memory for a pressure level.
    ///
    /// # Errors
    ///
    /// Backends may return [`Error`] when memory trimming fails.
    fn trim_memory(&mut self, _level: MemoryTrimLevel) -> Result<()> {
        Ok(())
    }
}

impl<T> BackendDiagnostics for T where T: DiagnosticsDevice {}

/// Async surface API. Default methods delegate to the synchronous trait.
pub trait AsyncSurfaceDevice: SurfaceDevice + Send {
    /// Creates a surface through the async API.
    fn create_surface_async<'a>(
        &'a mut self,
        target: &'a Self::SurfaceTarget,
        desc: &'a SurfaceDescriptor,
    ) -> BoxFuture<'a, SurfaceId>
    where
        Self::SurfaceTarget: Sync,
    {
        Box::pin(async move { self.create_surface(target, desc) })
    }

    /// Creates a swapchain through the async API.
    fn create_swapchain_async(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> BoxFuture<'_, SwapchainId> {
        Box::pin(async move { self.create_swapchain(surface, config) })
    }

    /// Destroys a swapchain through the async API.
    fn destroy_swapchain_async(&mut self, swapchain: SwapchainId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_swapchain(swapchain) })
    }

    /// Destroys a surface through the async API.
    fn destroy_surface_async(&mut self, surface: SurfaceId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_surface(surface) })
    }
}

impl<T> AsyncSurfaceDevice for T where T: SurfaceDevice + Send {}

/// Async resource API. Default methods delegate to the synchronous trait.
pub trait AsyncResourceDevice: ResourceDevice + Send {
    /// Creates a buffer through the async API.
    fn create_buffer_async<'a>(
        &'a mut self,
        desc: &'a BufferDescriptor,
    ) -> BoxFuture<'a, BufferId> {
        Box::pin(async move { self.create_buffer(desc) })
    }

    /// Writes buffer bytes through the async API.
    fn write_buffer_async<'a>(
        &'a mut self,
        buffer: BufferId,
        offset: u64,
        data: &'a [u8],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.write_buffer(buffer, offset, data) })
    }

    /// Creates a texture through the async API.
    fn create_texture_async<'a>(
        &'a mut self,
        desc: &'a TextureDescriptor,
    ) -> BoxFuture<'a, TextureId> {
        Box::pin(async move { self.create_texture(desc) })
    }

    /// Writes texture bytes through the async API.
    fn write_texture_async<'a>(
        &'a mut self,
        desc: TextureWriteDescriptor,
        data: &'a [u8],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.write_texture(desc, data) })
    }

    /// Creates a texture view through the async API.
    fn create_texture_view_async<'a>(
        &'a mut self,
        desc: &'a TextureViewDescriptor,
    ) -> BoxFuture<'a, TextureViewId> {
        Box::pin(async move { self.create_texture_view(desc) })
    }

    /// Creates a sampler through the async API.
    fn create_sampler_async<'a>(
        &'a mut self,
        desc: &'a SamplerDescriptor,
    ) -> BoxFuture<'a, SamplerId> {
        Box::pin(async move { self.create_sampler(desc) })
    }

    /// Creates a resource set layout through the async API.
    fn create_resource_set_layout_async<'a>(
        &'a mut self,
        desc: &'a ResourceSetLayoutDescriptor,
    ) -> BoxFuture<'a, ResourceSetLayoutId> {
        Box::pin(async move { self.create_resource_set_layout(desc) })
    }

    /// Creates a resource set through the async API.
    fn create_resource_set_async<'a>(
        &'a mut self,
        desc: &'a ResourceSetDescriptor,
    ) -> BoxFuture<'a, ResourceSetId> {
        Box::pin(async move { self.create_resource_set(desc) })
    }

    /// Destroys a buffer through the async API.
    fn destroy_buffer_async(&mut self, buffer: BufferId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_buffer(buffer) })
    }

    /// Destroys a texture through the async API.
    fn destroy_texture_async(&mut self, texture: TextureId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_texture(texture) })
    }

    /// Destroys a texture view through the async API.
    fn destroy_texture_view_async(&mut self, view: TextureViewId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_texture_view(view) })
    }

    /// Destroys a sampler through the async API.
    fn destroy_sampler_async(&mut self, sampler: SamplerId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_sampler(sampler) })
    }

    /// Destroys a resource set layout through the async API.
    fn destroy_resource_set_layout_async(
        &mut self,
        layout: ResourceSetLayoutId,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_resource_set_layout(layout) })
    }

    /// Destroys a resource set through the async API.
    fn destroy_resource_set_async(&mut self, resource_set: ResourceSetId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_resource_set(resource_set) })
    }
}

impl<T> AsyncResourceDevice for T where T: ResourceDevice + Send {}

/// Async pipeline API. Default methods delegate to the synchronous trait.
pub trait AsyncPipelineDevice: PipelineDevice + Send {
    /// Creates a pipeline layout through the async API.
    fn create_pipeline_layout_async<'a>(
        &'a mut self,
        desc: &'a PipelineLayoutDescriptor,
    ) -> BoxFuture<'a, PipelineLayoutId> {
        Box::pin(async move { self.create_pipeline_layout(desc) })
    }

    /// Creates a shader module through the async API.
    fn create_shader_module_async<'a>(
        &'a mut self,
        desc: &'a ShaderModuleDescriptor,
    ) -> BoxFuture<'a, ShaderModuleId> {
        Box::pin(async move { self.create_shader_module(desc) })
    }

    /// Creates a render pass through the async API.
    fn create_render_pass_async<'a>(
        &'a mut self,
        desc: &'a RenderPassDescriptor,
    ) -> BoxFuture<'a, RenderPassId> {
        Box::pin(async move { self.create_render_pass(desc) })
    }

    /// Creates a render pipeline through the async API.
    fn create_render_pipeline_async<'a>(
        &'a mut self,
        desc: &'a RenderPipelineDescriptor,
        viewport_extent: crate::Extent2d,
    ) -> BoxFuture<'a, RenderPipelineId> {
        Box::pin(async move { self.create_render_pipeline(desc, viewport_extent) })
    }

    /// Destroys a pipeline layout through the async API.
    fn destroy_pipeline_layout_async(&mut self, layout: PipelineLayoutId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_pipeline_layout(layout) })
    }

    /// Destroys a shader module through the async API.
    fn destroy_shader_module_async(&mut self, shader: ShaderModuleId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_shader_module(shader) })
    }

    /// Destroys a render pass through the async API.
    fn destroy_render_pass_async(&mut self, render_pass: RenderPassId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_render_pass(render_pass) })
    }

    /// Destroys a render pipeline through the async API.
    fn destroy_render_pipeline_async(&mut self, pipeline: RenderPipelineId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_render_pipeline(pipeline) })
    }
}

impl<T> AsyncPipelineDevice for T where T: PipelineDevice + Send {}

/// Async command and submission API.
pub trait AsyncCommandDevice: CommandDevice + SubmissionDevice + Send {
    /// Creates a command encoder through the async API.
    fn create_command_encoder_async<'a>(
        &'a mut self,
        desc: &'a CommandEncoderDescriptor,
    ) -> BoxFuture<'a, CommandEncoderId> {
        Box::pin(async move { self.create_command_encoder(desc) })
    }

    /// Records a draw descriptor through the async API.
    fn record_draw_desc_async(
        &mut self,
        encoder: CommandEncoderId,
        draw: DrawDescriptor,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.record_draw_desc(encoder, draw) })
    }

    /// Submits and waits using the synchronous compatibility semantics.
    fn submit_async(&mut self, encoder: CommandEncoderId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.submit(encoder) })
    }

    /// Submits without waiting and returns a submission handle.
    fn submit_deferred_async(&mut self, encoder: CommandEncoderId) -> BoxFuture<'_, SubmissionId> {
        Box::pin(async move { self.submit_deferred(encoder) })
    }

    /// Polls a submission through the async API.
    fn poll_submission_async(
        &mut self,
        submission: SubmissionId,
    ) -> BoxFuture<'_, SubmissionStatus> {
        Box::pin(async move { self.poll_submission(submission) })
    }

    /// Waits for a submission through the async API.
    fn wait_submission_async(&mut self, submission: SubmissionId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.wait_submission(submission) })
    }

    /// Destroys a command encoder through the async API.
    fn destroy_command_encoder_async(&mut self, encoder: CommandEncoderId) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.destroy_command_encoder(encoder) })
    }
}

impl<T> AsyncCommandDevice for T where T: CommandDevice + SubmissionDevice + Send {}

/// Async presentation API.
pub trait AsyncPresentationDevice: PresentationDevice + SubmissionDevice + Send {
    /// Draws and presents through the async API.
    fn draw_steps_and_present_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> BoxFuture<'a, ()> {
        Box::pin(
            async move { self.draw_steps_and_present(swapchain, render_pass, steps, clear_color) },
        )
    }

    /// Draws to a texture through the async API.
    fn draw_steps_to_texture_async<'a>(
        &'a mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.draw_steps_to_texture(texture_view, render_pass, steps, color_load_op)
        })
    }

    /// Draws, presents, and returns a deferred submission when available.
    fn draw_steps_and_present_deferred_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> BoxFuture<'a, SubmissionId> {
        Box::pin(async move {
            self.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)
        })
    }

    /// Renders compatibility render steps and presents through the async API.
    fn render_steps_and_present_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.render_steps_and_present_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }

    /// Renders borrowed render-step lists and presents through the async API.
    fn render_step_list_and_present_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'a>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.render_step_list_and_present_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }

    /// Renders compatibility render steps into a texture through the async API.
    fn render_steps_to_texture_async<'a>(
        &'a mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &'a [RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.render_steps_to_texture_compat(
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            )
        })
    }

    /// Renders borrowed render-step lists into a texture through the async API.
    fn render_step_list_to_texture_async<'a>(
        &'a mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: RenderStepList<'a>,
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.render_step_list_to_texture_compat(
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            )
        })
    }

    /// Renders compatibility render steps, presents, and returns a deferred submission.
    fn render_steps_and_present_deferred_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, SubmissionId> {
        Box::pin(async move {
            self.render_steps_and_present_deferred_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }

    /// Renders borrowed render-step lists, presents, and returns a deferred submission.
    fn render_step_list_and_present_deferred_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'a>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> BoxFuture<'a, SubmissionId> {
        Box::pin(async move {
            self.render_step_list_and_present_deferred_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }
}

impl<T> AsyncPresentationDevice for T where T: PresentationDevice + SubmissionDevice + Send {}

/// Async diagnostics API.
pub trait AsyncDiagnosticsDevice: DiagnosticsDevice + Send {
    /// Returns resource stats through the async API.
    fn resource_stats_async(&mut self) -> BoxFuture<'_, ResourceStats> {
        Box::pin(async move { Ok(self.resource_stats()) })
    }
}

impl<T> AsyncDiagnosticsDevice for T where T: DiagnosticsDevice + Send {}

/// Thread-safe serializing proxy for a nova-gfx device.
#[derive(Debug)]
pub struct SharedDevice<D> {
    inner: Arc<Mutex<D>>,
}

impl<D> SharedDevice<D> {
    /// Wraps a device in a thread-safe serializing proxy.
    #[must_use]
    pub fn new(device: D) -> Self {
        Self {
            inner: Arc::new(Mutex::new(device)),
        }
    }

    /// Runs a closure with exclusive device access.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Backend`] if the device mutex has been poisoned.
    pub fn with_device<R>(&self, callback: impl FnOnce(&mut D) -> Result<R>) -> Result<R> {
        let mut device = self
            .inner
            .lock()
            .map_err(|_| Error::Backend("shared graphics device mutex poisoned".to_string()))?;
        callback(&mut device)
    }
}

impl<D> SharedDevice<D>
where
    D: Send,
{
    /// Runs a closure with exclusive device access through the async API.
    pub fn with_device_async<'a, R: Send + 'a>(
        &'a self,
        callback: impl FnOnce(&mut D) -> Result<R> + Send + 'a,
    ) -> BoxFuture<'a, R> {
        Box::pin(async move { self.with_device(callback) })
    }
}

impl<D> Clone for SharedDevice<D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D> Backend for SharedDevice<D>
where
    D: Backend,
{
    const BACKEND_KIND: BackendKind = D::BACKEND_KIND;
}

impl<D> SubmissionDevice for SharedDevice<D>
where
    D: CommandDevice + SubmissionDevice,
{
    fn async_capabilities(&self) -> AsyncCapabilities {
        let Ok(device) = self.inner.lock() else {
            return AsyncCapabilities {
                threading_mode: ThreadingMode::MultiThreadDeviceProxy,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
                partial_presentation: false,
            };
        };
        let mut capabilities = device.async_capabilities();
        capabilities.threading_mode = ThreadingMode::MultiThreadDeviceProxy;
        capabilities
    }

    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        self.with_device(|device| device.submit_deferred(encoder))
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        self.with_device(|device| device.poll_submission(submission))
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        self.with_device(|device| device.wait_submission(submission))
    }
}

impl<D> SurfaceDevice for SharedDevice<D>
where
    D: SurfaceDevice,
{
    type SurfaceTarget = D::SurfaceTarget;

    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        self.with_device(|device| device.create_surface(target, desc))
    }

    fn create_swapchain(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        self.with_device(|device| device.create_swapchain(surface, config))
    }

    fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
        self.with_device(|device| device.destroy_swapchain(swapchain))
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
        self.with_device(|device| device.destroy_surface(surface))
    }
}

impl<D> ResourceDevice for SharedDevice<D>
where
    D: ResourceDevice,
{
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        self.with_device(|device| device.create_buffer(desc))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        self.with_device(|device| device.write_buffer(buffer, offset, data))
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        self.with_device(|device| device.create_texture(desc))
    }

    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        self.with_device(|device| device.write_texture(desc, data))
    }

    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        self.with_device(|device| device.create_texture_view(desc))
    }

    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        self.with_device(|device| device.create_sampler(desc))
    }

    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId> {
        self.with_device(|device| device.create_resource_set_layout(desc))
    }

    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId> {
        self.with_device(|device| device.create_resource_set(desc))
    }

    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
        self.with_device(|device| device.destroy_buffer(buffer))
    }

    fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
        self.with_device(|device| device.destroy_texture(texture))
    }

    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
        self.with_device(|device| device.destroy_texture_view(view))
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
        self.with_device(|device| device.destroy_sampler(sampler))
    }

    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
        self.with_device(|device| device.destroy_resource_set_layout(layout))
    }

    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
        self.with_device(|device| device.destroy_resource_set(resource_set))
    }
}

impl<D> PipelineDevice for SharedDevice<D>
where
    D: PipelineDevice,
{
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        self.with_device(|device| device.create_pipeline_layout(desc))
    }

    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        self.with_device(|device| device.create_shader_module(desc))
    }

    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        self.with_device(|device| device.create_render_pass(desc))
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: crate::Extent2d,
    ) -> Result<RenderPipelineId> {
        self.with_device(|device| device.create_render_pipeline(desc, viewport_extent))
    }

    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
        self.with_device(|device| device.destroy_pipeline_layout(layout))
    }

    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
        self.with_device(|device| device.destroy_shader_module(shader))
    }

    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
        self.with_device(|device| device.destroy_render_pass(render_pass))
    }

    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
        self.with_device(|device| device.destroy_render_pipeline(pipeline))
    }
}

impl<D> CommandDevice for SharedDevice<D>
where
    D: CommandDevice,
{
    fn create_command_encoder(
        &mut self,
        desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        self.with_device(|device| device.create_command_encoder(desc))
    }

    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDescriptor) -> Result<()> {
        self.with_device(|device| device.record_draw_desc(encoder, draw))
    }

    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.with_device(|device| device.submit(encoder))
    }

    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.with_device(|device| device.destroy_command_encoder(encoder))
    }
}

impl<D> PresentationDevice for SharedDevice<D>
where
    D: PresentationDevice + SubmissionDevice,
{
    fn arm_swapchain_frame_ready(
        &mut self,
        swapchain: SwapchainId,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        self.with_device(|device| device.arm_swapchain_frame_ready(swapchain, callback))
    }

    fn supports_partial_presentation(&self, swapchain: SwapchainId) -> bool {
        self.with_device(|device| Ok(device.supports_partial_presentation(swapchain)))
            .unwrap_or(false)
    }

    fn draw_steps_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> Result<()> {
        self.with_device(|device| {
            device.draw_steps_and_present(swapchain, render_pass, steps, clear_color)
        })
    }

    fn draw_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        self.with_device(|device| {
            device.draw_steps_to_texture(texture_view, render_pass, steps, color_load_op)
        })
    }

    fn draw_steps_and_present_deferred(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        clear_color: ClearColor,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        self.with_device(|device| {
            device.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)
        })
    }

    fn render_steps_and_present_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.with_device(|device| {
            device.render_steps_and_present_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }

    fn render_steps_to_texture_compat(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.with_device(|device| {
            device.render_steps_to_texture_compat(
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            )
        })
    }

    fn render_steps_and_present_deferred_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        self.with_device(|device| {
            device.render_steps_and_present_deferred_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        })
    }

    fn render_step_list_and_present_with_damage_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        damage: Option<ScissorRect>,
    ) -> Result<()> {
        self.with_device(|device| {
            device.render_step_list_and_present_with_damage_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                damage,
            )
        })
    }

    fn render_step_list_and_present_deferred_with_damage_compat(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        damage: Option<ScissorRect>,
    ) -> Result<SubmissionId>
    where
        Self: SubmissionDevice,
    {
        self.with_device(|device| {
            device.render_step_list_and_present_deferred_with_damage_compat(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                damage,
            )
        })
    }

    fn render_step_list_and_present_deferred_with_damage_measured(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        damage: Option<ScissorRect>,
    ) -> Result<Option<PresentationFrame>>
    where
        Self: SubmissionDevice,
    {
        self.with_device(|device| {
            device.render_step_list_and_present_deferred_with_damage_measured(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                damage,
            )
        })
    }
}

impl<D> DiagnosticsDevice for SharedDevice<D>
where
    D: DiagnosticsDevice,
{
    fn resource_stats(&self) -> ResourceStats {
        let Ok(device) = self.inner.lock() else {
            return ResourceStats::default();
        };
        device.resource_stats()
    }
}
