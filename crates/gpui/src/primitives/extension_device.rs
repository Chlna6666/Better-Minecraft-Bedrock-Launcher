//! Object-safe resource and pipeline access for renderer-owned extensions.
//!
//! GPUI hands this facade to [`RendererExtension`](super::RendererExtension)
//! implementations. It deliberately excludes window surfaces, command submission,
//! and presentation: the host window renderer owns those operations and calls the
//! extension back with resources it can create and destroy.

use gfx_core::{
    BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor, BindGroupLayoutId,
    BufferDescriptor, BufferId, BufferUploadStats, BufferWrite, PipelineDevice,
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
