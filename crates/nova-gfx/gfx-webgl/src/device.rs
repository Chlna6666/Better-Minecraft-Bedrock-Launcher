use gfx_core::{
    Backend, BackendKind, BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor,
    BindGroupLayoutId, BufferDescriptor, BufferId, ClearColor, CommandDevice,
    CommandEncoderDescriptor, CommandEncoderId, DepthAttachment, DeviceDescriptor,
    DiagnosticsDevice, DrawCommand, DrawDescriptor, Error, LoadOp, PipelineDevice,
    PipelineLayoutDescriptor, PipelineLayoutId, PresentationDevice, RenderCommand,
    RenderPassDescriptor, RenderPassId, RenderPipelineDescriptor, RenderPipelineId, ResourceDevice,
    ResourceStats, Result, SamplerDescriptor, SamplerId, ShaderModuleDescriptor, ShaderModuleId,
    SubmissionDevice, SubmissionId, SubmissionStatus, SurfaceConfig, SurfaceDescriptor,
    SurfaceDevice, SurfaceId, SwapchainId, TextureDescriptor, TextureId, TextureViewDescriptor,
    TextureViewId, TextureWriteDescriptor,
};

/// Stub WebGL device.
pub struct WebGlDevice;

impl WebGlDevice {
    /// Returns unavailable until WebGL integration is implemented.
    ///
    /// # Errors
    ///
    /// Always returns [`Error::Unavailable`].
    pub fn new(_desc: &DeviceDescriptor) -> Result<Self> {
        unavailable()
    }
}

fn unavailable<T>() -> Result<T> {
    Err(Error::Unavailable(
        "WebGL backend is not implemented yet".to_string(),
    ))
}

impl Backend for WebGlDevice {
    const BACKEND_KIND: BackendKind = BackendKind::WebGl;

    fn adapter_name(&self) -> &str {
        "WebGL"
    }
}

impl SurfaceDevice for WebGlDevice {
    type SurfaceTarget = ();

    fn create_surface(
        &mut self,
        _target: &Self::SurfaceTarget,
        _desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        unavailable()
    }

    fn create_swapchain(
        &mut self,
        _surface: SurfaceId,
        _config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        unavailable()
    }

    fn destroy_swapchain(&mut self, _swapchain: SwapchainId) -> Result<()> {
        unavailable()
    }

    fn destroy_surface(&mut self, _surface: SurfaceId) -> Result<()> {
        unavailable()
    }
}

impl ResourceDevice for WebGlDevice {
    fn create_buffer(&mut self, _desc: &BufferDescriptor) -> Result<BufferId> {
        unavailable()
    }

    fn write_buffer(&mut self, _buffer: BufferId, _offset: u64, _data: &[u8]) -> Result<()> {
        unavailable()
    }

    fn create_texture(&mut self, _desc: &TextureDescriptor) -> Result<TextureId> {
        unavailable()
    }

    fn write_texture(&mut self, _desc: TextureWriteDescriptor, _data: &[u8]) -> Result<()> {
        unavailable()
    }

    fn create_texture_view(&mut self, _desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        unavailable()
    }

    fn create_sampler(&mut self, _desc: &SamplerDescriptor) -> Result<SamplerId> {
        unavailable()
    }

    fn create_bind_group_layout(
        &mut self,
        _desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        unavailable()
    }

    fn create_bind_group(&mut self, _desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        unavailable()
    }

    fn destroy_buffer(&mut self, _buffer: BufferId) -> Result<()> {
        unavailable()
    }

    fn destroy_texture(&mut self, _texture: TextureId) -> Result<()> {
        unavailable()
    }

    fn destroy_texture_view(&mut self, _view: TextureViewId) -> Result<()> {
        unavailable()
    }

    fn destroy_sampler(&mut self, _sampler: SamplerId) -> Result<()> {
        unavailable()
    }

    fn destroy_bind_group_layout(&mut self, _layout: BindGroupLayoutId) -> Result<()> {
        unavailable()
    }

    fn destroy_bind_group(&mut self, _resource_set: BindGroupId) -> Result<()> {
        unavailable()
    }
}

impl PipelineDevice for WebGlDevice {
    fn create_pipeline_layout(
        &mut self,
        _desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        unavailable()
    }

    fn create_shader_module(&mut self, _desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        unavailable()
    }

    fn create_render_pass(&mut self, _desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        unavailable()
    }

    fn create_render_pipeline(
        &mut self,
        _desc: &RenderPipelineDescriptor,
        _viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId> {
        unavailable()
    }

    fn destroy_pipeline_layout(&mut self, _layout: PipelineLayoutId) -> Result<()> {
        unavailable()
    }

    fn destroy_shader_module(&mut self, _shader: ShaderModuleId) -> Result<()> {
        unavailable()
    }

    fn destroy_render_pass(&mut self, _render_pass: RenderPassId) -> Result<()> {
        unavailable()
    }

    fn destroy_render_pipeline(&mut self, _pipeline: RenderPipelineId) -> Result<()> {
        unavailable()
    }
}

impl CommandDevice for WebGlDevice {
    fn create_command_encoder(
        &mut self,
        _desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        unavailable()
    }

    fn record_draw_desc(
        &mut self,
        _encoder: CommandEncoderId,
        _draw: DrawDescriptor,
    ) -> Result<()> {
        unavailable()
    }

    fn submit(&mut self, _encoder: CommandEncoderId) -> Result<()> {
        unavailable()
    }

    fn destroy_command_encoder(&mut self, _encoder: CommandEncoderId) -> Result<()> {
        unavailable()
    }
}

impl SubmissionDevice for WebGlDevice {
    fn submit_deferred(&mut self, _encoder: CommandEncoderId) -> Result<SubmissionId> {
        unavailable()
    }

    fn poll_submission(&mut self, _submission: SubmissionId) -> Result<SubmissionStatus> {
        unavailable()
    }

    fn wait_submission(&mut self, _submission: SubmissionId) -> Result<()> {
        unavailable()
    }
}

impl PresentationDevice for WebGlDevice {
    fn render_and_present(
        &mut self,
        _frame: &gfx_core::PresentFrameDescriptor<'_>,
    ) -> Result<Option<gfx_core::PresentedFrame>> {
        unavailable()
    }
}

impl DiagnosticsDevice for WebGlDevice {
    fn resource_stats(&self) -> ResourceStats {
        ResourceStats::default()
    }
}
