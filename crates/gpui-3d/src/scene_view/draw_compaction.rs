//! Opt-in ordered GPU argument compaction for long identical-state runs.
//! Preserves exact command ordering (including blending) and first_instance.
//! No readback, per-frame native allocation, or CPU fence waits.
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext};
use crate::NativeOffscreenPass;
use std::sync::Arc;

const MAX_RUN: usize = 256;
const MIN_RUN: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    command: usize,
    // Index into the immutable gather list, not into the argument buffer.
    source: u32,
    count: u32,
    destination: u32,
}

struct Plan {
    source: BufferId,
    source_bytes: u64,
    records: u32,
    runs: Vec<Run>,
    gather: Vec<u32>,
}

fn plan(commands: &[RenderCommand], limit: u32) -> Option<Plan> {
    let max = MAX_RUN.min(limit as usize);
    if max < MIN_RUN || commands.len() < MIN_RUN { return None; }
    let mut source = None;
    let mut source_bytes = 0;
    let mut records = 0u32;
    let mut runs = Vec::new();
    let mut gather = Vec::new();
    let mut index = 0;
    while index < commands.len() {
        let RenderCommand::DrawIndirect(first) = &commands[index] else {
            index += 1;
            continue;
        };
        if first.offset % 16 != 0 || source.is_some_and(|id| id != first.arguments) {
            index += 1;
            continue;
        }
        let mut len = 1usize;
        while len < max {
            let Some(RenderCommand::DrawIndirect(next)) = commands.get(index + len) else { break; };
            if next.arguments != first.arguments || next.pipeline != first.pipeline
                || next.bind_groups != first.bind_groups || next.scissor != first.scissor
                || next.offset % 16 != 0
            { break; }
            len += 1;
        }
        if len >= MIN_RUN {
            let start = records;
            records = records.checked_add(len as u32)?;
            source = Some(first.arguments);
            for command in &commands[index..index + len] {
                let RenderCommand::DrawIndirect(draw) = command else { unreachable!() };
                let slot = u32::try_from(draw.offset / 16).ok()?;
                source_bytes = source_bytes.max((u64::from(slot) + 1) * 16);
                gather.push(slot);
            }
            runs.push(Run { command: index, source: start, count: len as u32, destination: start });
        }
        index += len;
    }
    Some(Plan { source: source?, source_bytes, records, runs, gather })
}

struct Scratch {
    buffers: [BufferId; 3],
    sizes: [u64; 3],
}

struct Prepared {
    commands: Vec<RenderCommand>,
    counts: Vec<IndirectCountBinding>,
    compute: [ComputeBufferBinding; 4],
    groups: u32,
}

// Strong Arc identity prevents address reuse and avoids hashing/scanning
// the immutable command list on a camera-only update. One bounded cache
// also retains the negative result for non-compaction-friendly scenes.
struct Cached {
    source: Arc<[RenderCommand]>,
    prepared: Option<Prepared>,
}

pub(super) struct Compactor {
    shader: ShaderModuleId,
    scratch: Option<Scratch>,
    cached: Option<Cached>,
}

impl Compactor {
    pub(super) fn new(device: &mut dyn ExtensionDevice, context: &RendererExtensionContext) -> Option<Self> {
        // Unvalidated driver cost: preserve default behavior until benchmarked.
        if context.backend_kind() != BackendKind::Vulkan
            || std::env::var_os("GPUI_3D_DRAW_COMPACTION").as_deref() != Some(std::ffi::OsStr::new("1"))
            || device.counted_offscreen_limit().is_none()
        { return None; }
        let caps = device.compute_capabilities();
        if caps.max_invocations_per_workgroup < 64 || caps.max_workgroup_size[0] < 64
            || caps.max_workgroup_shared_memory_bytes < 256
        { return None; }
        let shader = super::resources::embedded_shader(context, ShaderStage::Compute, "cs_compact_draws")
            .ok().and_then(|binary| device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d draw compaction".into()), binary,
            }).ok())?;
        Some(Self { shader, scratch: None, cached: None })
    }

    fn reserve(&mut self, device: &mut dyn ExtensionDevice, sizes: [u64; 3]) -> Result<[BufferId; 3]> {
        if let Some(scratch) = &self.scratch {
            if scratch.sizes.iter().zip(sizes).all(|(&capacity, size)| capacity >= size) {
                return Ok(scratch.buffers);
            }
        }
        let sizes = sizes.map(|size| size.next_power_of_two());
        let usages = [
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
        ];
        let mut allocated = Vec::new();
        for (size, usage) in sizes.into_iter().zip(usages) {
            match device.create_buffer(&BufferDescriptor {
                label: Some("gpui-3d compact indirect scratch".into()),
                size, usage, memory_location: MemoryLocation::GpuOnly,
            }) {
                Ok(buffer) => allocated.push(buffer),
                Err(error) => {
                    for buffer in allocated { let _ = device.destroy_buffer(buffer); }
                    return Err(error);
                }
            }
        }
        let buffers: [BufferId; 3] = allocated.try_into().expect("three compaction buffers");
        let old = self.scratch.replace(Scratch { buffers, sizes });
        if let Some(old) = old {
            for buffer in old.buffers { device.destroy_buffer(buffer)?; }
        }
        Ok(buffers)
    }

    fn prepare(&mut self, device: &mut dyn ExtensionDevice, pass: &NativeOffscreenPass) -> Result<Option<Prepared>> {
        let Some(limit) = device.counted_offscreen_limit() else { return Ok(None); };
        let Some(plan) = plan(&pass.commands, limit) else { return Ok(None); };
        let group_count = u32::try_from(plan.runs.len()).map_err(|_| Error::InvalidInput("compaction group count overflow".into()))?;
        if group_count > device.compute_capabilities().max_workgroups[0] { return Ok(None); }
        let header_words = group_count.checked_mul(4)
            .ok_or_else(|| Error::InvalidInput("compaction metadata header overflow".into()))?;
        let total_words = header_words.checked_add(plan.records)
            .ok_or_else(|| Error::InvalidInput("compaction metadata size overflow".into()))?;
        let sizes = [u64::from(total_words) * 4, u64::from(plan.records) * 16, u64::from(group_count) * 4];
        let limits = device.buffer_binding_limits();
        if limits.validate_range(0, plan.source_bytes, false).is_err()
            || sizes.iter().any(|&size| limits.validate_range(0, size, false).is_err())
        { return Ok(None); }
        let [metadata, output, counts] = self.reserve(device, sizes)?;
        let mut bytes = Vec::with_capacity(total_words as usize * 4);
        for run in &plan.runs {
            for word in [header_words + run.source, run.count, run.destination, 0] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        for slot in plan.gather {
            bytes.extend_from_slice(&slot.to_le_bytes());
        }
        device.write_buffer(metadata, 0, &bytes)?;
        let binding = |binding, buffer, size, access| ComputeBufferBinding {
            binding, buffer: BufferBinding { buffer, offset: 0, size, stride: None }, access,
        };
        let compute = [
            binding(0, plan.source, plan.source_bytes, ComputeBufferAccess::ReadOnly),
            binding(1, metadata, sizes[0], ComputeBufferAccess::ReadOnly),
            binding(2, output, sizes[1], ComputeBufferAccess::ReadWrite),
            binding(3, counts, sizes[2], ComputeBufferAccess::ReadWrite),
        ];
        let mut commands = Vec::with_capacity(pass.commands.len() - plan.records as usize + plan.runs.len());
        let mut bindings = Vec::with_capacity(plan.runs.len());
        let mut cursor = 0;
        for (index, run) in plan.runs.iter().enumerate() {
            commands.extend_from_slice(&pass.commands[cursor..run.command]);
            let RenderCommand::DrawIndirect(mut draw) = pass.commands[run.command].clone() else { unreachable!() };
            draw.arguments = output;
            draw.offset = u64::from(run.destination) * 16;
            bindings.push(IndirectCountBinding {
                command_index: commands.len(), count_buffer: counts,
                count_offset: index as u64 * 4, max_draw_count: run.count,
            });
            commands.push(RenderCommand::DrawIndirect(draw));
            cursor = run.command + run.count as usize;
        }
        commands.extend_from_slice(&pass.commands[cursor..]);
        Ok(Some(Prepared { commands, counts: bindings, compute, groups: group_count }))
    }

    pub(super) fn submit(&mut self, device: &mut dyn ExtensionDevice, pass: &NativeOffscreenPass) -> Result<bool> {
        if !self.cached.as_ref().is_some_and(|cached| Arc::ptr_eq(&cached.source, &pass.commands)) {
            // Invalidate before preparing: a failed allocation or upload may
            // have replaced scratch storage referenced by the previous plan.
            self.cached = None;
            let prepared = self.prepare(device, pass)?;
            self.cached = Some(Cached { source: Arc::clone(&pass.commands), prepared });
        }
        let Some(prepared) = self.cached.as_ref().and_then(|cached| cached.prepared.as_ref()) else {
            return Ok(false);
        };
        // Culling, compaction and counted drawing share the ordered owner queue.
        // The metadata is immutable until the next command snapshot.
        device.dispatch_compute(&ComputeDispatch {
            shader: self.shader, bindings: &prepared.compute, workgroups: [prepared.groups, 1, 1],
        })?;
        let mut native = pass.commands();
        native.commands = &prepared.commands;
        device.submit_counted_offscreen_pass(&native, &prepared.counts)?;
        Ok(true)
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> Result<()> {
        if let Some(scratch) = self.scratch {
            for buffer in scratch.buffers { device.destroy_buffer(buffer)?; }
        }
        device.destroy_shader_module(self.shader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(offset: u64) -> RenderCommand {
        RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1), bind_groups: bind_group_list([]),
            arguments: BufferId::from_parts(1, 1), offset, scissor: None,
        })
    }

    #[test]
    fn gathers_noncontiguous_runs_without_reordering() {
        let mut commands: Vec<_> = (0..300).map(|i| draw(i * 16)).collect();
        commands.extend((400..560).map(|i| draw(i * 16)));
        let plan = plan(&commands, 256).unwrap();
        assert_eq!(plan.runs, [
            Run { command: 0, source: 0, count: 256, destination: 0 },
            Run { command: 256, source: 256, count: 204, destination: 256 },
        ]);
        assert_eq!(plan.records, 460);
        assert_eq!(&plan.gather[298..303], &[298, 299, 400, 401, 402]);
        assert_eq!(plan.source_bytes, 560 * 16);
    }

    #[test]
    fn preserves_reverse_order_and_rejects_small_runs() {
        let commands: Vec<_> = (0..128).rev().map(|i| draw(i * 16)).collect();
        let gathered = plan(&commands, 256).unwrap();
        assert_eq!(gathered.gather, (0..128).rev().collect::<Vec<u32>>());
        assert!(plan(&commands[..127], 256).is_none());
        assert!(plan(&commands, 64).is_none());
    }

    #[test]
    fn never_merges_distinct_scissors() {
        let mut commands: Vec<_> = (0..256).map(|i| draw(i * 16)).collect();
        for command in &mut commands[128..] {
            command.set_scissor(Some(ScissorRect { x: 1, y: 2, width: 3, height: 4 }));
        }
        let gathered = plan(&commands, 256).unwrap();
        assert_eq!(gathered.runs.len(), 2);
        assert_eq!(gathered.runs[1].command, 128);
    }
}
