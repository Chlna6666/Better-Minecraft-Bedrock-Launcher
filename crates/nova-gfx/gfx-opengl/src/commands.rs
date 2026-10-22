use crate::device::OpenGlDevice;
use gfx_core::*;

impl CommandDevice for OpenGlDevice {
    fn create_command_encoder(
        &mut self,
        _desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        Ok(self.encoders.insert(Vec::new()))
    }
    fn record_draw_desc(&mut self, id: CommandEncoderId, draw: DrawDescriptor) -> Result<()> {
        self.passes.get(draw.pass.render_pass)?;
        self.pipelines.get(draw.pipeline)?;
        self.encoders.get_mut(id)?.push(draw);
        Ok(())
    }
    fn submit(&mut self, id: CommandEncoderId) -> Result<()> {
        self.execute_encoder(id)?;
        let submission = self.signal()?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn destroy_command_encoder(&mut self, id: CommandEncoderId) -> Result<()> {
        self.encoders.take(id)?;
        Ok(())
    }
}
impl OpenGlDevice {
    pub(crate) fn execute_encoder(&mut self, id: CommandEncoderId) -> Result<()> {
        let mut draws = std::mem::take(self.encoders.get_mut(id)?);
        let result = (|| {
            for draw in draws.drain(..) {
                let command = RenderCommand::Draw(DrawCommand {
                    pipeline: draw.pipeline,
                    bind_groups: draw.bind_groups,
                    vertex_count: draw.vertex_count,
                    first_vertex: draw.first_vertex,
                    instance_count: draw.instance_count,
                    first_instance: draw.first_instance,
                    scissor: draw.scissor,
                });
                self.render_target(
                    draw.pass.target,
                    draw.pass.render_pass,
                    std::slice::from_ref(&command),
                    draw.pass.color_load_op,
                    None,
                )?;
            }
            Ok(())
        })();
        // Reuse command storage after both successful submissions and rendering errors.
        *self.encoders.get_mut(id)? = draws;
        result
    }
}

impl PresentationDevice for OpenGlDevice {
    fn render_and_present(
        &mut self,
        frame: &PresentFrameDescriptor<'_>,
    ) -> Result<Option<PresentedFrame>> {
        let submission = self.present(
            frame.swapchain,
            frame.render_pass,
            frame.commands,
            frame.color_load_op,
            frame.depth_attachment,
        )?;
        Ok(Some(PresentedFrame {
            submission: Some(submission),
            timings: None,
        }))
    }

    fn present_retained_frame(&mut self, swapchain: SwapchainId, target: Extent2d) -> Result<()> {
        OpenGlDevice::present_retained_frame(self, swapchain, target)
    }

    fn render_to_texture(&mut self, pass: &RenderPassCommands<'_>) -> Result<()> {
        self.render_target_clipped(
            RenderTarget::TextureView(pass.texture_view),
            pass.render_pass,
            pass.commands,
            pass.color_load_op,
            pass.depth_attachment,
            pass.clear_region,
            pass.viewport,
        )
    }

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
