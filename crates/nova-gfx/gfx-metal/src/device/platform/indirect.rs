//! Experimental GPU-generated Metal ICBs for the portable counted-draw ABI.
//! The GPU owns both command contents and the execution range; no readback.
//! Separate compute and render encoders in one retained command buffer
//! establish producer/consumer ordering. No CPU fence wait is introduced.
use super::*;
use gfx_core::IndirectCountBinding;
use objc2_metal::{
    MTLArgumentEncoder, MTLComputeCommandEncoder, MTLComputePipelineState,
    MTLIndirectCommandBuffer, MTLIndirectCommandBufferDescriptor, MTLIndirectCommandType,
    MTLResource, MTLResourceUsage, MTLRenderStages, MTLLanguageVersion, MTLFunction,
};
use objc2_foundation::NSRange;

pub(super) const MAX_COUNT: u32 = 256;

// Match the portable argument ABI (16 or 20 bytes), not the DX12 ABI.
// All buffer bindings use offset zero. Word/element offsets are explicit
// to avoid imposing constant-buffer alignment on the portable 4-byte ABI.
const SOURCE: &str = r#"
#include <metal_stdlib>
#include <metal_indirect_command_buffers>
using namespace metal;
struct IcbTarget { command_buffer commands [[id(0)]]; };
struct Params {
    uint capacity;
    uint argument_word;
    uint count_word;
    uint topology;
    uint index_kind;
    uint index_first;
    uint index_length;
};
kernel void encode_counted(
    constant IcbTarget& target [[buffer(0)]],
    device const uint* arguments [[buffer(1)]],
    device const uint* counter [[buffer(2)]],
    device uint* range [[buffer(3)]],
    device const uint* indices [[buffer(4)]],
    constant Params& p [[buffer(5)]],
    uint tid [[thread_position_in_grid]]) {
    uint count = min(counter[p.count_word], p.capacity);
    if (tid == 0) { range[0] = 0; range[1] = count; }
    if (tid >= count) return;
    uint words = p.index_kind == 0 ? 4 : 5;
    device const uint* a = arguments + p.argument_word + tid * words;
    if (a[0] == 0 || a[1] == 0) return;
    render_command command(target.commands, tid);
    primitive_type primitive = p.topology == 0 ? primitive_type::triangle : primitive_type::triangle_strip;
    if (p.index_kind == 0) {
        command.draw_primitives(primitive, a[2], a[0], a[1], a[3]);
    } else {
        // Avoid out-of-bounds index ranges authored by a GPU producer.
        if (a[2] > p.index_length || a[0] > p.index_length - a[2]) return;
        if (p.index_kind == 1) {
            device const ushort* data = reinterpret_cast<device const ushort*>(indices);
            command.draw_indexed_primitives(primitive, a[0], data + p.index_first + a[2], a[1], as_type<int>(a[3]), a[4]);
        } else {
            command.draw_indexed_primitives(primitive, a[0], indices + p.index_first + a[2], a[1], as_type<int>(a[3]), a[4]);
        }
    }
}
"#;

const MAX_FREE_RUNS: usize = 128;
const MAX_FREE_COMMANDS: u32 = 16_384;

#[derive(Clone)]
pub(super) struct IcbStorage {
    indexed: bool,
    capacity: u32,
    icb: Retained<ProtocolObject<dyn MTLIndirectCommandBuffer>>,
    container: Retained<ProtocolObject<dyn MTLBuffer>>,
    range: Retained<ProtocolObject<dyn MTLBuffer>>,
}

#[derive(Default)]
struct IcbPool {
    free: Vec<IcbStorage>,
    pending: Vec<(Retained<ProtocolObject<dyn MTLCommandBuffer>>, Vec<IcbStorage>)>,
    free_commands: u32,
}

impl IcbPool {
    fn reclaim(&mut self) {
        let mut i = 0;
        while i < self.pending.len() {
            let status = self.pending[i].0.status();
            if !matches!(status, MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error) {
                i += 1;
                continue;
            }
            let (_, storage) = self.pending.swap_remove(i);
            if status != MTLCommandBufferStatus::Completed { continue; }
            for item in storage {
                if self.free.len() < MAX_FREE_RUNS
                    && self.free_commands.saturating_add(item.capacity) <= MAX_FREE_COMMANDS
                {
                    self.free_commands += item.capacity;
                    self.free.push(item);
                }
            }
        }
    }

    fn take(&mut self, indexed: bool, capacity: u32) -> Option<IcbStorage> {
        let index = self.free.iter().position(|item| item.indexed == indexed && item.capacity == capacity)?;
        let item = self.free.swap_remove(index);
        self.free_commands -= item.capacity;
        Some(item)
    }
}

pub(super) struct IcbProgram {
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    encoder: Retained<ProtocolObject<dyn MTLArgumentEncoder>>,
    pool: std::cell::RefCell<IcbPool>,
}

impl IcbProgram {
    pub(super) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(MTLLanguageVersion::Version2_3);
        let library = device.newLibraryWithSource_options_error(&NSString::from_str(SOURCE), Some(&options))
            .map_err(|error| Error::Shader(nserror_message(&error)))?;
        let function = library.newFunctionWithName(&NSString::from_str("encode_counted"))
            .ok_or_else(|| Error::Shader("Metal counted encoder entry unavailable".into()))?;
        let pipeline = device.newComputePipelineStateWithFunction_error(&function)
            .map_err(|error| Error::Shader(nserror_message(&error)))?;
        // SAFETY: buffer 0 is the argument-buffer struct declared above.
        let encoder = unsafe { function.newArgumentEncoderWithBufferIndex(0) };
        Ok(Self { pipeline, encoder, pool: Default::default() })
    }

    pub(super) fn reclaim(&self) { self.pool.borrow_mut().reclaim(); }

    pub(super) fn trim(&self) {
        let mut pool = self.pool.borrow_mut();
        pool.reclaim();
        pool.free.clear();
        pool.free_commands = 0;
    }

    pub(super) fn submitted(&self, commands: Retained<ProtocolObject<dyn MTLCommandBuffer>>, storage: Vec<IcbStorage>) {
        if !storage.is_empty() { self.pool.borrow_mut().pending.push((commands, storage)); }
    }

    fn acquire(&self, device: &ProtocolObject<dyn MTLDevice>, indexed: bool, count: u32) -> Result<IcbStorage> {
        let capacity = count.next_power_of_two();
        if let Some(storage) = self.pool.borrow_mut().take(indexed, capacity) {
            return Ok(storage);
        }
        let descriptor = MTLIndirectCommandBufferDescriptor::new();
        descriptor.setCommandTypes(if indexed { MTLIndirectCommandType::DrawIndexed } else { MTLIndirectCommandType::Draw });
        descriptor.setInheritPipelineState(true);
        descriptor.setInheritBuffers(true);
        // SAFETY: validated nonzero command capacity and native command type.
        let icb = unsafe { device.newIndirectCommandBufferWithDescriptor_maxCommandCount_options(
            &descriptor, capacity as usize, MTLResourceOptions::StorageModePrivate,
        ) }.ok_or_else(|| Error::Backend("Metal ICB allocation failed".into()))?;
        let container = device.newBufferWithLength_options(self.encoder.encodedLength(), MTLResourceOptions::StorageModeShared)
            .ok_or_else(|| Error::Backend("Metal ICB argument buffer allocation failed".into()))?;
        let range = device.newBufferWithLength_options(8, MTLResourceOptions::StorageModePrivate)
            .ok_or_else(|| Error::Backend("Metal ICB range allocation failed".into()))?;
        // SAFETY: the argument buffer owns a stable native ICB reference
        // throughout its pooled lifetime. No per-frame rewriting is needed.
        unsafe {
            self.encoder.setArgumentBuffer_offset(Some(&container), 0);
            self.encoder.setIndirectCommandBuffer_atIndex(Some(&icb), 0);
        }
        Ok(IcbStorage { indexed, capacity, icb, container, range })
    }
}

pub(super) struct CountedRun {
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    storage: IcbStorage,
    arguments: Retained<ProtocolObject<dyn MTLBuffer>>,
    counter: Retained<ProtocolObject<dyn MTLBuffer>>,
    indices: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
    params: [u32; 7],
}

impl MetalDevice {
    pub(super) fn prepare_counted_run(&self, command: &RenderCommand, count: IndirectCountBinding, pipeline: &MetalRenderPipeline) -> Result<CountedRun> {
        let program = self.icb.as_ref().ok_or_else(|| Error::Unavailable("Metal GPU ICB is disabled or unsupported".into()))?;
        let (argument_id, offset, stride, index) = match command {
            RenderCommand::DrawIndirect(draw) => (draw.arguments, draw.offset, 16, None),
            RenderCommand::DrawIndexedIndirect(draw) => (draw.arguments, draw.offset, 20, Some(draw.index_buffer)),
            _ => return Err(Error::InvalidInput("Metal counted draw requires an indirect command".into())),
        };
        let arguments = self.buffers.get(argument_id)?;
        let counter = self.buffers.get(count.count_buffer)?;
        if !arguments.desc.usage.contains(BufferUsage::INDIRECT) || !counter.desc.usage.contains(BufferUsage::INDIRECT) {
            return Err(Error::InvalidInput("Metal counted buffers require INDIRECT usage".into()));
        }
        count.validate_ranges(offset, arguments.desc.size, counter.desc.size, stride, MAX_COUNT)?;
        let word = |offset: u64| u32::try_from(offset / 4).map_err(|_| Error::InvalidInput("Metal counted word offset overflow".into()));
        let argument_word = word(offset)?;
        let count_word = word(count.count_offset)?;
        if argument_word.checked_add(count.max_draw_count * (stride as u32 / 4)).is_none() {
            return Err(Error::InvalidInput("Metal counted argument span overflows shader addressing".into()));
        }
        let (indices, index_kind, index_first, index_length) = if let Some(binding) = index {
            let buffer = self.buffers.get(binding.buffer)?;
            validate_index_buffer_range(buffer.desc.usage, buffer.desc.size, binding, 0, 1)?;
            let size = index_format_size(binding.format);
            let first = u32::try_from(binding.offset / size).map_err(|_| Error::InvalidInput("Metal counted index offset overflow".into()))?;
            let length = u32::try_from((buffer.desc.size - binding.offset) / size).unwrap_or(u32::MAX - first).min(u32::MAX - first);
            (Some(buffer.resource.clone().ok_or_else(|| Error::Backend("missing Metal index buffer".into()))?),
                if size == 2 { 1 } else { 2 }, first, length)
        } else { (None, 0, 0, 0) };
        let storage = program.acquire(&self.device, index.is_some(), count.max_draw_count)?;
        Ok(CountedRun {
            pipeline: program.pipeline.clone(), storage, indices,
            arguments: arguments.resource.clone().ok_or_else(|| Error::Backend("missing Metal argument buffer".into()))?,
            counter: counter.resource.clone().ok_or_else(|| Error::Backend("missing Metal count buffer".into()))?,
            params: [count.max_draw_count, argument_word, count_word,
                u32::from(pipeline.primitive_topology == PrimitiveTopology::TriangleStrip),
                index_kind, index_first, index_length],
        })
    }
}

impl CountedRun {
    pub(super) fn storage(&self) -> IcbStorage { self.storage.clone() }

    pub(super) fn reset(&self, blit: &ProtocolObject<dyn MTLBlitCommandEncoder>) {
        // Reset the complete executable prefix, including invalid/zero
        // records that would otherwise retain an earlier frame's commands.
        unsafe { blit.resetCommandsInBuffer_withRange(&self.storage.icb, NSRange { location: 0, length: self.params[0] as usize }); }
    }

    pub(super) fn encode(&self, compute: &ProtocolObject<dyn MTLComputeCommandEncoder>) {
        compute.setComputePipelineState(&self.pipeline);
        let local = self.pipeline.threadExecutionWidth().max(1).min(self.pipeline.maxTotalThreadsPerThreadgroup());
        // SAFETY: all native buffers and parameter spans were validated and
        // the ICB is marked writable through its indirect argument reference.
        unsafe {
            compute.setBuffer_offset_atIndex(Some(&self.storage.container), 0, 0);
            compute.setBuffer_offset_atIndex(Some(&self.arguments), 0, 1);
            compute.setBuffer_offset_atIndex(Some(&self.counter), 0, 2);
            compute.setBuffer_offset_atIndex(Some(&self.storage.range), 0, 3);
            compute.setBuffer_offset_atIndex(Some(self.indices.as_ref().unwrap_or(&self.arguments)), 0, 4);
            compute.setBytes_length_atIndex(NonNull::from(&self.params).cast::<c_void>(), std::mem::size_of_val(&self.params), 5);
            compute.useResource_usage(ProtocolObject::<dyn MTLResource>::from_ref(&*self.storage.icb), MTLResourceUsage::Write);
        }
        compute.dispatchThreadgroups_threadsPerThreadgroup(
            MTLSize { width: (self.params[0] as usize).div_ceil(local), height: 1, depth: 1 },
            MTLSize { width: local, height: 1, depth: 1 },
        );
    }

    pub(super) fn execute(&self, encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>) {
        // Index buffers are accessed indirectly by ICB commands, rather than
        // by an explicit drawIndexed call. Declare residency on the consumer.
        unsafe {
            if let Some(indices) = &self.indices {
                encoder.useResource_usage_stages(ProtocolObject::<dyn MTLResource>::from_ref(&**indices),
                    MTLResourceUsage::Read, MTLRenderStages::Vertex);
            }
            encoder.executeCommandsInBuffer_indirectBuffer_indirectBufferOffset(&self.storage.icb, &self.storage.range, 0);
        }
    }
}
