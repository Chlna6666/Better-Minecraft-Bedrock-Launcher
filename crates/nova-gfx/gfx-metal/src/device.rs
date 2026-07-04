//! Metal backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Metal on Apple
//! targets. Non-Apple builds expose a minimal stub that returns
//! `GfxError::Unavailable`.
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
    AddressMode, BackendKind, BeginRenderPassDesc, BlendMode, BufferDesc, BufferId, BufferUsage,
    ClearColor, CommandEncoderDesc, CommandEncoderId, DeviceDesc, DrawDesc, DrawStepDesc,
    FilterMode, Format, GfxBackend, GfxCommandDevice, GfxDiagnosticsDevice, GfxError,
    GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice, GfxSubmissionDevice,
    GfxSurfaceDevice, GfxThreadingMode, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation,
    PipelineLayoutDesc, PipelineLayoutId, PrimitiveTopology, RenderPassDepthAttachment,
    RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId, RenderStepDescriptor,
    RenderStepList, RenderStepRef, RenderTarget, ResourceBindingResource, ResourceSetDesc,
    ResourceSetId, ResourceSetLayoutDesc, ResourceSetLayoutId, ResourceStats, Result, SamplerDesc,
    SamplerId, ShaderCode, ShaderModuleDesc, ShaderModuleId, ShaderStage, SubmissionId,
    SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId, TextureDesc,
    TextureDimension, TextureId, TextureUsage, TextureViewDesc, TextureViewId, TextureWriteDesc,
};

#[cfg(target_vendor = "apple")]
mod platform {
    use super::*;
    use crate::registry::ResourceRegistry;
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use gfx_core::SwapchainId;
    use objc2::{ClassType, rc::Retained, runtime::ProtocolObject};
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::{NSError, NSInteger, NSString};
    use objc2_metal::{
        MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
        MTLCommandQueue, MTLCompileOptions, MTLDevice, MTLDrawable, MTLFunction, MTLIndexType,
        MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
        MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
        MTLResourceOptions, MTLScissorRect, MTLStoreAction, MTLTexture, MTLViewport,
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
        resource_set_layouts: ResourceRegistry<MetalResourceSetLayout>,
        resource_sets: ResourceRegistry<MetalResourceSet>,
        pipeline_layouts: ResourceRegistry<MetalPipelineLayout>,
        shader_modules: ResourceRegistry<MetalShaderModule>,
        render_passes: ResourceRegistry<MetalRenderPass>,
        render_pipelines: ResourceRegistry<MetalRenderPipeline>,
        command_encoders: ResourceRegistry<MetalCommandEncoder>,
        surfaces: ResourceRegistry<MetalSurface>,
        swapchains: ResourceRegistry<MetalSwapchain>,
        submitted_frames: u64,
    }

    impl MetalDevice {
        /// Creates a Metal device.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] if Metal initialization fails.
        pub fn new(_desc: &DeviceDesc) -> Result<Self> {
            let device = objc2_metal::MTLCreateSystemDefaultDevice()
                .ok_or_else(|| GfxError::Unavailable("no compatible Metal device".to_string()))?;
            let command_queue = device.newCommandQueue().ok_or_else(|| {
                GfxError::Unavailable("failed to create Metal command queue".to_string())
            })?;
            Ok(Self {
                device,
                command_queue,
                buffers: ResourceRegistry::new("buffer"),
                textures: ResourceRegistry::new("texture"),
                texture_views: ResourceRegistry::new("texture view"),
                samplers: ResourceRegistry::new("sampler"),
                resource_set_layouts: ResourceRegistry::new("resource set layout"),
                resource_sets: ResourceRegistry::new("resource set"),
                pipeline_layouts: ResourceRegistry::new("pipeline layout"),
                shader_modules: ResourceRegistry::new("shader module"),
                render_passes: ResourceRegistry::new("render pass"),
                render_pipelines: ResourceRegistry::new("render pipeline"),
                command_encoders: ResourceRegistry::new("command encoder"),
                surfaces: ResourceRegistry::new("surface"),
                swapchains: ResourceRegistry::new("swapchain"),
                submitted_frames: 0,
            })
        }

        /// Creates a native Metal surface from raw-window-handle traits.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when native handles are invalid.
        fn create_surface<W>(&mut self, window: &W, desc: &SurfaceDesc) -> Result<SurfaceId>
        where
            W: HasDisplayHandle + HasWindowHandle + ?Sized,
        {
            let _display = window
                .display_handle()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let window = window
                .window_handle()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let ns_view = match window.as_raw() {
                RawWindowHandle::AppKit(handle) => handle.ns_view,
                other => {
                    return Err(GfxError::InvalidInput(format!(
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
        fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
            desc.validate()?;
            let length = usize::try_from(desc.size).map_err(|error| {
                GfxError::InvalidInput(format!("buffer size overflow: {error}"))
            })?;
            let resource = self
                .device
                .newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| GfxError::Backend("failed to create Metal buffer".to_string()))?;
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
                GfxError::Unavailable(
                    "Metal GPU-only staging upload is not enabled in this build".to_string(),
                )
            })?;
            let offset = usize::try_from(offset)
                .map_err(|error| GfxError::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| GfxError::InvalidInput("buffer write range overflow".to_string()))?;
            let target = storage.get_mut(offset..end).ok_or_else(|| {
                GfxError::InvalidInput("buffer write range is out of bounds".to_string())
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

        /// Creates a 2D texture record.
        fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
            desc.validate()?;
            if desc.dimension != TextureDimension::D2 {
                return Err(GfxError::InvalidInput(
                    "only 2D textures are supported".to_string(),
                ));
            }
            Ok(self.textures.insert(MetalTexture {
                desc: desc.clone(),
                resource: None,
            }))
        }

        /// Writes data into a texture.
        fn write_texture(&mut self, _desc: TextureWriteDesc, _data: &[u8]) -> Result<()> {
            Err(GfxError::Unavailable(
                "Metal texture upload is not enabled in this build".to_string(),
            ))
        }

        /// Creates a texture view record.
        fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
            let _texture = self.textures.get(desc.texture)?;
            Ok(self.texture_views.insert(MetalTextureView {
                texture: desc.texture,
                format: desc.format,
            }))
        }

        /// Creates a sampler record.
        fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
            Ok(self.samplers.insert(MetalSampler {
                mag_filter: desc.mag_filter,
                min_filter: desc.min_filter,
                address_mode_u: desc.address_mode_u,
                address_mode_v: desc.address_mode_v,
            }))
        }

        fn create_resource_set_layout(
            &mut self,
            desc: &ResourceSetLayoutDesc,
        ) -> Result<ResourceSetLayoutId> {
            desc.validate()?;
            Ok(self
                .resource_set_layouts
                .insert(MetalResourceSetLayout { desc: desc.clone() }))
        }

        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
            desc.validate()?;
            for layout in &desc.resource_set_layouts {
                let _ = self.resource_set_layouts.get(*layout)?;
            }
            Ok(self.pipeline_layouts.insert(MetalPipelineLayout {
                resource_set_layouts: desc.resource_set_layouts.clone(),
            }))
        }

        fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
            let layout = self.resource_set_layouts.get(desc.layout)?.desc.clone();
            desc.validate_against(&layout)?;
            let mut bindings = Vec::with_capacity(desc.bindings.len());
            for binding in &desc.bindings {
                match binding.resource {
                    ResourceBindingResource::Buffer(buffer_binding) => {
                        let _ = self.buffers.get(buffer_binding.buffer)?;
                    }
                    ResourceBindingResource::Texture(texture_binding) => {
                        let _ = self.texture_views.get(texture_binding.texture_view)?;
                    }
                    ResourceBindingResource::Sampler(sampler_binding) => {
                        let _ = self.samplers.get(sampler_binding.sampler)?;
                    }
                }
                bindings.push(*binding);
            }
            Ok(self.resource_sets.insert(MetalResourceSet {
                layout: desc.layout,
                bindings,
            }))
        }

        /// Creates and validates a shader module.
        fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
            desc.validate()?;
            let (source, library) = match &desc.binary.code {
                ShaderCode::Msl(source) => {
                    let source_string = NSString::from_str(source);
                    let options = MTLCompileOptions::new();
                    let library = self
                        .device
                        .newLibraryWithSource_options_error(&source_string, Some(&options))
                        .map_err(|error| GfxError::Shader(nserror_message(&error)))?;
                    (source.clone(), library)
                }
                ShaderCode::Hlsl(_) | ShaderCode::DxBytecode(_) | ShaderCode::Spirv(_) => {
                    return Err(GfxError::Shader(
                        "Metal shader module requires MSL source".to_string(),
                    ));
                }
            };
            Ok(self.shader_modules.insert(MetalShaderModule {
                stage: desc.binary.stage,
                entry_point: desc.binary.entry_point.clone(),
                source,
                library,
            }))
        }

        /// Creates a render pass record.
        fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
            Ok(self.render_passes.insert(MetalRenderPass {
                color_format: desc.color_attachment.format,
            }))
        }

        /// Creates a graphics render pipeline record.
        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDesc,
            _viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            desc.validate()?;
            let vertex_shader = self.shader_modules.get(desc.vertex_shader)?;
            let fragment_shader = self.shader_modules.get(desc.fragment_shader)?;
            if vertex_shader.stage != ShaderStage::Vertex {
                return Err(GfxError::InvalidInput(
                    "vertex shader module must use ShaderStage::Vertex".to_string(),
                ));
            }
            if fragment_shader.stage != ShaderStage::Fragment {
                return Err(GfxError::InvalidInput(
                    "fragment shader module must use ShaderStage::Fragment".to_string(),
                ));
            }
            let render_pass = self.render_passes.get(desc.render_pass)?;
            if render_pass.color_format != desc.color_format {
                return Err(GfxError::InvalidInput(
                    "pipeline color_format must match render pass color attachment".to_string(),
                ));
            }
            let pipeline_state = create_pipeline_state(
                &self.device,
                desc,
                vertex_shader,
                fragment_shader,
                render_pass.color_format,
            )?;
            Ok(self.render_pipelines.insert(MetalRenderPipeline {
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: desc.primitive_topology,
                pipeline_state,
                pipeline_layout: desc.pipeline_layout,
            }))
        }

        fn create_command_encoder(
            &mut self,
            _desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
            Ok(self
                .command_encoders
                .insert(MetalCommandEncoder { in_flight: false }))
        }

        fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: &DrawDesc) -> Result<()> {
            let _encoder = self.command_encoders.get(encoder)?;
            let RenderTarget::Swapchain { swapchain, .. } = draw.pass.target else {
                return Err(GfxError::Unavailable(
                    "Metal offscreen render target is not implemented yet".to_string(),
                ));
            };
            let _swapchain = self.swapchains.get(swapchain)?;
            let _render_pass = self.render_passes.get(draw.pass.render_pass)?;
            let _pipeline = self.render_pipelines.get(draw.pipeline)?;
            for resource_set in &draw.resource_sets {
                let _ = self.resource_sets.get(*resource_set)?;
            }
            Ok(())
        }

        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            let _encoder = self.command_encoders.get(encoder)?;
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            Ok(())
        }

        fn draw_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: gfx_core::ClearColor,
        ) -> Result<()> {
            self.draw_internal(
                swapchain,
                render_pass,
                RenderStepList::from_draw_steps(steps),
                clear_color,
            )
        }

        fn render_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: gfx_core::ClearColor,
        ) -> Result<()> {
            self.draw_internal(
                swapchain,
                render_pass,
                RenderStepList::from_render_steps(steps),
                clear_color,
            )
        }

        fn draw_steps_to_texture(
            &mut self,
            _texture_view: TextureViewId,
            _render_pass: RenderPassId,
            _steps: &[DrawStepDesc],
            _color_load_op: gfx_core::LoadOp<gfx_core::ClearColor>,
        ) -> Result<()> {
            Err(GfxError::Unavailable(
                "Metal offscreen render target is not implemented yet".to_string(),
            ))
        }

        fn draw_internal(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: gfx_core::ClearColor,
        ) -> Result<()> {
            if steps.is_empty() {
                return Err(GfxError::InvalidInput(
                    "Metal draw step list must not be empty".to_string(),
                ));
            }
            let swapchain = self.swapchains.get(swapchain)?;
            let render_pass = self.render_passes.get(render_pass)?;
            let render_steps = steps
                .iter()
                .map(|step| self.prepare_render_step(render_pass, step))
                .collect::<Result<Vec<_>>>()?;

            objc2::rc::autoreleasepool(|_| {
                let drawable = swapchain.layer.nextDrawable().ok_or_else(|| {
                    GfxError::Backend("CAMetalLayer did not provide a drawable".to_string())
                })?;
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

                let command_buffer = self.command_queue.commandBuffer().ok_or_else(|| {
                    GfxError::Backend("failed to create Metal command buffer".to_string())
                })?;
                let encoder = command_buffer
                    .renderCommandEncoderWithDescriptor(&descriptor)
                    .ok_or_else(|| {
                        GfxError::Backend("failed to create Metal render encoder".to_string())
                    })?;
                encode_draw_steps(&encoder, &render_steps, swapchain.config);
                encoder.endEncoding();
                command_buffer.presentDrawable(drawable.as_ref());
                command_buffer.commit();
                Ok(())
            })?;
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            Ok(())
        }

        fn prepare_render_step(
            &self,
            render_pass: &MetalRenderPass,
            step: RenderStepRef<'_>,
        ) -> Result<PreparedMetalRenderStep> {
            let pipeline = self.render_pipelines.get(step.pipeline())?.clone();
            if render_pass.color_format != pipeline.color_format {
                return Err(GfxError::InvalidInput(
                    "render pass and pipeline color formats do not match".to_string(),
                ));
            }
            let resource_sets = step
                .resource_sets()
                .iter()
                .copied()
                .map(|resource_set| Ok(self.resource_sets.get(resource_set)?.clone()))
                .collect::<Result<Vec<_>>>()?;
            let draw = match step {
                RenderStepRef::Draw(step) => PreparedMetalDraw::NonIndexed {
                    vertex_start: step.first_vertex,
                    vertex_count: step.vertex_count,
                    instance_count: step.instance_count,
                    base_instance: step.first_instance,
                },
                RenderStepRef::DrawIndexed(step) => {
                    let buffer = self.buffers.get(step.index_buffer.buffer)?;
                    validate_index_buffer_range(
                        buffer.desc.usage,
                        buffer.desc.size,
                        step.index_buffer,
                        step.first_index,
                        step.index_count,
                    )?;
                    let resource = buffer.resource.clone().ok_or_else(|| {
                        GfxError::Backend("Metal index buffer has no native resource".to_string())
                    })?;
                    let first_index_offset = u64::from(step.first_index)
                        .checked_mul(index_format_size(step.index_buffer.format))
                        .ok_or_else(|| {
                            GfxError::InvalidInput(
                                "Metal index buffer first index offset overflow".to_string(),
                            )
                        })?;
                    let index_buffer_offset = step
                        .index_buffer
                        .offset
                        .checked_add(first_index_offset)
                        .ok_or_else(|| {
                            GfxError::InvalidInput(
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
                resource_sets,
                draw,
                scissor: step.scissor(),
            })
        }

        fn present(&mut self, swapchain: SwapchainId, _image_index: u32) -> Result<()> {
            let swapchain = self.swapchains.get(swapchain)?;
            objc2::rc::autoreleasepool(|_| {
                let drawable = swapchain.layer.nextDrawable().ok_or_else(|| {
                    GfxError::Backend("CAMetalLayer did not provide a drawable".to_string())
                })?;
                let command_buffer = self.command_queue.commandBuffer().ok_or_else(|| {
                    GfxError::Backend("failed to create Metal command buffer".to_string())
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

        fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
            let _layout = self.resource_set_layouts.take(layout)?;
            Ok(())
        }

        fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
            let _resource_set = self.resource_sets.take(resource_set)?;
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
            ResourceStats {
                buffers: self.buffers.live_len(),
                textures: self.textures.live_len(),
                texture_views: self.texture_views.live_len(),
                samplers: self.samplers.live_len(),
                resource_set_layouts: self.resource_set_layouts.live_len(),
                resource_sets: self.resource_sets.live_len(),
                pipeline_layouts: self.pipeline_layouts.live_len(),
                shader_modules: self.shader_modules.live_len(),
                render_passes: self.render_passes.live_len(),
                render_pipelines: self.render_pipelines.live_len(),
                command_encoders: self.command_encoders.live_len(),
                submissions: 0,
                surfaces: self.surfaces.live_len(),
                swapchains: self.swapchains.live_len(),
                allocated_bytes: 0,
                reserved_bytes: 0,
            }
        }
    }

    impl GfxBackend for MetalDevice {
        const BACKEND_KIND: BackendKind = BackendKind::Metal;
    }

    impl GfxSurfaceDevice for MetalDevice {
        type SurfaceTarget = dyn MetalSurfaceTarget;

        fn create_surface(
            &mut self,
            target: &Self::SurfaceTarget,
            desc: &SurfaceDesc,
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

    impl GfxResourceDevice for MetalDevice {
        fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
            Self::create_buffer(self, desc)
        }

        fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
            Self::write_buffer(self, buffer, offset, data)
        }

        fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
            Self::create_texture(self, desc)
        }

        fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
            Self::write_texture(self, desc, data)
        }

        fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
            Self::create_texture_view(self, desc)
        }

        fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
            Self::create_sampler(self, desc)
        }

        fn create_resource_set_layout(
            &mut self,
            desc: &ResourceSetLayoutDesc,
        ) -> Result<ResourceSetLayoutId> {
            Self::create_resource_set_layout(self, desc)
        }

        fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
            Self::create_resource_set(self, desc)
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

        fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
            Self::destroy_resource_set_layout(self, layout)
        }

        fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
            Self::destroy_resource_set(self, resource_set)
        }
    }

    impl GfxPipelineDevice for MetalDevice {
        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
            Self::create_pipeline_layout(self, desc)
        }

        fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
            Self::create_shader_module(self, desc)
        }

        fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
            Self::create_render_pass(self, desc)
        }

        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDesc,
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

    impl GfxCommandDevice for MetalDevice {
        fn create_command_encoder(
            &mut self,
            desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
            Self::create_command_encoder(self, desc)
        }

        fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDesc) -> Result<()> {
            Self::record_draw_desc(self, encoder, &draw)
        }

        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::submit(self, encoder)
        }

        fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::destroy_command_encoder(self, encoder)
        }
    }

    impl GfxSubmissionDevice for MetalDevice {
        fn async_capabilities(&self) -> gfx_core::GfxAsyncCapabilities {
            gfx_core::GfxAsyncCapabilities {
                threading_mode: GfxThreadingMode::OwnerThreadOnly,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
                partial_presentation: false,
            }
        }

        fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
            self.submit(encoder)?;
            Ok(SubmissionId::from_parts(0, 0))
        }

        fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
            if submission.raw() == 0 {
                Ok(SubmissionStatus::Complete)
            } else {
                Err(GfxError::InvalidInput(format!(
                    "unknown Metal submission {}",
                    submission.raw()
                )))
            }
        }

        fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
            match self.poll_submission(submission)? {
                SubmissionStatus::Complete => Ok(()),
                SubmissionStatus::Pending => Err(GfxError::Unavailable(
                    "Metal deferred wait is not implemented yet".to_string(),
                )),
                SubmissionStatus::Failed(error) => Err(GfxError::Backend(error)),
            }
        }
    }

    impl GfxPresentationDevice for MetalDevice {
        fn draw_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: ClearColor,
        ) -> Result<()> {
            Self::draw_steps_and_present(self, swapchain, render_pass, steps, clear_color)
        }

        fn draw_steps_to_texture(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            Self::draw_steps_to_texture(self, texture_view, render_pass, steps, color_load_op)
        }

        fn render_steps_and_present_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            _depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Self::render_steps_and_present(self, swapchain, render_pass, steps, clear_color)
        }

        fn render_steps_to_texture_compat(
            &mut self,
            _texture_view: TextureViewId,
            _render_pass: RenderPassId,
            _steps: &[RenderStepDescriptor],
            _color_load_op: LoadOp<ClearColor>,
            _depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Err(GfxError::Unavailable(
                "Metal offscreen render target is not implemented yet".to_string(),
            ))
        }

        fn render_steps_and_present_deferred_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            _depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<SubmissionId>
        where
            Self: GfxSubmissionDevice,
        {
            Self::render_steps_and_present(self, swapchain, render_pass, steps, clear_color)?;
            Ok(SubmissionId::from_parts(0, 0))
        }
    }

    impl GfxDiagnosticsDevice for MetalDevice {
        fn resource_stats(&self) -> ResourceStats {
            Self::resource_stats(self)
        }
    }

    #[derive(Clone)]
    struct MetalBuffer {
        desc: BufferDesc,
        resource: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        data: Option<Vec<u8>>,
    }

    #[derive(Clone)]
    struct MetalTexture {
        desc: TextureDesc,
        resource: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    }

    #[derive(Clone, Copy)]
    struct MetalTextureView {
        texture: TextureId,
        format: Format,
    }

    #[derive(Clone, Copy)]
    struct MetalSampler {
        mag_filter: FilterMode,
        min_filter: FilterMode,
        address_mode_u: AddressMode,
        address_mode_v: AddressMode,
    }

    #[derive(Clone)]
    struct MetalResourceSetLayout {
        desc: ResourceSetLayoutDesc,
    }

    #[derive(Clone)]
    struct MetalResourceSet {
        layout: ResourceSetLayoutId,
        bindings: Vec<gfx_core::ResourceBinding>,
    }

    #[derive(Clone)]
    struct MetalPipelineLayout {
        resource_set_layouts: Vec<ResourceSetLayoutId>,
    }

    #[derive(Clone)]
    struct MetalShaderModule {
        stage: ShaderStage,
        entry_point: String,
        source: String,
        library: Retained<ProtocolObject<dyn MTLLibrary>>,
    }

    #[derive(Clone, Copy)]
    struct MetalRenderPass {
        color_format: Format,
    }

    #[derive(Clone)]
    struct MetalRenderPipeline {
        color_format: Format,
        blend_mode: BlendMode,
        primitive_topology: PrimitiveTopology,
        pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
        pipeline_layout: Option<PipelineLayoutId>,
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
                GfxError::InvalidInput("AppKit ns_view is not retainable".to_string())
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
        layer.setAllowsNextDrawableTimeout(false);
    }

    fn create_pipeline_state(
        device: &ProtocolObject<dyn MTLDevice>,
        desc: &RenderPipelineDesc,
        vertex_shader: &MetalShaderModule,
        fragment_shader: &MetalShaderModule,
        color_format: Format,
    ) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
        let vertex_entry = NSString::from_str(&desc.vertex_entry_point);
        let fragment_entry = NSString::from_str(&desc.fragment_entry_point);
        let vertex_function = vertex_shader
            .library
            .newFunctionWithName(&vertex_entry)
            .ok_or_else(|| {
                GfxError::Shader(format!(
                    "Metal vertex entry point '{}' was not found",
                    desc.vertex_entry_point
                ))
            })?;
        let fragment_function = fragment_shader
            .library
            .newFunctionWithName(&fragment_entry)
            .ok_or_else(|| {
                GfxError::Shader(format!(
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
        // SAFETY: color attachment index 0 is valid for a single render target pipeline.
        let color_attachment =
            unsafe { pipeline_desc.colorAttachments().objectAtIndexedSubscript(0) };
        color_attachment.setPixelFormat(format_to_metal(color_format));
        match desc.blend_mode {
            BlendMode::Replace => color_attachment.setBlendingEnabled(false),
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
            .map_err(|error| GfxError::Shader(nserror_message(&error)))
    }

    struct PreparedMetalRenderStep {
        pipeline: MetalRenderPipeline,
        resource_sets: Vec<MetalResourceSet>,
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
        config: SurfaceConfig,
    ) {
        encoder.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(config.size.width()),
            height: f64::from(config.size.height()),
            znear: 0.0,
            zfar: 1.0,
        });
        for step in steps {
            let pipeline = &step.pipeline;
            let _ = (pipeline.blend_mode, pipeline.pipeline_layout);
            encoder.setRenderPipelineState(&pipeline.pipeline_state);
            encoder.setScissorRect(metal_scissor_rect(step.scissor, config.size));
            for resource_set in &step.resource_sets {
                let _ = (resource_set.layout, resource_set.bindings.len());
            }
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
                    // SAFETY: The index buffer is retained by the prepared draw step and the
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
            return Err(GfxError::InvalidInput(
                "index buffer must include INDEX usage".to_string(),
            ));
        }
        let stride = index_format_size(binding.format);
        if binding.offset % stride != 0 {
            return Err(GfxError::InvalidInput(
                "index buffer offset must be aligned to the index format size".to_string(),
            ));
        }
        let first_index_byte = u64::from(first_index).checked_mul(stride).ok_or_else(|| {
            GfxError::InvalidInput("first index byte offset overflow".to_string())
        })?;
        let index_bytes = u64::from(index_count)
            .checked_mul(stride)
            .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
        let byte_end = binding
            .offset
            .checked_add(first_index_byte)
            .and_then(|start| start.checked_add(index_bytes))
            .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
        if byte_end > buffer_size {
            return Err(GfxError::InvalidInput(
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

    fn metal_scissor_rect(
        scissor: Option<gfx_core::ScissorRect>,
        extent: gfx_core::Extent2d,
    ) -> MTLScissorRect {
        let Some(scissor) = scissor.filter(|scissor| !scissor.is_empty()) else {
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

    fn format_to_metal(format: Format) -> MTLPixelFormat {
        match format {
            Format::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
            Format::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
            Format::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
            Format::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
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
                desc: BufferDesc {
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
        BackendKind, BufferDesc, BufferId, ClearColor, CommandEncoderDesc, CommandEncoderId,
        DeviceDesc, DrawDesc, DrawStepDesc, GfxBackend, GfxCommandDevice, GfxDiagnosticsDevice,
        GfxError, GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice, GfxSubmissionDevice,
        GfxSurfaceDevice, LoadOp, PipelineLayoutDesc, PipelineLayoutId, RenderPassDepthAttachment,
        RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId, RenderStepDescriptor,
        ResourceSetDesc, ResourceSetId, ResourceSetLayoutDesc, ResourceSetLayoutId, ResourceStats,
        Result, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, SubmissionId,
        SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId, TextureDesc,
        TextureId, TextureViewDesc, TextureViewId, TextureWriteDesc,
    };

    /// Stub Metal device for non-Apple targets.
    pub struct MetalDevice;

    impl MetalDevice {
        /// Returns unavailable on non-Apple targets.
        ///
        /// # Errors
        ///
        /// Always returns [`GfxError::Unavailable`] on non-Apple targets.
        pub fn new(_desc: &DeviceDesc) -> Result<Self> {
            Err(GfxError::Unavailable(
                "Metal backend is only available on Apple targets".to_string(),
            ))
        }
    }

    fn unavailable<T>() -> Result<T> {
        Err(GfxError::Unavailable(
            "Metal backend is only available on Apple targets".to_string(),
        ))
    }

    impl GfxBackend for MetalDevice {
        const BACKEND_KIND: BackendKind = BackendKind::Metal;
    }

    impl GfxSurfaceDevice for MetalDevice {
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

    impl GfxResourceDevice for MetalDevice {
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

    impl GfxPipelineDevice for MetalDevice {
        fn create_pipeline_layout(
            &mut self,
            _desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
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

    impl GfxCommandDevice for MetalDevice {
        fn create_command_encoder(
            &mut self,
            _desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
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

    impl GfxSubmissionDevice for MetalDevice {
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

    impl GfxPresentationDevice for MetalDevice {
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

    impl GfxDiagnosticsDevice for MetalDevice {
        fn resource_stats(&self) -> ResourceStats {
            ResourceStats::default()
        }
    }
}

pub use platform::*;
