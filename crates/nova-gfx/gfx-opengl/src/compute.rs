//! Buffer compute and native draw-parameter support for desktop GL.

use crate::{
    device::{OpenGlDevice, native},
    pipelines::BufferSlot,
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
        dispatch.validate(self.compute.capabilities)?;
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
