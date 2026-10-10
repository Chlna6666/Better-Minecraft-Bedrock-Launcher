use crate::{device::Dx11Device, resources::Bound};
use gfx_core::*;
use windows::Win32::{
    Foundation::RECT,
    Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::*},
};

// Only cache state within one immediate-context render pass. Other D3D11
// operations and target switches can mutate the context between passes.
#[derive(Default)]
struct Dx11PassState {
    pipeline: Option<u64>,
    scissor: Option<(i32, i32, i32, i32)>,
}

impl CommandDevice for Dx11Device {
    fn create_command_encoder(
        &mut self,
        _desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        Ok(self.encoders.insert(Vec::new()))
    }
    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDescriptor) -> Result<()> {
        self.passes.get(draw.pass.render_pass)?;
        self.pipelines.get(draw.pipeline)?;
        self.encoders.get_mut(encoder)?.push(draw);
        Ok(())
    }
    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.execute_encoder(encoder)?;
        let submission = self.signal()?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.encoders.take(encoder)?;
        Ok(())
    }
}

impl Dx11Device {
    pub(crate) fn execute_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        let mut draws = std::mem::take(self.encoders.get_mut(encoder)?);
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
        *self.encoders.get_mut(encoder)? = draws;
        result
    }
    pub(crate) fn render_target(
        &mut self,
        target: RenderTarget,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        load: LoadOp<ClearColor>,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        let pass = self.passes.get(pass)?;
        let (rtv, size, target_format) = match target {
            RenderTarget::TextureView(id) => {
                let view = self.views.get(id)?;
                (
                    view.rtv.as_ref().ok_or_else(|| {
                        Error::InvalidInput("target view is not renderable".into())
                    })?,
                    view.size,
                    view.format,
                )
            }
            RenderTarget::Swapchain {
                swapchain,
                image_index,
            } => {
                let chain = self.swapchains.get(swapchain)?;
                if image_index != 0 {
                    return Err(Error::InvalidInput(
                        "D3D11 presents logical backbuffer zero".into(),
                    ));
                }
                (
                    chain.view.as_ref().ok_or_else(|| {
                        Error::Unavailable("D3D11 backbuffer view is unavailable".into())
                    })?,
                    chain.config.size,
                    chain.config.format,
                )
            }
        };
        if pass.color_attachment.format != target_format {
            return Err(Error::InvalidInput(
                "render pass color format mismatch".into(),
            ));
        }
        let depth_view = depth
            .map(|depth| self.views.get(depth.target))
            .transpose()?;
        let dsv = depth_view
            .map(|view| {
                view.dsv.as_ref().ok_or_else(|| {
                    Error::InvalidInput("depth view is not a depth attachment".into())
                })
            })
            .transpose()?;
        if depth_view.is_some_and(|view| view.size != size) {
            return Err(Error::InvalidInput("depth extent mismatch".into()));
        }
        // SAFETY: the immediate context is owned by this device. Clear all SRV slots
        // actually populated by earlier draws before rebinding an RTV, but avoid two
        // 128-entry native calls for every offscreen/blur pass.
        unsafe {
            let none: [Option<ID3D11ShaderResourceView>; 128] = [const { None }; 128];
            for (mask, vertex) in [
                (&self.vs_srv_slots, true),
                (&self.ps_srv_slots, false),
            ] {
                let occupied = mask.replace(0);
                if occupied == 0 {
                    continue;
                }
                let first = occupied.trailing_zeros();
                let end = u128::BITS - occupied.leading_zeros();
                let empty_span = &none[..(end - first) as usize];
                if vertex {
                    self.context.VSSetShaderResources(first, Some(empty_span));
                } else {
                    self.context.PSSetShaderResources(first, Some(empty_span));
                }
            }
            self.context
                .OMSetRenderTargets(Some(&[Some(rtv.clone())]), dsv);
            self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: size.width() as f32,
                Height: size.height() as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            if let LoadOp::Clear(color) = load {
                self.context
                    .ClearRenderTargetView(rtv, &[color.red, color.green, color.blue, color.alpha]);
            }
            if let (Some(depth), Some(dsv)) = (depth, dsv) {
                if let LoadOp::Clear(value) = depth.depth_load_op {
                    self.context
                        .ClearDepthStencilView(dsv, D3D11_CLEAR_DEPTH.0 as u32, value, 0);
                }
            }
        }
        let mut state = Dx11PassState::default();
        for step in steps.iter() {
            self.draw_step(step, size, target_format, dsv.is_some(), &mut state)?;
        }
        // SAFETY: remove output bindings before these targets are sampled by another pass.
        unsafe {
            self.context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
    fn draw_step(
        &self,
        step: RenderStepRef<'_>,
        size: Extent2d,
        target_format: Format,
        has_depth: bool,
        state: &mut Dx11PassState,
    ) -> Result<()> {
        let pipeline = self.pipelines.get(step.pipeline())?;
        if pipeline.desc.color_format != target_format
            || (pipeline.desc.depth_state.is_some() && !has_depth)
        {
            return Err(Error::InvalidInput(
                "D3D11 pipeline attachment mismatch".into(),
            ));
        }
        let layouts = pipeline
            .desc
            .pipeline_layout
            .map(|id| self.pipeline_layouts.get(id))
            .transpose()?;
        let expected = layouts.map_or(&[][..], |value| value.resource_set_layouts.as_slice());
        if step.resource_sets().len() != expected.len() {
            return Err(Error::InvalidInput(
                "resource set count differs from pipeline layout".into(),
            ));
        }
        let scissor = step.scissor().unwrap_or(ScissorRect {
            x: 0,
            y: 0,
            width: size.width(),
            height: size.height(),
        });
        let rect = RECT {
            left: scissor.x.min(size.width()) as i32,
            top: scissor.y.min(size.height()) as i32,
            right: (i64::from(scissor.x) + i64::from(scissor.width)).min(i64::from(size.width()))
                as i32,
            bottom: (i64::from(scissor.y) + i64::from(scissor.height)).min(i64::from(size.height()))
                as i32,
        };
        // SAFETY: native state objects are retained by this pipeline; all slices
        // remain live through calls. Shader/resource bindings still update for
        // every draw; only unchanged pass-local pipeline/scissor state is skipped.
        unsafe {
            let pipeline_key = step.pipeline().raw();
            if state.pipeline != Some(pipeline_key) {
                self.context.VSSetShader(&pipeline.vertex, None);
                self.context.PSSetShader(&pipeline.fragment, None);
                self.context
                    .OMSetBlendState(&pipeline.blend, None, u32::MAX);
                self.context.RSSetState(&pipeline.raster);
                self.context.OMSetDepthStencilState(&pipeline.depth, 0);
                self.context.IASetInputLayout(None);
                self.context
                    .IASetPrimitiveTopology(match pipeline.desc.primitive_topology {
                        PrimitiveTopology::TriangleList => D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                        PrimitiveTopology::TriangleStrip => D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
                    });
                state.pipeline = Some(pipeline_key);
            }
            let scissor_key = (rect.left, rect.top, rect.right, rect.bottom);
            if state.scissor != Some(scissor_key) {
                self.context.RSSetScissorRects(Some(&[rect]));
                state.scissor = Some(scissor_key);
            }
        }
        for (id, layout) in step.resource_sets().iter().zip(expected) {
            let set = self.sets.get(*id)?;
            if &set.layout != layout {
                return Err(Error::InvalidInput("resource set layout mismatch".into()));
            }
            for (slot, stages, binding) in &set.bindings {
                self.bind(*slot, *stages, binding);
            }
        }
        let (vertex_offset, instance_offset) = match step {
            RenderStepRef::Draw(step) => (step.first_vertex, step.first_instance),
            RenderStepRef::DrawIndexed(step) => (step.base_vertex as u32, step.first_instance),
        };
        let constants = [vertex_offset, instance_offset, 0, 0];
        // SAFETY: constants contain 16 initialized bytes and match the reserved NagaConstants ABI.
        unsafe {
            self.context.UpdateSubresource(
                &self.draw_constants,
                0,
                None,
                constants.as_ptr().cast(),
                0,
                0,
            );
            self.context
                .VSSetConstantBuffers(13, Some(&[Some(self.draw_constants.clone())]));
            match step {
                RenderStepRef::Draw(step) => {
                    self.context
                        .DrawInstanced(step.vertex_count, step.instance_count, 0, 0)
                }
                RenderStepRef::DrawIndexed(step) => {
                    let buffer = self.buffers.get(step.index_buffer.buffer)?;
                    let index_size = match step.index_buffer.format {
                        IndexFormat::Uint16 => 2u64,
                        IndexFormat::Uint32 => 4,
                    };
                    if !buffer.desc.usage.contains(BufferUsage::INDEX)
                        || step.index_buffer.offset % index_size != 0
                        || step
                            .index_buffer
                            .offset
                            .checked_add(
                                (u64::from(step.first_index) + u64::from(step.index_count))
                                    * index_size,
                            )
                            .is_none_or(|end| end > buffer.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "index buffer range/usage mismatch".into(),
                        ));
                    }
                    self.context.IASetIndexBuffer(
                        &buffer.native,
                        if index_size == 2 {
                            DXGI_FORMAT_R16_UINT
                        } else {
                            DXGI_FORMAT_R32_UINT
                        },
                        step.index_buffer.offset as u32,
                    );
                    self.context.DrawIndexedInstanced(
                        step.index_count,
                        step.instance_count,
                        step.first_index,
                        0,
                        0,
                    );
                }
            }
        }
        Ok(())
    }
    fn bind(&self, slot: u32, stages: ShaderStages, binding: &Bound) {
        // SAFETY: each bound COM resource is retained by its set, and layout creation
        // validated slots against native limits. Context1 constant offsets use 16-byte units.
        unsafe {
            match binding {
                Bound::Uniform {
                    buffer,
                    first,
                    count,
                } => {
                    let values = [Some(buffer.clone())];
                    if stages.contains(ShaderStages::VERTEX) {
                        self.context.VSSetConstantBuffers1(
                            slot,
                            1,
                            Some(values.as_ptr()),
                            Some(first),
                            Some(count),
                        );
                    }
                    if stages.contains(ShaderStages::FRAGMENT) {
                        self.context.PSSetConstantBuffers1(
                            slot,
                            1,
                            Some(values.as_ptr()),
                            Some(first),
                            Some(count),
                        );
                    }
                }
                Bound::Srv(view) => {
                    if stages.contains(ShaderStages::VERTEX) {
                        self.context
                            .VSSetShaderResources(slot, Some(&[Some(view.clone())]));
                        self.vs_srv_slots.set(self.vs_srv_slots.get() | (1_u128 << slot));
                    }
                    if stages.contains(ShaderStages::FRAGMENT) {
                        self.context
                            .PSSetShaderResources(slot, Some(&[Some(view.clone())]));
                        self.ps_srv_slots.set(self.ps_srv_slots.get() | (1_u128 << slot));
                    }
                }
                Bound::Sampler(sampler) => {
                    if stages.contains(ShaderStages::VERTEX) {
                        self.context
                            .VSSetSamplers(slot, Some(&[Some(sampler.clone())]));
                    }
                    if stages.contains(ShaderStages::FRAGMENT) {
                        self.context
                            .PSSetSamplers(slot, Some(&[Some(sampler.clone())]));
                    }
                }
            }
        }
    }
}

impl PresentationDevice for Dx11Device {
    fn render_steps_to_texture_compat(
        &mut self,
        view: TextureViewId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        load: LoadOp<ClearColor>,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_to_texture_compat(
            view,
            pass,
            RenderStepList::Render(steps),
            load,
            depth,
        )
    }
    fn render_steps_and_present_compat(
        &mut self,
        chain: SwapchainId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.render_step_list_and_present_compat(
            chain,
            pass,
            RenderStepList::Render(steps),
            color,
            depth,
        )
    }
    fn render_steps_and_present_deferred_compat(
        &mut self,
        chain: SwapchainId,
        pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.present_steps(chain, pass, RenderStepList::Render(steps), color, depth)
    }
    fn draw_steps_and_present(
        &mut self,
        chain: SwapchainId,
        pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        color: ClearColor,
    ) -> Result<()> {
        self.render_step_list_and_present_compat(
            chain,
            pass,
            RenderStepList::Draw(steps),
            color,
            None,
        )
    }
    fn draw_steps_to_texture(
        &mut self,
        view: TextureViewId,
        pass: RenderPassId,
        steps: &[DrawStepDescriptor],
        load: LoadOp<ClearColor>,
    ) -> Result<()> {
        self.render_step_list_to_texture_compat(view, pass, RenderStepList::Draw(steps), load, None)
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
        chain: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        let submission = self.present_steps(chain, pass, steps, color, depth)?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn render_step_list_and_present_deferred_compat(
        &mut self,
        chain: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.present_steps(chain, pass, steps, color, depth)
    }
    fn arm_swapchain_frame_ready(
        &mut self,
        chain: SwapchainId,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        let chain = self.swapchains.get_mut(chain)?;
        chain.pacing.arm(chain.latency, callback)?;
        Ok(true)
    }
    fn presentation_capabilities(&self, chain: SwapchainId) -> PresentationCapabilities {
        PresentationCapabilities {
            partial_presentation: false,
            frame_ready_notification: self.swapchains.get(chain).is_ok(),
        }
    }
    fn set_swapchain_content_stretch(
        &mut self,
        chain: SwapchainId,
        scale: Option<[f32; 2]>,
    ) -> Result<()> {
        self.stretch(chain, scale)
    }
}
