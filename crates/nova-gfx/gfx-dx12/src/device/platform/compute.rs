//! Ordered buffer compute and GPU-side expansion of the portable indirect ABI.

use super::*;
use gfx_core::{ComputeBufferAccess, ComputeCapabilities, ComputeDispatch};
use std::collections::HashMap;
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R32_TYPELESS};

// The shader compiler runs in build.rs; production devices only load DXBC.
mod indirect_artifacts {
    include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
}

#[cfg(test)]
mod tests;

pub(super) struct State {
    programs: HashMap<ShaderModuleId, Arc<Program>>,
    resource_set_programs: HashMap<(ShaderModuleId, BindGroupLayoutId), Arc<Program>>,
    indirect_pack: NativeIndirectPack,
    indirect_buffers: Vec<Indirect>,
}
struct NativeIndirectPack {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}

struct Program {
    signature: Vec<(u32, u8)>,
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}
// Indirect argument buffers retire through the frame fence just like command
// encoders; this pool bound is independent of the device's encoder tuning.
const DEFAULT_INDIRECT_BUFFER_POOL_CAPACITY: usize = 16;

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
        // D3D12 feature-level 11.0 guaranteed compute minimums; hardware may
        // report larger optional limits, which this struct does not model yet.
        max_workgroups: [65535; 3],
        // D3D12_CS_THREAD_GROUP_MAX_THREADS_PER_GROUP.
        max_invocations_per_workgroup: 1024,
        max_workgroup_size: [1024, 1024, 64],
        max_workgroup_shared_memory_bytes: 32 * 1024,
        // ExecuteIndirect tier 1.0 is guaranteed above feature level 11.0.
        indirect_draw: true,
    }
}

impl State {
    pub(super) fn new(device: &ID3D12Device) -> Result<Self> {
        let indirect_pack = create_native_indirect_pack(device)?;
        Ok(Self {
            programs: HashMap::new(),
            resource_set_programs: HashMap::new(),
            indirect_pack,
            indirect_buffers: Vec::new(),
        })
    }
    pub(super) fn remove_shader(&mut self, id: ShaderModuleId) {
        self.programs.remove(&id);
        self.resource_set_programs.retain(|(shader, _), _| *shader != id);
    }

    pub(super) fn recycle_indirect(&mut self, mut resource: Indirect, submitted: bool) {
        if submitted {
            resource.state = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
        }
        resource.signatures.clear();
        if self.indirect_buffers.len() < DEFAULT_INDIRECT_BUFFER_POOL_CAPACITY {
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
                usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
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
    /// Full native compute resource-set submission. Generated Mesh/Hi-Z keep
    /// their optimized buffer-only dispatch path without an extra bind group.
    pub(super) fn dispatch_resource_set(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
    ) -> Result<()> {
        dispatch.validate(self.compute_capabilities())?;
        let set = self.bind_groups.get(dispatch.bind_group)?.clone();
        let layout = self.bind_group_layouts.get(set.layout)?.desc.clone();
        set.desc.validate_against(&layout)?;
        let mut buffers = Vec::new();
        let mut images = Vec::<(TextureId, bool)>::new();
        for entry in &layout.entries {
            if !entry.stages.contains(ShaderStages::COMPUTE) {
                return Err(Error::InvalidInput("DX12 compute resource is not visible to compute".into()));
            }
            let resource = set.desc.entries.iter()
                .find(|binding| binding.binding == entry.binding)
                .ok_or_else(|| Error::InvalidInput("missing compute resource binding".into()))?;
            match (entry.binding_type, resource.resource) {
                (BindingType::UniformBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBufferReadWrite, BindingResource::Buffer(buffer)) => {
                    let access = match entry.binding_type {
                        BindingType::UniformBuffer => ComputeBufferAccess::Uniform,
                        BindingType::StorageBuffer => ComputeBufferAccess::ReadOnly,
                        _ => ComputeBufferAccess::ReadWrite,
                    };
                    buffers.push(gfx_core::ComputeBufferBinding {
                        binding: entry.binding, buffer, access,
                    });
                }
                (BindingType::SampledTexture, BindingResource::Texture(view))
                | (BindingType::StorageTexture { .. }, BindingResource::Texture(view)) => {
                    let id = self.texture_views.get(view.texture_view)?.texture;
                    let writable = matches!(entry.binding_type, BindingType::StorageTexture { .. });
                    if let Some((_, prior_write)) = images.iter().find(|(image, _)| *image == id) {
                        if writable || *prior_write {
                            return Err(Error::InvalidInput(
                                "DX12 compute writable image aliases another binding".into()
                            ));
                        }
                    } else {
                        images.push((id, writable));
                    }
                }
                (BindingType::Sampler, BindingResource::Sampler(_)) => {}
                _ => return Err(Error::InvalidInput("DX12 compute resource type mismatch".into())),
            }
        }
        ComputeDispatch {
            shader: dispatch.shader,
            bindings: &buffers,
            workgroups: dispatch.workgroups,
        }.validate_with_limits(self.compute_capabilities(), self.buffer_binding_limits())?;
        let key = (dispatch.shader, set.layout);
        let program = if let Some(cached) = self.compute.resource_set_programs.get(&key) {
            cached.clone()
        } else {
            let shader = self.shader_modules.get(dispatch.shader)?;
            let program = Arc::new(create_resource_set_program(&self.device, shader, &layout)?);
            self.compute.resource_set_programs.insert(key, program.clone());
            program
        };

        let encoder = self.create_command_encoder(&CommandEncoderDescriptor { label: None })?;
        self.command_encoders.get_mut(encoder)?.compute_resources.push(Resources {
            slots: Vec::new(), owned_slots: Vec::new(), program: program.clone(),
        });
        let mut final_buffers = Vec::<BufferId>::new();
        let result = (|| -> Result<()> {
            self.begin_texture_command_encoder(encoder, None)?;
            let command = self.command_encoders.get(encoder)?.command_list.as_ref()
                .ok_or_else(|| Error::Backend("DX12 compute command list is missing".into()))?
                .clone();
            let has_sampler = layout.entries.iter().any(|e| e.binding_type == BindingType::Sampler);
            let heaps = if has_sampler {
                vec![Some(self.resource_heap.heap.clone()), Some(self.sampler_heap.heap.clone())]
            } else {
                vec![Some(self.resource_heap.heap.clone())]
            };
            // SAFETY: root signature/PSO and both shader-visible heaps stay owned
            // by the device; the encoder holds a PSO reference through its fence.
            unsafe {
                command.SetDescriptorHeaps(&heaps);
                command.SetComputeRootSignature(&program.root);
                command.SetPipelineState(&program.pipeline);
            }
            let mut slot = 0u32;
            let mut sampler_emitted = false;
            for entry in &layout.entries {
                if entry.binding_type == BindingType::Sampler {
                    if sampler_emitted { continue; }
                    sampler_emitted = true;
                    let indices = set.sampler_index_table.as_ref().ok_or_else(|| {
                        Error::InvalidInput("DX12 compute sampler index descriptor is missing".into())
                    })?;
                    unsafe {
                        command.SetComputeRootDescriptorTable(slot, self.sampler_heap.gpu_start());
                        command.SetComputeRootDescriptorTable(slot + 1, indices.gpu_handle);
                    }
                    slot += 2;
                    continue;
                }
                let table = set.resource_tables.iter()
                    .find(|table| table.binding == entry.binding)
                    .ok_or_else(|| Error::InvalidInput("missing DX12 compute root table".into()))?;
                unsafe { command.SetComputeRootDescriptorTable(slot, table.gpu_handle) };
                slot += 1;
            }
            let constants = draw_step_constants_root_index(std::slice::from_ref(&layout))?;
            if constants != slot {
                return Err(Error::Backend("DX12 compute root layout ABI mismatch".into()));
            }
            unsafe {
                command.SetComputeRoot32BitConstants(
                    constants, 3, dispatch.workgroups.as_ptr().cast(), 0,
                );
            }
            for binding in &buffers {
                // Multiple read-only descriptors may reference the same physical
                // buffer. Emit exactly one transition per native resource:
                // repeating the original StateBefore after the first barrier
                // would make the second transition invalid in this command list.
                if final_buffers.contains(&binding.buffer.buffer) {
                    continue;
                }
                let buffer = self.buffers.get(binding.buffer.buffer)?;
                let resource = buffer.resource.as_ref().ok_or_else(|| {
                    Error::Backend("missing DX12 compute storage resource".into())
                })?;
                let target = if binding.access == ComputeBufferAccess::ReadWrite {
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                } else {
                    D3D12_RESOURCE_STATE_GENERIC_READ
                };
                if buffer.state != target {
                    record_transition_barrier(&command, resource, buffer.state, target);
                }
                final_buffers.push(binding.buffer.buffer);
            }
            for &(id, writable) in &images {
                let texture = self.textures.get(id)?;
                let resource = texture.resource.as_ref().ok_or_else(|| {
                    Error::Backend("missing DX12 compute image".into())
                })?;
                let target = if writable {
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                } else {
                    D3D12_RESOURCE_STATE_GENERIC_READ
                };
                if texture.state != target {
                    record_transition_barrier(&command, resource, texture.state, target);
                }
            }
            unsafe {
                command.Dispatch(
                    dispatch.workgroups[0], dispatch.workgroups[1], dispatch.workgroups[2],
                );
            }
            // Writable UAV images must leave the UAV state before GPUI
            // samples them. Sampled-only images can remain GENERIC_READ,
            // which already includes both pixel and non-pixel shader reads.
            // Avoid forcing every compute sample through a redundant
            // GENERIC_READ -> PIXEL_SHADER_RESOURCE round trip.
            for &(id, storage) in &images {
                if !storage {
                    continue;
                }
                let texture = self.textures.get(id)?;
                let resource = texture.resource.as_ref().ok_or_else(|| {
                    Error::Backend("missing DX12 compute image".into())
                })?;
                record_transition_barrier(
                    &command,
                    resource,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                );
            }
            for binding in &buffers {
                if binding.access == ComputeBufferAccess::ReadWrite {
                    let resource = self.buffers.get(binding.buffer.buffer)?.resource.as_ref()
                        .ok_or_else(|| Error::Backend("missing writable compute resource".into()))?;
                    record_transition_barrier(
                        &command, resource,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_GENERIC_READ,
                    );
                }
            }
            self.finish_texture_command_encoder(encoder)?;
            let fence_value = self.submit_without_wait(encoder)?;
            let submitted = self.command_encoders.take(encoder)?;
            self.deferred_command_encoders.push(DeferredDx12CommandEncoder {
                fence_value, _encoder: submitted,
            });
            self.poll_cleanup();
            Ok(())
        })();
        let was_submitted = self.command_encoders.get(encoder)
            .is_ok_and(|pending| pending.submitted);
        if result.is_ok() || was_submitted {
            for id in final_buffers {
                self.buffers.get_mut(id)?.state = D3D12_RESOURCE_STATE_GENERIC_READ;
            }
            for (id, storage) in images {
                self.textures.get_mut(id)?.state = if storage {
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
                } else {
                    D3D12_RESOURCE_STATE_GENERIC_READ
                };
            }
        }
        if result.is_err() {
            let _ = self.release_temporary_command_encoder_after_failure(encoder);
        }
        result
    }

    pub(super) fn dispatch_buffers(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffer_batch(std::slice::from_ref(dispatch))
    }
    fn prepare_dispatch(
        &mut self,
        dispatch: &ComputeDispatch<'_>,
        views: &mut HashMap<ViewKey, DescriptorSlot>,
    ) -> Result<Option<Resources>> {
        let limits = self.buffer_binding_limits();
        dispatch.validate_with_limits(capabilities(), limits)?;
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
                || binding.buffer.offset
                    % if uniform {
                        limits.uniform_offset_alignment.max(1)
                    } else {
                        limits.storage_offset_alignment.max(1)
                    }
                    != 0
                || (!uniform && binding.buffer.size % 4 != 0)
                || (uniform && binding.buffer.size > limits.max_uniform_binding_bytes)
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
        self.dispatch_buffer_batch_profiled(dispatches, None)
    }

    pub(super) fn dispatch_buffer_batch_profiled(
        &mut self,
        dispatches: &[ComputeDispatch<'_>],
        tag: Option<gfx_core::GpuTimestampTag>,
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
            let queries = if tag.is_some() {
                self.command_encoders.get(encoder)?.timestamps.as_ref().map(|queries| {
                    (queries.heap.clone(), queries.readback.clone())
                })
            } else { None };
            if let Some((heap, _)) = &queries {
                let command = self.command_encoders.get(encoder)?.command_list
                    .as_ref().ok_or_else(|| Error::Backend("missing DX12 compute command list".into()))?;
                // SAFETY: The encoder is recording on the graphics queue and query slot zero is valid.
                unsafe { command.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, 0) };
            }
            for (call_index, &index) in indices.iter().enumerate() {
                self.record_dispatch(encoder, call_index, &dispatches[index], &mut states)?;
            }
            if let Some((heap, readback)) = &queries {
                let command = self.command_encoders.get(encoder)?.command_list
                    .as_ref().ok_or_else(|| Error::Backend("missing DX12 compute command list".into()))?;
                // SAFETY: Both timestamp queries belong to this encoder and resolve into 16 readback bytes.
                unsafe {
                    command.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, 1);
                    command.ResolveQueryData(
                        heap, D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, readback, 0,
                    );
                }
                self.command_encoders.get_mut(encoder)?.gpu_work_tag = tag;
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
        // A large generated Section scene can have tens of thousands of
        // indirect draws. Count and validate in the SAME pass rather than
        // scanning the entire command table twice before the actual GPU
        // packing pass. Keep validation before acquiring transient storage.
        let mut count = 0usize;
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
                count += 1;
            }
        }
        if count == 0 {
            return Ok(None);
        }
        let size = (count as u64)
            .checked_mul(32)
            .ok_or_else(|| Error::InvalidInput("indirect size overflow".into()))?;
        let mut indirect = self.compute.acquire_indirect(&self.device, size)?;
        if indirect.state != D3D12_RESOURCE_STATE_UNORDERED_ACCESS {
            record_transition_barrier(
                command,
                &indirect.buffer,
                indirect.state,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
        }
        let mut index = 0;
        // Most voxel draws read adjacent 16-byte records from the SAME GPU
        // culling argument buffer and use the same native pipeline signature.
        // Cache the last native handles to avoid per-draw arena lookup, COM
        // reference clone and redundant state tracking. Changing either ID
        // still goes through the full validation and transition path.
        let mut last_source: Option<(BufferId, ID3D12Resource)> = None;
        let mut last_signature: Option<(RenderPipelineId, ID3D12CommandSignature)> = None;
        // One compute dispatch per contiguous source span, not three
        // CopyBufferRegion commands per DrawIndirect.
        let mut pending: Option<(BufferId, ID3D12Resource, u64, usize, u32)> = None;
        for step in steps {
            let RenderCommand::DrawIndirect(draw) = step else {
                continue;
            };
            if last_source
                .as_ref()
                .is_none_or(|(id, _)| *id != draw.arguments)
            {
                let buffer = self.buffers.get(draw.arguments)?;
                let source = buffer
                    .resource
                    .as_ref()
                    .ok_or_else(|| Error::Backend("missing indirect argument buffer".into()))?
                    .clone();
                if buffer.state != D3D12_RESOURCE_STATE_GENERIC_READ {
                    self.command_encoders
                        .get_mut(encoder)?
                        .buffer_states
                        .entry(draw.arguments)
                        .or_insert(buffer.state);
                    record_transition_barrier(
                        command,
                        &source,
                        buffer.state,
                        D3D12_RESOURCE_STATE_GENERIC_READ,
                    );
                    self.buffers.get_mut(draw.arguments)?.state = D3D12_RESOURCE_STATE_GENERIC_READ;
                }
                last_source = Some((draw.arguments, source));
            }
            let source = &last_source
                .as_ref()
                .expect("validated indirect source must be cached")
                .1;
            let signature = if let Some((id, signature)) = &last_signature
                && *id == draw.pipeline
            {
                signature.clone()
            } else {
                let pipeline = self.render_pipelines.get(draw.pipeline)?;
                let signature = pipeline
                    .indirect_signature
                    .as_ref()
                    .ok_or_else(|| Error::Backend("missing indirect command signature".into()))?
                    .clone();
                last_signature = Some((draw.pipeline, signature.clone()));
                signature
            };
            let contiguous =
                pending
                    .as_ref()
                    .is_some_and(|(buffer, _, source_offset, _, length)| {
                        *buffer == draw.arguments
                    && source_offset.checked_add(u64::from(*length) * 16) == Some(draw.offset)
                    // The compiled 64-thread kernel uses one dispatch grid.
                    && *length < 65535 * 64
                    });
            if !contiguous {
                if let Some((_, resource, source_offset, target, length)) = pending.take() {
                    self.pack_native_indirect_run(
                        command,
                        &resource,
                        &indirect.buffer,
                        source_offset,
                        target,
                        length,
                    )?;
                }
                pending = Some((
                    draw.arguments,
                    source.clone(),
                    draw.offset,
                    index as usize,
                    1,
                ));
            } else if let Some((_, _, _, _, length)) = pending.as_mut() {
                *length += 1;
            }
            indirect.signatures.push(signature);
            index += 1;
        }
        if let Some((_, resource, source_offset, target, length)) = pending.take() {
            self.pack_native_indirect_run(
                command,
                &resource,
                &indirect.buffer,
                source_offset,
                target,
                length,
            )?;
        }
        record_transition_barrier(
            command,
            &indirect.buffer,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
        );
        let resources = &mut self.command_encoders.get_mut(encoder)?.indirect_resources;
        let slot = resources.len();
        resources.push(indirect);
        Ok(Some(slot))
    }

    fn pack_native_indirect_run(
        &self,
        command: &ID3D12GraphicsCommandList,
        source: &ID3D12Resource,
        target: &ID3D12Resource,
        source_offset: u64,
        first_target: usize,
        count: u32,
    ) -> Result<()> {
        let first_word = u32::try_from(source_offset / 4).map_err(|_| {
            Error::InvalidInput("DX12 indirect input offset exceeds shader address range".into())
        })?;
        let target_word = first_target
            .checked_mul(8)
            .and_then(|word| u32::try_from(word).ok())
            .ok_or_else(|| {
                Error::InvalidInput(
                    "DX12 indirect output offset exceeds shader address range".into(),
                )
            })?;
        let input_end = count
            .checked_mul(4)
            .and_then(|len| first_word.checked_add(len))
            .ok_or_else(|| {
                Error::InvalidInput("DX12 indirect input span exceeds shader address range".into())
            })?;
        let output_end = count
            .checked_mul(8)
            .and_then(|len| target_word.checked_add(len))
            .ok_or_else(|| {
                Error::InvalidInput("DX12 indirect output span exceeds shader address range".into())
            })?;
        debug_assert!(input_end > first_word && output_end > target_word);
        let words = [first_word, target_word, count, 0u32];
        // SAFETY: command list is recording on its graphics queue; source is
        // transitioned to GENERIC_READ, target is UAV, and all writes target
        // disjoint 32-byte records. The UAV->INDIRECT transition follows the
        // final dispatch before ExecuteIndirect.
        unsafe {
            command.SetComputeRootSignature(&self.compute.indirect_pack.root);
            command.SetPipelineState(&self.compute.indirect_pack.pipeline);
            command.SetComputeRootShaderResourceView(0, source.GetGPUVirtualAddress());
            command.SetComputeRootUnorderedAccessView(1, target.GetGPUVirtualAddress());
            command.SetComputeRoot32BitConstants(2, 4, words.as_ptr().cast(), 0);
            command.Dispatch(count.div_ceil(64), 1, 1);
        }
        Ok(())
    }

    pub(super) fn execute_indirect(
        &self,
        command: &ID3D12GraphicsCommandList,
        encoder: CommandEncoderId,
        slot: usize,
        index: usize,
        count: usize,
    ) -> Result<()> {
        let indirect = &self.command_encoders.get(encoder)?.indirect_resources[slot];
        let end = index
            .checked_add(count)
            .ok_or_else(|| Error::InvalidInput("indirect draw range overflow".into()))?;
        if count == 0 || end > indirect.signatures.len() {
            return Err(Error::InvalidInput(
                "invalid staged indirect draw range".into(),
            ));
        }
        let native_count = u32::try_from(count)
            .map_err(|_| Error::InvalidInput("indirect draw count exceeds u32".into()))?;
        let signature = indirect
            .signatures
            .get(index)
            .ok_or_else(|| Error::InvalidInput("invalid staged indirect index".into()))?;
        // SAFETY: scratch and signature are retained by the submitting encoder until its fence.
        unsafe {
            command.ExecuteIndirect(
                signature,
                native_count,
                &indirect.buffer,
                index as u64 * 32,
                None,
                0,
            );
        }
        Ok(())
    }
}

fn create_native_indirect_pack(device: &ID3D12Device) -> Result<NativeIndirectPack> {
    let artifact = indirect_artifacts::native_indirect_dx12_shader("cs_pack_native_indirect")
        .ok_or_else(|| Error::Shader("missing offline DX12 indirect pack shader".into()))?;
    let gfx_core::EmbeddedShader::DxBytecode(bytecode) = artifact else {
        return Err(Error::Shader(
            "DX12 native indirect pack must be precompiled".into(),
        ));
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 2,
                    RegisterSpace: 0,
                    Num32BitValues: 4,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let root = serialize_root_signature(device, &parameters)?;
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC::default();
    desc.pRootSignature = core::mem::ManuallyDrop::new(Some(root.clone()));
    desc.CS = shader_bytecode(bytecode);
    // SAFETY: offline bytecode is static and the root signature remains live.
    let result = unsafe { device.CreateComputePipelineState(&desc) };
    // SAFETY: descriptor owns one temporary COM reference.
    unsafe { core::mem::ManuallyDrop::drop(&mut desc.pRootSignature) };
    Ok(NativeIndirectPack {
        root,
        pipeline: result.map_err(dx_error)?,
    })
}

/// Build a compute PSO from the exact resource-set layout. This native path
/// handles sampled textures, UAV images and samplers, including Naga's sampler
/// index buffer ABI; it does not alter the buffer-only compute fast path.
fn create_resource_set_program(
    device: &ID3D12Device,
    shader: &Dx12ShaderModule,
    layout: &BindGroupLayoutDescriptor,
) -> Result<Program> {
    if shader.stage != ShaderStage::Compute {
        return Err(Error::InvalidInput("resource set requires a compute shader".into()));
    }
    let root = create_root_signature(device, std::slice::from_ref(layout))?;
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC::default();
    desc.pRootSignature = core::mem::ManuallyDrop::new(Some(root.clone()));
    desc.CS = shader_bytecode(shader.bytecode.as_slice());
    let result = unsafe { device.CreateComputePipelineState(&desc) };
    unsafe { core::mem::ManuallyDrop::drop(&mut desc.pRootSignature) };
    Ok(Program {
        signature: Vec::new(),
        root,
        pipeline: result.map_err(dx_error)?,
    })
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

/// Signature for `D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED` argument records.
///
/// Indexed records are 20 bytes, so this is a separate signature from the packed
/// non-indexed one; the constant root parameter keeps Naga's draw constants usable.
pub(super) fn create_indexed_indirect_signature(
    device: &ID3D12Device,
) -> Result<ID3D12CommandSignature> {
    // Standard 20-byte DRAW_INDEXED records. The signature consumes no root parameters, so it
    // is created without a root signature and the arguments stay portable across backends.
    let arguments = [D3D12_INDIRECT_ARGUMENT_DESC {
        Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED,
        ..Default::default()
    }];
    // SAFETY: The signature consumes initialized 20-byte DRAW_INDEXED records and needs no
    // root signature because it references no root parameters.
    let mut signature = None;
    unsafe {
        device.CreateCommandSignature(
            &D3D12_COMMAND_SIGNATURE_DESC {
                ByteStride: 20,
                NumArgumentDescs: arguments.len() as u32,
                pArgumentDescs: arguments.as_ptr(),
                NodeMask: 0,
            },
            None::<&ID3D12RootSignature>,
            &mut signature,
        )
    }
    .map_err(dx_error)?;
    signature.ok_or_else(|| Error::Backend("DX12 returned no indexed indirect signature".into()))
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
