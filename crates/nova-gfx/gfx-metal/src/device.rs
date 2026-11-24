//! Metal backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Metal on Apple
//! targets. Non-Apple builds expose a minimal stub that returns
//! `Error::Unavailable`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

#![cfg_attr(
    target_vendor = "apple",
    expect(
        unsafe_code,
        reason = "Metal Objective-C bindings require unsafe calls; each unsafe block documents its safety invariant"
    )
)]
#![cfg_attr(
    target_vendor = "apple",
    expect(
        dead_code,
        reason = "phase-one backend stores resource metadata before every upload path is wired"
    )
)]
#![cfg_attr(
    not(target_vendor = "apple"),
    expect(
        unused_imports,
        reason = "non-Apple targets compile only the stub while sharing the public type list"
    )
)]

use crate::error::MetalError;
use gfx_core::{
    AddressMode, Backend, BackendKind, BeginRenderPassDescriptor, BindGroupDescriptor, BindGroupId,
    BindGroupLayoutDescriptor, BindGroupLayoutId, BindingResource, BlendMode, BufferDescriptor,
    BufferId, BufferUsage, ClearColor, CommandDevice, CommandEncoderDescriptor, CommandEncoderId,
    CompareFunction, DepthAttachment, DeviceDescriptor, DiagnosticsDevice, DrawCommand, DrawDescriptor, Error,
    FilterMode, Format, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation, PipelineDevice,
    PipelineLayoutDescriptor, PipelineLayoutId, PresentFrameDescriptor, PresentationDevice,
    PresentedFrame, PrimitiveTopology, RenderCommand, RenderPassCommands, RenderPassDescriptor,
    RenderPassId, RenderPipelineDescriptor, RenderPipelineId, RenderTarget, ResourceDevice,
    ResourceStats, Result, SamplerDescriptor, SamplerId, ShaderCode, ShaderModuleDescriptor,
    ShaderModuleId, ShaderStage, SubmissionDevice, SubmissionId, SubmissionStatus,
    SurfaceCapabilities, SurfaceConfig, SurfaceDescriptor, SurfaceDevice, SurfaceId, SwapchainId,
    TextureDescriptor, TextureDimension, TextureId, TextureUsage, TextureViewDescriptor,
    TextureViewId, TextureWriteDescriptor, ThreadingMode,
};

#[cfg(target_vendor = "apple")]
mod platform {
    mod buffer_upload;
    mod relocation;
    mod texture;
    mod bindings;
    mod offscreen;
    use bindings::{NativeBindGroup, PreparedBindings};
    use super::*;
    use crate::registry::ResourceRegistry;
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use dispatch2::DispatchData;
    use gfx_core::{FrameGpuTimestamps, PresentTimings};
    use std::time::Instant;
    use objc2::{ClassType, rc::Retained, runtime::ProtocolObject};
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::{NSError, NSInteger, NSObjectProtocol, NSString};
    use objc2_metal::{MTLCompareFunction, MTLDepthStencilDescriptor, MTLDepthStencilState, MTLSamplerState};
    use objc2_metal::{
        MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
        MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue, MTLCompileOptions, MTLDevice, MTLDrawable, MTLFunction,
        MTLIndexType, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
        MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
        MTLRenderPipelineState, MTLResourceOptions, MTLScissorRect, MTLStoreAction, MTLTexture,
        MTLViewport,
    };
    use objc2_quartz_core::{CALayer, CAMetalDrawable, CAMetalLayer};
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};

    /// Native presentation target accepted by the Metal backend.
    pub trait MetalSurfaceTarget: HasDisplayHandle + HasWindowHandle {}

    impl<T> MetalSurfaceTarget for T where T: HasDisplayHandle + HasWindowHandle + ?Sized {}

    /// Generic Metal device and resource owner.
    pub struct MetalDevice {
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
        buffers: ResourceRegistry<MetalBuffer>,
        textures: ResourceRegistry<MetalTexture>,
        texture_views: ResourceRegistry<MetalTextureView>,
        samplers: ResourceRegistry<MetalSampler>,
        bind_group_layouts: ResourceRegistry<MetalResourceSetLayout>,
        bind_groups: ResourceRegistry<MetalResourceSet>,
        pipeline_layouts: ResourceRegistry<MetalPipelineLayout>,
        shader_modules: ResourceRegistry<MetalShaderModule>,
        render_passes: ResourceRegistry<MetalRenderPass>,
        render_pipelines: ResourceRegistry<MetalRenderPipeline>,
        command_encoders: ResourceRegistry<MetalCommandEncoder>,
        surfaces: ResourceRegistry<MetalSurface>,
        swapchains: ResourceRegistry<MetalSwapchain>,
        submissions: ResourceRegistry<MetalSubmission>,
        last_frame_gpu_timestamps: Option<FrameGpuTimestamps>,
        submitted_frames: u64,
    }

    impl MetalDevice {
        /// Creates a Metal device.
        ///
        /// # Errors
        ///
        /// Returns [`Error`] if Metal initialization fails.
        pub fn new(_desc: &DeviceDescriptor) -> Result<Self> {
            let device = objc2_metal::MTLCreateSystemDefaultDevice()
                .ok_or_else(|| Error::Unavailable("no compatible Metal device".to_string()))?;
            let command_queue = device.newCommandQueue().ok_or_else(|| {
                Error::Unavailable("failed to create Metal command queue".to_string())
            })?;
            Ok(Self {
                device,
                command_queue,
                buffers: ResourceRegistry::new("buffer"),
                textures: ResourceRegistry::new("texture"),
                texture_views: ResourceRegistry::new("texture view"),
                samplers: ResourceRegistry::new("sampler"),
                bind_group_layouts: ResourceRegistry::new("bind group layout"),
                bind_groups: ResourceRegistry::new("bind group"),
                pipeline_layouts: ResourceRegistry::new("pipeline layout"),
                shader_modules: ResourceRegistry::new("shader module"),
                render_passes: ResourceRegistry::new("render pass"),
                render_pipelines: ResourceRegistry::new("render pipeline"),
                command_encoders: ResourceRegistry::new("command encoder"),
                surfaces: ResourceRegistry::new("surface"),
                swapchains: ResourceRegistry::new("swapchain"),
                submissions: ResourceRegistry::new("submission"),
                last_frame_gpu_timestamps: None,
                submitted_frames: 0,
            })
        }

        /// Creates a native Metal surface from raw-window-handle traits.
        ///
        /// # Errors
        ///
        /// Returns [`Error`] when native handles are invalid.
        fn create_surface<W>(&mut self, window: &W, desc: &SurfaceDescriptor) -> Result<SurfaceId>
        where
            W: HasDisplayHandle + HasWindowHandle + ?Sized,
        {
            let _display = window
                .display_handle()
                .map_err(|error| Error::Backend(error.to_string()))?;
            let window = window
                .window_handle()
                .map_err(|error| Error::Backend(error.to_string()))?;
            let ns_view = match window.as_raw() {
                RawWindowHandle::AppKit(handle) => handle.ns_view,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "Metal surface requires RawWindowHandle::AppKit, got {other:?}"
                    )));
                }
            };
            Ok(self.surfaces.insert(MetalSurface {
                label: desc.label.clone(),
                ns_view,
            }))
        }

        /// Configures a surface swapchain.
        pub fn configure_surface(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SurfaceId> {
            let _ = self.create_swapchain(surface, config)?;
            Ok(surface)
        }

        /// Creates or replaces a swapchain for a surface.
        fn create_swapchain(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            let surface_record = self.surfaces.get(surface)?;
            let layer = create_or_replace_metal_layer(&self.device, surface_record, config)?;
            Ok(self.swapchains.insert(MetalSwapchain {
                surface,
                config,
                layer,
                last_submission: None,
            }))
        }

        /// Recreates an existing swapchain.
        pub fn resize_swapchain(
            &mut self,
            swapchain: SwapchainId,
            width: u32,
            height: u32,
        ) -> Result<()> {
            if width == 0 || height == 0 {
                return Ok(());
            }
            let swapchain = self.swapchains.get_mut(swapchain)?;
            swapchain.config.size = gfx_core::Extent2d::new(width, height)?;
            swapchain
                .layer
                .setDrawableSize(drawable_size(swapchain.config));
            Ok(())
        }

        /// Creates a buffer record.
        fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
            desc.validate()?;
            let length = usize::try_from(desc.size)
                .map_err(|error| Error::InvalidInput(format!("buffer size overflow: {error}")))?;
            let resource = self
                .device
                .newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| Error::Backend("failed to create Metal buffer".to_string()))?;
            Ok(self.buffers.insert(MetalBuffer {
                desc: desc.clone(),
                resource: Some(resource),
                data: if desc.memory_location == MemoryLocation::CpuToGpu {
                    Some(vec![0; length])
                } else {
                    None
                },
            }))
        }

        /// Writes data into a CPU-visible buffer record.
        fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
            let buffer = self.buffers.get_mut(buffer)?;
            let storage = buffer.data.as_mut().ok_or_else(|| {
                Error::Unavailable(
                    "Metal GPU-only staging upload is not enabled in this build".to_string(),
                )
            })?;
            let offset = usize::try_from(offset)
                .map_err(|error| Error::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| Error::InvalidInput("buffer write range overflow".to_string()))?;
            let target = storage.get_mut(offset..end).ok_or_else(|| {
                Error::InvalidInput("buffer write range is out of bounds".to_string())
            })?;
            target.copy_from_slice(data);
            if let Some(resource) = &buffer.resource {
                // SAFETY: The Metal buffer was allocated with shared storage and is at least
                // `buffer.desc.size` bytes; the checked range above is within that allocation.
                unsafe {
                    let destination = resource.contents().as_ptr().cast::<u8>().add(offset);
                    std::ptr::copy_nonoverlapping(data.as_ptr(), destination, data.len());
                }
            }
            Ok(())
        }

        fn create_bind_group_layout(
            &mut self,
            desc: &BindGroupLayoutDescriptor,
        ) -> Result<BindGroupLayoutId> {
            desc.validate()?;
            for entry in &desc.entries {
                if gfx_core::metal_binding_slot(0, entry.binding, entry.binding_type).is_none() {
                    return Err(Error::Unavailable(format!("unsupported Metal layout slot {}", entry.binding)));
                }
            }
            Ok(self
                .bind_group_layouts
                .insert(MetalResourceSetLayout { desc: desc.clone() }))
        }

        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDescriptor,
        ) -> Result<PipelineLayoutId> {
            desc.validate()?;
            if desc.bind_group_layouts.len() > 1 {
                return Err(Error::Unavailable("Metal currently supports one descriptor group".into()));
            }
            for layout in &desc.bind_group_layouts {
                let _ = self.bind_group_layouts.get(*layout)?;
            }
            Ok(self.pipeline_layouts.insert(MetalPipelineLayout {
                bind_group_layouts: desc.bind_group_layouts.clone(),
            }))
        }

        fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
            let layout = self.bind_group_layouts.get(desc.layout)?.desc.clone();
            desc.validate_against(&layout)?;
            let native = self.create_native_bind_group(desc, &layout)?;
            Ok(self.bind_groups.insert(MetalResourceSet {
                layout: desc.layout,
                native: std::rc::Rc::new(native),
            }))
        }

        /// Creates and validates a shader module.
        fn create_shader_module(
            &mut self,
            desc: &ShaderModuleDescriptor,
        ) -> Result<ShaderModuleId> {
            desc.validate()?;
            let library = match &desc.binary.code {
                ShaderCode::MetallibStatic(bytes) => {
                    let data = DispatchData::from_static_bytes(*bytes);
                    self.device
                        .newLibraryWithData_error(&data)
                        .map_err(|error| Error::Shader(nserror_message(&error)))?
                }
                ShaderCode::Msl(source) => {
                    let source_string = NSString::from_str(source);
                    let options = MTLCompileOptions::new();
                    self.device
                        .newLibraryWithSource_options_error(&source_string, Some(&options))
                        .map_err(|error| Error::Shader(nserror_message(&error)))?
                }
                ShaderCode::Glsl(_)
                | ShaderCode::Hlsl(_)
                | ShaderCode::DxBytecode(_)
                | ShaderCode::DxBytecodeStatic(_)
                | ShaderCode::Spirv(_) => {
                    return Err(Error::Shader(
                        "Metal shader module requires precompiled metallib or MSL source"
                            .to_string(),
                    ));
                }
            };
            Ok(self.shader_modules.insert(MetalShaderModule {
                stage: desc.binary.stage,
                entry_point: desc.binary.entry_point.clone(),
                bindings: desc.binary.metal.clone(),
                library,
            }))
        }

        /// Creates a render pass record.
        fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
            desc.validate()?;
            if desc.color_attachments.len() != 1 {
                return Err(Error::Unavailable(
                    "Metal MRT is not implemented; use exactly one color attachment".into(),
                ));
            }
            if desc.sample_count > 1
                || desc
                    .depth_attachment
                    .as_ref()
                    .is_some_and(|attachment| attachment.stencil_format.is_some())
            {
                return Err(Error::Unavailable(
                    "Metal MSAA/stencil is not implemented".into(),
                ));
            }
            Ok(self.render_passes.insert(MetalRenderPass {
                depth_format: desc.depth_attachment.map(|attachment| attachment.format),
                color_formats: desc
                    .color_attachments
                    .iter()
                    .map(|attachment| attachment.format)
                    .collect(),
            }))
        }

        /// Creates a graphics render pipeline record.
        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDescriptor,
            _viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            desc.validate()?;
            let vertex_shader = self.shader_modules.get(desc.vertex_shader)?;
            let fragment_shader = self.shader_modules.get(desc.fragment_shader)?;
            if vertex_shader.stage != ShaderStage::Vertex {
                return Err(Error::InvalidInput(
                    "vertex shader module must use ShaderStage::Vertex".to_string(),
                ));
            }
            if fragment_shader.stage != ShaderStage::Fragment {
                return Err(Error::InvalidInput(
                    "fragment shader module must use ShaderStage::Fragment".to_string(),
                ));
            }
            if !desc.vertex_buffers.is_empty() {
                return Err(Error::Unavailable("Metal vertex-buffer layouts are not supported; use shader storage vertex pulling".into()));
            }
            let group_layout = match desc.pipeline_layout {
                Some(id) => self.pipeline_layouts.get(id)?.bind_group_layouts.first().copied(),
                None => None,
            };
            self.validate_shader_bindings(group_layout, vertex_shader.bindings.as_ref(), gfx_core::ShaderStages::VERTEX)?;
            self.validate_shader_bindings(group_layout, fragment_shader.bindings.as_ref(), gfx_core::ShaderStages::FRAGMENT)?;
            let render_pass = self.render_passes.get(desc.render_pass)?;
            let target = desc.color_targets.get(0).ok_or_else(|| {
                Error::InvalidInput("Metal pipeline requires one color target".to_string())
            })?;
            if render_pass.color_formats.get(0) != Some(&target.format) {
                return Err(Error::InvalidInput(
                    "pipeline color target must match render pass color attachment".to_string(),
                ));
            }
            if desc.color_targets.len() != 1 {
                return Err(Error::Unavailable("Metal MRT pipelines are not implemented".into()));
            }
            if desc.depth_state.is_some() && render_pass.depth_format.is_none() {
                return Err(Error::InvalidInput("Metal depth state requires a depth attachment".into()));
            }
            if desc.depth_state.is_some_and(|state| state.stencil.is_some()) {
                return Err(Error::Unavailable("Metal stencil state is not implemented".into()));
            }
            let depth_format = render_pass.depth_format;
            let depth_state = if depth_format.is_some() {
                let native = MTLDepthStencilDescriptor::new();
                let compare = desc.depth_state.map_or(CompareFunction::Always, |state| state.compare);
                native.setDepthCompareFunction(compare_to_metal(compare));
                native.setDepthWriteEnabled(desc.depth_state.is_some_and(|state| state.write_enabled));
                Some(self.device.newDepthStencilStateWithDescriptor(&native)
                    .ok_or_else(|| Error::Backend("failed to create Metal depth state".into()))?)
            } else {
                None
            };
            let pipeline_state = create_pipeline_state(
                &self.device,
                desc,
                vertex_shader,
                fragment_shader,
                target.format,
                depth_format,
            )?;
            Ok(self.render_pipelines.insert(MetalRenderPipeline {
                depth_format,
                depth_state,
                color_formats: desc
                    .color_targets
                    .iter()
                    .map(|target| target.format)
                    .collect(),
                blend_modes: desc
                    .color_targets
                    .iter()
                    .map(|target| target.blend)
                    .collect(),
                primitive_topology: desc.primitive_topology,
                raster: desc.raster,
                pipeline_state,
                pipeline_layout: desc.pipeline_layout,
                group_layout,
                vertex_bindings: vertex_shader.bindings.clone(),
                fragment_bindings: fragment_shader.bindings.clone(),
            }))
        }

        fn create_command_encoder(
            &mut self,
            _desc: &CommandEncoderDescriptor,
        ) -> Result<CommandEncoderId> {
            Ok(self
                .command_encoders
                .insert(MetalCommandEncoder { in_flight: false }))
        }

        fn record_draw_desc(
            &mut self,
            encoder: CommandEncoderId,
            draw: &DrawDescriptor,
        ) -> Result<()> {
            let _encoder = self.command_encoders.get(encoder)?;
            let RenderTarget::Swapchain { swapchain, .. } = draw.pass.target else {
                return Err(Error::Unavailable(
                    "Metal offscreen render target is not implemented yet".to_string(),
                ));
            };
            let _swapchain = self.swapchains.get(swapchain)?;
            let _render_pass = self.render_passes.get(draw.pass.render_pass)?;
            let _pipeline = self.render_pipelines.get(draw.pipeline)?;
            for bind_group in &draw.bind_groups {
                let _ = self.bind_groups.get(*bind_group)?;
            }
            Ok(())
        }

        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            let _encoder = self.command_encoders.get(encoder)?;
            Err(Error::Unavailable("Metal generic encoder recording is not implemented".into()))
        }

        fn draw_internal(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderCommand],
            clear_color: gfx_core::ClearColor,
            depth_attachment: Option<DepthAttachment>,
        ) -> Result<Option<PresentedFrame>> {
            if !self.poll_present_readiness(swapchain)? {
                return Ok(None);
            }
            let started = Instant::now();
            let swapchain_id = swapchain;
            let swapchain = self.swapchains.get(swapchain_id)?;
            let render_pass = self.render_passes.get(render_pass)?;
            if render_pass.color_formats.as_slice() != [swapchain.config.format] {
                return Err(Error::InvalidInput("Metal swapchain and render pass formats differ".into()));
            }
            let depth = match (render_pass.depth_format, depth_attachment) {
                (None, None) => None,
                (Some(format), Some(attachment)) => {
                    if attachment.stencil_load_op.is_some() {
                        return Err(Error::Unavailable("Metal stencil attachments are not implemented".into()));
                    }
                    let view = self.texture_views.get(attachment.target)?;
                    let texture = self.textures.get(view.texture)?;
                    if view.format != format
                        || !texture.desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT)
                        || view.resource.width() != swapchain.config.size.width() as usize
                        || view.resource.height() != swapchain.config.size.height() as usize
                    {
                        return Err(Error::InvalidInput("Metal depth view does not match render target".into()));
                    }
                    if let LoadOp::Clear(value) = attachment.depth_load_op {
                        if !(0.0..=1.0).contains(&value) {
                            return Err(Error::InvalidInput("Metal clear depth must be in 0..=1".into()));
                        }
                    }
                    Some((view.resource.clone(), attachment.depth_load_op))
                }
                _ => return Err(Error::InvalidInput("Metal render pass depth attachment mismatch".into())),
            };
            let render_steps = steps
                .iter()
                .map(|step| self.prepare_render_step(render_pass, step))
                .collect::<Result<Vec<_>>>()?;

            let frame = objc2::rc::autoreleasepool(|_| -> Result<Option<(Retained<ProtocolObject<dyn MTLCommandBuffer>>, PresentTimings)>> {
                let acquire_started = Instant::now();
                let Some(drawable) = swapchain.layer.nextDrawable() else {
                    return Ok(None);
                };
                let image_acquire = acquire_started.elapsed();
                let texture = drawable.texture();
                let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
                // SAFETY: color attachment index 0 is valid for the descriptor's color array.
                let color_attachment =
                    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                color_attachment.setTexture(Some(&texture));
                color_attachment.setLoadAction(MTLLoadAction::Clear);
                color_attachment.setStoreAction(MTLStoreAction::Store);
                color_attachment.setClearColor(MTLClearColor {
                    red: f64::from(clear_color.red),
                    green: f64::from(clear_color.green),
                    blue: f64::from(clear_color.blue),
                    alpha: f64::from(clear_color.alpha),
                });
                if let Some((resource, load_op)) = &depth {
                    let attachment = descriptor.depthAttachment();
                    attachment.setTexture(Some(resource));
                    attachment.setStoreAction(MTLStoreAction::Store);
                    match load_op {
                        LoadOp::Load => attachment.setLoadAction(MTLLoadAction::Load),
                        LoadOp::Clear(value) => {
                            attachment.setLoadAction(MTLLoadAction::Clear);
                            attachment.setClearDepth(f64::from(*value));
                        }
                    }
                }

                let command_buffer = self.command_queue.commandBuffer().ok_or_else(|| {
                    Error::Backend("failed to create Metal command buffer".to_string())
                })?;
                let encoder = command_buffer
                    .renderCommandEncoderWithDescriptor(&descriptor)
                    .ok_or_else(|| {
                        Error::Backend("failed to create Metal render encoder".to_string())
                    })?;
                encode_draw_steps(&encoder, &render_steps, swapchain.config.size, full_viewport(swapchain.config.size));
                encoder.endEncoding();
                let record_elapsed = started.elapsed().saturating_sub(image_acquire);
                let present_started = Instant::now();
                command_buffer.presentDrawable(drawable.as_ref());
                let queue_present = present_started.elapsed();
                let submit_started = Instant::now();
                command_buffer.commit();
                Ok(Some((command_buffer, PresentTimings {
                    image_acquire,
                    command_record: record_elapsed,
                    queue_present,
                    queue_submit: submit_started.elapsed(),
                    ..PresentTimings::default()
                })))
            })?;
            let Some((command_buffer, timings)) = frame else {
                return Ok(None);
            };
            let submission = self.submissions.insert(MetalSubmission { command_buffer, presented: true });
            self.swapchains.get_mut(swapchain_id)?.last_submission = Some(submission);
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            Ok(Some(PresentedFrame { submission: Some(submission), timings: Some(timings) }))
        }

        fn prepare_render_step(
            &self,
            render_pass: &MetalRenderPass,
            step: &RenderCommand,
        ) -> Result<PreparedMetalRenderStep> {
            let pipeline = self.render_pipelines.get(step.pipeline())?.clone();
            if render_pass.color_formats != pipeline.color_formats
                || render_pass.depth_format != pipeline.depth_format
            {
                return Err(Error::InvalidInput(
                    "render pass and pipeline color targets do not match".to_string(),
                ));
            }
            let bindings = self.prepare_bindings(&pipeline, step.bind_groups())?;
            let draw = match step {
                RenderCommand::DrawIndirect(_) => {
                    return Err(Error::Unavailable(
                        "indirect draws are not implemented by this backend".into(),
                    ));
                }
                RenderCommand::Draw(step) => PreparedMetalDraw::NonIndexed {
                    vertex_start: step.first_vertex,
                    vertex_count: step.vertex_count,
                    instance_count: step.instance_count,
                    base_instance: step.first_instance,
                },
                RenderCommand::DrawIndexed(step) => {
                    let buffer = self.buffers.get(step.index_buffer.buffer)?;
                    validate_index_buffer_range(
                        buffer.desc.usage,
                        buffer.desc.size,
                        step.index_buffer,
                        step.first_index,
                        step.index_count,
                    )?;
                    let resource = buffer.resource.clone().ok_or_else(|| {
                        Error::Backend("Metal index buffer has no native resource".to_string())
                    })?;
                    let first_index_offset = u64::from(step.first_index)
                        .checked_mul(index_format_size(step.index_buffer.format))
                        .ok_or_else(|| {
                            Error::InvalidInput(
                                "Metal index buffer first index offset overflow".to_string(),
                            )
                        })?;
                    let index_buffer_offset = step
                        .index_buffer
                        .offset
                        .checked_add(first_index_offset)
                        .ok_or_else(|| {
                            Error::InvalidInput(
                                "Metal index buffer byte offset overflow".to_string(),
                            )
                        })?;
                    PreparedMetalDraw::Indexed {
                        index_buffer: resource,
                        index_buffer_offset,
                        index_type: index_format_to_metal(step.index_buffer.format),
                        index_count: step.index_count,
                        base_vertex: step.base_vertex,
                        instance_count: step.instance_count,
                        base_instance: step.first_instance,
                    }
                }
            };
            Ok(PreparedMetalRenderStep {
                pipeline,
                bindings,
                draw,
                scissor: step.scissor(),
            })
        }

        fn present(&mut self, swapchain: SwapchainId, _image_index: u32) -> Result<()> {
            let swapchain = self.swapchains.get(swapchain)?;
            objc2::rc::autoreleasepool(|_| -> Result<()> {
                let drawable = swapchain.layer.nextDrawable().ok_or_else(|| {
                    Error::Backend("CAMetalLayer did not provide a drawable".to_string())
                })?;
                let command_buffer = self.command_queue.commandBuffer().ok_or_else(|| {
                    Error::Backend("failed to create Metal command buffer".to_string())
                })?;
                command_buffer.presentDrawable(drawable.as_ref());
                command_buffer.commit();
                Ok(())
            })?;
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            Ok(())
        }

        fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
            let _buffer = self.buffers.take(buffer)?;
            Ok(())
        }

        fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
            let _texture = self.textures.take(texture)?;
            Ok(())
        }

        fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
            let _view = self.texture_views.take(view)?;
            Ok(())
        }

        fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
            let _sampler = self.samplers.take(sampler)?;
            Ok(())
        }

        fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()> {
            let _layout = self.bind_group_layouts.take(layout)?;
            Ok(())
        }

        fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()> {
            let _resource_set = self.bind_groups.take(bind_group)?;
            Ok(())
        }

        fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
            let _layout = self.pipeline_layouts.take(layout)?;
            Ok(())
        }

        fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
            let _shader = self.shader_modules.take(shader)?;
            Ok(())
        }

        fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
            let _render_pass = self.render_passes.take(render_pass)?;
            Ok(())
        }

        fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
            let _pipeline = self.render_pipelines.take(pipeline)?;
            Ok(())
        }

        fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
            let _encoder = self.command_encoders.take(encoder)?;
            Ok(())
        }

        fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
            let _swapchain = self.swapchains.take(swapchain)?;
            Ok(())
        }

        fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
            let _surface = self.surfaces.take(surface)?;
            Ok(())
        }

        pub fn poll_cleanup(&mut self) {}

        #[must_use]
        fn resource_stats(&self) -> ResourceStats {
            let allocated_bytes = self
                .buffers
                .values()
                .filter(|buffer| buffer.resource.is_some())
                .map(|buffer| buffer.desc.size)
                .sum::<u64>()
                .saturating_add(
                    self.textures
                        .values()
                        .filter(|texture| texture.resource.is_some())
                        .map(|texture| texture.desc.byte_size())
                        .sum::<u64>(),
                );
            ResourceStats {
                memory_accounting: gfx_core::MemoryAccounting::ResourceSizes,
                cpu_shadow_bytes: self
                    .buffers
                    .values()
                    .filter_map(|buffer| buffer.data.as_ref())
                    .map(|bytes| bytes.capacity() as u64)
                    .sum(),
                buffers: self.buffers.live_len(),
                textures: self.textures.live_len(),
                texture_views: self.texture_views.live_len(),
                samplers: self.samplers.live_len(),
                bind_group_layouts: self.bind_group_layouts.live_len(),
                bind_groups: self.bind_groups.live_len(),
                pipeline_layouts: self.pipeline_layouts.live_len(),
                shader_modules: self.shader_modules.live_len(),
                render_passes: self.render_passes.live_len(),
                render_pipelines: self.render_pipelines.live_len(),
                command_encoders: self.command_encoders.live_len(),
                submissions: self.submissions.live_len(),
                surfaces: self.surfaces.live_len(),
                swapchains: self.swapchains.live_len(),
                allocated_bytes,
                reserved_bytes: allocated_bytes,
                ..ResourceStats::default()
            }
        }
    }

    impl Backend for MetalDevice {
        const BACKEND_KIND: BackendKind = BackendKind::Metal;

        fn adapter_name(&self) -> &str {
            "Metal"
        }
    }

    impl SurfaceDevice for MetalDevice {
        type SurfaceTarget = dyn MetalSurfaceTarget;

        fn create_surface(
            &mut self,
            target: &Self::SurfaceTarget,
            desc: &SurfaceDescriptor,
        ) -> Result<SurfaceId> {
            Self::create_surface(self, target, desc)
        }

        fn create_swapchain(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            Self::create_swapchain(self, surface, config)
        }

        fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
            Self::destroy_swapchain(self, swapchain)
        }

        fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
            Self::destroy_surface(self, surface)
        }
    }

    impl ResourceDevice for MetalDevice {
        fn multisample_capabilities(&self) -> gfx_core::MultisampleCapabilities {
            // Metal MSAA is not implemented on this backend yet.
            gfx_core::MultisampleCapabilities {
                max_sample_count: 1,
                stencil_supported: false,
            }
        }

        fn copy_texture_batch(&mut self, copies: &[gfx_core::TextureCopy]) -> Result<()> {
            self.copy_texture_regions(copies)
        }

        fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
            Self::create_buffer(self, desc)
        }

        fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
            Self::write_buffer(self, buffer, offset, data)
        }

        fn write_buffer_batch<'a>(
            &mut self,
            writes: impl IntoIterator<Item = gfx_core::BufferWrite<'a>>,
        ) -> Result<gfx_core::BufferUploadStats> {
            self.upload_buffer_batch(writes)
        }

        fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
            Self::create_texture(self, desc)
        }

        fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
            Self::write_texture(self, desc, data)
        }

        fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
            Self::create_texture_view(self, desc)
        }

        fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
            Self::create_sampler(self, desc)
        }

        fn create_bind_group_layout(
            &mut self,
            desc: &BindGroupLayoutDescriptor,
        ) -> Result<BindGroupLayoutId> {
            Self::create_bind_group_layout(self, desc)
        }

        fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
            Self::create_bind_group(self, desc)
        }

        fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
            Self::destroy_buffer(self, buffer)
        }

        fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
            Self::destroy_texture(self, texture)
        }

        fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
            Self::destroy_texture_view(self, view)
        }

        fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
            Self::destroy_sampler(self, sampler)
        }

        fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()> {
            Self::destroy_bind_group_layout(self, layout)
        }

        fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()> {
            Self::destroy_bind_group(self, bind_group)
        }
    }

    impl PipelineDevice for MetalDevice {
        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDescriptor,
        ) -> Result<PipelineLayoutId> {
            Self::create_pipeline_layout(self, desc)
        }

        fn create_shader_module(
            &mut self,
            desc: &ShaderModuleDescriptor,
        ) -> Result<ShaderModuleId> {
            Self::create_shader_module(self, desc)
        }

        fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
            Self::create_render_pass(self, desc)
        }

        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDescriptor,
            viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            Self::create_render_pipeline(self, desc, viewport_extent)
        }

        fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
            Self::destroy_pipeline_layout(self, layout)
        }

        fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
            Self::destroy_shader_module(self, shader)
        }

        fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
            Self::destroy_render_pass(self, render_pass)
        }

        fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
            Self::destroy_render_pipeline(self, pipeline)
        }
    }

    impl CommandDevice for MetalDevice {
        fn create_command_encoder(
            &mut self,
            desc: &CommandEncoderDescriptor,
        ) -> Result<CommandEncoderId> {
            Self::create_command_encoder(self, desc)
        }

        fn record_draw_desc(
            &mut self,
            encoder: CommandEncoderId,
            draw: DrawDescriptor,
        ) -> Result<()> {
            Self::record_draw_desc(self, encoder, &draw)
        }

        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::submit(self, encoder)
        }

        fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::destroy_command_encoder(self, encoder)
        }
    }

    impl SubmissionDevice for MetalDevice {
        fn async_capabilities(&self) -> gfx_core::AsyncCapabilities {
            gfx_core::AsyncCapabilities {
                threading_mode: ThreadingMode::OwnerThreadOnly,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
            }
        }

        fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
            let _ = self.command_encoders.get(encoder)?;
            Err(Error::Unavailable("Metal generic encoder recording is not implemented".into()))
        }

        fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
            self.poll_submission_with_frame_timestamps(submission).map(|(status, _)| status)
        }

        fn poll_submission_with_frame_timestamps(&mut self, submission: SubmissionId)
            -> Result<(SubmissionStatus, Option<FrameGpuTimestamps>)> {
            let record = self.submissions.get(submission)?;
            let status = record.command_buffer.status();
            if status != MTLCommandBufferStatus::Completed && status != MTLCommandBufferStatus::Error {
                return Ok((SubmissionStatus::Pending, None));
            }
            let record = self.submissions.take(submission)?;
            if status == MTLCommandBufferStatus::Error {
                let error = record.command_buffer.error().as_deref().map(nserror_message)
                    .unwrap_or_else(|| "Metal command buffer failed".into());
                return Ok((SubmissionStatus::Failed(error), None));
            }
            let start = record.command_buffer.GPUStartTime();
            let end = record.command_buffer.GPUEndTime();
            let sample = if start.is_finite() && end.is_finite() && start > 0.0 && end > start {
                let ns = (end - start) * 1_000_000_000.0;
                (ns < u64::MAX as f64).then_some(FrameGpuTimestamps {
                    duration_ticks: ns as u64, frequency: 1_000_000_000,
                })
            } else { None };
            if record.presented && sample.is_some() {
                self.last_frame_gpu_timestamps = sample;
            }
            Ok((SubmissionStatus::Complete, sample))
        }

        fn request_submission_ready(
            &mut self,
            submission: SubmissionId,
            callback: Box<dyn FnOnce() + Send + 'static>,
        ) -> Result<bool> {
            let record = self.submissions.get(submission)?;
            if matches!(
                record.command_buffer.status(),
                MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
            ) {
                callback();
                return Ok(true);
            }
            // Metal invokes completion handlers on its own threads. Only the
            // Send callback crosses threads; no owner-thread Metal state does.
            let callback = std::sync::Mutex::new(Some(callback));
            let handler = block2::RcBlock::new(
                move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                    let wake = callback.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                    if let Some(wake) = wake {
                        // Never unwind through the Objective-C block ABI.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(wake));
                    }
                },
            );
            // SAFETY: Metal copies the block and calls it when the command buffer
            // completes (including failure). The mutex makes its FnOnce payload
            // safe to consume from a Metal callback thread.
            unsafe {
                record.command_buffer.addCompletedHandler(block2::RcBlock::as_ptr(&handler));
            }
            Ok(true)
        }

        fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
            self.submissions.get(submission)?.command_buffer.waitUntilCompleted();
            match self.poll_submission(submission)? {
                SubmissionStatus::Complete => Ok(()),
                SubmissionStatus::Pending => Err(Error::Backend("Metal command buffer did not complete".into())),
                SubmissionStatus::Failed(error) => Err(Error::Backend(error)),
            }
        }
    }

    impl PresentationDevice for MetalDevice {
        fn poll_present_readiness(&mut self, id: SwapchainId) -> Result<bool> {
            let chain = self.swapchains.get(id)?;
            let Some(submission) = chain.last_submission else { return Ok(true); };
            // GPU completion is not scanout completion. CAMetalLayer owns
            // drawable availability and native display sync.
            Ok(self.submissions.get(submission).map_or(true, |record| matches!(
                record.command_buffer.status(),
                MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
            )))
        }

        fn render_and_present(
            &mut self,
            frame: &PresentFrameDescriptor<'_>,
        ) -> Result<Option<PresentedFrame>> {
            let LoadOp::Clear(color) = frame.color_load_op else {
                return Err(Error::Unavailable("Metal drawable LoadOp::Load is not implemented".into()));
            };
            self.draw_internal(frame.swapchain, frame.render_pass, frame.commands, color, frame.depth_attachment)
        }

        fn render_to_texture(&mut self, pass: &RenderPassCommands<'_>) -> Result<()> {
            self.submit_texture_passes(std::slice::from_ref(pass), false).map(|_| ())
        }

        fn render_to_textures(&mut self, passes: &[RenderPassCommands<'_>]) -> Result<Option<SubmissionId>> {
            self.submit_texture_passes(passes, true)
        }
    }

    impl DiagnosticsDevice for MetalDevice {
        fn resource_stats(&self) -> ResourceStats {
            Self::resource_stats(self)
        }

        fn frame_gpu_timestamps_supported(&self) -> bool { true }

        fn last_frame_gpu_timestamps(&mut self) -> Result<Option<FrameGpuTimestamps>> {
            Ok(self.last_frame_gpu_timestamps)
        }

        fn memory_architecture(&self) -> Result<gfx_core::MemoryArchitecture> {
            Ok(if self.device.hasUnifiedMemory() {
                gfx_core::MemoryArchitecture::Unified
            } else {
                gfx_core::MemoryArchitecture::Discrete
            })
        }
    }

    #[derive(Clone)]
    struct MetalBuffer {
        desc: BufferDescriptor,
        resource: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        data: Option<Vec<u8>>,
    }

    #[derive(Clone)]
    struct MetalTexture {
        desc: TextureDescriptor,
        resource: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    }

    #[derive(Clone)]
    struct MetalTextureView {
        texture: TextureId,
        format: Format,
        resource: Retained<ProtocolObject<dyn MTLTexture>>,
    }

    #[derive(Clone)]
    struct MetalSampler {
        resource: Retained<ProtocolObject<dyn MTLSamplerState>>,
    }

    #[derive(Clone)]
    struct MetalResourceSetLayout {
        desc: BindGroupLayoutDescriptor,
    }

    #[derive(Clone)]
    struct MetalResourceSet {
        layout: BindGroupLayoutId,
        native: std::rc::Rc<NativeBindGroup>,
    }

    #[derive(Clone)]
    struct MetalPipelineLayout {
        bind_group_layouts: Vec<BindGroupLayoutId>,
    }

    #[derive(Clone)]
    struct MetalShaderModule {
        stage: ShaderStage,
        entry_point: String,
        bindings: Option<gfx_core::MetalShaderBindings>,
        library: Retained<ProtocolObject<dyn MTLLibrary>>,
    }

    #[derive(Clone)]
    struct MetalRenderPass {
        color_formats: Vec<Format>,
        depth_format: Option<Format>,
    }

    #[derive(Clone)]
    struct MetalRenderPipeline {
        depth_format: Option<Format>,
        depth_state: Option<Retained<ProtocolObject<dyn MTLDepthStencilState>>>,
        color_formats: Vec<Format>,
        blend_modes: Vec<BlendMode>,
        primitive_topology: PrimitiveTopology,
        raster: gfx_core::RasterState,
        pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
        pipeline_layout: Option<PipelineLayoutId>,
        group_layout: Option<BindGroupLayoutId>,
        vertex_bindings: Option<gfx_core::MetalShaderBindings>,
        fragment_bindings: Option<gfx_core::MetalShaderBindings>,
    }

    #[derive(Clone, Copy)]
    struct MetalCommandEncoder {
        in_flight: bool,
    }

    #[derive(Clone)]
    struct MetalSurface {
        label: Option<String>,
        ns_view: NonNull<c_void>,
    }

    #[derive(Clone)]
    struct MetalSwapchain {
        surface: SurfaceId,
        config: SurfaceConfig,
        layer: Retained<CAMetalLayer>,
        last_submission: Option<SubmissionId>,
    }

    struct MetalSubmission {
        command_buffer: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
        presented: bool,
    }

    fn create_or_replace_metal_layer(
        device: &ProtocolObject<dyn MTLDevice>,
        surface: &MetalSurface,
        config: SurfaceConfig,
    ) -> Result<Retained<CAMetalLayer>> {
        // SAFETY: raw-window-handle guarantees AppKitWindowHandle::ns_view is a live NSView
        // pointer for the borrowed handle lifetime. The backend stores it only for this surface.
        let ns_view = unsafe {
            Retained::retain(surface.ns_view.as_ptr().cast::<NSView>()).ok_or_else(|| {
                Error::InvalidInput("AppKit ns_view is not retainable".to_string())
            })?
        };
        // SAFETY: AppKit layer access is performed on the thread that owns the NSView.
        let existing_layer = unsafe { ns_view.layer() };
        let layer = if let Some(layer) = existing_layer {
            if layer.isKindOfClass(CAMetalLayer::class()) {
                // SAFETY: isKindOfClass verified that the retained CALayer is a CAMetalLayer.
                unsafe { Retained::cast_unchecked(layer) }
            } else {
                let metal_layer = CAMetalLayer::new();
                // SAFETY: Replacing the NSView backing layer with a live CAMetalLayer is the
                // documented AppKit path for layer-backed Metal rendering.
                unsafe {
                    ns_view.setLayer(Some(metal_layer.as_ref()));
                }
                ns_view.setWantsLayer(true);
                metal_layer
            }
        } else {
            let metal_layer = CAMetalLayer::new();
            // SAFETY: The view is retained above and remains alive while assigning its layer.
            unsafe {
                ns_view.setLayer(Some(metal_layer.as_ref()));
            }
            ns_view.setWantsLayer(true);
            metal_layer
        };
        configure_layer(device, &layer, config);
        Ok(layer)
    }

    fn configure_layer(
        device: &ProtocolObject<dyn MTLDevice>,
        layer: &CAMetalLayer,
        config: SurfaceConfig,
    ) {
        layer.setDevice(Some(device));
        layer.setPixelFormat(format_to_metal(config.format));
        layer.setFramebufferOnly(true);
        layer.setMaximumDrawableCount(2);
        layer.setDrawableSize(drawable_size(config));
        layer.setDisplaySyncEnabled(!matches!(
            config.present_mode,
            gfx_core::PresentMode::Immediate
        ));
        layer.setPresentsWithTransaction(false);
        // Bound the native drawable wait if the compositor suspends the layer.
        layer.setAllowsNextDrawableTimeout(true);
    }

    fn create_pipeline_state(
        device: &ProtocolObject<dyn MTLDevice>,
        desc: &RenderPipelineDescriptor,
        vertex_shader: &MetalShaderModule,
        fragment_shader: &MetalShaderModule,
        color_format: Format,
        depth_format: Option<Format>,
    ) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
        if desc.vertex_entry_point != vertex_shader.entry_point || desc.fragment_entry_point != fragment_shader.entry_point {
            return Err(Error::InvalidInput("Metal pipeline entry point differs from shader module".into()));
        }
        let vertex_entry = NSString::from_str(vertex_shader.bindings.as_ref().map_or(vertex_shader.entry_point.as_str(), |binding| binding.native_entry.as_ref()));
        let fragment_entry = NSString::from_str(fragment_shader.bindings.as_ref().map_or(fragment_shader.entry_point.as_str(), |binding| binding.native_entry.as_ref()));
        let vertex_function = vertex_shader
            .library
            .newFunctionWithName(&vertex_entry)
            .ok_or_else(|| {
                Error::Shader(format!(
                    "Metal vertex entry point '{}' was not found",
                    desc.vertex_entry_point
                ))
            })?;
        let fragment_function = fragment_shader
            .library
            .newFunctionWithName(&fragment_entry)
            .ok_or_else(|| {
                Error::Shader(format!(
                    "Metal fragment entry point '{}' was not found",
                    desc.fragment_entry_point
                ))
            })?;
        let pipeline_desc = MTLRenderPipelineDescriptor::new();
        if let Some(label) = &desc.label {
            pipeline_desc.setLabel(Some(&NSString::from_str(label)));
        }
        pipeline_desc.setVertexFunction(Some(&vertex_function));
        pipeline_desc.setFragmentFunction(Some(&fragment_function));
        pipeline_desc.setRasterSampleCount(1);
        if let Some(format) = depth_format {
            pipeline_desc.setDepthAttachmentPixelFormat(format_to_metal(format));
        }
        // SAFETY: color attachment index 0 is valid for a single render target pipeline.
        let color_attachment =
            unsafe { pipeline_desc.colorAttachments().objectAtIndexedSubscript(0) };
        color_attachment.setPixelFormat(format_to_metal(color_format));
        match desc
            .color_targets
            .get(0)
            .map(|target| target.blend)
            .unwrap_or(BlendMode::Replace)
        {
            BlendMode::Replace => color_attachment.setBlendingEnabled(false),
            BlendMode::Alpha => {
                color_attachment.setBlendingEnabled(true);
                color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
                color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
                color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                color_attachment
                    .setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            BlendMode::PremultipliedAlpha => {
                color_attachment.setBlendingEnabled(true);
                color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
                color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
                color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
                color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                color_attachment
                    .setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            BlendMode::AdditiveAlpha => {
                color_attachment.setBlendingEnabled(true);
                color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
                color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
                color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
                color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::One);
            }
        }
        device
            .newRenderPipelineStateWithDescriptor_error(&pipeline_desc)
            .map_err(|error| Error::Shader(nserror_message(&error)))
    }

    struct PreparedMetalRenderStep {
        pipeline: MetalRenderPipeline,
        bindings: PreparedBindings,
        draw: PreparedMetalDraw,
        scissor: Option<gfx_core::ScissorRect>,
    }

    enum PreparedMetalDraw {
        NonIndexed {
            vertex_start: u32,
            vertex_count: u32,
            instance_count: u32,
            base_instance: u32,
        },
        Indexed {
            index_buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
            index_buffer_offset: u64,
            index_type: MTLIndexType,
            index_count: u32,
            base_vertex: i32,
            instance_count: u32,
            base_instance: u32,
        },
    }

    fn encode_draw_steps(
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        steps: &[PreparedMetalRenderStep],
        extent: gfx_core::Extent2d,
        viewport: MTLViewport,
    ) {
        encoder.setViewport(viewport);
        for step in steps {
            let scissor = metal_scissor_rect(step.scissor, extent);
            if scissor.width == 0 || scissor.height == 0 {
                continue;
            }
            let pipeline = &step.pipeline;
            let _ = (pipeline.pipeline_layout,);
            encoder.setRenderPipelineState(&pipeline.pipeline_state);
            encoder.setDepthStencilState(pipeline.depth_state.as_deref());
            encoder.setFrontFacingWinding(match pipeline.raster.front_face {
                gfx_core::FrontFace::Ccw => objc2_metal::MTLWinding::CounterClockwise,
                gfx_core::FrontFace::Cw => objc2_metal::MTLWinding::Clockwise,
            });
            encoder.setCullMode(match pipeline.raster.cull_mode {
                None => objc2_metal::MTLCullMode::None,
                Some(gfx_core::Face::Front) => objc2_metal::MTLCullMode::Front,
                Some(gfx_core::Face::Back) => objc2_metal::MTLCullMode::Back,
            });
            encoder.setScissorRect(scissor);
            step.bindings.encode(encoder);
            let primitive_type = primitive_topology_to_metal(pipeline.primitive_topology);
            match &step.draw {
                PreparedMetalDraw::NonIndexed {
                    vertex_start,
                    vertex_count,
                    instance_count,
                    base_instance,
                } => {
                    // SAFETY: The shader uses vertex_index and bound resources are managed by Metal objects.
                    unsafe {
                        encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                            primitive_type,
                            usize::try_from(*vertex_start).unwrap_or(usize::MAX),
                            usize::try_from(*vertex_count).unwrap_or(usize::MAX),
                            usize::try_from(*instance_count).unwrap_or(usize::MAX),
                            usize::try_from(*base_instance).unwrap_or(usize::MAX),
                        );
                    }
                }
                PreparedMetalDraw::Indexed {
                    index_buffer,
                    index_buffer_offset,
                    index_type,
                    index_count,
                    base_vertex,
                    instance_count,
                    base_instance,
                } => {
                    // SAFETY: The index buffer is retained by the prepared render command and the
                    // checked index range is within that buffer.
                    unsafe {
                        encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                            primitive_type,
                            usize::try_from(*index_count).unwrap_or(usize::MAX),
                            *index_type,
                            index_buffer.as_ref(),
                            usize::try_from(*index_buffer_offset).unwrap_or(usize::MAX),
                            usize::try_from(*instance_count).unwrap_or(usize::MAX),
                            *base_vertex as NSInteger,
                            usize::try_from(*base_instance).unwrap_or(usize::MAX),
                        );
                    }
                }
            }
        }
    }

    fn primitive_topology_to_metal(topology: PrimitiveTopology) -> MTLPrimitiveType {
        match topology {
            PrimitiveTopology::TriangleList => MTLPrimitiveType::Triangle,
            PrimitiveTopology::TriangleStrip => MTLPrimitiveType::TriangleStrip,
        }
    }

    fn validate_index_buffer_range(
        usage: BufferUsage,
        buffer_size: u64,
        binding: IndexBufferBinding,
        first_index: u32,
        index_count: u32,
    ) -> Result<()> {
        if !usage.contains(BufferUsage::INDEX) {
            return Err(Error::InvalidInput(
                "index buffer must include INDEX usage".to_string(),
            ));
        }
        let stride = index_format_size(binding.format);
        if binding.offset % stride != 0 {
            return Err(Error::InvalidInput(
                "index buffer offset must be aligned to the index format size".to_string(),
            ));
        }
        let first_index_byte = u64::from(first_index)
            .checked_mul(stride)
            .ok_or_else(|| Error::InvalidInput("first index byte offset overflow".to_string()))?;
        let index_bytes = u64::from(index_count)
            .checked_mul(stride)
            .ok_or_else(|| Error::InvalidInput("index buffer range overflow".to_string()))?;
        let byte_end = binding
            .offset
            .checked_add(first_index_byte)
            .and_then(|start| start.checked_add(index_bytes))
            .ok_or_else(|| Error::InvalidInput("index buffer range overflow".to_string()))?;
        if byte_end > buffer_size {
            return Err(Error::InvalidInput(
                "index buffer range is out of bounds".to_string(),
            ));
        }
        Ok(())
    }

    fn index_format_size(format: IndexFormat) -> u64 {
        match format {
            IndexFormat::Uint16 => 2,
            IndexFormat::Uint32 => 4,
        }
    }

    fn index_format_to_metal(format: IndexFormat) -> MTLIndexType {
        match format {
            IndexFormat::Uint16 => MTLIndexType::UInt16,
            IndexFormat::Uint32 => MTLIndexType::UInt32,
        }
    }

    fn full_viewport(extent: gfx_core::Extent2d) -> MTLViewport {
        MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(extent.width()),
            height: f64::from(extent.height()),
            znear: 0.0,
            zfar: 1.0,
        }
    }

    fn metal_scissor_rect(
        scissor: Option<gfx_core::ScissorRect>,
        extent: gfx_core::Extent2d,
    ) -> MTLScissorRect {
        let Some(scissor) = scissor else {
            return MTLScissorRect {
                x: 0,
                y: 0,
                width: usize::try_from(extent.width()).unwrap_or(usize::MAX),
                height: usize::try_from(extent.height()).unwrap_or(usize::MAX),
            };
        };

        let x = scissor.x.min(extent.width());
        let y = scissor.y.min(extent.height());
        let right = scissor.x.saturating_add(scissor.width).min(extent.width());
        let bottom = scissor
            .y
            .saturating_add(scissor.height)
            .min(extent.height());
        MTLScissorRect {
            x: usize::try_from(x).unwrap_or(usize::MAX),
            y: usize::try_from(y).unwrap_or(usize::MAX),
            width: usize::try_from(right.saturating_sub(x)).unwrap_or(usize::MAX),
            height: usize::try_from(bottom.saturating_sub(y)).unwrap_or(usize::MAX),
        }
    }

    fn drawable_size(config: SurfaceConfig) -> CGSize {
        CGSize {
            width: f64::from(config.size.width()),
            height: f64::from(config.size.height()),
        }
    }

    fn compare_to_metal(compare: CompareFunction) -> MTLCompareFunction {
        match compare {
            CompareFunction::Never => MTLCompareFunction::Never,
            CompareFunction::Less => MTLCompareFunction::Less,
            CompareFunction::Equal => MTLCompareFunction::Equal,
            CompareFunction::LessEqual => MTLCompareFunction::LessEqual,
            CompareFunction::Greater => MTLCompareFunction::Greater,
            CompareFunction::NotEqual => MTLCompareFunction::NotEqual,
            CompareFunction::GreaterEqual => MTLCompareFunction::GreaterEqual,
            CompareFunction::Always => MTLCompareFunction::Always,
        }
    }

    fn format_to_metal(format: Format) -> MTLPixelFormat {
        match format {
            Format::R8Unorm => MTLPixelFormat::R8Unorm,
            Format::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
            Format::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
            Format::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
            Format::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
            Format::Rgba16Float => MTLPixelFormat::RGBA16Float,
            Format::Rg8Unorm => MTLPixelFormat::RG8Unorm,
            Format::R16Float => MTLPixelFormat::R16Float,
            Format::Rg16Float => MTLPixelFormat::RG16Float,
            Format::R32Float => MTLPixelFormat::R32Float,
            Format::Rgba32Float => MTLPixelFormat::RGBA32Float,
            Format::R8Uint => MTLPixelFormat::R8Uint,
            Format::Rg8Uint => MTLPixelFormat::RG8Uint,
            Format::Rgba8Uint => MTLPixelFormat::RGBA8Uint,
            Format::Depth32Float => MTLPixelFormat::Depth32Float,
        }
    }

    fn nserror_message(error: &NSError) -> String {
        error.localizedDescription().to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn registry_rejects_stale_handle() {
            let mut registry = ResourceRegistry::new("buffer");
            let id: BufferId = registry.insert(MetalBuffer {
                desc: BufferDescriptor {
                    label: None,
                    size: 1,
                    usage: BufferUsage::VERTEX,
                    memory_location: MemoryLocation::CpuToGpu,
                },
                resource: None,
                data: Some(vec![0]),
            });

            let _removed = registry.take(id).expect("handle should be live");

            let error = match registry.get(id) {
                Ok(_) => panic!("stale handle should fail"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("stale or invalid buffer handle"));
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
mod platform {
    use gfx_core::{
        Backend, BackendKind, BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor,
        BindGroupLayoutId, BufferDescriptor, BufferId, ClearColor, CommandDevice,
        CommandEncoderDescriptor, CommandEncoderId, DepthAttachment, DeviceDescriptor,
        DiagnosticsDevice, DrawCommand, DrawDescriptor, Error, LoadOp, PipelineDevice,
        PipelineLayoutDescriptor, PipelineLayoutId, PresentFrameDescriptor, PresentationDevice,
        PresentedFrame, RenderCommand, RenderPassDescriptor, RenderPassId,
        RenderPipelineDescriptor, RenderPipelineId, ResourceDevice, ResourceStats, Result,
        SamplerDescriptor, SamplerId, ShaderModuleDescriptor, ShaderModuleId, SubmissionDevice,
        SubmissionId, SubmissionStatus, SurfaceConfig, SurfaceDescriptor, SurfaceDevice, SurfaceId,
        SwapchainId, TextureDescriptor, TextureId, TextureViewDescriptor, TextureViewId,
        TextureWriteDescriptor,
    };

    /// Stub Metal device for non-Apple targets.
    pub struct MetalDevice;

    impl MetalDevice {
        /// Returns unavailable on non-Apple targets.
        ///
        /// # Errors
        ///
        /// Always returns [`Error::Unavailable`] on non-Apple targets.
        pub fn new(_desc: &DeviceDescriptor) -> Result<Self> {
            Err(Error::Unavailable(
                "Metal backend is only available on Apple targets".to_string(),
            ))
        }
    }

    fn unavailable<T>() -> Result<T> {
        Err(Error::Unavailable(
            "Metal backend is only available on Apple targets".to_string(),
        ))
    }

    impl Backend for MetalDevice {
        const BACKEND_KIND: BackendKind = BackendKind::Metal;

        fn adapter_name(&self) -> &str {
            "Metal"
        }
    }

    impl SurfaceDevice for MetalDevice {
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

    impl ResourceDevice for MetalDevice {
        fn copy_texture_batch(&mut self, _copies: &[gfx_core::TextureCopy]) -> Result<()> {
            unavailable()
        }

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

    impl PipelineDevice for MetalDevice {
        fn create_pipeline_layout(
            &mut self,
            _desc: &PipelineLayoutDescriptor,
        ) -> Result<PipelineLayoutId> {
            unavailable()
        }

        fn create_shader_module(
            &mut self,
            _desc: &ShaderModuleDescriptor,
        ) -> Result<ShaderModuleId> {
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

    impl CommandDevice for MetalDevice {
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

    impl SubmissionDevice for MetalDevice {
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

    impl PresentationDevice for MetalDevice {
        fn render_and_present(
            &mut self,
            _frame: &PresentFrameDescriptor<'_>,
        ) -> Result<Option<PresentedFrame>> {
            unavailable()
        }
    }

    impl DiagnosticsDevice for MetalDevice {
        fn resource_stats(&self) -> ResourceStats {
            ResourceStats::default()
        }
    }
}

pub use platform::*;

#[cfg(all(test, not(target_vendor = "apple")))]
mod non_apple_stub_tests {
    use super::*;
    use gfx_core::ResourceDevice as _;

    #[test]
    fn metal_stub_is_unavailable_without_apple_platform() {
        assert!(matches!(
            MetalDevice::new(&gfx_core::DeviceDescriptor::default()),
            Err(Error::Unavailable(_))
        ));
        let mut stub = MetalDevice;
        assert!(matches!(
            stub.copy_texture_batch(&[]),
            Err(Error::Unavailable(_))
        ));
    }
}
