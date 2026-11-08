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
        steps: &[RenderCommand],
        load: LoadOp<ClearColor>,
        depth: Option<DepthAttachment>,
    ) -> Result<()> {
        self.render_target_clipped(
            std::slice::from_ref(&target),
            None,
            pass,
            steps,
            load,
            depth,
            None,
            None,
        )
    }

    pub(crate) fn render_target_clipped(
        &self,
        targets: &[RenderTarget],
        resolve_view: Option<TextureViewId>,
        pass: RenderPassId,
        steps: &[RenderCommand],
        load: LoadOp<ClearColor>,
        depth: Option<DepthAttachment>,
        clear_region: Option<ScissorRect>,
        viewport: Option<Viewport>,
    ) -> Result<()> {
        self.park()?;
        let pass = self.passes.get(pass)?;
        if targets.is_empty() || targets.len() != pass.color_attachments.len() {
            return Err(Error::InvalidInput(
                "OpenGL target count does not match the render pass attachments".into(),
            ));
        }
        let mut textures = Vec::with_capacity(targets.len());
        // Multisampled attachments must be attached with their multisample target.
        let mut attach_targets = Vec::with_capacity(targets.len());
        let mut sizes = Vec::with_capacity(targets.len());
        let mut formats = Vec::with_capacity(targets.len());
        let mut samples = Vec::with_capacity(targets.len());
        let mut framebuffer = self.framebuffer;
        let mut viewport_size = None;
        let mut viewport_origin = SignedOrigin2d::ZERO;
        let mut scissor_extent = None;
        for target in targets {
            match target {
                RenderTarget::TextureView(id) => {
                    let view = self.views.get(*id)?;
                    if !view.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
                        return Err(Error::InvalidInput(
                            "OpenGL target is not a color attachment".into(),
                        ));
                    }
                    viewport_size = Some(
                        viewport
                            .map(|viewport| viewport.for_target(view.size))
                            .transpose()?
                            .unwrap_or(view.size),
                    );
                    viewport_origin = viewport
                        .map(|viewport| viewport.mapped_origin(view.size))
                        .transpose()?
                        .unwrap_or(SignedOrigin2d::ZERO);
                    scissor_extent = Some(
                        viewport
                            .map(|viewport| viewport.mapped_scissor_extent(view.size))
                            .transpose()?
                            .unwrap_or(view.size),
                    );
                    textures.push(view.native);
                    attach_targets.push(view.target);
                    sizes.push(view.size);
                    samples.push(view.sample_count);
                    formats.push(view.format);
                }
                RenderTarget::Swapchain {
                    swapchain,
                    image_index,
                } => {
                    if *image_index != 0 {
                        return Err(Error::InvalidInput(
                            "OpenGL presents logical image zero".into(),
                        ));
                    }
                    let chain = self.swapchains.get(*swapchain)?;
                    framebuffer = chain.framebuffer;
                    viewport_size = Some(chain.source_extent);
                    viewport_origin = SignedOrigin2d::ZERO;
                    scissor_extent = Some(chain.source_extent);
                    textures.push(chain.color);
                    attach_targets.push(glow::TEXTURE_2D);
                    sizes.push(chain.config.size);
                    samples.push(1);
                    formats.push(chain.config.format);
                }
            };
        }
        if pass
            .color_attachments
            .iter()
            .zip(formats.iter())
            .any(|(attachment, format)| attachment.format != *format)
        {
            return Err(Error::InvalidInput(
                "OpenGL render pass color mismatch".into(),
            ));
        }
        let size = sizes[0];
        let viewport_size = viewport_size.unwrap_or(size);
        let scissor_extent = scissor_extent.unwrap_or(size);
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
        // Shared invariant: attachments match the pass sample count and a resolve
        // destination is single-sampled.
        gfx_core::validate_pass_attachments(
            pass.sample_count,
            samples.into_iter(),
            depth_view.map(|view| view.sample_count),
            resolve_view
                .map(|id| self.views.get(id).map(|view| view.sample_count))
                .transpose()?,
        )?;
        let clear_region = match (viewport, clear_region) {
            (Some(viewport), Some(region)) => Some(viewport.map_scissor(region, size)?),
            (_, clear_region) => clear_region,
        };
        // ScissorRect is expressed from the top-left. OpenGL consumes a bottom-left
        // framebuffer origin; PixelExact maps that source rectangle into the tile first.
        let clear_scissor = clear_region.and_then(|scissor| {
            let x = scissor.x.min(size.width());
            let y = scissor.y.min(size.height());
            let width = scissor.width.min(size.width() - x);
            let height = scissor.height.min(size.height() - y);
            if width == 0 || height == 0 {
                return None;
            }
            let gl_y = if viewport
                .is_some_and(|viewport| viewport.mapping == ViewportMapping::PixelExact)
            {
                size.height().saturating_sub(y.saturating_add(height))
            } else {
                y
            };
            Some((
                i32::try_from(x).ok()?,
                i32::try_from(gl_y).ok()?,
                i32::try_from(width).ok()?,
                i32::try_from(height).ok()?,
            ))
        });
        let viewport_x = viewport_origin.x;
        let viewport_y = i32::try_from(
            i64::from(size.height())
                - (i64::from(viewport_origin.y) + i64::from(viewport_size.height())),
        )
        .map_err(|error| {
            Error::InvalidInput(format!("OpenGL viewport origin overflow: {error}"))
        })?;
        let viewport_width = i32::try_from(viewport_size.width()).map_err(|error| {
            Error::InvalidInput(format!("OpenGL viewport width overflow: {error}"))
        })?;
        let viewport_height = i32::try_from(viewport_size.height()).map_err(|error| {
            Error::InvalidInput(format!("OpenGL viewport height overflow: {error}"))
        })?;
        // SAFETY: live texture views are attached to one owner FBO. Scissor/depth writes
        // are reset before clears so previous draw state cannot limit this pass.
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            let draw_buffers = textures
                .iter()
                .enumerate()
                .map(|(index, _)| glow::COLOR_ATTACHMENT0 + index as u32)
                .collect::<Vec<_>>();
            for (index, texture) in textures.iter().enumerate() {
                self.gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0 + index as u32,
                    attach_targets[index],
                    Some(*texture),
                    0,
                );
            }
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                depth_view.map_or(glow::TEXTURE_2D, |view| view.target),
                depth_view.map(|view| view.native),
                0,
            );
            self.gl.draw_buffers(&draw_buffers);
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
                .viewport(viewport_x, viewport_y, viewport_width, viewport_height);
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.depth_mask(true);
            if formats[0].is_srgb() {
                self.gl.enable(glow::FRAMEBUFFER_SRGB);
            } else {
                self.gl.disable(glow::FRAMEBUFFER_SRGB);
            }
            if let Some((x, y, width, height)) = clear_scissor {
                self.gl.enable(glow::SCISSOR_TEST);
                self.gl.scissor(x, y, width, height);
            }
            if let LoadOp::Clear(color) = load
                && (clear_region.is_none() || clear_scissor.is_some())
            {
                self.gl
                    .clear_color(color.red, color.green, color.blue, color.alpha);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
            }
            if let Some(attachment) = depth {
                if let LoadOp::Clear(value) = attachment.depth_load_op
                    && (clear_region.is_none() || clear_scissor.is_some())
                {
                    self.gl.clear_depth_f32(value);
                    self.gl.clear(glow::DEPTH_BUFFER_BIT);
                }
            }
            // Draw-step scissor setup starts from a known state on every pass.
            self.gl.disable(glow::SCISSOR_TEST);
        }
        let result = (|| {
            let mut state = GlPassState::default();
            let mut bindings = self.binding_cache.borrow_mut();
            bindings.reset();
            for step in steps.iter() {
                let scissor = match (viewport, step.scissor()) {
                    (Some(viewport), Some(scissor)) => Some(viewport.map_scissor(scissor, size)?),
                    (_, scissor) => scissor,
                };
                if scissor.is_some_and(ScissorRect::is_empty) {
                    continue;
                }
                self.draw_step(
                    step,
                    scissor,
                    size,
                    viewport_size,
                    scissor_extent,
                    viewport
                        .is_some_and(|viewport| viewport.mapping == ViewportMapping::PixelExact),
                    &formats,
                    depth_view.is_some(),
                    &mut state,
                    &mut bindings,
                )?;
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
        if let Some(resolve_view) = resolve_view {
            let msaa_color = textures.first().copied().ok_or_else(|| {
                Error::InvalidInput("OpenGL resolve needs an MSAA color source".into())
            })?;
            let resolve = self.views.get(resolve_view)?;
            // SAFETY: A transient resolve FBO is created, bound as DRAW, and deleted
            // after the glBlitFramebuffer blits the MSAA draw FBO into it.
            unsafe {
                let resolve_fbo = self.gl.create_framebuffer().map_err(native)?;
                self.gl
                    .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(resolve_fbo));
                self.gl.framebuffer_texture_2d(
                    glow::DRAW_FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(resolve.native),
                    0,
                );
                self.gl
                    .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
                self.gl.blit_framebuffer(
                    0,
                    0,
                    size.width() as i32,
                    size.height() as i32,
                    0,
                    0,
                    size.width() as i32,
                    size.height() as i32,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
                self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                self.gl.delete_framebuffer(resolve_fbo);
                let _ = msaa_color;
            }
        }
        // SAFETY: detach transient targets before they are sampled, resized or destroyed.
        unsafe {
            // The resolve blit unbinds both framebuffer targets; re-attach to the pass
            // framebuffer before detaching, since the default one cannot be modified.
            if resolve_view.is_some() {
                self.gl
                    .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            }
            // Detach with the target the attachment was created from: some drivers
            // reject a mismatched textarget even when the texture name is zero.
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                depth_view.map_or(glow::TEXTURE_2D, |view| view.target),
                None,
                0,
            );
            for (index, texture_target) in targets.iter().enumerate() {
                if matches!(texture_target, RenderTarget::TextureView(_)) {
                    self.gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0 + index as u32,
                        attach_targets[index],
                        None,
                        0,
                    );
                }
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        result
    }
    fn draw_step(
        &self,
        step: &RenderCommand,
        step_scissor: Option<ScissorRect>,
        target_size: Extent2d,
        viewport_size: Extent2d,
        scissor_extent: Extent2d,
        pixel_exact: bool,
        formats: &[Format],
        depth: bool,
        state: &mut GlPassState,
        bindings: &mut crate::bindings::Cache,
    ) -> Result<()> {
        let pipeline = self.pipelines.get(step.pipeline())?;
        let formats_match = pipeline
            .desc
            .color_targets
            .iter()
            .map(|target| target.format)
            .collect::<Vec<_>>()
            .as_slice()
            == formats;
        if !formats_match || (pipeline.desc.depth_state.is_some() && !depth) {
            return Err(Error::InvalidInput(
                "OpenGL pipeline attachments mismatch".into(),
            ));
        }
        let layout = pipeline
            .desc
            .pipeline_layout
            .map(|id| self.pipeline_layouts.get(id))
            .transpose()?;
        let expected = layout.map_or(&[][..], |layout| layout.bind_group_layouts.as_slice());
        if expected.len() != step.bind_groups().len() {
            return Err(Error::InvalidInput(
                "OpenGL set count differs from pipeline".into(),
            ));
        }
        let set = if let Some(id) = step.bind_groups().first() {
            let set = self.sets.get(*id)?;
            if expected[0] != set.desc.layout {
                return Err(Error::InvalidInput(
                    "OpenGL bind group layout mismatch".into(),
                ));
            }
            Some(set)
        } else {
            None
        };
        let scissor = step_scissor.unwrap_or(ScissorRect {
            x: 0,
            y: 0,
            width: scissor_extent.width(),
            height: scissor_extent.height(),
        });
        let x = scissor.x.min(scissor_extent.width());
        let y = scissor.y.min(scissor_extent.height());
        let width = scissor.width.min(scissor_extent.width() - x);
        let height = scissor.height.min(scissor_extent.height() - y);
        // SAFETY: the graphics owner holds the current context throughout this pass.
        // Reuse program/VAO/depth/blend state between successive draws of one pipeline.
        // Mirror copies still run for every draw; unchanged native bindings can be skipped.
        unsafe {
            let pipeline_key = step.pipeline().raw();
            if state.pipeline.is_none() {
                self.gl.bind_vertex_array(Some(self.vao));
                self.gl.enable(glow::SCISSOR_TEST);
            }
            if state.pipeline != Some(pipeline_key) {
                self.gl.use_program(Some(pipeline.native));
                self.gl.front_face(match pipeline.desc.raster.front_face {
                    FrontFace::Ccw => glow::CCW,
                    FrontFace::Cw => glow::CW,
                });
                if let Some(face) = pipeline.desc.raster.cull_mode {
                    self.gl.enable(glow::CULL_FACE);
                    self.gl.cull_face(match face {
                        Face::Front => glow::FRONT,
                        Face::Back => glow::BACK,
                    });
                } else {
                    self.gl.disable(glow::CULL_FACE);
                }
                if let Some(depth) = pipeline.desc.depth_state {
                    self.gl.enable(glow::DEPTH_TEST);
                    self.gl.depth_func(comparison(depth.compare));
                    self.gl.depth_mask(depth.write_enabled);
                    if let Some(stencil) = depth.stencil {
                        self.gl.enable(glow::STENCIL_TEST);
                        self.gl.stencil_func_separate(
                            glow::FRONT,
                            comparison(stencil.front.compare),
                            0,
                            stencil.read_mask,
                        );
                        self.gl.stencil_func_separate(
                            glow::BACK,
                            comparison(stencil.back.compare),
                            0,
                            stencil.read_mask,
                        );
                        self.gl.stencil_op_separate(
                            glow::FRONT,
                            stencil_op_to_gl(stencil.front.fail_op),
                            stencil_op_to_gl(stencil.front.depth_fail_op),
                            stencil_op_to_gl(stencil.front.pass_op),
                        );
                        self.gl.stencil_op_separate(
                            glow::BACK,
                            stencil_op_to_gl(stencil.back.fail_op),
                            stencil_op_to_gl(stencil.back.depth_fail_op),
                            stencil_op_to_gl(stencil.back.pass_op),
                        );
                        self.gl.stencil_mask(stencil.write_mask);
                    } else {
                        self.gl.disable(glow::STENCIL_TEST);
                    }
                } else {
                    self.gl.disable(glow::DEPTH_TEST);
                    self.gl.depth_mask(false);
                    self.gl.disable(glow::STENCIL_TEST);
                }
                for (index, target) in pipeline.desc.color_targets.iter().enumerate() {
                    let blend_mode = target.blend;
                    if blend_mode == BlendMode::Replace {
                        self.gl.disable_draw_buffer(glow::BLEND, index as u32);
                    } else {
                        self.gl.enable_draw_buffer(glow::BLEND, index as u32);
                    }
                    self.gl.blend_equation_separate_draw_buffer(
                        index as u32,
                        glow::FUNC_ADD,
                        glow::FUNC_ADD,
                    );
                    let (source, destination) = match blend_mode {
                        BlendMode::Replace => (glow::ONE, glow::ZERO),
                        BlendMode::Alpha => (glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA),
                        #[cfg(windows)]
                        BlendMode::SubpixelDualSource => {
                            (glow::SRC1_COLOR, glow::ONE_MINUS_SRC1_COLOR)
                        }
                        _ => (glow::ONE, glow::ONE_MINUS_SRC_ALPHA),
                    };
                    self.gl.blend_func_separate_draw_buffer(
                        index as u32,
                        source,
                        destination,
                        glow::ONE,
                        if blend_mode == BlendMode::AdditiveAlpha {
                            glow::ONE
                        } else {
                            glow::ONE_MINUS_SRC_ALPHA
                        },
                    );
                    let mask = target.write_mask;
                    self.gl.color_mask_draw_buffer(
                        index as u32,
                        mask.contains(ColorWrites::RED),
                        mask.contains(ColorWrites::GREEN),
                        mask.contains(ColorWrites::BLUE),
                        mask.contains(ColorWrites::ALPHA),
                    );
                }
                state.pipeline = Some(pipeline_key);
            }
            let gl_y = if pixel_exact {
                target_size
                    .height()
                    .saturating_sub(y.saturating_add(height))
            } else {
                target_size.height() - viewport_size.height() + y
            };
            let clamped_scissor = (x as i32, gl_y as i32, width as i32, height as i32);
            if state.scissor != Some(clamped_scissor) {
                self.gl.scissor(
                    clamped_scissor.0,
                    clamped_scissor.1,
                    clamped_scissor.2,
                    clamped_scissor.3,
                );
                state.scissor = Some(clamped_scissor);
            }
            self.bind_resources(pipeline, set, bindings)?;
            let topology = if pipeline.desc.primitive_topology == PrimitiveTopology::TriangleList {
                glow::TRIANGLES
            } else {
                glow::TRIANGLE_STRIP
            };
            match step {
                RenderCommand::DrawIndirect(draw) => {
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "OpenGL indirect draws require ARB_shader_draw_parameters".into(),
                        ));
                    }
                    let buffer = self.buffers.get(draw.arguments)?;
                    if !buffer.desc.usage.contains(BufferUsage::INDIRECT)
                        || draw.offset % 4 != 0
                        || draw
                            .offset
                            .checked_add(16)
                            .is_none_or(|end| end > buffer.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid OpenGL indirect argument range or usage".into(),
                        ));
                    }
                    self.gl
                        .bind_buffer(glow::DRAW_INDIRECT_BUFFER, Some(buffer.native));
                    self.gl.draw_arrays_indirect_offset(
                        topology,
                        i32::try_from(draw.offset).map_err(native)?,
                    );
                    self.gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER, None);
                }
                RenderCommand::DrawIndexedIndirect(draw) => {
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "OpenGL indirect draws require ARB_shader_draw_parameters".into(),
                        ));
                    }
                    let arguments = self.buffers.get(draw.arguments)?;
                    if !arguments.desc.usage.contains(BufferUsage::INDIRECT)
                        || draw.offset % 4 != 0
                        || draw
                            .offset
                            .checked_add(20)
                            .is_none_or(|end| end > arguments.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid OpenGL indexed indirect argument range or usage".into(),
                        ));
                    }
                    let index = self.buffers.get(draw.index_buffer.buffer)?;
                    let (bytes, kind) = if draw.index_buffer.format == IndexFormat::Uint16 {
                        (2u64, glow::UNSIGNED_SHORT)
                    } else {
                        (4, glow::UNSIGNED_INT)
                    };
                    if !index.desc.usage.contains(BufferUsage::INDEX)
                        || draw.index_buffer.offset % bytes != 0
                        || draw
                            .index_buffer
                            .offset
                            .checked_add(bytes)
                            .is_none_or(|end| end > index.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "OpenGL index range/usage mismatch".into(),
                        ));
                    }
                    self.gl
                        .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index.native));
                    self.gl
                        .bind_buffer(glow::DRAW_INDIRECT_BUFFER, Some(arguments.native));
                    self.gl.draw_elements_indirect_offset(
                        topology,
                        kind,
                        i32::try_from(draw.offset).map_err(native)?,
                    );
                    self.gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER, None);
                }
                RenderCommand::Draw(draw) => {
                    self.gl
                        .uniform_1_u32(pipeline.first_instance.as_ref(), draw.first_instance);
                    if self.compute.capabilities.indirect_draw {
                        self.gl.draw_arrays_instanced_base_instance(
                            topology,
                            i32::try_from(draw.first_vertex).map_err(native)?,
                            i32::try_from(draw.vertex_count).map_err(native)?,
                            i32::try_from(draw.instance_count).map_err(native)?,
                            draw.first_instance,
                        );
                    } else {
                        self.gl.draw_arrays_instanced(
                            topology,
                            i32::try_from(draw.first_vertex).map_err(native)?,
                            i32::try_from(draw.vertex_count).map_err(native)?,
                            i32::try_from(draw.instance_count).map_err(native)?,
                        );
                    }
                }
                RenderCommand::DrawIndexed(draw) => {
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
                    if self.compute.capabilities.indirect_draw {
                        self.gl.draw_elements_instanced_base_vertex_base_instance(
                            topology,
                            i32::try_from(draw.index_count).map_err(native)?,
                            kind,
                            i32::try_from(offset).map_err(native)?,
                            i32::try_from(draw.instance_count).map_err(native)?,
                            draw.base_vertex,
                            draw.first_instance,
                        );
                    } else {
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
        }
        Ok(())
    }
}
pub(crate) fn comparison(value: CompareFunction) -> u32 {
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

pub(crate) fn stencil_op_to_gl(op: StencilOp) -> u32 {
    match op {
        StencilOp::Keep => glow::KEEP,
        StencilOp::Zero => glow::ZERO,
        StencilOp::Replace => glow::REPLACE,
        StencilOp::IncrementClamp => glow::INCR,
        StencilOp::DecrementClamp => glow::DECR,
        StencilOp::Invert => glow::INVERT,
        StencilOp::IncrementWrap => glow::INCR_WRAP,
        StencilOp::DecrementWrap => glow::DECR_WRAP,
    }
}
