//! Object-safe resource and pipeline access for renderer-owned extensions.
//!
//! GPUI hands this facade to [`RendererExtension`](super::RendererExtension)
//! implementations. It deliberately excludes window surfaces, command submission,
//! and presentation: the host window renderer owns those operations and calls the
//! extension back with resources it can create and destroy.
//! Optional ordered buffer compute is queued before subsequent host draws; it does not
//! grant extensions ownership of frame submission or presentation.

use gfx_core::{
    BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor, BindGroupLayoutId,
    BufferDescriptor, BufferId, BufferUploadStats, BufferWrite, GpuTimestampSample,
    GpuTimestampTag, PipelineDevice,
    PipelineLayoutDescriptor, PipelineLayoutId, RenderPassDescriptor, RenderPassId,
    RenderPipelineDescriptor, RenderPipelineId, ResourceDevice, Result, SamplerDescriptor,
    SamplerId, ShaderModuleDescriptor, ShaderModuleId, TextureDescriptor, TextureId,
    TextureViewDescriptor, TextureViewId, TextureWrite, TextureWriteDescriptor,
};

/// Object-safe resource and pipeline access for renderer-owned extensions.
///
/// Implemented for every device that provides [`ResourceDevice`] and
/// [`PipelineDevice`], so a renderer can pass `&mut dyn ExtensionDevice` without
/// knowing the concrete nova-gfx backend type.
pub trait ExtensionDevice {
    /// Physical array-texture limits and end-to-end layered sampling support.
    /// Extensions can plan resource sizes without hard-coding any backend.
    fn texture_array_capabilities(&self) -> gfx_core::TextureArrayCapabilities {
        gfx_core::TextureArrayCapabilities::default()
    }

    /// Device-specific uniform/storage buffer offsets and per-binding ranges.
    /// The host forwards the active native device's limits; renderer
    /// extensions must not infer these from a backend name or fixed constants.
    fn buffer_binding_limits(&self) -> gfx_core::BufferBindingLimits {
        gfx_core::BufferBindingLimits::default()
    }

    /// Multisampling and stencil capabilities of the active device.
    ///
    /// Extensions use this single unified query to decide whether to enable
    /// MSAA and stencil paths, independent of the concrete backend.
    fn multisample_capabilities(&self) -> gfx_core::MultisampleCapabilities {
        gfx_core::MultisampleCapabilities::default()
    }

    /// Returns the device's optional ordered compute/indirect capabilities.
    fn compute_capabilities(&self) -> gfx_core::ComputeCapabilities {
        gfx_core::ComputeCapabilities::default()
    }

    /// Queues GPU buffer compute before subsequent extension draws, without waiting.
    ///
    /// # Errors
    /// Returns `Unavailable` when unsupported, or the backend's validation/dispatch error.
    fn dispatch_compute(&mut self, _dispatch: &gfx_core::ComputeDispatch<'_>) -> Result<()> {
        Err(gfx_core::Error::Unavailable(
            "ordered buffer compute is unsupported".into(),
        ))
    }
    /// Queues ordered compute stages; backends may combine their native submissions.
    /// # Errors
    /// Returns the first backend error; preceding stages may already be queued.
    fn dispatch_compute_batch(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
    ) -> Result<()> {
        for dispatch in dispatches {
            self.dispatch_compute(dispatch)?;
        }
        Ok(())
    }

    /// Whether the native backend can capture GPU work timestamps without
    /// inserting an additional queue submission or a CPU wait.
    fn gpu_work_timestamps_supported(&self) -> bool {
        false
    }

    /// Tags a compute batch for hardware time measurement when supported.
    fn dispatch_compute_batch_profiled(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
        _tag: GpuTimestampTag,
    ) -> Result<()> {
        self.dispatch_compute_batch(dispatches)
    }

    /// Tags an upload batch for hardware time measurement when supported.
    fn write_buffer_batch_profiled(
        &mut self,
        writes: &[BufferWrite<'_>],
        _tag: GpuTimestampTag,
    ) -> Result<BufferUploadStats> {
        self.write_buffer_batch(writes)
    }

    /// Polls the already-completed native hardware probes; never waits.
    fn drain_gpu_work_timestamps(&mut self, _samples: &mut Vec<GpuTimestampSample>) -> Result<()> {
        Ok(())
    }

    /// Creates a GPU buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the descriptor is invalid or native allocation fails.
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId>;

    /// Writes bytes to a live GPU buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the buffer is invalid, the write is out of bounds, or upload fails.
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()>;

    /// Writes an ordered buffer batch, returning destination upload accounting.
    ///
    /// # Errors
    ///
    /// Returns the first validation or upload error; earlier writes may already have taken effect.
    fn write_buffer_batch(&mut self, writes: &[BufferWrite<'_>]) -> Result<BufferUploadStats>;

    /// Creates a GPU texture.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the descriptor is invalid or native allocation fails.
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId>;

    /// Writes pixels to a live GPU texture.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the texture, region, or data is invalid, or upload fails.
    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()>;

    /// Upload one array-texture layer and mip in native submission order.
    fn write_texture_array_layer(
        &mut self,
        _desc: gfx_core::TextureArrayLayerWriteDescriptor,
        _data: &[u8],
    ) -> Result<()> {
        Err(gfx_core::Error::Unavailable(
            "layered texture uploads are unsupported".into(),
        ))
    }

    /// Writes several texture subresources as one backend upload batch.
    ///
    /// # Errors
    ///
    /// Returns the first error reported by the backend. Earlier writes may have taken effect.
    fn write_texture_batch(&mut self, writes: &[TextureWrite<'_>]) -> Result<()>;

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the texture is invalid or the view is unsupported.
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId>;

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the sampler descriptor is unsupported.
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId>;

    /// Creates a bind-group layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the layout is invalid or unsupported.
    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId>;

    /// Creates a bind group.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the layout or any bound resource is invalid.
    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId>;

    /// Creates a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the descriptor is invalid or references stale layouts.
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId>;

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the source is invalid or unsupported by the backend.
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId>;

    /// Creates a render pass layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the descriptor is unsupported.
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId>;

    /// Creates a render pipeline for a viewport extent.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if a referenced handle is invalid or pipeline creation fails.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId>;

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or cannot be retired safely.
    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()>;

    /// Retires buffers together when the backend supports shared cleanup submissions.
    /// # Errors
    /// Returns the first retirement error; earlier handles may already be invalidated.
    fn destroy_buffer_batch(&mut self, buffers: &[BufferId]) -> Result<()> {
        for &buffer in buffers {
            self.destroy_buffer(buffer)?;
        }
        Ok(())
    }

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or cannot be retired safely.
    fn destroy_texture(&mut self, texture: TextureId) -> Result<()>;

    /// Destroys a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()>;

    /// Destroys a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()>;

    /// Destroys a bind-group layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()>;

    /// Destroys a bind group.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()>;

    /// Destroys a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()>;

    /// Destroys a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()>;

    /// Destroys a render pass layout.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()>;

    /// Destroys a render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] if the handle is stale or invalid.
    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()>;
}

impl<T> ExtensionDevice for T
where
    T: ResourceDevice + PipelineDevice,
{
    fn texture_array_capabilities(&self) -> gfx_core::TextureArrayCapabilities {
        ResourceDevice::texture_array_capabilities(self)
    }

    fn buffer_binding_limits(&self) -> gfx_core::BufferBindingLimits {
        ResourceDevice::buffer_binding_limits(self)
    }

    fn multisample_capabilities(&self) -> gfx_core::MultisampleCapabilities {
        ResourceDevice::multisample_capabilities(self)
    }

    fn compute_capabilities(&self) -> gfx_core::ComputeCapabilities {
        ResourceDevice::compute_capabilities(self)
    }

    fn dispatch_compute(&mut self, dispatch: &gfx_core::ComputeDispatch<'_>) -> Result<()> {
        ResourceDevice::dispatch_compute(self, dispatch)
    }
    fn dispatch_compute_batch(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
    ) -> Result<()> {
        ResourceDevice::dispatch_compute_batch(self, dispatches)
    }
    fn gpu_work_timestamps_supported(&self) -> bool {
        ResourceDevice::gpu_work_timestamps_supported(self)
    }
    fn dispatch_compute_batch_profiled(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
        tag: GpuTimestampTag,
    ) -> Result<()> {
        ResourceDevice::dispatch_compute_batch_profiled(self, dispatches, tag)
    }
    fn write_buffer_batch_profiled(
        &mut self,
        writes: &[BufferWrite<'_>],
        tag: GpuTimestampTag,
    ) -> Result<BufferUploadStats> {
        ResourceDevice::write_buffer_batch_profiled(self, writes.iter().copied(), tag)
    }
    fn drain_gpu_work_timestamps(&mut self, samples: &mut Vec<GpuTimestampSample>) -> Result<()> {
        ResourceDevice::drain_gpu_work_timestamps(self, samples)
    }
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        ResourceDevice::create_buffer(self, desc)
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        ResourceDevice::write_buffer(self, buffer, offset, data)
    }

    fn write_buffer_batch(&mut self, writes: &[BufferWrite<'_>]) -> Result<BufferUploadStats> {
        ResourceDevice::write_buffer_batch(self, writes.iter().copied())
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        ResourceDevice::create_texture(self, desc)
    }

    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        ResourceDevice::write_texture(self, desc, data)
    }

    fn write_texture_array_layer(
        &mut self,
        desc: gfx_core::TextureArrayLayerWriteDescriptor,
        data: &[u8],
    ) -> Result<()> {
        ResourceDevice::write_texture_array_layer(self, desc, data)
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

    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        ResourceDevice::create_bind_group_layout(self, desc)
    }

    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        ResourceDevice::create_bind_group(self, desc)
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
        viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId> {
        PipelineDevice::create_render_pipeline(self, desc, viewport_extent)
    }

    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
        ResourceDevice::destroy_buffer(self, buffer)
    }

    fn destroy_buffer_batch(&mut self, buffers: &[BufferId]) -> Result<()> {
        ResourceDevice::destroy_buffer_batch(self, buffers)
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

    fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()> {
        ResourceDevice::destroy_bind_group_layout(self, layout)
    }

    fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()> {
        ResourceDevice::destroy_bind_group(self, bind_group)
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
