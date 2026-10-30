//! Ordered buffer compute and GPU-side expansion of the portable indirect ABI.

use super::*;
use gfx_core::{ComputeBufferAccess, ComputeCapabilities, ComputeDispatch};
use std::collections::HashMap;
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R32_TYPELESS};

#[cfg(test)]
mod tests;

pub(super) struct State {
    programs: HashMap<ShaderModuleId, Arc<Program>>,
    zero: ID3D12Resource,
    indirect_buffers: Vec<Indirect>,
}
struct Program {
    signature: Vec<(u32, u8)>,
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}
type ViewKey = (BufferId, u64, u64, u8);
pub(super) struct Resources {
    slots: Vec<DescriptorSlot>,
    owned_slots: Vec<DescriptorSlot>,
    program: Arc<Program>,
}
impl Resources {
    pub(super) fn release(self, heap: &mut DescriptorHeapAllocator) {
        for slot in self.owned_slots {
            heap.free(slot);
        }
    }
}

pub(super) struct Indirect {
    buffer: ID3D12Resource,
    capacity: u64,
    state: D3D12_RESOURCE_STATES,
    signatures: Vec<ID3D12CommandSignature>,
}

pub(super) fn capabilities() -> ComputeCapabilities {
    ComputeCapabilities {
        max_workgroups: [65535; 3],
        indirect_draw: true,
    }
}

impl State {
    pub(super) fn new(device: &ID3D12Device) -> Result<Self> {
        let zero = create_buffer_resource(
            device,
            &BufferDescriptor {
                label: None,
                size: 256,
                usage: BufferUsage::COPY_SRC,
                memory_location: MemoryLocation::CpuToGpu,
            },
        )?;
        let mut pointer = ptr::null_mut();
        // SAFETY: this upload allocation owns 256 bytes and is initialized before any submission.
        unsafe {
            zero.Map(0, None, Some(&mut pointer)).map_err(dx_error)?;
            ptr::write_bytes(pointer.cast::<u8>(), 0, 256);
            zero.Unmap(0, None);
        }
        Ok(Self {
            programs: HashMap::new(),
            zero,
            indirect_buffers: Vec::new(),
        })
    }
    pub(super) fn remove_shader(&mut self, id: ShaderModuleId) {
        self.programs.remove(&id);
    }

    pub(super) fn recycle_indirect(&mut self, mut resource: Indirect, submitted: bool) {
        if submitted {
            resource.state = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
        }
        resource.signatures.clear();
        if self.indirect_buffers.len() < DX12_FRAME_COMMAND_POOL_CAPACITY {
            self.indirect_buffers.push(resource);
        }
    }

    pub(super) fn trim_indirect(&mut self) {
        self.indirect_buffers.clear();
    }

    pub(super) fn indirect_bytes(&self) -> u64 {
        self.indirect_buffers
            .iter()
            .map(|resource| resource.capacity)
            .sum()
    }

    fn acquire_indirect(&mut self, device: &ID3D12Device, size: u64) -> Result<Indirect> {
        let reusable = self
            .indirect_buffers
            .iter()
            .enumerate()
            .filter(|(_, resource)| resource.capacity >= size)
            .min_by_key(|(_, resource)| resource.capacity)
            .map(|(index, _)| index);
        if let Some(index) = reusable {
            return Ok(self.indirect_buffers.swap_remove(index));
        }
        let capacity = size
            .checked_next_power_of_two()
            .ok_or_else(|| Error::InvalidInput("indirect capacity overflow".into()))?;
        let buffer = create_buffer_resource(
            device,
            &BufferDescriptor {
                label: None,
                size: capacity,
                usage: BufferUsage::COPY_DST | BufferUsage::INDIRECT,
                memory_location: MemoryLocation::GpuOnly,
            },
        )?;
        Ok(Indirect {
            buffer,
            capacity,
            state: D3D12_RESOURCE_STATE_COPY_DEST,
            signatures: Vec::new(),
        })
    }
}

fn dx_error(error: windows::core::Error) -> Error {
    Error::Backend(error.to_string())
}

impl Dx12Device {
    pub(super) fn dispatch_buffers(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffer_batch(std::slice::from_ref(dispatch))
    }
    fn prepare_dispatch(
        &mut self,
        dispatch: &ComputeDispatch<'_>,
        views: &mut HashMap<ViewKey, DescriptorSlot>,
    ) -> Result<Option<Resources>> {
        dispatch.validate(capabilities())?;
        let shader = self.shader_modules.get(dispatch.shader)?;
        if shader.stage != ShaderStage::Compute {
            return Err(Error::InvalidInput(
                "DX12 dispatch requires a compute shader".into(),
            ));
        }
        let mut signature = Vec::with_capacity(dispatch.bindings.len());
        for binding in dispatch.bindings {
            let buffer = self.buffers.get(binding.buffer.buffer)?;
            let uniform = binding.access == ComputeBufferAccess::Uniform;
            let writable = binding.access == ComputeBufferAccess::ReadWrite;
            if !buffer.desc.usage.contains(if uniform {
                BufferUsage::UNIFORM
            } else {
                BufferUsage::STORAGE
            }) || (writable
                && (!buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE)
                    || buffer.desc.memory_location != MemoryLocation::GpuOnly))
                || buffer.desc.memory_location == MemoryLocation::GpuToCpu
                || binding.buffer.size == 0
                || binding.buffer.offset % if uniform { 256 } else { 4 } != 0
                || (!uniform && binding.buffer.size % 4 != 0)
                || (uniform && binding.buffer.size > 65536)
                || binding
                    .buffer
                    .offset
                    .checked_add(binding.buffer.size)
                    .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "invalid DX12 compute buffer range or usage".into(),
                ));
            }
            signature.push((
                binding.binding,
                if uniform {
                    0
                } else if writable {
                    2
                } else {
                    1
                },
            ));
        }
        if signature.len() > 61 {
            return Err(Error::InvalidInput(
                "DX12 compute exceeds root parameter capacity".into(),
            ));
        }
        if dispatch.workgroups.contains(&0) {
            return Ok(None);
        }
        signature.sort_unstable();
        let program = if let Some(program) = self
            .compute
            .programs
            .get(&dispatch.shader)
            .filter(|p| p.signature == signature)
        {
            program.clone()
        } else {
            let program = Arc::new(create_program(&self.device, shader, signature)?);
            self.compute
                .programs
                .insert(dispatch.shader, program.clone());
            program
        };
        let mut resources = Resources {
            slots: Vec::new(),
            owned_slots: Vec::new(),
            program,
        };
        let prepare = (|| {
            for &(number, kind) in &resources.program.signature {
                let binding = dispatch
                    .bindings
                    .iter()
                    .find(|b| b.binding == number)
                    .ok_or_else(|| Error::InvalidInput("missing compute binding".into()))?;
                let buffer = self.buffers.get(binding.buffer.buffer)?;
                let resource = buffer
                    .resource
                    .as_ref()
                    .ok_or_else(|| Error::Backend("compute buffer has no resource".into()))?;
                let key = (
                    binding.buffer.buffer,
                    binding.buffer.offset,
                    binding.buffer.size,
                    kind,
                );
                if let Some(&slot) = views.get(&key) {
                    resources.slots.push(slot);
                    continue;
                }
                let slot = self.resource_heap.allocate()?;
                resources.slots.push(slot);
                resources.owned_slots.push(slot);
                // SAFETY: descriptors address validated ranges in live resources and allocated heap slots.
                unsafe {
                    if kind == 0 {
                        self.device.CreateConstantBufferView(
                            Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC {
                                BufferLocation: resource.GetGPUVirtualAddress()
                                    + binding.buffer.offset,
                                SizeInBytes: align_to_u32(binding.buffer.size, 256)?,
                            }),
                            slot.cpu_handle,
                        );
                    } else if kind == 2 {
                        self.device.CreateUnorderedAccessView(
                            resource,
                            None,
                            Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                                Format: DXGI_FORMAT_R32_TYPELESS,
                                ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                                    Buffer: D3D12_BUFFER_UAV {
                                        FirstElement: binding.buffer.offset / 4,
                                        NumElements: u32::try_from(binding.buffer.size / 4)
                                            .map_err(|e| Error::InvalidInput(e.to_string()))?,
                                        Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                                        ..Default::default()
                                    },
                                },
                            }),
                            slot.cpu_handle,
                        );
                    } else {
                        self.device.CreateShaderResourceView(
                            resource,
                            Some(&raw_buffer_srv_desc(
                                binding.buffer.offset / 4,
                                u32::try_from(binding.buffer.size / 4)
                                    .map_err(|e| Error::InvalidInput(e.to_string()))?,
                            )),
                            slot.cpu_handle,
                        );
                    }
                }
                views.insert(key, slot);
            }
            Ok(())
        })();
        if let Err(error) = prepare {
            resources.release(&mut self.resource_heap);
            return Err(error);
        }
        Ok(Some(resources))
    }
    fn record_dispatch(
        &self,
        encoder: CommandEncoderId,
        call_index: usize,
        dispatch: &ComputeDispatch<'_>,
        states: &mut HashMap<BufferId, D3D12_RESOURCE_STATES>,
    ) -> Result<()> {
        let command = self
            .command_encoders
            .get(encoder)?
            .command_list
            .as_ref()
            .ok_or_else(|| Error::Backend("missing compute command list".into()))?
            .clone();
        let resources = &self.command_encoders.get(encoder)?.compute_resources[call_index];
        // SAFETY: this list is open; descriptors/program stay owned by its fence-retired encoder.
        unsafe {
            command.SetDescriptorHeaps(&[Some(self.resource_heap.heap.clone())]);
            command.SetComputeRootSignature(&resources.program.root);
            command.SetPipelineState(&resources.program.pipeline);
            for (index, slot) in resources.slots.iter().enumerate() {
                command.SetComputeRootDescriptorTable(index as u32, slot.gpu_handle);
            }
            command.SetComputeRoot32BitConstants(
                resources.slots.len() as u32,
                3,
                dispatch.workgroups.as_ptr().cast(),
                0,
            );
        }
        for (index, binding) in dispatch.bindings.iter().enumerate() {
            if dispatch.bindings[..index]
                .iter()
                .any(|previous| previous.buffer.buffer == binding.buffer.buffer)
            {
                continue;
            }
            let buffer = self.buffers.get(binding.buffer.buffer)?;
            let next_state = if binding.access == ComputeBufferAccess::ReadWrite {
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS
            } else {
                D3D12_RESOURCE_STATE_GENERIC_READ
            };
            let state = *states.get(&binding.buffer.buffer).unwrap_or(&buffer.state);
            if state != next_state {
                record_transition_barrier(
                    &command,
                    buffer
                        .resource
                        .as_ref()
                        .ok_or_else(|| Error::Backend("missing compute resource".into()))?,
                    state,
                    next_state,
                );
            }
        }
        // SAFETY: dispatch limits and all resource bindings were validated above.
        unsafe {
            command.Dispatch(
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
            );
        }
        for binding in dispatch.bindings {
            let buffer = self.buffers.get(binding.buffer.buffer)?;
            if binding.access == ComputeBufferAccess::ReadWrite {
                record_transition_barrier(
                    &command,
                    buffer
                        .resource
                        .as_ref()
                        .ok_or_else(|| Error::Backend("missing compute resource".into()))?,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                );
            }
        }
        for binding in dispatch.bindings {
            states.insert(binding.buffer.buffer, D3D12_RESOURCE_STATE_GENERIC_READ);
        }
        Ok(())
    }
    pub(super) fn dispatch_buffer_batch(
        &mut self,
        dispatches: &[ComputeDispatch<'_>],
    ) -> Result<()> {
        let mut prepared: Vec<Resources> = Vec::new();
        let mut indices: Vec<usize> = Vec::new();
        let mut descriptors = HashMap::<ViewKey, DescriptorSlot>::new();
        for (index, dispatch) in dispatches.iter().enumerate() {
            match self.prepare_dispatch(dispatch, &mut descriptors) {
                Ok(Some(resources)) => {
                    prepared.push(resources);
                    indices.push(index);
                }
                Ok(None) => {}
                Err(error) => {
                    for resources in prepared {
                        resources.release(&mut self.resource_heap);
                    }
                    return Err(error);
                }
            }
        }
        if prepared.is_empty() {
            return Ok(());
        }
        let encoder = match self.create_command_encoder(&CommandEncoderDescriptor { label: None }) {
            Ok(encoder) => encoder,
            Err(error) => {
                for resources in prepared {
                    resources.release(&mut self.resource_heap);
                }
                return Err(error);
            }
        };
        self.command_encoders
            .get_mut(encoder)?
            .compute_resources
            .extend(prepared);
        let mut states = HashMap::new();
        let result = (|| {
            self.begin_texture_command_encoder(encoder, None)?;
            for (call_index, &index) in indices.iter().enumerate() {
                self.record_dispatch(encoder, call_index, &dispatches[index], &mut states)?;
            }
            self.finish_texture_command_encoder(encoder)?;
            let fence_value = self.submit_without_wait(encoder)?;
            let encoder = self.command_encoders.take(encoder)?;
            self.deferred_command_encoders
                .push(DeferredDx12CommandEncoder {
                    fence_value,
                    _encoder: encoder,
                });
            self.poll_cleanup();
            Ok(())
        })();
        if result.is_ok()
            || self
                .command_encoders
                .get(encoder)
                .is_ok_and(|encoder| encoder.submitted)
        {
            for (buffer, state) in states {
                self.buffers.get_mut(buffer)?.state = state;
            }
        }
        if result.is_err() {
            let _ = self.release_temporary_command_encoder_after_failure(encoder);
        }
        result
    }

    pub(super) fn stage_indirect(
        &mut self,
        encoder: CommandEncoderId,
        command: &ID3D12GraphicsCommandList,
        steps: &[RenderCommand],
    ) -> Result<Option<usize>> {
        let count = steps
            .iter()
            .filter(|s| matches!(s, RenderCommand::DrawIndirect(_)))
            .count();
        if count == 0 {
            return Ok(None);
        }
        for step in steps {
            if let RenderCommand::DrawIndirect(draw) = step {
                let buffer = self.buffers.get(draw.arguments)?;
                if !buffer.desc.usage.contains(BufferUsage::INDIRECT)
                    || draw.offset % 4 != 0
                    || draw
                        .offset
                        .checked_add(16)
                        .is_none_or(|end| end > buffer.desc.size)
                {
                    return Err(Error::InvalidInput(
                        "invalid DX12 indirect argument range or usage".into(),
                    ));
                }
            }
        }
        let size = (count as u64)
            .checked_mul(32)
            .ok_or_else(|| Error::InvalidInput("indirect size overflow".into()))?;
        let mut indirect = self.compute.acquire_indirect(&self.device, size)?;
        if indirect.state != D3D12_RESOURCE_STATE_COPY_DEST {
            record_transition_barrier(
                command,
                &indirect.buffer,
                indirect.state,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
        }
        let mut index = 0;
        for step in steps {
            let RenderCommand::DrawIndirect(draw) = step else {
                continue;
            };
            let pipeline = self.render_pipelines.get(draw.pipeline)?;
            let signature = pipeline
                .indirect_signature
                .as_ref()
                .ok_or_else(|| Error::Backend("missing indirect command signature".into()))?;
            let buffer = self.buffers.get(draw.arguments)?;
            let source = buffer
                .resource
                .as_ref()
                .ok_or_else(|| Error::Backend("missing indirect argument buffer".into()))?;
            if buffer.state != D3D12_RESOURCE_STATE_GENERIC_READ {
                self.command_encoders
                    .get_mut(encoder)?
                    .buffer_states
                    .entry(draw.arguments)
                    .or_insert(buffer.state);
                record_transition_barrier(
                    command,
                    source,
                    buffer.state,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                );
            }
            // Portable 16B record -> 2 root constants + native draw args with zero starts.
            // SAFETY: source ranges were validated; each 32B destination is disjoint and
            // the zero upload allocation was initialized at device creation.
            unsafe {
                command.CopyBufferRegion(&indirect.buffer, index * 32, source, draw.offset + 8, 8);
                command.CopyBufferRegion(&indirect.buffer, index * 32 + 8, source, draw.offset, 8);
                command.CopyBufferRegion(
                    &indirect.buffer,
                    index * 32 + 16,
                    &self.compute.zero,
                    0,
                    8,
                );
            }
            self.buffers.get_mut(draw.arguments)?.state = D3D12_RESOURCE_STATE_GENERIC_READ;
            indirect.signatures.push(signature.clone());
            index += 1;
        }
        record_transition_barrier(
            command,
            &indirect.buffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
        );
        let resources = &mut self.command_encoders.get_mut(encoder)?.indirect_resources;
        let slot = resources.len();
        resources.push(indirect);
        Ok(Some(slot))
    }

    pub(super) fn execute_indirect(
        &self,
        command: &ID3D12GraphicsCommandList,
        encoder: CommandEncoderId,
        slot: usize,
        index: usize,
    ) -> Result<()> {
        let indirect = &self.command_encoders.get(encoder)?.indirect_resources[slot];
        let signature = indirect
            .signatures
            .get(index)
            .ok_or_else(|| Error::InvalidInput("invalid staged indirect index".into()))?;
        // SAFETY: scratch and signature are retained by the submitting encoder until its fence.
        unsafe {
            command.ExecuteIndirect(signature, 1, &indirect.buffer, index as u64 * 32, None, 0);
        }
        Ok(())
    }
}

fn create_program(
    device: &ID3D12Device,
    shader: &Dx12ShaderModule,
    signature: Vec<(u32, u8)>,
) -> Result<Program> {
    let ranges = signature
        .iter()
        .map(|&(binding, kind)| D3D12_DESCRIPTOR_RANGE {
            RangeType: if kind == 0 {
                D3D12_DESCRIPTOR_RANGE_TYPE_CBV
            } else if kind == 2 {
                D3D12_DESCRIPTOR_RANGE_TYPE_UAV
            } else {
                D3D12_DESCRIPTOR_RANGE_TYPE_SRV
            },
            NumDescriptors: 1,
            BaseShaderRegister: binding,
            RegisterSpace: PHASE1_RESOURCE_SET_SPACE,
            OffsetInDescriptorsFromTableStart: 0,
        })
        .collect::<Vec<_>>();
    let mut parameters = ranges
        .iter()
        .map(|range| D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        })
        .collect::<Vec<_>>();
    parameters.push(draw_step_constants_parameter());
    let root = serialize_root_signature(device, &parameters)?;
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC::default();
    desc.pRootSignature = core::mem::ManuallyDrop::new(Some(root.clone()));
    desc.CS = shader_bytecode(shader.bytecode.as_slice());
    // SAFETY: root and bytecode remain live for the synchronous native pipeline creation.
    let result = unsafe { device.CreateComputePipelineState(&desc) };
    // SAFETY: release the temporary COM reference owned by the descriptor exactly once.
    unsafe {
        core::mem::ManuallyDrop::drop(&mut desc.pRootSignature);
    }
    Ok(Program {
        signature,
        root,
        pipeline: result.map_err(dx_error)?,
    })
}

pub(super) fn create_indirect_signature(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    constants: u32,
) -> Result<ID3D12CommandSignature> {
    let arguments = [
        D3D12_INDIRECT_ARGUMENT_DESC {
            Type: D3D12_INDIRECT_ARGUMENT_TYPE_CONSTANT,
            Anonymous: D3D12_INDIRECT_ARGUMENT_DESC_0 {
                Constant: D3D12_INDIRECT_ARGUMENT_DESC_0_1 {
                    RootParameterIndex: constants,
                    DestOffsetIn32BitValues: 0,
                    Num32BitValuesToSet: 2,
                },
            },
        },
        D3D12_INDIRECT_ARGUMENT_DESC {
            Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW,
            ..Default::default()
        },
    ];
    // SAFETY: signature consumes 24 initialized bytes per 32B record; root constants match Naga.
    let mut signature = None;
    unsafe {
        device.CreateCommandSignature(
            &D3D12_COMMAND_SIGNATURE_DESC {
                ByteStride: 32,
                NumArgumentDescs: arguments.len() as u32,
                pArgumentDescs: arguments.as_ptr(),
                NodeMask: 0,
            },
            root,
            &mut signature,
        )
    }
    .map_err(dx_error)?;
    signature.ok_or_else(|| Error::Backend("DX12 returned no indirect signature".into()))
}

fn raw_buffer_srv_desc(first: u64, count: u32) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Buffer: D3D12_BUFFER_SRV {
                FirstElement: first,
                NumElements: count,
                StructureByteStride: 0,
                Flags: D3D12_BUFFER_SRV_FLAG_RAW,
            },
        },
    }
}
