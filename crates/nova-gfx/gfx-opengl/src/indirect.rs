//! GPU-side translation of portable indexed arguments to GL's absolute
//! element-buffer addressing. Zero-offset draws bypass this path entirely.
use crate::device::{OpenGlDevice, native};
use gfx_core::{Error, Result};
use glow::HasContext as _;

const SOURCE: &str = r#"#version 450 core
layout(local_size_x = 64) in;
layout(std430, binding = 0) readonly buffer InputArguments { uint src[]; };
layout(std430, binding = 1) writeonly buffer OutputArguments { uint dst[]; };
layout(location = 0) uniform uint source_word;
layout(location = 1) uniform uint records;
layout(location = 2) uniform uint index_base;
layout(location = 3) uniform uint available;
void main() {
    uint i = gl_GlobalInvocationID.x;
    if (i >= records) return;
    uint s = source_word + 5u * i;
    uint d = 5u * i;
    uint count = src[s];
    uint first = src[s + 2u];
    bool valid = first <= available && count <= available - min(first, available)
        && first <= 0xffffffffu - index_base;
    dst[d] = valid ? count : 0u;
    dst[d + 1u] = src[s + 1u];
    dst[d + 2u] = valid ? first + index_base : 0u;
    dst[d + 3u] = src[s + 3u];
    dst[d + 4u] = src[s + 4u];
}
"#;

pub(crate) struct RebasePlan {
    source_offset: i32,
    source_size: i32,
    source_word: u32,
    records: u32,
    index_base: u32,
    available: u32,
    output_size: i32,
}

impl OpenGlDevice {
    pub(crate) fn index_rebase_plan(&self, offset: u64, argument_size: u64, index_offset: u64,
        index_size: u64, index_stride: u64, records: u32) -> Result<RebasePlan> {
        let invalid = || Error::InvalidInput("OpenGL indexed argument rebasing range exceeds native limits".into());
        if offset % 4 != 0 || index_stride == 0 || index_offset % index_stride != 0
            || index_offset >= index_size || records == 0
            || records.div_ceil(64) > self.compute.capabilities.max_workgroups[0] {
            return Err(invalid());
        }
        let alignment = self.storage_alignment.max(4);
        let base = offset / alignment * alignment;
        let length = u64::from(records) * 20;
        let size = offset.checked_sub(base).and_then(|prefix| prefix.checked_add(length)).ok_or_else(invalid)?;
        if base.checked_add(size).is_none_or(|end| end > argument_size)
            || size > self.binding_limits.max_storage_binding_bytes
            || length > self.binding_limits.max_storage_binding_bytes {
            return Err(invalid());
        }
        Ok(RebasePlan {
            source_offset: i32::try_from(base).map_err(|_| invalid())?,
            source_size: i32::try_from(size).map_err(|_| invalid())?,
            source_word: u32::try_from((offset - base) / 4).map_err(|_| invalid())?,
            records,
            index_base: u32::try_from(index_offset / index_stride).map_err(|_| invalid())?,
            available: u32::try_from((index_size - index_offset) / index_stride).unwrap_or(u32::MAX),
            output_size: i32::try_from(length).map_err(|_| invalid())?,
        })
    }
}

#[derive(Default)]
pub(crate) struct IndexRebase {
    program: Option<glow::NativeProgram>,
    uniforms: [Option<glow::NativeUniformLocation>; 4],
    scratch: Option<(glow::NativeBuffer, i32)>,
}

impl IndexRebase {
    pub(crate) fn encode(&mut self, gl: &glow::Context, source: glow::NativeBuffer, plan: RebasePlan) -> Result<glow::NativeBuffer> {
        // SAFETY: owner context is current; all ranges were validated against
        // allocation and SSBO limits. The original argument buffer is read-only.
        unsafe {
            let program = if let Some(program) = self.program { program } else {
                let shader = gl.create_shader(glow::COMPUTE_SHADER).map_err(native)?;
                gl.shader_source(shader, SOURCE);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let error = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    return Err(native(error));
                }
                let program = match gl.create_program() {
                    Ok(program) => program,
                    Err(error) => { gl.delete_shader(shader); return Err(native(error)); }
                };
                gl.attach_shader(program, shader);
                gl.link_program(program);
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
                if !gl.get_program_link_status(program) {
                    let error = gl.get_program_info_log(program);
                    gl.delete_program(program);
                    return Err(native(error));
                }
                self.uniforms = ["source_word", "records", "index_base", "available"].map(|name| gl.get_uniform_location(program, name));
                if self.uniforms.iter().any(Option::is_none) {
                    gl.delete_program(program);
                    return Err(Error::Backend("OpenGL index rebase uniform unavailable".into()));
                }
                self.program = Some(program);
                program
            };
            let scratch = match self.scratch {
                Some((buffer, capacity)) if capacity >= plan.output_size => buffer,
                _ => {
                    let buffer = gl.create_buffer().map_err(native)?;
                    gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer));
                    // Geometric capacity avoids reallocating for every new
                    // high-water mark; the bound SSBO range remains exact.
                    let capacity = u32::try_from(plan.output_size).unwrap_or(0)
                        .checked_next_power_of_two().and_then(|size| i32::try_from(size).ok())
                        .unwrap_or(plan.output_size);
                    gl.buffer_data_size(glow::COPY_WRITE_BUFFER, capacity, glow::DYNAMIC_COPY);
                    gl.bind_buffer(glow::COPY_WRITE_BUFFER, None);
                    let error = gl.get_error();
                    if error != glow::NO_ERROR {
                        gl.delete_buffer(buffer);
                        return Err(native(format!("index rebase scratch allocation error 0x{error:x}")));
                    }
                    if let Some((old, _)) = self.scratch.replace((buffer, capacity)) {
                        // GL retains storage referenced by earlier commands.
                        gl.delete_buffer(old);
                    }
                    buffer
                }
            };
            // GPU-side ordering before scratch reuse and source reads.
            gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT | glow::COMMAND_BARRIER_BIT);
            gl.use_program(Some(program));
            gl.bind_buffer_range(glow::SHADER_STORAGE_BUFFER, 0, Some(source), plan.source_offset, plan.source_size);
            gl.bind_buffer_range(glow::SHADER_STORAGE_BUFFER, 1, Some(scratch), 0, plan.output_size);
            for (location, value) in self.uniforms.iter().zip([
                plan.source_word, plan.records, plan.index_base, plan.available,
            ]) {
                gl.uniform_1_u32(location.as_ref(), value);
            }
            gl.dispatch_compute(plan.records.div_ceil(64), 1, 1);
            gl.memory_barrier(glow::COMMAND_BARRIER_BIT | glow::SHADER_STORAGE_BARRIER_BIT);
            gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);
            gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 1, None);
            Ok(scratch)
        }
    }

    pub(crate) fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: device destruction has completed all native work.
        unsafe {
            if let Some(program) = self.program.take() { gl.delete_program(program); }
            if let Some((buffer, _)) = self.scratch.take() { gl.delete_buffer(buffer); }
        }
    }
}
