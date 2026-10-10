use crate::device::{OpenGlDevice, native};
use gfx_core::*;
use glow::HasContext as _;

// State is valid for one native render pass only. Never carry it across context
// changes, pass boundaries or external GL calls.
#[derive(Default)]
struct GlPassState {
    pipeline: Option<u64>,
    scissor: Option<(i32, i32, i32, i32)>,
}

impl OpenGlDevice {
    pub(crate) fn render_target(
        &self,
        target: RenderTarget,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        load: LoadOp<ClearColor>,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        self.park()?;
        let pass = self.passes.get(pass)?;
        let (texture, framebuffer, size, format) = match target {
            RenderTarget::TextureView(id) => {
                let view = self.views.get(id)?;
                if !view.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
                    return Err(Error::InvalidInput(
                        "OpenGL target is not a color attachment".into(),
                    ));
                }
                (view.native, self.framebuffer, view.size, view.format)
            }
            RenderTarget::Swapchain {
                swapchain,
                image_index,
            } => {
                if image_index != 0 {
                    return Err(Error::InvalidInput(
                        "OpenGL presents logical image zero".into(),
                    ));
                }
                let chain = self.swapchains.get(swapchain)?;
                (
                    chain.color,
                    chain.framebuffer,
                    chain.config.size,
                    chain.config.format,
                )
            }
        };
        if pass.color_attachment.format != format {
            return Err(Error::InvalidInput(
                "OpenGL render pass color mismatch".into(),
            ));
        }
        let depth_view = depth
            .map(|attachment| self.views.get(attachment.target))
            .transpose()?;
        if depth_view.is_some_and(|view| {
            view.format != Format::Depth32Float
                || view.size != size
                || !view.usage.contains(TextureUsage::DEPTH_ATTACHMENT)
        }) {
            return Err(Error::InvalidInput(
                "OpenGL depth extent/usage mismatch".into(),
            ));
        }
        // SAFETY: live texture views are attached to one owner FBO. Scissor/depth writes
        // are reset before clears so previous draw state cannot limit this pass.
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                depth_view.map(|view| view.native),
                0,
            );
            self.gl.draw_buffer(glow::COLOR_ATTACHMENT0);
            self.gl.read_buffer(glow::COLOR_ATTACHMENT0);
            // Validation forces a driver-side framebuffer completeness query. Blur uses
            // many short-lived attachments per frame, so check in debug builds only;
            // attachment format, extent and usage were validated above in all builds.
            if cfg!(debug_assertions)
                && self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE
            {
                return Err(Error::Backend("OpenGL pass framebuffer incomplete".into()));
            }
            self.gl
                .viewport(0, 0, size.width() as i32, size.height() as i32);
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.depth_mask(true);
            if format.is_srgb() {
                self.gl.enable(glow::FRAMEBUFFER_SRGB);
            } else {
                self.gl.disable(glow::FRAMEBUFFER_SRGB);
            }
            if let LoadOp::Clear(color) = load {
                self.gl
                    .clear_color(color.red, color.green, color.blue, color.alpha);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
            }
            if let Some(attachment) = depth {
                if let LoadOp::Clear(value) = attachment.depth_load_op {
                    self.gl.clear_depth_f32(value);
                    self.gl.clear(glow::DEPTH_BUFFER_BIT);
                }
            }
        }
        let result = (|| {
            let mut state = GlPassState::default();
            for step in steps.iter() {
                self.draw_step(step, size, format, depth_view.is_some(), &mut state)?;
            }
            // A blur frame executes many short offscreen passes. glGetError after each
            // pass forces a driver query in the hottest loop. Keep per-pass diagnostics
            // in debug builds; release builds check accumulated GL errors once during
            // the final swapchain present, after the blit.
            if cfg!(debug_assertions) {
                self.check()
            } else {
                Ok(())
            }
        })();
        // SAFETY: detach transient targets before they are sampled, resized or destroyed.
        unsafe {
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                None,
                0,
            );
            if matches!(target, RenderTarget::TextureView(_)) {
                self.gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    None,
                    0,
                );
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        result
    }
    fn draw_step(
        &self,
        step: RenderStepRef<'_>,
        size: Extent2d,
        format: Format,
        depth: bool,
        state: &mut GlPassState,
    ) -> Result<()> {
        let pipeline = self.pipelines.get(step.pipeline())?;
        if pipeline.desc.color_format != format || (pipeline.desc.depth_state.is_some() && !depth) {
            return Err(Error::InvalidInput(
                "OpenGL pipeline attachments mismatch".into(),
            ));
        }
        let layout = pipeline
            .desc
            .pipeline_layout
            .map(|id| self.pipeline_layouts.get(id))
            .transpose()?;
        let expected = layout.map_or(&[][..], |layout| layout.resource_set_layouts.as_slice());
        if expected.len() != step.resource_sets().len() {
            return Err(Error::InvalidInput(
                "OpenGL set count differs from pipeline".into(),
            ));
        }
        let set = if let Some(id) = step.resource_sets().first() {
            let set = self.sets.get(*id)?;
            if expected[0] != set.desc.layout {
                return Err(Error::InvalidInput(
                    "OpenGL resource set layout mismatch".into(),
                ));
            }
            Some(set)
        } else {
            None
        };
        let scissor = step.scissor().unwrap_or(ScissorRect {
            x: 0,
            y: 0,
            width: size.width(),
            height: size.height(),
        });
        let x = scissor.x.min(size.width());
        let y = scissor.y.min(size.height());
        // SAFETY: the graphics owner holds the current context throughout this pass.
        // Reuse program/VAO/depth/blend state between successive draws of one pipeline.
        // Resource bindings still run for every draw (they may include GPU mirror copies).
        unsafe {
            let pipeline_key = step.pipeline().raw();
            if state.pipeline.is_none() {
                self.gl.bind_vertex_array(Some(self.vao));
                self.gl.disable(glow::CULL_FACE);
                self.gl.enable(glow::SCISSOR_TEST);
            }
            if state.pipeline != Some(pipeline_key) {
                self.gl.use_program(Some(pipeline.native));
                if let Some(depth) = pipeline.desc.depth_state {
                    self.gl.enable(glow::DEPTH_TEST);
                    self.gl.depth_func(comparison(depth.compare));
                    self.gl.depth_mask(depth.write_enabled);
                } else {
                    self.gl.disable(glow::DEPTH_TEST);
                    self.gl.depth_mask(false);
                }
                if pipeline.desc.blend_mode == BlendMode::Replace {
                    self.gl.disable(glow::BLEND);
                } else {
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_equation(glow::FUNC_ADD);
                    let (source, destination) = match pipeline.desc.blend_mode {
                        BlendMode::Alpha => (glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA),
                        #[cfg(windows)]
                        BlendMode::SubpixelDualSource => (glow::SRC1_COLOR, glow::ONE_MINUS_SRC1_COLOR),
                        _ => (glow::ONE, glow::ONE_MINUS_SRC_ALPHA),
                    };
                    self.gl.blend_func_separate(
                        source,
                        destination,
                        glow::ONE,
                        if pipeline.desc.blend_mode == BlendMode::AdditiveAlpha {
                            glow::ONE
                        } else {
                            glow::ONE_MINUS_SRC_ALPHA
                        },
                    );
                }
                state.pipeline = Some(pipeline_key);
            }
            let clamped_scissor = (
                x as i32,
                y as i32,
                scissor.width.min(size.width() - x) as i32,
                scissor.height.min(size.height() - y) as i32,
            );
            if state.scissor != Some(clamped_scissor) {
                self.gl.scissor(
                    clamped_scissor.0,
                    clamped_scissor.1,
                    clamped_scissor.2,
                    clamped_scissor.3,
                );
                state.scissor = Some(clamped_scissor);
            }
            self.bind_resources(pipeline, set)?;
            let topology = if pipeline.desc.primitive_topology == PrimitiveTopology::TriangleList {
                glow::TRIANGLES
            } else {
                glow::TRIANGLE_STRIP
            };
            match step {
                RenderStepRef::Draw(draw) => {
                    self.gl
                        .uniform_1_u32(pipeline.first_instance.as_ref(), draw.first_instance);
                    self.gl.draw_arrays_instanced(
                        topology,
                        i32::try_from(draw.first_vertex).map_err(native)?,
                        i32::try_from(draw.vertex_count).map_err(native)?,
                        i32::try_from(draw.instance_count).map_err(native)?,
                    );
                }
                RenderStepRef::DrawIndexed(draw) => {
                    let buffer = self.buffers.get(draw.index_buffer.buffer)?;
                    let (bytes, kind) = if draw.index_buffer.format == IndexFormat::Uint16 {
                        (2u64, glow::UNSIGNED_SHORT)
                    } else {
                        (4, glow::UNSIGNED_INT)
                    };
                    let offset = draw
                        .index_buffer
                        .offset
                        .checked_add(u64::from(draw.first_index) * bytes)
                        .ok_or_else(|| {
                            Error::InvalidInput("OpenGL index offset overflow".into())
                        })?;
                    if !buffer.desc.usage.contains(BufferUsage::INDEX)
                        || offset % bytes != 0
                        || offset
                            .checked_add(u64::from(draw.index_count) * bytes)
                            .is_none_or(|end| end > buffer.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "OpenGL index range/usage mismatch".into(),
                        ));
                    }
                    self.gl
                        .uniform_1_u32(pipeline.first_instance.as_ref(), draw.first_instance);
                    self.gl
                        .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.native));
                    self.gl.draw_elements_instanced_base_vertex(
                        topology,
                        i32::try_from(draw.index_count).map_err(native)?,
                        kind,
                        i32::try_from(offset).map_err(native)?,
                        i32::try_from(draw.instance_count).map_err(native)?,
                        draw.base_vertex,
                    );
                }
            }
        }
        Ok(())
    }
}
fn comparison(value: CompareFunction) -> u32 {
    match value {
        CompareFunction::Never => glow::NEVER,
        CompareFunction::Less => glow::LESS,
        CompareFunction::Equal => glow::EQUAL,
        CompareFunction::LessEqual => glow::LEQUAL,
        CompareFunction::Greater => glow::GREATER,
        CompareFunction::NotEqual => glow::NOTEQUAL,
        CompareFunction::GreaterEqual => glow::GEQUAL,
        CompareFunction::Always => glow::ALWAYS,
    }
}
