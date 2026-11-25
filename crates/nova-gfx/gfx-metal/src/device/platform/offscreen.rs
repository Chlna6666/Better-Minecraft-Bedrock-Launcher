//! Ordered native offscreen rendering. One batch owns one command buffer;
//! same-queue attachment transitions and resource retention are Metal-managed.
use super::*;
use gfx_core::{Extent2d, ScissorRect, SignedOrigin2d};

struct PreparedPass {
    colors: Vec<Retained<ProtocolObject<dyn MTLTexture>>>,
    resolve: Option<(Retained<ProtocolObject<dyn MTLTexture>>, bool)>,
    color_load: LoadOp<ClearColor>,
    depth: Option<(Retained<ProtocolObject<dyn MTLTexture>>, LoadOp<f32>)>,
    extent: Extent2d,
    viewport: MTLViewport,
    steps: Vec<PreparedMetalRenderStep>,
}

impl MetalDevice {
    fn prepare_texture_pass(&self, pass: &RenderPassCommands<'_>, counts: &[gfx_core::IndirectCountBinding],
        plan: Option<&gfx_core::DrawRunPlan>) -> Result<PreparedPass> {
        let layout = self.render_passes.get(pass.render_pass)?;
        if pass.color_attachments.is_empty() || pass.color_attachments.len() != layout.color_formats.len() {
            return Err(Error::InvalidInput("Metal offscreen attachment count mismatch".into()));
        }
        let mut colors = Vec::with_capacity(pass.color_attachments.len());
        let mut color_ids = Vec::with_capacity(pass.color_attachments.len());
        let mut extent = None;
        for (index, (&target, &format)) in pass.color_attachments.iter().zip(&layout.color_formats).enumerate() {
            let view = self.texture_views.get(target)?;
            let texture = self.textures.get(view.texture)?;
            let size = Extent2d::new(
                u32::try_from(view.resource.width()).map_err(|_| Error::InvalidInput("Metal texture width overflow".into()))?,
                u32::try_from(view.resource.height()).map_err(|_| Error::InvalidInput("Metal texture height overflow".into()))?,
            )?;
            if !texture.desc.usage.contains(TextureUsage::COLOR_ATTACHMENT)
                || view.format != format || view.resource.mipmapLevelCount() != 1
                || texture.desc.sample_count != layout.sample_count
                || extent.is_some_and(|previous| previous != size) || color_ids.contains(&view.texture)
            {
                return Err(Error::InvalidInput("invalid Metal offscreen color view".into()));
            }
            if texture.desc.usage.contains(TextureUsage::TRANSIENT_ATTACHMENT)
                && (index != 0 || pass.resolve_target.is_none()
                    || !matches!(pass.color_load_op, LoadOp::Clear(_)) || pass.clear_region.is_some())
            {
                return Err(Error::InvalidInput("Metal transient MSAA requires a full clear and first-attachment resolve".into()));
            }
            extent = Some(size);
            color_ids.push(view.texture);
            colors.push(view.resource.clone());
        }
        let extent = extent.expect("validated nonempty attachments");
        let resolve = if let Some(target) = pass.resolve_target {
            let view = self.texture_views.get(target)?;
            let texture = self.textures.get(view.texture)?;
            if layout.sample_count <= 1 || texture.desc.sample_count != 1
                || view.format != layout.color_formats[0] || color_ids.contains(&view.texture)
                || !texture.desc.usage.contains(TextureUsage::COLOR_ATTACHMENT)
                || view.resource.mipmapLevelCount() != 1
                || view.resource.width() != extent.width() as usize
                || view.resource.height() != extent.height() as usize
                || matches!(view.format, Format::R8Uint | Format::Rg8Uint | Format::Rgba8Uint)
            {
                return Err(Error::InvalidInput("invalid Metal resolve destination".into()));
            }
            let source = self.textures.get(color_ids[0])?;
            let discard = source.desc.usage.contains(TextureUsage::DISCARD_AFTER_RESOLVE)
                && matches!(pass.color_load_op, LoadOp::Clear(_)) && pass.clear_region.is_none();
            Some((view.texture, view.resource.clone(), discard))
        } else { None };
        let empty_clear = match (pass.viewport, pass.clear_region) {
            (Some(viewport), Some(region)) => viewport.map_scissor(region, extent)?.is_empty(),
            (None, Some(region)) => region.is_empty(),
            _ => false,
        };
        let color_load = if empty_clear { LoadOp::Load } else { pass.color_load_op };
        let depth_view = match (layout.depth_format, pass.depth_attachment) {
            (None, None) => None,
            (Some(format), Some(attachment)) => {
                if attachment.stencil_load_op.is_some() {
                    return Err(Error::Unavailable("Metal offscreen stencil is not implemented".into()));
                }
                let view = self.texture_views.get(attachment.target)?;
                let texture = self.textures.get(view.texture)?;
                if view.format != format || !texture.desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT)
                    || texture.desc.sample_count != layout.sample_count
                    || view.resource.width() != extent.width() as usize
                    || view.resource.height() != extent.height() as usize
                    || view.resource.mipmapLevelCount() != 1
                {
                    return Err(Error::InvalidInput("invalid Metal offscreen depth view".into()));
                }
                let load = if empty_clear { LoadOp::Load } else { attachment.depth_load_op };
                if let LoadOp::Clear(value) = load {
                    if !(0.0..=1.0).contains(&value) {
                        return Err(Error::InvalidInput("Metal clear depth must be in 0..=1".into()));
                    }
                }
                Some((view.texture, view.resource.clone(), load))
            }
            _ => return Err(Error::InvalidInput("Metal offscreen depth attachment mismatch".into())),
        };
        let (origin, viewport_extent, scissor_extent) = match pass.viewport {
            Some(viewport) => (
                viewport.mapped_origin(extent)?, viewport.for_target(extent)?,
                viewport.mapped_scissor_extent(extent)?,
            ),
            None => (SignedOrigin2d::ZERO, extent, extent),
        };
        let viewport = MTLViewport {
            originX: f64::from(origin.x), originY: f64::from(origin.y),
            width: f64::from(viewport_extent.width()), height: f64::from(viewport_extent.height()),
            znear: 0.0, zfar: 1.0,
        };
        const MIN_FIXED_RUN: u32 = 32;
        let compact = plan.is_some_and(|plan| plan.is_compressed());
        let capacity = plan.map_or(pass.commands.len(), |plan| if compact { pass.commands.len() } else {
            plan.runs().iter().map(|&length| if length >= MIN_FIXED_RUN { 1 } else { length as usize }).sum()
        });
        let mut steps = Vec::with_capacity(capacity);
        let mut preparation = RenderStateCache::default();
        let mut counts = counts.iter().peekable();
        let mut checked_bindings = std::collections::HashSet::new();
        let mut runs = plan.into_iter().flat_map(|plan| plan.runs().iter().copied());
        let mut remaining = 0u32;
        let mut index = 0;
        while index < pass.commands.len() {
            if remaining == 0 { remaining = runs.next().unwrap_or(1); }
            let command = &pass.commands[index];
            let mut step = self.prepare_render_step(layout, command, &mut preparation)?;
            let mut consumed = 1;
            if counts.peek().is_some_and(|count| count.command_index == index) {
                let count = *counts.next().expect("counted command");
                step.draw = PreparedMetalDraw::Counted(self.prepare_counted_run(command, count, &step.pipeline)?);
            } else if remaining >= MIN_FIXED_RUN || (compact && remaining > 1) {
                step.draw = PreparedMetalDraw::Counted(self.prepare_fixed_run(command, remaining, &step.pipeline)?);
                consumed = remaining;
            }
            index += if compact { 1 } else { consumed as usize };
            remaining -= consumed;
            if checked_bindings.insert(std::rc::Rc::as_ptr(&step.bindings))
                && (color_ids.iter().any(|&id| step.bindings.samples_texture(id))
                    || resolve.as_ref().is_some_and(|(id, _, _)| step.bindings.samples_texture(*id))
                    || depth_view.as_ref().is_some_and(|(id, _, _)| step.bindings.samples_texture(*id)))
            {
                return Err(Error::InvalidInput("Metal offscreen pass samples its own attachment".into()));
            }
            let scissor = match (pass.viewport, step.scissor) {
                (Some(viewport), Some(rect)) => viewport.map_scissor(rect, extent)?,
                (_, Some(rect)) => rect,
                (_, None) => ScissorRect { x: 0, y: 0, width: scissor_extent.width(), height: scissor_extent.height() },
            };
            let right = scissor.x.saturating_add(scissor.width).min(scissor_extent.width());
            let bottom = scissor.y.saturating_add(scissor.height).min(scissor_extent.height());
            step.scissor = Some(ScissorRect {
                x: scissor.x, y: scissor.y,
                width: right.saturating_sub(scissor.x), height: bottom.saturating_sub(scissor.y),
            });
            steps.push(step);
        }
        if counts.next().is_some() { return Err(Error::InvalidInput("Metal count bindings must be ordered and reference a draw".into())); }
        Ok(PreparedPass {
            colors,
            resolve: resolve.map(|(_, texture, discard)| (texture, discard)),
            color_load,
            depth: depth_view.map(|(_, resource, load)| (resource, load)),
            extent, viewport, steps,
        })
    }

    pub(super) fn submit_texture_passes(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tracked: bool,
    ) -> Result<Option<SubmissionId>> {
        self.submit_texture_passes_timed(passes, None, tracked)
    }

    pub(super) fn submit_texture_passes_timed(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tags: Option<&[Option<gfx_core::GpuTimestampTag>]>,
        tracked: bool,
    ) -> Result<Option<SubmissionId>> {
        self.submit_texture_passes_counted(passes, tags, &[], tracked)
    }

    pub(super) fn submit_texture_passes_counted(
        &mut self, passes: &[RenderPassCommands<'_>],
        tags: Option<&[Option<gfx_core::GpuTimestampTag>]>,
        counts: &[&[gfx_core::IndirectCountBinding]], tracked: bool,
    ) -> Result<Option<SubmissionId>> {
        self.submit_texture_passes_planned(passes, tags, counts, tracked, &[])
    }

    pub(super) fn submit_texture_passes_planned(
        &mut self, passes: &[RenderPassCommands<'_>],
        tags: Option<&[Option<gfx_core::GpuTimestampTag>]>,
        counts: &[&[gfx_core::IndirectCountBinding]], tracked: bool,
        plans: &[std::sync::Arc<gfx_core::DrawRunPlan>],
    ) -> Result<Option<SubmissionId>> {
        if !plans.is_empty() && (plans.len() != passes.len() || !counts.is_empty()) {
            return Err(Error::InvalidInput("Metal fixed run metadata mismatch".into()));
        }
        if !counts.is_empty() && counts.len() != passes.len() {
            return Err(Error::InvalidInput("Metal counted pass metadata length mismatch".into()));
        }
        if tags.is_some_and(|tags| tags.len() != passes.len()) {
            return Err(Error::InvalidInput("Metal render tag count mismatch".into()));
        }
        if passes.is_empty() { return Ok(None); }
        if let Some(icb) = &self.icb { icb.reclaim(); }
        // Validate the complete batch before submitting anything. A rejected
        // later pass must never leave a partially committed producer graph.
        let prepared = passes.iter().enumerate().map(|(index, pass)| self.prepare_texture_pass(
            pass, counts.get(index).copied().unwrap_or(&[]), plans.get(index).map(std::sync::Arc::as_ref)))
            .collect::<Result<Vec<_>>>()?;
        let ranges = tags.map(|tags| tags.iter().enumerate().filter_map(|(index, tag)| {
            tag.map(|tag| timestamps::TaggedRange { first: index, last: index, tag })
        }).collect::<Vec<_>>()).unwrap_or_default();
        let has_counted = |pass: &PreparedPass| pass.steps.iter().any(|step| matches!(&step.draw, PreparedMetalDraw::Counted(_)));
        let probe = self.work_timestamps.acquire_render_with_compute(&self.device, &ranges, prepared.iter().any(has_counted));
        let command_buffer = objc2::rc::autoreleasepool(|_| -> Result<_> {
            let command_buffer = self.command_queue.commandBuffer()
                .ok_or_else(|| Error::Backend("Metal offscreen command buffer unavailable".into()))?;
            for (pass_index, pass) in prepared.iter().enumerate() {
                // The GPU conversion executes before the render encoder.
                // All temporary native objects remain referenced by this
                // same ordered command buffer until its execution completes.
                let counted = has_counted(pass);
                if counted {
                    // One reset encoder and one compute encoder per pass,
                    // independent of the number of native draw runs.
                    let blit = command_buffer.blitCommandEncoder().ok_or_else(|| Error::Backend("Metal ICB reset encoder unavailable".into()))?;
                    for step in &pass.steps {
                        if let PreparedMetalDraw::Counted(run) = &step.draw { run.reset(&blit); }
                    }
                    blit.endEncoding();
                    let compute = if let Some(probe) = &probe {
                        probe.compute_begin_encoder(&command_buffer, pass_index)
                    } else { command_buffer.computeCommandEncoder() }
                        .ok_or_else(|| Error::Backend("Metal ICB compute encoder unavailable".into()))?;
                    if let Some(probe) = &probe { probe.sample_dispatch(&compute, pass_index, true); }
                    for step in &pass.steps {
                        if let PreparedMetalDraw::Counted(run) = &step.draw { run.encode(&compute); }
                    }
                    compute.endEncoding();
                }
                let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
                for (index, texture) in pass.colors.iter().enumerate() {
                // SAFETY: all attachments were validated before encoding.
                let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(index) };
                color.setTexture(Some(texture));
                if let Some((resolve, discard)) = pass.resolve.as_ref().filter(|_| index == 0) {
                    color.setResolveTexture(Some(resolve));
                    color.setStoreAction(if *discard { MTLStoreAction::MultisampleResolve } else { MTLStoreAction::StoreAndMultisampleResolve });
                } else {
                    color.setStoreAction(MTLStoreAction::Store);
                }
                match pass.color_load {
                    LoadOp::Load => color.setLoadAction(MTLLoadAction::Load),
                    LoadOp::Clear(clear) => {
                        color.setLoadAction(MTLLoadAction::Clear);
                        color.setClearColor(MTLClearColor {
                            red: f64::from(clear.red), green: f64::from(clear.green),
                            blue: f64::from(clear.blue), alpha: f64::from(clear.alpha),
                        });
                    }
                }
                }
                if let Some((texture, load)) = &pass.depth {
                    let depth = descriptor.depthAttachment();
                    depth.setTexture(Some(texture));
                    depth.setStoreAction(MTLStoreAction::Store);
                    match load {
                        LoadOp::Load => depth.setLoadAction(MTLLoadAction::Load),
                        LoadOp::Clear(value) => {
                            depth.setLoadAction(MTLLoadAction::Clear);
                            depth.setClearDepth(f64::from(*value));
                        }
                    }
                }
                if let Some(probe) = &probe { probe.configure_render_from(&descriptor, pass_index, !counted); }
                let encoder = command_buffer.renderCommandEncoderWithDescriptor(&descriptor)
                    .ok_or_else(|| Error::Backend("Metal offscreen render encoder unavailable".into()))?;
                if !counted && let Some(probe) = &probe { probe.sample_render(&encoder, pass_index, true); }
                encode_draw_steps(&encoder, &pass.steps, pass.extent, pass.viewport);
                if let Some(probe) = &probe { probe.sample_render(&encoder, pass_index, false); }
                encoder.endEncoding();
            }
            // Default command buffers retain references to all native resources
            // and Metal orders successive encoders on the primary graphics queue.
            if let Some(probe) = &probe { probe.resolve(&command_buffer)?; }
            command_buffer.commit();
            Ok(command_buffer)
        })?;
        if let Some(icb) = &self.icb {
            let storage = prepared.iter().flat_map(|pass| pass.steps.iter()).filter_map(|step| match &step.draw {
                PreparedMetalDraw::Counted(run) => Some(run.storage()),
                _ => None,
            }).collect();
            icb.submitted(command_buffer.clone(), storage);
        }
        if let Some(probe) = probe {
            self.work_timestamps.submitted(probe, command_buffer.clone());
        }
        Ok(if tracked {
            Some(self.submissions.insert(MetalSubmission::new(command_buffer, false)))
        } else {
            None
        })
    }
}
