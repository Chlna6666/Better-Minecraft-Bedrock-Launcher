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
    fn surface_capabilities(&self, swapchain: SwapchainId) -> SurfaceCapabilities {
        SurfaceCapabilities {
            partial_presentation: false,
            frame_ready_notification: self.swapchains.get(swapchain).is_ok(),
        }
    }

    fn request_frame_ready(
        &mut self, swapchain: SwapchainId, callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        if self.poll_present_readiness(swapchain)? {
            callback();
            return Ok(true);
        }
        let chain = self.swapchains.get(swapchain)?;
        let Some(id) = chain.present_history[chain.present_limit - 1] else {
            return Ok(false);
        };
        SubmissionDevice::request_submission_ready(self, id, callback)
    }

    fn ordered_mixed_graphics_compute_supported(&self) -> bool {
        // Render and Compute use the same current GL context; writable
        // resource-set dispatch publishes the corresponding memory barriers.
        true
    }

    // GLsync completion is GPU backpressure, not compositor presentation feedback.
    // The ordered context guarantees completion of earlier work when this fence signals.
    // Two inline IDs suffice; no additional fences or per-frame allocation.
    fn poll_present_readiness(&mut self, swapchain: SwapchainId) -> Result<bool> {
        let chain = self.swapchains.get(swapchain)?;
        if chain.config.present_mode != PresentMode::Fifo {
            return Ok(true);
        }
        let index = chain.present_limit - 1;
        let Some(submission) = chain.present_history[index] else {
            return Ok(true);
        };
        match self.poll_submission(submission)? {
            SubmissionStatus::Complete => {
                let history = &mut self.swapchains.get_mut(swapchain)?.present_history;
                history[index..].fill(None);
                Ok(true)
            }
            SubmissionStatus::Pending => Ok(false),
            SubmissionStatus::Failed(error) => Err(Error::Backend(error)),
        }
    }

    fn set_frame_latency_limit(&mut self, swapchain: SwapchainId, requested: u32) -> Result<Option<u32>> {
        let chain = self.swapchains.get_mut(swapchain)?;
        if chain.config.present_mode != PresentMode::Fifo {
            return Ok(None);
        }
        let limit = requested.clamp(1, 2);
        // Never discard an outstanding fence on a budget change.
        chain.present_limit = limit as usize;
        Ok(Some(limit))
    }

    fn render_and_present(
        &mut self,
        frame: &PresentFrameDescriptor<'_>,
    ) -> Result<Option<PresentedFrame>> {
        if !self.poll_present_readiness(frame.swapchain)? {
            return Ok(None);
        }
        let (submission, timings) = self.present(
            frame.swapchain,
            frame.render_pass,
            frame.commands,
            frame.color_load_op,
            frame.depth_attachment,
        )?;
        Ok(Some(PresentedFrame {
            submission: Some(submission),
            timings: Some(timings),
        }))
    }

    fn present_previous_frame(&mut self, swapchain: SwapchainId, target: Extent2d) -> Result<()> {
        OpenGlDevice::present_previous_frame(self, swapchain, target)
    }

    fn render_to_texture(&mut self, pass: &RenderPassCommands<'_>) -> Result<()> {
        let targets = pass
            .color_attachments
            .iter()
            .map(|&view| RenderTarget::TextureView(view))
            .collect::<Vec<_>>();
        self.render_target_clipped(
            &targets,
            pass.resolve_target,
            pass.render_pass,
            pass.commands,
            pass.color_load_op,
            pass.depth_attachment,
            pass.clear_region,
            pass.viewport,
            None,
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

    fn indirect_draw_count_limit(&self) -> Option<u32> {
        self.indirect_count.map(|_| i32::MAX as u32)
    }

    fn render_to_texture_counted(
        &mut self, pass: &RenderPassCommands<'_>, counts: &[IndirectCountBinding],
    ) -> Result<()> {
        self.render_to_texture_counted_profiled(pass, counts, None)
    }

    fn render_to_texture_counted_profiled(
        &mut self, pass: &RenderPassCommands<'_>, counts: &[IndirectCountBinding],
        tag: Option<GpuTimestampTag>,
    ) -> Result<()> {
        self.render_to_textures_counted_profiled(std::slice::from_ref(pass), &[counts], &[tag]).map(|_| ())
    }

    fn render_to_textures_counted_profiled(
        &mut self, passes: &[RenderPassCommands<'_>], counts: &[&[IndirectCountBinding]],
        tags: &[Option<GpuTimestampTag>],
    ) -> Result<Option<SubmissionId>> {
        if passes.len() != counts.len() || passes.len() != tags.len() {
            return Err(Error::InvalidInput("counted pass metadata length mismatch".into()));
        }
        // Validate every count table before any rendering starts.
        for (pass, &table) in passes.iter().zip(counts) {
            if table.is_empty() { continue; }
            let limit = self.indirect_draw_count_limit().ok_or_else(|| Error::Unavailable("OpenGL indirect count unavailable".into()))?;
            let mut previous = None;
            for &count in table {
                if previous.is_some_and(|index| index >= count.command_index) {
                    return Err(Error::InvalidInput("indirect count bindings must be strictly ordered".into()));
                }
                previous = Some(count.command_index);
                let (arguments, offset, stride) = match pass.commands.get(count.command_index) {
                    Some(RenderCommand::DrawIndirect(draw)) => (draw.arguments, draw.offset, 16),
                    Some(RenderCommand::DrawIndexedIndirect(draw)) => {
                        let index = self.buffers.get(draw.index_buffer.buffer)?;
                        let bytes = if draw.index_buffer.format == IndexFormat::Uint16 { 2 } else { 4 };
                        if !index.desc.usage.contains(BufferUsage::INDEX)
                            || draw.index_buffer.offset % bytes != 0
                            || draw.index_buffer.offset.checked_add(bytes).is_none_or(|end| end > index.desc.size) {
                            return Err(Error::InvalidInput("invalid OpenGL counted index buffer".into()));
                        }
                        if draw.index_buffer.offset != 0 {
                            let args = self.buffers.get(draw.arguments)?;
                            self.index_rebase_plan(draw.offset, args.desc.size, draw.index_buffer.offset,
                                index.desc.size, bytes, count.max_draw_count)?;
                        }
                        (draw.arguments, draw.offset, 20)
                    }
                    _ => return Err(Error::InvalidInput("OpenGL counted draw requires indirect arguments".into())),
                };
                let args = self.buffers.get(arguments)?;
                let counter = self.buffers.get(count.count_buffer)?;
                if !args.desc.usage.contains(BufferUsage::INDIRECT)
                    || !counter.desc.usage.contains(BufferUsage::INDIRECT)
                    || i32::try_from(offset).is_err() || isize::try_from(count.count_offset).is_err()
                {
                    return Err(Error::InvalidInput("invalid OpenGL counted buffer usage or offset".into()));
                }
                count.validate_ranges(offset, args.desc.size, counter.desc.size, stride, limit)?;
            }
        }
        for ((pass, &table), &tag) in passes.iter().zip(counts).zip(tags) {
            let tag = tag.filter(|tag| tag.stage == GpuTimestampStage::DrawIndirect
                && !pass.commands.is_empty()
                && pass.commands.iter().all(|command| matches!(command,
                    RenderCommand::DrawIndirect(_) | RenderCommand::DrawIndexedIndirect(_))));
            let targets = pass.color_attachments.iter().copied().map(RenderTarget::TextureView).collect::<Vec<_>>();
            self.render_target_counted(&targets, pass.resolve_target, pass.render_pass,
                pass.commands, pass.color_load_op, pass.depth_attachment, pass.clear_region,
                pass.viewport, tag, table)?;
        }
        Ok(None)
    }

    fn compact_draw_run_limit(&self) -> u32 {
        if self.multi_draw.is_some() { i32::MAX as u32 } else { 0 }
    }

    fn render_to_textures_runs_profiled(
        &mut self, passes: &[RenderPassCommands<'_>],
        owners: &[std::sync::Arc<[RenderCommand]>], supplied: &[Option<std::sync::Arc<DrawRunPlan>>],
        tables: &[Option<BindGroupId>], tags: &[Option<GpuTimestampTag>],
    ) -> Result<Option<SubmissionId>> {
        if supplied.len() != passes.len() || passes.len() != owners.len() || passes.len() != tags.len()
            || (!tables.is_empty() && tables.len() != passes.len())
            || passes.iter().zip(owners).any(|(pass, owner)| !std::ptr::eq(pass.commands, owner.as_ref()))
        { return Err(Error::InvalidInput("OpenGL representative stream metadata mismatch".into())); }
        if tables.iter().any(Option::is_some) {
            return Err(Error::Unavailable("OpenGL pass-level sampled tables unsupported".into()));
        }
        let limit = PresentationDevice::compact_draw_run_limit(self);
        if supplied.iter().zip(owners).any(|(plan, owner)| plan.as_ref().is_some_and(|plan|
            !plan.matches_representatives(owner, limit)))
        { return Err(Error::InvalidInput("OpenGL representative stream identity or limit mismatch".into())); }
        if supplied.iter().all(Option::is_none) {
            return self.render_to_textures_owned_profiled(passes, owners, tables, tags);
        }
        for (((pass, owner), supplied), &tag) in passes.iter().zip(owners).zip(supplied).zip(tags) {
            let plan = supplied.clone().unwrap_or_else(|| std::sync::Arc::new(DrawRunPlan::new(owner, limit)));
            let targets = pass.color_attachments.iter().copied().map(RenderTarget::TextureView).collect::<Vec<_>>();
            let tag = tag.filter(|tag| tag.stage == GpuTimestampStage::DrawIndirect && plan.indirect_only());
            self.render_target_planned(&targets, pass.resolve_target, pass.render_pass,
                pass.commands, pass.color_load_op, pass.depth_attachment, pass.clear_region,
                pass.viewport, tag, &[], Some(&plan))?;
        }
        Ok(None)
    }

    fn render_to_textures_owned_profiled(
        &mut self, passes: &[RenderPassCommands<'_>],
        owners: &[std::sync::Arc<[RenderCommand]>], tables: &[Option<BindGroupId>],
        tags: &[Option<GpuTimestampTag>],
    ) -> Result<Option<SubmissionId>> {
        if passes.len() != owners.len() || passes.len() != tags.len()
            || (!tables.is_empty() && tables.len() != passes.len())
            || passes.iter().zip(owners).any(|(pass, owner)| !std::ptr::eq(pass.commands, owner.as_ref()))
        { return Err(Error::InvalidInput("OpenGL owned draw stream metadata mismatch".into())); }
        if tables.iter().any(Option::is_some) {
            return Err(Error::Unavailable("OpenGL pass-level sampled tables unsupported".into()));
        }
        if self.multi_draw.is_none() {
            return self.render_to_textures_on_queue(passes, tags, OffscreenQueuePreference::OrderedGraphics);
        }
        self.draw_run_plans.retain(|plan| plan.is_live());
        let limit = i32::MAX as u32;
        for ((pass, owner), &tag) in passes.iter().zip(owners).zip(tags) {
            let plan = if let Some(index) = self.draw_run_plans.iter().position(|plan| plan.matches(owner, limit)) {
                self.draw_run_plans.remove(index)
            } else {
                if self.draw_run_plans.len() >= 32 { self.draw_run_plans.remove(0); }
                std::sync::Arc::new(DrawRunPlan::new(owner, limit))
            };
            self.draw_run_plans.push(std::sync::Arc::clone(&plan));
            let targets = pass.color_attachments.iter().copied().map(RenderTarget::TextureView).collect::<Vec<_>>();
            let tag = tag.filter(|tag| tag.stage == GpuTimestampStage::DrawIndirect && plan.indirect_only());
            self.render_target_planned(&targets, pass.resolve_target, pass.render_pass,
                pass.commands, pass.color_load_op, pass.depth_attachment, pass.clear_region,
                pass.viewport, tag, &[], Some(&plan))?;
        }
        Ok(None)
    }

    fn render_to_textures_on_queue(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tags: &[Option<GpuTimestampTag>],
        _queue: OffscreenQueuePreference,
    ) -> Result<Option<SubmissionId>> {
        if passes.len() != tags.len() {
            return Err(Error::InvalidInput(
                "offscreen GPU tag count mismatch".into(),
            ));
        }
        for (pass, tag) in passes.iter().zip(tags) {
            let tag = tag.filter(|tag| {
                tag.stage == GpuTimestampStage::DrawIndirect
                    && !pass.commands.is_empty()
                    && pass.commands.iter().all(|command| {
                        matches!(
                            command,
                            RenderCommand::DrawIndirect(_) | RenderCommand::DrawIndexedIndirect(_)
                        )
                    })
            });
            let targets = pass
                .color_attachments
                .iter()
                .copied()
                .map(RenderTarget::TextureView)
                .collect::<Vec<_>>();
            self.render_target_clipped(
                &targets,
                pass.resolve_target,
                pass.render_pass,
                pass.commands,
                pass.color_load_op,
                pass.depth_attachment,
                pass.clear_region,
                pass.viewport,
                tag,
            )?;
        }
        Ok(None)
    }
}
