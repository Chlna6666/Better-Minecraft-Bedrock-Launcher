use crate::device::{OpenGlDevice, native};
use gfx_core::*;
use glow::HasContext as _;
use std::collections::HashMap;

pub(crate) struct Shader {
    pub(crate) native: glow::NativeShader,
    pub(crate) stage: ShaderStage,
    entry: String,
    pub(crate) reflection: GlslShader,
}
pub(crate) struct BufferSlot {
    pub(crate) logical: u32,
    pub(crate) slot: u32,
    pub(crate) kind: BindingType,
}
pub(crate) struct TextureSlot {
    pub(crate) logical: u32,
    pub(crate) sampler: Option<u32>,
    pub(crate) unit: u32,
}
pub(crate) struct ImageSlot {
    pub(crate) logical: u32,
    pub(crate) unit: u32,
    pub(crate) format: Format,
    pub(crate) read_only: bool,
}
pub(crate) struct Pipeline {
    pub(crate) native: glow::NativeProgram,
    pub(crate) desc: RenderPipelineDescriptor,
    pub(crate) buffers: Vec<BufferSlot>,
    pub(crate) textures: Vec<TextureSlot>,
    pub(crate) images: Vec<ImageSlot>,
    pub(crate) first_instance: Option<glow::NativeUniformLocation>,
}

impl PipelineDevice for OpenGlDevice {
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        if desc.bind_group_layouts.len() > 1 {
            return Err(Error::Unavailable(
                "OpenGL supports resource group zero".into(),
            ));
        }
        for id in &desc.bind_group_layouts {
            self.layouts.get(*id)?;
        }
        Ok(self.pipeline_layouts.insert(desc.clone()))
    }
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        desc.validate()?;
        let ShaderCode::Glsl(reflection) = &desc.binary.code else {
            return Err(Error::Shader(
                "OpenGL requires build-translated GLSL/reflection".into(),
            ));
        };
        self.park()?;
        // SAFETY: context is current; the driver copies and compiles this owned source.
        let shader = unsafe {
            let shader = self
                .gl
                .create_shader(if desc.binary.stage == ShaderStage::Vertex {
                    glow::VERTEX_SHADER
                } else if desc.binary.stage == ShaderStage::Compute {
                    glow::COMPUTE_SHADER
                } else {
                    glow::FRAGMENT_SHADER
                })
                .map_err(native)?;
            let source = if desc.binary.stage == ShaderStage::Vertex
                && self.compute.capabilities.indirect_draw
            {
                crate::compute::native_instance_source(&reflection.source)
            } else {
                std::borrow::Cow::Borrowed(reflection.source.as_ref())
            };
            self.gl.shader_source(shader, &source);
            self.gl.compile_shader(shader);
            if !self.gl.get_shader_compile_status(shader) {
                let error = self.gl.get_shader_info_log(shader);
                self.gl.delete_shader(shader);
                return Err(Error::Shader(format!(
                    "OpenGL {}: {error}",
                    desc.binary.entry_point
                )));
            }
            shader
        };
        Ok(self.shaders.insert(Shader {
            native: shader,
            stage: desc.binary.stage,
            entry: desc.binary.entry_point.clone(),
            reflection: reflection.clone(),
        }))
    }
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        desc.validate()?;
        Ok(self.passes.insert(desc.clone()))
    }
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        _extent: Extent2d,
    ) -> Result<RenderPipelineId> {
        desc.validate()?;
        let pass = self.passes.get(desc.render_pass)?;
        if pass.color_attachments.len() != desc.color_targets.len()
            || pass
                .color_attachments
                .iter()
                .zip(&desc.color_targets)
                .any(|(attachment, target)| attachment.format != target.format)
            || (desc.depth_state.is_some() && pass.depth_attachment.is_none())
        {
            return Err(Error::InvalidInput(
                "OpenGL pipeline/pass attachments differ".into(),
            ));
        }
        if !desc.vertex_buffers.is_empty() {
            return Err(Error::Unavailable(
                "OpenGL uses shader-pulled vertex buffers".into(),
            ));
        }
        if let Some(id) = desc.pipeline_layout {
            self.pipeline_layouts.get(id)?;
        }
        let vertex = self.shaders.get(desc.vertex_shader)?;
        let fragment = self.shaders.get(desc.fragment_shader)?;
        if vertex.stage != ShaderStage::Vertex
            || fragment.stage != ShaderStage::Fragment
            || vertex.entry != desc.vertex_entry_point
            || fragment.entry != desc.fragment_entry_point
        {
            return Err(Error::Shader("OpenGL shader stage/entry mismatch".into()));
        }
        self.park()?;
        // SAFETY: shaders and linked program belong to the same current context.
        let program = unsafe {
            let program = self.gl.create_program().map_err(native)?;
            self.gl.attach_shader(program, vertex.native);
            self.gl.attach_shader(program, fragment.native);
            self.gl.link_program(program);
            self.gl.detach_shader(program, vertex.native);
            self.gl.detach_shader(program, fragment.native);
            if !self.gl.get_program_link_status(program) {
                let error = self.gl.get_program_info_log(program);
                self.gl.delete_program(program);
                return Err(Error::Shader(format!("OpenGL program link: {error}")));
            }
            program
        };
        let result =
            self.reflect_program(program, desc, &[&vertex.reflection, &fragment.reflection]);
        match result {
            Ok(pipeline) => Ok(self.pipelines.insert(pipeline)),
            Err(error) => {
                unsafe {
                    self.gl.delete_program(program);
                }
                Err(error)
            }
        }
    }
    fn destroy_pipeline_layout(&mut self, id: PipelineLayoutId) -> Result<()> {
        self.pipeline_layouts.take(id)?;
        Ok(())
    }
    fn destroy_shader_module(&mut self, id: ShaderModuleId) -> Result<()> {
        self.park()?;
        self.compute.remove(&self.gl, id);
        let shader = self.shaders.take(id)?;
        unsafe {
            self.gl.delete_shader(shader.native);
        }
        self.check()
    }
    fn destroy_render_pass(&mut self, id: RenderPassId) -> Result<()> {
        self.passes.take(id)?;
        Ok(())
    }
    fn destroy_render_pipeline(&mut self, id: RenderPipelineId) -> Result<()> {
        self.park()?;
        let pipeline = self.pipelines.take(id)?;
        unsafe {
            self.gl.delete_program(pipeline.native);
        }
        self.check()
    }
}
impl OpenGlDevice {
    pub(crate) fn reflect_buffers(
        &self,
        program: glow::NativeProgram,
        shaders: &[&GlslShader],
    ) -> Result<Vec<BufferSlot>> {
        let mut uniform_slots = HashMap::new();
        let mut storage_slots = HashMap::new();
        let mut buffers = Vec::new();
        // SAFETY: the context is current and reflection addresses this linked program.
        unsafe {
            for shader in shaders {
                for binding in &shader.buffers {
                    let uniform = binding.kind == BindingType::UniformBuffer;
                    let index = if uniform {
                        self.gl.get_uniform_block_index(program, &binding.name)
                    } else {
                        self.gl
                            .get_shader_storage_block_index(program, &binding.name)
                    };
                    let Some(index) = index else { continue };
                    let slots = if uniform {
                        &mut uniform_slots
                    } else {
                        &mut storage_slots
                    };
                    let next = slots.len() as u32;
                    let limit = self.gl.get_parameter_i32(if uniform {
                        glow::MAX_UNIFORM_BUFFER_BINDINGS
                    } else {
                        glow::MAX_SHADER_STORAGE_BUFFER_BINDINGS
                    }) as u32;
                    let slot = *slots.entry(binding.binding).or_insert(next);
                    if slot >= limit {
                        return Err(Error::Unavailable(
                            "OpenGL program exceeds native buffer slots".into(),
                        ));
                    }
                    if uniform {
                        self.gl.uniform_block_binding(program, index, slot);
                    } else {
                        self.gl.shader_storage_block_binding(program, index, slot);
                    }
                    if !buffers.iter().any(|buffer: &BufferSlot| {
                        buffer.kind == binding.kind && buffer.logical == binding.binding
                    }) {
                        buffers.push(BufferSlot {
                            logical: binding.binding,
                            slot,
                            kind: binding.kind,
                        });
                    }
                }
            }
        }
        self.check()?;
        Ok(buffers)
    }
    fn reflect_program(
        &self,
        program: glow::NativeProgram,
        desc: &RenderPipelineDescriptor,
        shaders: &[&GlslShader],
    ) -> Result<Pipeline> {
        let buffers = self.reflect_buffers(program, shaders)?;
        let mut textures = Vec::new();
        let mut texture_names = HashMap::new();
        // SAFETY: reflection queries and assignments address this live linked native program.
        unsafe {
            self.gl.use_program(Some(program));
            for shader in shaders {
                for binding in &shader.textures {
                    let Some(location) = self.gl.get_uniform_location(program, &binding.name)
                    else {
                        continue;
                    };
                    let next = textures.len() as u32;
                    let unit = *texture_names.entry(binding.name.clone()).or_insert(next);
                    if unit
                        >= self
                            .gl
                            .get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)
                            as u32
                    {
                        return Err(Error::Unavailable(
                            "OpenGL program exceeds texture units".into(),
                        ));
                    }
                    self.gl.uniform_1_i32(Some(&location), unit as i32);
                    if unit == next {
                        textures.push(TextureSlot {
                            logical: binding.texture,
                            sampler: binding.sampler,
                            unit,
                        });
                    }
                }
            }
            // Storage images live in their own unit namespace and need an immutable
            // format plus an access qualifier, both taken from the WGSL declaration.
            let mut images = Vec::new();
            for shader in shaders {
                for binding in &shader.images {
                    let Some(location) = self.gl.get_uniform_location(program, &binding.name)
                    else {
                        continue;
                    };
                    let unit = images.len() as u32;
                    if unit >= self.gl.get_parameter_i32(glow::MAX_IMAGE_UNITS) as u32 {
                        return Err(Error::Unavailable(
                            "OpenGL program exceeds native image units".into(),
                        ));
                    }
                    self.gl.uniform_1_i32(Some(&location), unit as i32);
                    if !images
                        .iter()
                        .any(|image: &ImageSlot| image.logical == binding.binding)
                    {
                        images.push(ImageSlot {
                            logical: binding.binding,
                            unit,
                            format: binding.format,
                            read_only: binding.read_only,
                        });
                    }
                }
            }
            let first_instance = self
                .gl
                .get_uniform_location(program, "naga_vs_first_instance");
            self.check()?;
            Ok(Pipeline {
                native: program,
                desc: desc.clone(),
                buffers,
                textures,
                images,
                first_instance,
            })
        }
    }
}

impl OpenGlDevice {
    /// Reflects sampler/texture units for a linked program.
    pub(crate) fn reflect_textures(
        &self,
        program: glow::NativeProgram,
        shaders: &[&GlslShader],
    ) -> Result<Vec<TextureSlot>> {
        let mut textures = Vec::new();
        let mut texture_names = HashMap::new();
        // SAFETY: reflection queries and assignments address this live linked native program.
        unsafe {
            for shader in shaders {
                for binding in &shader.textures {
                    let Some(location) = self.gl.get_uniform_location(program, &binding.name)
                    else {
                        continue;
                    };
                    let next = textures.len() as u32;
                    let unit = *texture_names.entry(binding.name.clone()).or_insert(next);
                    if unit
                        >= self
                            .gl
                            .get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)
                            as u32
                    {
                        return Err(Error::Unavailable(
                            "OpenGL program exceeds texture units".into(),
                        ));
                    }
                    self.gl.uniform_1_i32(Some(&location), unit as i32);
                    if unit == next {
                        textures.push(TextureSlot {
                            logical: binding.texture,
                            sampler: binding.sampler,
                            unit,
                        });
                    }
                }
            }
        }
        self.check()?;
        Ok(textures)
    }

    /// Reflects storage image units for a linked program.
    pub(crate) fn reflect_images(
        &self,
        program: glow::NativeProgram,
        shaders: &[&GlslShader],
    ) -> Result<Vec<ImageSlot>> {
        let mut images = Vec::new();
        // SAFETY: reflection queries and assignments address this live linked native program.
        unsafe {
            for shader in shaders {
                for binding in &shader.images {
                    let Some(location) = self.gl.get_uniform_location(program, &binding.name)
                    else {
                        continue;
                    };
                    let unit = images.len() as u32;
                    if unit >= self.gl.get_parameter_i32(glow::MAX_IMAGE_UNITS) as u32 {
                        return Err(Error::Unavailable(
                            "OpenGL program exceeds native image units".into(),
                        ));
                    }
                    self.gl.uniform_1_i32(Some(&location), unit as i32);
                    if !images
                        .iter()
                        .any(|image: &ImageSlot| image.logical == binding.binding)
                    {
                        images.push(ImageSlot {
                            logical: binding.binding,
                            unit,
                            format: binding.format,
                            read_only: binding.read_only,
                        });
                    }
                }
            }
        }
        self.check()?;
        Ok(images)
    }
}
