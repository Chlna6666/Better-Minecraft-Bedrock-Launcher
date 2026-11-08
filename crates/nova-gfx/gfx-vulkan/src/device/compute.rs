//! Ordered buffer compute on the graphics queue; submitted resources follow encoder fences.

use super::*;
use gfx_core::{ComputeBufferAccess, ComputeCapabilities, ComputeDispatch};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    pub capabilities: ComputeCapabilities,
    programs: HashMap<ShaderModuleId, Arc<Program>>,
}

struct Program {
    device: Arc<ash::Device>,
    signature: Vec<(u32, bool)>,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl Drop for Program {
    fn drop(&mut self) {
        // SAFETY: cached programs are retained by every submitted encoder using them.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

pub(super) struct Call {
    program: Arc<Program>,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    _descriptor_owner: Option<Arc<Call>>,
}

impl Drop for Call {
    fn drop(&mut self) {
        // SAFETY: the owning command encoder is completed or was never submitted.
        if self.pool != vk::DescriptorPool::null() {
            unsafe { self.program.device.destroy_descriptor_pool(self.pool, None) };
        }
    }
}

impl State {
    pub(super) fn new(capabilities: ComputeCapabilities) -> Self {
        Self {
            capabilities,
            ..Default::default()
        }
    }
    pub(super) fn remove_shader(&mut self, shader: ShaderModuleId) {
        self.programs.remove(&shader);
    }
    pub(super) fn clear(&mut self) {
        self.programs.clear();
    }
}

impl VulkanDevice {
    pub(super) fn dispatch_buffers(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffer_batch(std::slice::from_ref(dispatch))
    }
    fn prepare_dispatch(
        &mut self,
        dispatch: &ComputeDispatch<'_>,
        reuse: Option<&Arc<Call>>,
    ) -> Result<Option<Arc<Call>>> {
        dispatch.validate_with_limits(self.compute.capabilities, self.buffer_binding_limits())?;
        let shader = self.shader_modules.get(dispatch.shader)?;
        if shader.stage != ShaderStage::Compute {
            return Err(Error::InvalidInput(
                "compute dispatch requires a compute shader".into(),
            ));
        }
        if dispatch.workgroups.contains(&0) {
            return Ok(None);
        }
        // Limits were captured from this physical adapter at device creation.
        // Never issue a Vulkan property query per streaming-Section dispatch.
        let limits = self.binding_limits;
        let mut signature = Vec::with_capacity(dispatch.bindings.len());
        let mut infos = Vec::with_capacity(dispatch.bindings.len());
        for binding in dispatch.bindings {
            let buffer = self.buffers.get(binding.buffer.buffer)?;
            let uniform = binding.access == ComputeBufferAccess::Uniform;
            let usage = if uniform {
                BufferUsage::UNIFORM
            } else {
                BufferUsage::STORAGE
            };
            let alignment = if uniform {
                limits.uniform_offset_alignment
            } else {
                limits.storage_offset_alignment
            };
            let maximum = if uniform {
                limits.max_uniform_binding_bytes
            } else {
                limits.max_storage_binding_bytes
            };
            if !buffer.desc.usage.contains(usage)
                || (binding.access == ComputeBufferAccess::ReadWrite
                    && !buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE))
                || binding.buffer.size == 0
                || binding.buffer.size > maximum
                || binding.buffer.offset % alignment != 0
                || binding
                    .buffer
                    .offset
                    .checked_add(binding.buffer.size)
                    .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "invalid Vulkan compute buffer range or usage".into(),
                ));
            }
            signature.push((binding.binding, uniform));
            infos.push(
                vk::DescriptorBufferInfo::default()
                    .buffer(buffer.buffer)
                    .offset(binding.buffer.offset)
                    .range(binding.buffer.size),
            );
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
            let program = Arc::new(create_program(
                self.device.clone(),
                self.pipeline_cache,
                shader,
                signature,
            )?);
            self.compute
                .programs
                .insert(dispatch.shader, program.clone());
            program
        };
        let call = if let Some(owner) = reuse {
            Arc::new(Call {
                program,
                pool: vk::DescriptorPool::null(),
                set: owner.set,
                _descriptor_owner: Some(owner.clone()),
            })
        } else {
            Arc::new(create_call(program, dispatch, &infos)?)
        };
        Ok(Some(call))
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
        let mut calls: Vec<(usize, Arc<Call>)> = Vec::new();
        let mut descriptors = HashMap::<Vec<(u32, BufferId, u64, u64, u8)>, usize>::new();
        for (index, dispatch) in dispatches.iter().enumerate() {
            let mut key = dispatch
                .bindings
                .iter()
                .map(|binding| {
                    (
                        binding.binding,
                        binding.buffer.buffer,
                        binding.buffer.offset,
                        binding.buffer.size,
                        match binding.access {
                            ComputeBufferAccess::Uniform => 0,
                            ComputeBufferAccess::ReadOnly => 1,
                            ComputeBufferAccess::ReadWrite => 2,
                        },
                    )
                })
                .collect::<Vec<_>>();
            key.sort_unstable_by_key(|value| value.0);
            let reusable = descriptors.get(&key).map(|&slot| &calls[slot].1);
            if let Some(call) = self.prepare_dispatch(dispatch, reusable)? {
                descriptors.entry(key).or_insert(calls.len());
                calls.push((index, call));
            }
        }
        if calls.is_empty() {
            return Ok(());
        }
        let encoder = self.create_command_encoder_with_buffer_count(1)?;
        let command = self.command_encoders.get(encoder)?.command_buffers[0];
        let query = self.command_encoders.get(encoder)?
            .gpu_trace_query_pool.filter(|_| tag.is_some());
        self.command_encoders.get_mut(encoder)?.gpu_work_tag =
            tag.filter(|_| query.is_some());
        self.command_encoders
            .get_mut(encoder)?
            .compute_resources
            .extend(calls.iter().map(|(_, call)| call.clone()));
        let result = (|| {
            // SAFETY: this idle encoder retains every descriptor set and program until its submission completes.
            unsafe {
                self.device
                    .begin_command_buffer(
                        command,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                    .map_err(VulkanError::from)?;
                if let Some(query) = query {
                    self.device.cmd_reset_query_pool(command, query, 0, 2);
                    self.device.cmd_write_timestamp(
                        command, vk::PipelineStageFlags::TOP_OF_PIPE, query, 0,
                    );
                }
                for (index, call) in &calls {
                    let dispatch = &dispatches[*index];
                    let mut before = Vec::new();
                    let mut after = Vec::new();
                    for binding in dispatch.bindings {
                        let buffer = self.buffers.get(binding.buffer.buffer)?;
                        let read = if binding.access == ComputeBufferAccess::Uniform {
                            vk::AccessFlags::UNIFORM_READ
                        } else {
                            vk::AccessFlags::SHADER_READ
                        };
                        let write = if binding.access == ComputeBufferAccess::ReadWrite {
                            vk::AccessFlags::SHADER_WRITE
                        } else {
                            vk::AccessFlags::empty()
                        };
                        let barrier = vk::BufferMemoryBarrier::default()
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .buffer(buffer.buffer)
                            .offset(binding.buffer.offset)
                            .size(binding.buffer.size);
                        before.push(
                            barrier
                                .src_access_mask(
                                    vk::AccessFlags::MEMORY_READ
                                        | vk::AccessFlags::MEMORY_WRITE
                                        | vk::AccessFlags::HOST_WRITE,
                                )
                                .dst_access_mask(read | write),
                        );
                        if binding.access == ComputeBufferAccess::ReadWrite {
                            after.push(
                                barrier
                                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                                    .dst_access_mask(
                                        vk::AccessFlags::MEMORY_READ
                                            | vk::AccessFlags::MEMORY_WRITE,
                                    ),
                            );
                        }
                    }
                    self.device.cmd_pipeline_barrier(
                        command,
                        vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &before,
                        &[],
                    );
                    self.device.cmd_bind_pipeline(
                        command,
                        vk::PipelineBindPoint::COMPUTE,
                        call.program.pipeline,
                    );
                    self.device.cmd_bind_descriptor_sets(
                        command,
                        vk::PipelineBindPoint::COMPUTE,
                        call.program.layout,
                        0,
                        &[call.set],
                        &[],
                    );
                    self.device.cmd_dispatch(
                        command,
                        dispatch.workgroups[0],
                        dispatch.workgroups[1],
                        dispatch.workgroups[2],
                    );
                    self.device.cmd_pipeline_barrier(
                        command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::DependencyFlags::empty(),
                        &[],
                        &after,
                        &[],
                    );
                }
                if let Some(query) = query {
                    self.device.cmd_write_timestamp(
                        command, vk::PipelineStageFlags::BOTTOM_OF_PIPE, query, 1,
                    );
                }
                self.device
                    .end_command_buffer(command)
                    .map_err(VulkanError::from)?;
            }
            self.submit_command_encoder_deferred(encoder)
        })();
        if result.is_err() {
            let _ = self.destroy_temporary_command_encoder_now(encoder);
        }
        result
    }
}

fn descriptor_type(uniform: bool) -> vk::DescriptorType {
    if uniform {
        vk::DescriptorType::UNIFORM_BUFFER
    } else {
        vk::DescriptorType::STORAGE_BUFFER
    }
}

fn create_program(
    device: Arc<ash::Device>,
    cache: vk::PipelineCache,
    shader: &VulkanShaderModule,
    signature: Vec<(u32, bool)>,
) -> Result<Program> {
    let mut program = Program {
        device,
        signature,
        set_layout: vk::DescriptorSetLayout::null(),
        layout: vk::PipelineLayout::null(),
        pipeline: vk::Pipeline::null(),
    };
    let bindings = program
        .signature
        .iter()
        .map(|&(binding, uniform)| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(descriptor_type(uniform))
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
        })
        .collect::<Vec<_>>();
    let entry = CString::new(shader.entry_point.as_str())
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    // SAFETY: descriptors reference initialized slices; the RAII program cleans partial creation.
    unsafe {
        program.set_layout = program
            .device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .map_err(VulkanError::from)?;
        program.layout = program
            .device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[program.set_layout]),
                None,
            )
            .map_err(VulkanError::from)?;
        let info = vk::ComputePipelineCreateInfo::default()
            .layout(program.layout)
            .stage(
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::COMPUTE)
                    .module(shader.module)
                    .name(&entry),
            );
        match program
            .device
            .create_compute_pipelines(cache, &[info], None)
        {
            Ok(pipelines) => program.pipeline = pipelines[0],
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    program.device.destroy_pipeline(pipeline, None);
                }
                return Err(VulkanError::from(error).into());
            }
        }
    }
    Ok(program)
}

fn create_call(
    program: Arc<Program>,
    dispatch: &ComputeDispatch<'_>,
    infos: &[vk::DescriptorBufferInfo],
) -> Result<Call> {
    let sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: dispatch.bindings.len().max(1) as u32,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: dispatch.bindings.len().max(1) as u32,
        },
    ];
    // SAFETY: the pool owns one set and is retained until the dispatch fence completes.
    let pool = unsafe {
        program.device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&sizes),
            None,
        )
    }
    .map_err(VulkanError::from)?;
    let mut call = Call {
        program,
        pool,
        set: vk::DescriptorSet::null(),
        _descriptor_owner: None,
    };
    // SAFETY: pool/layout and buffer descriptors are live, and updates are copied immediately.
    unsafe {
        call.set = call
            .program
            .device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&[call.program.set_layout]),
            )
            .map_err(VulkanError::from)?[0];
        let writes = dispatch
            .bindings
            .iter()
            .zip(infos)
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(call.set)
                    .dst_binding(binding.binding)
                    .descriptor_type(descriptor_type(
                        binding.access == ComputeBufferAccess::Uniform,
                    ))
                    .buffer_info(std::slice::from_ref(info))
            })
            .collect::<Vec<_>>();
        call.program.device.update_descriptor_sets(&writes, &[]);
    }
    Ok(call)
}
