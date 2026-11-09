//! Buffer compute and native draw-parameter support for desktop GL.

use crate::{
    device::{OpenGlDevice, native},
    pipelines::{BufferSlot, ImageSlot, TextureSlot},
};
use gfx_core::*;
use glow::HasContext as _;
use std::{borrow::Cow, collections::HashMap};

pub(crate) struct State {
    pub capabilities: ComputeCapabilities,
    programs: HashMap<ShaderModuleId, Program>,
}
struct Program {
    native: glow::NativeProgram,
    buffers: Vec<BufferSlot>,
    textures: Vec<TextureSlot>,
    images: Vec<ImageSlot>,
}

impl State {
    pub(crate) fn new(gl: &glow::Context) -> Self {
        // SAFETY: the owning GL4.5 context is current during device construction.
        let max_workgroups = unsafe {
            std::array::from_fn(|i| {
                gl.get_parameter_indexed_i32(glow::MAX_COMPUTE_WORK_GROUP_COUNT, i as u32)
                    .max(0) as u32
            })
        };
        Self {
            capabilities: ComputeCapabilities {
                max_workgroups,
                max_invocations_per_workgroup: unsafe {
                    gl.get_parameter_i32(glow::MAX_COMPUTE_WORK_GROUP_INVOCATIONS)
                        .max(0) as u32
                },
                max_workgroup_size: unsafe {
                    std::array::from_fn(|i| {
                        gl.get_parameter_indexed_i32(glow::MAX_COMPUTE_WORK_GROUP_SIZE, i as u32)
                            .max(0) as u32
                    })
                },
                max_workgroup_shared_memory_bytes: unsafe {
                    gl.get_parameter_i32(glow::MAX_COMPUTE_SHARED_MEMORY_SIZE)
                        .max(0) as u32
                },
                indirect_draw: gl
                    .supported_extensions()
                    .contains("GL_ARB_shader_draw_parameters"),
            },
            programs: HashMap::new(),
        }
    }
    pub(crate) fn remove(&mut self, gl: &glow::Context, id: ShaderModuleId) {
        if let Some(program) = self.programs.remove(&id) {
            // SAFETY: the context is current; GL retains pending command references.
            unsafe { gl.delete_program(program.native) };
        }
    }
    pub(crate) fn clear(&mut self, gl: &glow::Context) {
        for (_, program) in self.programs.drain() {
            // SAFETY: device cleanup made its context current and completed pending work.
            unsafe { gl.delete_program(program.native) };
        }
    }
}

impl OpenGlDevice {
    pub(crate) fn dispatch_buffers(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        dispatch.validate_with_limits(self.compute.capabilities, self.buffer_binding_limits())?;
        self.park()?;
        let shader = self.shaders.get(dispatch.shader)?;
        if shader.stage != ShaderStage::Compute || !shader.reflection.textures.is_empty() {
            return Err(Error::InvalidInput(
                "OpenGL buffer compute requires a buffer-only compute shader".into(),
            ));
        }
        for binding in dispatch.bindings {
            let buffer = self.buffers.get(binding.buffer.buffer)?;
            let uniform = binding.access == ComputeBufferAccess::Uniform;
            let alignment = if uniform {
                self.uniform_alignment
            } else {
                self.storage_alignment
            }
            .max(1);
            let usage = if uniform {
                BufferUsage::UNIFORM
            } else {
                BufferUsage::STORAGE
            };
            if !buffer.desc.usage.contains(usage)
                || (binding.access == ComputeBufferAccess::ReadWrite
                    && !buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE))
                || binding.buffer.size == 0
                || binding.buffer.offset % alignment != 0
                || binding
                    .buffer
                    .offset
                    .checked_add(binding.buffer.size)
                    .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "invalid OpenGL compute buffer range or usage".into(),
                ));
            }
        }
        if dispatch.workgroups.contains(&0) {
            return Ok(());
        }
        if !self.compute.programs.contains_key(&dispatch.shader) {
            // SAFETY: the current context owns the shader and the newly linked program.
            let program = unsafe {
                let program = self.gl.create_program().map_err(native)?;
                self.gl.attach_shader(program, shader.native);
                self.gl.link_program(program);
                if !self.gl.get_program_link_status(program) {
                    let message = self.gl.get_program_info_log(program);
                    self.gl.delete_program(program);
                    return Err(Error::Shader(message));
                }
                program
            };
            let buffers = match self.reflect_buffers(program, &[&shader.reflection]) {
                Ok(buffers) => buffers,
                Err(error) => {
                    // SAFETY: linking failed before any dispatch referenced this program.
                    unsafe { self.gl.delete_program(program) };
                    return Err(error);
                }
            };
            self.compute.programs.insert(
                dispatch.shader,
                Program {
                    native: program,
                    buffers,
                    textures: Vec::new(),
                    images: Vec::new(),
                },
            );
        }
        let program = &self.compute.programs[&dispatch.shader];
        // SAFETY: all bindings were checked before any dispatch, and the context is current.
        unsafe {
            for slot in &program.buffers {
                let binding = dispatch
                    .bindings
                    .iter()
                    .find(|binding| binding.binding == slot.logical)
                    .ok_or_else(|| {
                        Error::InvalidInput("missing OpenGL compute buffer binding".into())
                    })?;
                let uniform = slot.kind == BindingType::UniformBuffer;
                if uniform != (binding.access == ComputeBufferAccess::Uniform) {
                    return Err(Error::InvalidInput(
                        "OpenGL compute binding type mismatch".into(),
                    ));
                }
                let buffer = self.buffers.get(binding.buffer.buffer)?;
                self.gl.bind_buffer_range(
                    if uniform {
                        glow::UNIFORM_BUFFER
                    } else {
                        glow::SHADER_STORAGE_BUFFER
                    },
                    slot.slot,
                    Some(buffer.native),
                    i32::try_from(binding.buffer.offset).map_err(native)?,
                    i32::try_from(binding.buffer.size).map_err(native)?,
                );
            }
            self.gl.use_program(Some(program.native));
            self.gl.dispatch_compute(
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
            );
            self.gl.memory_barrier(
                glow::SHADER_STORAGE_BARRIER_BIT
                    | glow::COMMAND_BARRIER_BIT
                    | glow::BUFFER_UPDATE_BARRIER_BIT
                    | glow::UNIFORM_BARRIER_BIT
                    | glow::VERTEX_ATTRIB_ARRAY_BARRIER_BIT
                    | glow::ELEMENT_ARRAY_BARRIER_BIT,
            );
            self.gl.use_program(None);
        }
        self.check()
    }
}

pub(crate) fn native_instance_source(source: &str) -> Cow<'_, str> {
    const UNIFORM: &str = "uniform uint naga_vs_first_instance;";
    if !source.contains(UNIFORM) {
        return Cow::Borrowed(source);
    }
    let source = source.replace(
        UNIFORM,
        "#define naga_vs_first_instance uint(gl_BaseInstanceARB)",
    );
    let Some(newline) = source.find('\n') else {
        return Cow::Owned(source);
    };
    Cow::Owned(format!(
        "{}\n#extension GL_ARB_shader_draw_parameters : require\n{}",
        &source[..newline],
        &source[newline + 1..]
    ))
}

impl OpenGlDevice {
    /// Dispatches a compute shader with a full resource set bound.
    ///
    /// The buffer-only [`crate::compute::State::dispatch_buffers`] path cannot express
    /// textures or storage images; this one binds the whole set, so a compute shader can
    /// write a storage image.
    pub(crate) fn dispatch_bind_group(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
    ) -> Result<()> {
        dispatch.validate(self.compute.capabilities)?;
        // Validate the resource-set's compute visibility before mutating GL
        // state. A vertex/fragment-only set is not a compute descriptor set.
        let set = self.sets.get(dispatch.bind_group)?;
        let layout = self.layouts.get(set.desc.layout)?;
        set.desc.validate_against(layout)?;
        for entry in &layout.entries {
            if !entry.stages.contains(ShaderStages::COMPUTE) {
                return Err(Error::InvalidInput(format!(
                    "binding {} is not visible to compute shaders",
                    entry.binding
                )));
            }
        }
        self.park()?;
        let shader = self.shaders.get(dispatch.shader)?;
        if shader.stage != ShaderStage::Compute {
            return Err(Error::InvalidInput(
                "OpenGL resource-set dispatch requires a compute shader".into(),
            ));
        }
        if !self.compute.programs.contains_key(&dispatch.shader) {
            // SAFETY: the current context owns the shader and the newly linked program.
            let program = unsafe {
                let program = self.gl.create_program().map_err(native)?;
                self.gl.attach_shader(program, shader.native);
                self.gl.link_program(program);
                if !self.gl.get_program_link_status(program) {
                    let message = self.gl.get_program_info_log(program);
                    self.gl.delete_program(program);
                    return Err(Error::Shader(message));
                }
                program
            };
            // SAFETY: the freshly linked program belongs to this context; uniform locations
            // and image/texture units require it to be current before reflection.
            unsafe { self.gl.use_program(Some(program)) };
            let reflected = (|| {
                Ok::<_, Error>((
                    self.reflect_buffers(program, &[&shader.reflection])?,
                    self.reflect_textures(program, &[&shader.reflection])?,
                    self.reflect_images(program, &[&shader.reflection])?,
                ))
            })();
            match reflected {
                Ok((buffers, textures, images)) => {
                    self.compute.programs.insert(
                        dispatch.shader,
                        Program {
                            native: program,
                            buffers,
                            textures,
                            images,
                        },
                    );
                }
                Err(error) => {
                    // SAFETY: linking/reflection failed before any dispatch used this program.
                    unsafe { self.gl.delete_program(program) };
                    return Err(error);
                }
            }
        }
        if dispatch.workgroups.contains(&0) {
            return Ok(());
        }
        let set = self.sets.get(dispatch.bind_group)?;
        let program = &self.compute.programs[&dispatch.shader];
        let resource = |logical: u32| -> Result<BindingResource> {
            set.desc
                .entries
                .iter()
                .find(|binding| binding.binding == logical)
                .map(|binding| binding.resource)
                .ok_or_else(|| Error::InvalidInput(format!("OpenGL missing resource {logical}")))
        };
        // SAFETY: every binding is validated before the dispatch; the context is current.
        unsafe {
            for slot in &program.buffers {
                let BindingResource::Buffer(range) = resource(slot.logical)? else {
                    return Err(Error::InvalidInput(
                        "OpenGL compute buffer bound to a non-buffer".into(),
                    ));
                };
                let buffer = self.buffers.get(range.buffer)?;
                let layout_entry = layout.entries.iter().find(|entry| entry.binding == slot.logical)
                    .ok_or_else(|| Error::InvalidInput("OpenGL compute buffer missing layout entry".into()))?;
                if layout_entry.binding_type == BindingType::StorageBufferReadWrite
                    && !buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE)
                {
                    return Err(Error::InvalidInput(
                        "OpenGL writable compute storage lacks STORAGE_WRITE".into(),
                    ));
                }
                let uniform = slot.kind == BindingType::UniformBuffer;
                if !buffer.desc.usage.contains(if uniform {
                    BufferUsage::UNIFORM
                } else {
                    BufferUsage::STORAGE
                }) {
                    return Err(Error::InvalidInput(
                        "OpenGL compute buffer lacks its usage".into(),
                    ));
                }
                self.gl.bind_buffer_range(
                    if uniform {
                        glow::UNIFORM_BUFFER
                    } else {
                        glow::SHADER_STORAGE_BUFFER
                    },
                    slot.slot,
                    Some(buffer.native),
                    i32::try_from(range.offset).map_err(native)?,
                    i32::try_from(range.size).map_err(native)?,
                );
            }
            for slot in &program.textures {
                let BindingResource::Texture(texture) = resource(slot.logical)? else {
                    return Err(Error::InvalidInput(
                        "OpenGL compute texture bound to a non-texture".into(),
                    ));
                };
                let view = self.views.get(texture.texture_view)?;
                if !view.usage.contains(TextureUsage::SAMPLED) {
                    return Err(Error::InvalidInput(
                        "OpenGL compute texture view lacks SAMPLED".into(),
                    ));
                }
                let sampler = match slot.sampler {
                    Some(logical) => {
                        let BindingResource::Sampler(binding) = resource(logical)? else {
                            return Err(Error::InvalidInput(
                                "OpenGL compute sampler bound to a non-sampler".into(),
                            ));
                        };
                        Some(*self.samplers.get(binding.sampler)?)
                    }
                    None => None,
                };
                self.gl.active_texture(glow::TEXTURE0 + slot.unit);
                self.gl.bind_texture(view.target, Some(view.native));
                self.gl.bind_sampler(slot.unit, sampler);
            }
            for slot in &program.images {
                let BindingResource::Texture(texture) = resource(slot.logical)? else {
                    return Err(Error::InvalidInput(
                        "OpenGL compute storage image bound to a non-texture".into(),
                    ));
                };
                let view = self.views.get(texture.texture_view)?;
                if !view.usage.contains(TextureUsage::STORAGE) || view.format != slot.format {
                    return Err(Error::InvalidInput(
                        "OpenGL compute storage image view mismatches the shader".into(),
                    ));
                }
                let (internal, _, _) = crate::resources::format(slot.format);
                self.gl.bind_image_texture(
                    slot.unit,
                    Some(view.native),
                    0,
                    false,
                    0,
                    if slot.read_only {
                        glow::READ_ONLY
                    } else {
                        glow::READ_WRITE
                    },
                    internal,
                );
            }
            self.gl.use_program(Some(program.native));
            self.gl.dispatch_compute(
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
            );
            self.gl.memory_barrier(
                glow::SHADER_STORAGE_BARRIER_BIT
                    | glow::SHADER_IMAGE_ACCESS_BARRIER_BIT
                    | glow::TEXTURE_FETCH_BARRIER_BIT
                    | glow::COMMAND_BARRIER_BIT
                    | glow::BUFFER_UPDATE_BARRIER_BIT,
            );
        }
        self.check()
    }
}
