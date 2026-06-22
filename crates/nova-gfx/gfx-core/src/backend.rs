//! Backend capability traits for nova-gfx.
//!
//! These traits are the public contract implemented by concrete nova-gfx
//! backends. They keep backend users generic over Vulkan, Direct3D 12, and
//! Metal while preserving static dispatch for hot rendering paths.

use std::sync::{Arc, Mutex};

use crate::{
    BackendKind, BufferDesc, BufferId, ClearColor, CommandEncoderDesc, CommandEncoderId, DrawDesc,
    DrawStepDesc, GfxAsyncCapabilities, GfxError, GfxFuture, GfxThreadingMode, LoadOp,
    PipelineLayoutDesc, PipelineLayoutId, RenderPassDesc, RenderPassId, RenderPipelineDesc,
    RenderPipelineId, ResourceSetDesc, ResourceSetId, ResourceSetLayoutDesc, ResourceSetLayoutId,
    ResourceStats, Result, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, SubmissionId,
    SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId, TextureDesc, TextureId,
    TextureViewDesc, TextureViewId, TextureWriteDesc,
};

/// Identifies the graphics API implemented by a backend type.
///
/// Implementors should use this associated constant for diagnostics, adapter
/// selection, and logs. It must describe the concrete backend used by the
/// implementing type.
pub trait GfxBackend {
    /// Graphics API exposed by this backend implementation.
    const BACKEND_KIND: BackendKind;
}

/// Complete device contract for a nova-gfx backend.
///
/// This is a convenience trait for call sites that need the full backend API.
/// Prefer narrower traits such as [`GfxResourceDevice`] or [`GfxPipelineDevice`]
/// on helper functions that only need part of the device surface.
pub trait GfxDevice:
    GfxBackend
    + GfxSurfaceDevice
    + GfxResourceDevice
    + GfxPipelineDevice
    + GfxCommandDevice
    + GfxSubmissionDevice
    + GfxPresentationDevice
    + GfxDiagnosticsDevice
{
}

impl<T> GfxDevice for T where
    T: GfxBackend
        + GfxSurfaceDevice
        + GfxResourceDevice
        + GfxPipelineDevice
        + GfxCommandDevice
        + GfxSubmissionDevice
        + GfxPresentationDevice
        + GfxDiagnosticsDevice
{
}

/// Complete async-capable device contract for a nova-gfx backend or proxy.
pub trait GfxAsyncDevice:
    GfxBackend
    + GfxAsyncSurfaceDevice
    + GfxAsyncResourceDevice
    + GfxAsyncPipelineDevice
    + GfxAsyncCommandDevice
    + GfxAsyncPresentationDevice
    + GfxAsyncDiagnosticsDevice
{
}

impl<T> GfxAsyncDevice for T where
    T: GfxBackend
        + GfxAsyncSurfaceDevice
        + GfxAsyncResourceDevice
        + GfxAsyncPipelineDevice
        + GfxAsyncCommandDevice
        + GfxAsyncPresentationDevice
        + GfxAsyncDiagnosticsDevice
{
}

/// Creates and destroys native window surfaces and swapchains.
///
/// Surface and swapchain handles are owned by the device that created them.
/// Passing a handle to another device, or reusing it after destruction, must
/// return [`GfxError::InvalidInput`].
pub trait GfxSurfaceDevice {
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
    /// Returns [`GfxError`] if the target handles are invalid, the platform is
    /// unsupported, or the backend cannot create a presentable surface.
    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDesc,
    ) -> Result<SurfaceId>;

    /// Creates a swapchain for an existing surface.
    ///
    /// The `surface` handle must have been created by this device and must not
    /// already be destroyed. The returned swapchain is tied to that surface.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the surface handle is invalid, the configuration
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
    /// Returns [`GfxError`] if the handle is stale, invalid, or still in use by
    /// backend work that cannot be retired.
    fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()>;

    /// Destroys a surface created by this device.
    ///
    /// After this call succeeds, the handle must not be used again.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale, invalid, or still owns live
    /// swapchain resources.
    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()>;
}

/// Creates, updates, and destroys GPU resource objects.
///
/// All handles passed to these methods must belong to the same device. Backends
/// should validate descriptors before creating native resources and report bad
/// inputs with [`GfxError::InvalidInput`].
pub trait GfxResourceDevice {
    /// Creates a buffer resource.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the descriptor is invalid or native allocation
    /// fails.
    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId>;

    /// Writes bytes into a buffer.
    ///
    /// `buffer` must identify a live CPU-visible or upload-compatible buffer
    /// created by this device. `offset + data.len()` must fit inside the buffer.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is invalid, the write is out of bounds,
    /// or the backend cannot map or stage the upload.
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()>;

    /// Creates a texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the descriptor is invalid or native allocation
    /// fails.
    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId>;

    /// Writes pixel bytes into a texture.
    ///
    /// `desc.texture` must identify a live texture created by this device. The
    /// data layout and source byte slice must cover the requested upload region.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is invalid, the layout is invalid, the
    /// data slice is too short, or the backend cannot stage the upload.
    fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()>;

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the source texture handle is invalid or the view
    /// descriptor is incompatible with the texture.
    fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId>;

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the sampler descriptor is unsupported by the
    /// backend.
    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId>;

    /// Creates a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the layout descriptor is invalid or unsupported.
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDesc,
    ) -> Result<ResourceSetLayoutId>;

    /// Creates a resource set from live resources.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the layout or any bound resource handle is invalid,
    /// or if bindings do not match the layout.
    fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId>;

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()>;

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_texture(&mut self, texture: TextureId) -> Result<()>;

    /// Destroys a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()>;

    /// Destroys a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()>;

    /// Destroys a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()>;

    /// Destroys a resource set.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()>;
}

/// Creates and destroys shader and pipeline objects.
///
/// Pipeline handles and layout handles are device-local. Callers must keep
/// dependent shader modules, render passes, and layouts alive while creating
/// pipelines that reference them.
pub trait GfxPipelineDevice {
    /// Creates a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the descriptor is invalid or references stale
    /// resource set layouts.
    fn create_pipeline_layout(&mut self, desc: &PipelineLayoutDesc) -> Result<PipelineLayoutId>;

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if shader code is empty or not compatible with the
    /// backend.
    fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId>;

    /// Creates a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the render pass descriptor is unsupported.
    fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId>;

    /// Creates a render pipeline.
    ///
    /// `viewport_extent` is the size used for fixed-function viewport and
    /// scissor state in backends that bake it into the pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if any referenced handle is invalid, shader stages do
    /// not match, or the backend cannot create the native pipeline.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
        viewport_extent: crate::Extent2d,
    ) -> Result<RenderPipelineId>;

    /// Destroys a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()>;

    /// Destroys a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()>;

    /// Destroys a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()>;

    /// Destroys a render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale or invalid.
    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()>;
}

/// Records and submits explicit command encoder work.
pub trait GfxCommandDevice {
    /// Creates a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the backend cannot allocate command recording
    /// resources.
    fn create_command_encoder(&mut self, desc: &CommandEncoderDesc) -> Result<CommandEncoderId>;

    /// Records one draw pass into a command encoder.
    ///
    /// All handles referenced by `draw` must be live and belong to this device.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the encoder or any referenced resource is invalid,
    /// or if the backend rejects the draw state.
    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDesc) -> Result<()>;

    /// Submits a command encoder for execution.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the encoder handle is invalid or queue submission
    /// fails.
    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()>;

    /// Destroys a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the handle is stale, invalid, or cannot be safely
    /// retired yet.
    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()>;
}

/// Tracks deferred GPU submissions.
pub trait GfxSubmissionDevice {
    /// Returns async and threading capabilities for this device.
    #[must_use]
    fn async_capabilities(&self) -> GfxAsyncCapabilities {
        GfxAsyncCapabilities::default()
    }

    /// Submits a command encoder without waiting for GPU completion.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the encoder is invalid, the backend cannot submit
    /// it, or the backend cannot track a deferred submission.
    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId>
    where
        Self: GfxCommandDevice,
    {
        self.submit(encoder)?;
        Ok(SubmissionId::from_parts(0, 0))
    }

    /// Polls a previously returned submission.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when `submission` is not known to this device.
    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        if submission.raw() == 0 {
            Ok(SubmissionStatus::Complete)
        } else {
            Err(GfxError::InvalidInput(format!(
                "unknown submission {}",
                submission.raw()
            )))
        }
    }

    /// Blocks until a submission has completed.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when waiting fails or the backend reports a failed
    /// submission.
    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        match self.poll_submission(submission)? {
            SubmissionStatus::Complete => Ok(()),
            SubmissionStatus::Pending => Err(GfxError::Unavailable(
                "submission wait is not implemented by this backend".to_string(),
            )),
            SubmissionStatus::Failed(error) => Err(GfxError::Backend(error)),
        }
    }
}

/// Provides frame presentation and offscreen draw helpers.
///
/// These helpers are the normalized high-level presentation API. Backend-specific
/// acquire/present synchronization details stay inside backend crates.
pub trait GfxPresentationDevice {
    /// Acquires a swapchain image, records draw steps, submits them, and presents.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when acquire, command recording, submission, or
    /// presentation fails.
    fn draw_steps_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<()>;

    /// Records and submits draw steps into a regular texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when command recording, submission, or render target
    /// validation fails. Backends that do not support offscreen rendering yet
    /// should return [`GfxError::Unavailable`].
    fn draw_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()>;

    /// Draws one non-indexed pipeline with no resource sets and presents it.
    ///
    /// This is a convenience method for simple examples. Production renderers
    /// should usually call [`Self::draw_steps_and_present`] directly.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] from [`Self::draw_steps_and_present`].
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
    /// Returns [`GfxError`] from [`Self::draw_steps_and_present`].
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
            &[DrawStepDesc {
                pipeline,
                resource_sets: resource_sets.to_vec(),
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
    /// Returns [`GfxError`] when drawing or presentation fails.
    fn draw_steps_and_present_deferred(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<SubmissionId>
    where
        Self: GfxSubmissionDevice,
    {
        self.draw_steps_and_present(swapchain, render_pass, steps, clear_color)?;
        Ok(SubmissionId::from_parts(0, 0))
    }
}

/// Provides backend resource diagnostics.
pub trait GfxDiagnosticsDevice {
    /// Returns the current live resource counts known to the backend.
    #[must_use]
    fn resource_stats(&self) -> ResourceStats;
}

/// Async surface API. Default methods delegate to the synchronous trait.
pub trait GfxAsyncSurfaceDevice: GfxSurfaceDevice + Send {
    /// Creates a surface through the async API.
    fn create_surface_async<'a>(
        &'a mut self,
        target: &'a Self::SurfaceTarget,
        desc: &'a SurfaceDesc,
    ) -> GfxFuture<'a, SurfaceId>
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
    ) -> GfxFuture<'_, SwapchainId> {
        Box::pin(async move { self.create_swapchain(surface, config) })
    }

    /// Destroys a swapchain through the async API.
    fn destroy_swapchain_async(&mut self, swapchain: SwapchainId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_swapchain(swapchain) })
    }

    /// Destroys a surface through the async API.
    fn destroy_surface_async(&mut self, surface: SurfaceId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_surface(surface) })
    }
}

impl<T> GfxAsyncSurfaceDevice for T where T: GfxSurfaceDevice + Send {}

/// Async resource API. Default methods delegate to the synchronous trait.
pub trait GfxAsyncResourceDevice: GfxResourceDevice + Send {
    /// Creates a buffer through the async API.
    fn create_buffer_async<'a>(&'a mut self, desc: &'a BufferDesc) -> GfxFuture<'a, BufferId> {
        Box::pin(async move { self.create_buffer(desc) })
    }

    /// Writes buffer bytes through the async API.
    fn write_buffer_async<'a>(
        &'a mut self,
        buffer: BufferId,
        offset: u64,
        data: &'a [u8],
    ) -> GfxFuture<'a, ()> {
        Box::pin(async move { self.write_buffer(buffer, offset, data) })
    }

    /// Creates a texture through the async API.
    fn create_texture_async<'a>(&'a mut self, desc: &'a TextureDesc) -> GfxFuture<'a, TextureId> {
        Box::pin(async move { self.create_texture(desc) })
    }

    /// Writes texture bytes through the async API.
    fn write_texture_async<'a>(
        &'a mut self,
        desc: TextureWriteDesc,
        data: &'a [u8],
    ) -> GfxFuture<'a, ()> {
        Box::pin(async move { self.write_texture(desc, data) })
    }

    /// Creates a texture view through the async API.
    fn create_texture_view_async<'a>(
        &'a mut self,
        desc: &'a TextureViewDesc,
    ) -> GfxFuture<'a, TextureViewId> {
        Box::pin(async move { self.create_texture_view(desc) })
    }

    /// Creates a sampler through the async API.
    fn create_sampler_async<'a>(&'a mut self, desc: &'a SamplerDesc) -> GfxFuture<'a, SamplerId> {
        Box::pin(async move { self.create_sampler(desc) })
    }

    /// Creates a resource set layout through the async API.
    fn create_resource_set_layout_async<'a>(
        &'a mut self,
        desc: &'a ResourceSetLayoutDesc,
    ) -> GfxFuture<'a, ResourceSetLayoutId> {
        Box::pin(async move { self.create_resource_set_layout(desc) })
    }

    /// Creates a resource set through the async API.
    fn create_resource_set_async<'a>(
        &'a mut self,
        desc: &'a ResourceSetDesc,
    ) -> GfxFuture<'a, ResourceSetId> {
        Box::pin(async move { self.create_resource_set(desc) })
    }

    /// Destroys a buffer through the async API.
    fn destroy_buffer_async(&mut self, buffer: BufferId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_buffer(buffer) })
    }

    /// Destroys a texture through the async API.
    fn destroy_texture_async(&mut self, texture: TextureId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_texture(texture) })
    }

    /// Destroys a texture view through the async API.
    fn destroy_texture_view_async(&mut self, view: TextureViewId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_texture_view(view) })
    }

    /// Destroys a sampler through the async API.
    fn destroy_sampler_async(&mut self, sampler: SamplerId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_sampler(sampler) })
    }

    /// Destroys a resource set layout through the async API.
    fn destroy_resource_set_layout_async(
        &mut self,
        layout: ResourceSetLayoutId,
    ) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_resource_set_layout(layout) })
    }

    /// Destroys a resource set through the async API.
    fn destroy_resource_set_async(&mut self, resource_set: ResourceSetId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_resource_set(resource_set) })
    }
}

impl<T> GfxAsyncResourceDevice for T where T: GfxResourceDevice + Send {}

/// Async pipeline API. Default methods delegate to the synchronous trait.
pub trait GfxAsyncPipelineDevice: GfxPipelineDevice + Send {
    /// Creates a pipeline layout through the async API.
    fn create_pipeline_layout_async<'a>(
        &'a mut self,
        desc: &'a PipelineLayoutDesc,
    ) -> GfxFuture<'a, PipelineLayoutId> {
        Box::pin(async move { self.create_pipeline_layout(desc) })
    }

    /// Creates a shader module through the async API.
    fn create_shader_module_async<'a>(
        &'a mut self,
        desc: &'a ShaderModuleDesc,
    ) -> GfxFuture<'a, ShaderModuleId> {
        Box::pin(async move { self.create_shader_module(desc) })
    }

    /// Creates a render pass through the async API.
    fn create_render_pass_async<'a>(
        &'a mut self,
        desc: &'a RenderPassDesc,
    ) -> GfxFuture<'a, RenderPassId> {
        Box::pin(async move { self.create_render_pass(desc) })
    }

    /// Creates a render pipeline through the async API.
    fn create_render_pipeline_async<'a>(
        &'a mut self,
        desc: &'a RenderPipelineDesc,
        viewport_extent: crate::Extent2d,
    ) -> GfxFuture<'a, RenderPipelineId> {
        Box::pin(async move { self.create_render_pipeline(desc, viewport_extent) })
    }

    /// Destroys a pipeline layout through the async API.
    fn destroy_pipeline_layout_async(&mut self, layout: PipelineLayoutId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_pipeline_layout(layout) })
    }

    /// Destroys a shader module through the async API.
    fn destroy_shader_module_async(&mut self, shader: ShaderModuleId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_shader_module(shader) })
    }

    /// Destroys a render pass through the async API.
    fn destroy_render_pass_async(&mut self, render_pass: RenderPassId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_render_pass(render_pass) })
    }

    /// Destroys a render pipeline through the async API.
    fn destroy_render_pipeline_async(&mut self, pipeline: RenderPipelineId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_render_pipeline(pipeline) })
    }
}

impl<T> GfxAsyncPipelineDevice for T where T: GfxPipelineDevice + Send {}

/// Async command and submission API.
pub trait GfxAsyncCommandDevice: GfxCommandDevice + GfxSubmissionDevice + Send {
    /// Creates a command encoder through the async API.
    fn create_command_encoder_async<'a>(
        &'a mut self,
        desc: &'a CommandEncoderDesc,
    ) -> GfxFuture<'a, CommandEncoderId> {
        Box::pin(async move { self.create_command_encoder(desc) })
    }

    /// Records a draw descriptor through the async API.
    fn record_draw_desc_async(
        &mut self,
        encoder: CommandEncoderId,
        draw: DrawDesc,
    ) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.record_draw_desc(encoder, draw) })
    }

    /// Submits and waits using the synchronous compatibility semantics.
    fn submit_async(&mut self, encoder: CommandEncoderId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.submit(encoder) })
    }

    /// Submits without waiting and returns a submission handle.
    fn submit_deferred_async(&mut self, encoder: CommandEncoderId) -> GfxFuture<'_, SubmissionId> {
        Box::pin(async move { self.submit_deferred(encoder) })
    }

    /// Polls a submission through the async API.
    fn poll_submission_async(
        &mut self,
        submission: SubmissionId,
    ) -> GfxFuture<'_, SubmissionStatus> {
        Box::pin(async move { self.poll_submission(submission) })
    }

    /// Waits for a submission through the async API.
    fn wait_submission_async(&mut self, submission: SubmissionId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.wait_submission(submission) })
    }

    /// Destroys a command encoder through the async API.
    fn destroy_command_encoder_async(&mut self, encoder: CommandEncoderId) -> GfxFuture<'_, ()> {
        Box::pin(async move { self.destroy_command_encoder(encoder) })
    }
}

impl<T> GfxAsyncCommandDevice for T where T: GfxCommandDevice + GfxSubmissionDevice + Send {}

/// Async presentation API.
pub trait GfxAsyncPresentationDevice: GfxPresentationDevice + GfxSubmissionDevice + Send {
    /// Draws and presents through the async API.
    fn draw_steps_and_present_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDesc],
        clear_color: ClearColor,
    ) -> GfxFuture<'a, ()> {
        Box::pin(
            async move { self.draw_steps_and_present(swapchain, render_pass, steps, clear_color) },
        )
    }

    /// Draws to a texture through the async API.
    fn draw_steps_to_texture_async<'a>(
        &'a mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDesc],
        color_load_op: LoadOp<ClearColor>,
    ) -> GfxFuture<'a, ()> {
        Box::pin(async move {
            self.draw_steps_to_texture(texture_view, render_pass, steps, color_load_op)
        })
    }

    /// Draws, presents, and returns a deferred submission when available.
    fn draw_steps_and_present_deferred_async<'a>(
        &'a mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &'a [DrawStepDesc],
        clear_color: ClearColor,
    ) -> GfxFuture<'a, SubmissionId> {
        Box::pin(async move {
            self.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)
        })
    }
}

impl<T> GfxAsyncPresentationDevice for T where T: GfxPresentationDevice + GfxSubmissionDevice + Send {}

/// Async diagnostics API.
pub trait GfxAsyncDiagnosticsDevice: GfxDiagnosticsDevice + Send {
    /// Returns resource stats through the async API.
    fn resource_stats_async(&mut self) -> GfxFuture<'_, ResourceStats> {
        Box::pin(async move { Ok(self.resource_stats()) })
    }
}

impl<T> GfxAsyncDiagnosticsDevice for T where T: GfxDiagnosticsDevice + Send {}

/// Thread-safe serializing proxy for a nova-gfx device.
#[derive(Debug)]
pub struct SharedGfxDevice<D> {
    inner: Arc<Mutex<D>>,
}

impl<D> SharedGfxDevice<D> {
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
    /// Returns [`GfxError::Backend`] if the device mutex has been poisoned.
    pub fn with_device<R>(&self, callback: impl FnOnce(&mut D) -> Result<R>) -> Result<R> {
        let mut device = self
            .inner
            .lock()
            .map_err(|_| GfxError::Backend("shared graphics device mutex poisoned".to_string()))?;
        callback(&mut device)
    }
}

impl<D> SharedGfxDevice<D>
where
    D: Send,
{
    /// Runs a closure with exclusive device access through the async API.
    pub fn with_device_async<'a, R: Send + 'a>(
        &'a self,
        callback: impl FnOnce(&mut D) -> Result<R> + Send + 'a,
    ) -> GfxFuture<'a, R> {
        Box::pin(async move { self.with_device(callback) })
    }
}

impl<D> Clone for SharedGfxDevice<D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D> GfxBackend for SharedGfxDevice<D>
where
    D: GfxBackend,
{
    const BACKEND_KIND: BackendKind = D::BACKEND_KIND;
}

impl<D> GfxSubmissionDevice for SharedGfxDevice<D>
where
    D: GfxCommandDevice + GfxSubmissionDevice,
{
    fn async_capabilities(&self) -> GfxAsyncCapabilities {
        let Ok(device) = self.inner.lock() else {
            return GfxAsyncCapabilities {
                threading_mode: GfxThreadingMode::MultiThreadDeviceProxy,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
                partial_presentation: false,
            };
        };
        let mut capabilities = device.async_capabilities();
        capabilities.threading_mode = GfxThreadingMode::MultiThreadDeviceProxy;
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

impl<D> GfxSurfaceDevice for SharedGfxDevice<D>
where
    D: GfxSurfaceDevice,
{
    type SurfaceTarget = D::SurfaceTarget;

    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDesc,
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

impl<D> GfxResourceDevice for SharedGfxDevice<D>
where
    D: GfxResourceDevice,
{
    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
        self.with_device(|device| device.create_buffer(desc))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        self.with_device(|device| device.write_buffer(buffer, offset, data))
    }

    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
        self.with_device(|device| device.create_texture(desc))
    }

    fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
        self.with_device(|device| device.write_texture(desc, data))
    }

    fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
        self.with_device(|device| device.create_texture_view(desc))
    }

    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
        self.with_device(|device| device.create_sampler(desc))
    }

    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDesc,
    ) -> Result<ResourceSetLayoutId> {
        self.with_device(|device| device.create_resource_set_layout(desc))
    }

    fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
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

impl<D> GfxPipelineDevice for SharedGfxDevice<D>
where
    D: GfxPipelineDevice,
{
    fn create_pipeline_layout(&mut self, desc: &PipelineLayoutDesc) -> Result<PipelineLayoutId> {
        self.with_device(|device| device.create_pipeline_layout(desc))
    }

    fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
        self.with_device(|device| device.create_shader_module(desc))
    }

    fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
        self.with_device(|device| device.create_render_pass(desc))
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
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

impl<D> GfxCommandDevice for SharedGfxDevice<D>
where
    D: GfxCommandDevice,
{
    fn create_command_encoder(&mut self, desc: &CommandEncoderDesc) -> Result<CommandEncoderId> {
        self.with_device(|device| device.create_command_encoder(desc))
    }

    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDesc) -> Result<()> {
        self.with_device(|device| device.record_draw_desc(encoder, draw))
    }

    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.with_device(|device| device.submit(encoder))
    }

    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.with_device(|device| device.destroy_command_encoder(encoder))
    }
}

impl<D> GfxPresentationDevice for SharedGfxDevice<D>
where
    D: GfxPresentationDevice + GfxSubmissionDevice,
{
    fn draw_steps_and_present(
        &mut self,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
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
        steps: &[DrawStepDesc],
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
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<SubmissionId>
    where
        Self: GfxSubmissionDevice,
    {
        self.with_device(|device| {
            device.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)
        })
    }
}

impl<D> GfxDiagnosticsDevice for SharedGfxDevice<D>
where
    D: GfxDiagnosticsDevice,
{
    fn resource_stats(&self) -> ResourceStats {
        let Ok(device) = self.inner.lock() else {
            return ResourceStats::default();
        };
        device.resource_stats()
    }
}
