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
                let step = DrawStepDescriptor {
                    pipeline: draw.pipeline,
                    resource_sets: draw.resource_sets,
                    vertex_count: draw.vertex_count,
                    first_vertex: draw.first_vertex,
                    instance_count: draw.instance_count,
                    first_instance: draw.first_instance,
                    scissor: draw.scissor,
                };
                self.render_target(
                    draw.pass.target,
                    draw.pass.render_pass,
                    RenderStepList::Draw(&[step]),
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
    fn draw_steps_to_texture(
        &mut self,
        view: TextureViewId,
        pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        load: LoadOp<ClearColor>,
    ) -> Result<()> {
        self.render_target(
            RenderTarget::TextureView(view),
            pass,
            RenderStepList::Draw(steps),
            load,
            None,
        )
    }
    fn draw_steps_and_present(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        color: ClearColor,
    ) -> Result<()> {
        let submission = self.present(id, pass, RenderStepList::Draw(steps), color, None)?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn render_steps_to_texture_compat(
        &mut self,
        view: TextureViewId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        load: LoadOp<ClearColor>,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_target(
            RenderTarget::TextureView(view),
            pass,
            RenderStepList::Render(steps),
            load,
            depth,
        )
    }
    fn render_steps_and_present_compat(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_and_present_compat(
            id,
            pass,
            RenderStepList::Render(steps),
            color,
            depth,
        )
    }
    fn render_steps_and_present_deferred_compat(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.present(id, pass, RenderStepList::Render(steps), color, depth)
    }
    fn render_step_list_to_texture_compat(
        &mut self,
        view: TextureViewId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        load: LoadOp<ClearColor>,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_target(RenderTarget::TextureView(view), pass, steps, load, depth)
    }
    fn render_step_list_and_present_compat(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        let submission = self.present(id, pass, steps, color, depth)?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn render_step_list_and_present_deferred_compat(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.present(id, pass, steps, color, depth)
    }
}
