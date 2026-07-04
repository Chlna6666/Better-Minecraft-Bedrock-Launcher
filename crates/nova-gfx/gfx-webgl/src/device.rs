use gfx_core::{
    BackendKind, BufferDesc, BufferId, ClearColor, CommandEncoderDesc, CommandEncoderId,
    DeviceDesc, DrawDesc, DrawStepDesc, GfxBackend, GfxCommandDevice, GfxDiagnosticsDevice,
    GfxError, GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice, GfxSubmissionDevice,
    GfxSurfaceDevice, LoadOp, PipelineLayoutDesc, PipelineLayoutId, RenderPassDepthAttachment,
    RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId, RenderStepDescriptor,
    ResourceSetDesc, ResourceSetId, ResourceSetLayoutDesc, ResourceSetLayoutId, ResourceStats,
    Result, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, SubmissionId,
    SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId, TextureDesc, TextureId,
    TextureViewDesc, TextureViewId, TextureWriteDesc,
};

/// Stub WebGL device.
pub struct WebGlDevice;

impl WebGlDevice {
    /// Returns unavailable until WebGL integration is implemented.
    ///
    /// # Errors
    ///
    /// Always returns [`GfxError::Unavailable`].
    pub fn new(_desc: &DeviceDesc) -> Result<Self> {
        unavailable()
    }
}

fn unavailable<T>() -> Result<T> {
    Err(GfxError::Unavailable(
        "WebGL backend is not implemented yet".to_string(),
    ))
}

impl GfxBackend for WebGlDevice {
    const BACKEND_KIND: BackendKind = BackendKind::WebGl;
}

impl GfxSurfaceDevice for WebGlDevice {
    type SurfaceTarget = ();

    fn create_surface(
        &mut self,
        _target: &Self::SurfaceTarget,
        _desc: &SurfaceDesc,
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

impl GfxResourceDevice for WebGlDevice {
    fn create_buffer(&mut self, _desc: &BufferDesc) -> Result<BufferId> {
        unavailable()
    }

    fn write_buffer(&mut self, _buffer: BufferId, _offset: u64, _data: &[u8]) -> Result<()> {
        unavailable()
    }

    fn create_texture(&mut self, _desc: &TextureDesc) -> Result<TextureId> {
        unavailable()
    }

    fn write_texture(&mut self, _desc: TextureWriteDesc, _data: &[u8]) -> Result<()> {
        unavailable()
    }

    fn create_texture_view(&mut self, _desc: &TextureViewDesc) -> Result<TextureViewId> {
        unavailable()
    }

    fn create_sampler(&mut self, _desc: &SamplerDesc) -> Result<SamplerId> {
        unavailable()
    }

    fn create_resource_set_layout(
        &mut self,
        _desc: &ResourceSetLayoutDesc,
    ) -> Result<ResourceSetLayoutId> {
        unavailable()
    }

    fn create_resource_set(&mut self, _desc: &ResourceSetDesc) -> Result<ResourceSetId> {
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

    fn destroy_resource_set_layout(&mut self, _layout: ResourceSetLayoutId) -> Result<()> {
        unavailable()
    }

    fn destroy_resource_set(&mut self, _resource_set: ResourceSetId) -> Result<()> {
        unavailable()
    }
}

impl GfxPipelineDevice for WebGlDevice {
    fn create_pipeline_layout(&mut self, _desc: &PipelineLayoutDesc) -> Result<PipelineLayoutId> {
        unavailable()
    }

    fn create_shader_module(&mut self, _desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
        unavailable()
    }

    fn create_render_pass(&mut self, _desc: &RenderPassDesc) -> Result<RenderPassId> {
        unavailable()
    }

    fn create_render_pipeline(
        &mut self,
        _desc: &RenderPipelineDesc,
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

impl GfxCommandDevice for WebGlDevice {
    fn create_command_encoder(&mut self, _desc: &CommandEncoderDesc) -> Result<CommandEncoderId> {
        unavailable()
    }

    fn record_draw_desc(&mut self, _encoder: CommandEncoderId, _draw: DrawDesc) -> Result<()> {
        unavailable()
    }

    fn submit(&mut self, _encoder: CommandEncoderId) -> Result<()> {
        unavailable()
    }

    fn destroy_command_encoder(&mut self, _encoder: CommandEncoderId) -> Result<()> {
        unavailable()
    }
}

impl GfxSubmissionDevice for WebGlDevice {
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

impl GfxPresentationDevice for WebGlDevice {
    fn draw_steps_and_present(
        &mut self,
        _swapchain: SwapchainId,
        _render_pass: RenderPassId,
        _steps: &[DrawStepDesc],
        _clear_color: ClearColor,
    ) -> Result<()> {
        unavailable()
    }

    fn draw_steps_to_texture(
        &mut self,
        _texture_view: TextureViewId,
        _render_pass: RenderPassId,
        _steps: &[DrawStepDesc],
        _color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        unavailable()
    }

    fn render_steps_and_present_compat(
        &mut self,
        _swapchain: SwapchainId,
        _render_pass: RenderPassId,
        _steps: &[RenderStepDescriptor],
        _clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        unavailable()
    }

    fn render_steps_to_texture_compat(
        &mut self,
        _texture_view: TextureViewId,
        _render_pass: RenderPassId,
        _steps: &[RenderStepDescriptor],
        _color_load_op: LoadOp<ClearColor>,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        unavailable()
    }

    fn render_steps_and_present_deferred_compat(
        &mut self,
        _swapchain: SwapchainId,
        _render_pass: RenderPassId,
        _steps: &[RenderStepDescriptor],
        _clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId>
    where
        Self: GfxSubmissionDevice,
    {
        unavailable()
    }
}

impl GfxDiagnosticsDevice for WebGlDevice {
    fn resource_stats(&self) -> ResourceStats {
        ResourceStats::default()
    }
}
