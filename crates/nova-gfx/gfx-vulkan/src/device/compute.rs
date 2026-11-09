//! Ordered buffer compute on the graphics queue; submitted resources follow encoder fences.

use super::*;
use gfx_core::{ComputeBufferAccess, ComputeCapabilities, ComputeDispatch};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    pub capabilities: ComputeCapabilities,
    programs: HashMap<ShaderModuleId, Arc<Program>>,
    resource_set_programs: HashMap<(ShaderModuleId, BindGroupLayoutId), Arc<ResourceSetProgram>>,
}

/// Compute pipeline using a live bind-group layout and its native descriptor set.
struct ResourceSetProgram {
    device: Arc<ash::Device>,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl Drop for ResourceSetProgram {
    fn drop(&mut self) {
        // All cached programs are cleared only after outstanding work retires.
        unsafe {
            if self.pipeline != vk::Pipeline::null() {
                self.device.destroy_pipeline(self.pipeline, None);
            }
            if self.layout != vk::PipelineLayout::null() {
                self.device.destroy_pipeline_layout(self.layout, None);
            }
        }
    }
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
        self.resource_set_programs.retain(|(id, _), _| *id != shader);
    }
    pub(super) fn clear(&mut self) {
        self.programs.clear();
        self.resource_set_programs.clear();
    }
}

impl VulkanDevice {
    /// Dispatch a complete native compute resource set. Buffer-only
    /// Generated Mesh / culling continues to use dispatch_buffer_batch.
    pub(super) fn dispatch_resource_set(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
    ) -> Result<()> {
        self.dispatch_resource_set_profiled(dispatch, None)
    }

    pub(super) fn dispatch_resource_set_profiled(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
        tag: Option<gfx_core::GpuTimestampTag>,
    ) -> Result<()> {
        dispatch.validate(self.compute.capabilities)?;
        let (layout_id, set_handle, desc) = {
            let set = self.bind_groups.get(dispatch.bind_group)?;
            (set.layout, set.descriptor_set, set.desc.clone())
        };
        let (set_layout, layout) = {
            let layout = self.bind_group_layouts.get(layout_id)?;
            (layout.layout, layout.desc.clone())
        };
        desc.validate_against(&layout)?;

        let mut buffer_bindings = Vec::new();
        // Track whole texture identities, not just view IDs. Storage images
        // must not alias any simultaneous sampled or writable view.
        let mut images = Vec::<(TextureId, bool)>::new();
        for entry in &layout.entries {
            if !entry.stages.contains(ShaderStages::COMPUTE) {
                return Err(Error::InvalidInput("Vulkan compute resource lacks stage visibility".into()));
            }
            let binding = desc.entries.iter().find(|b| b.binding == entry.binding)
                .ok_or_else(|| Error::InvalidInput("missing compute binding".into()))?;
            match (entry.binding_type, binding.resource) {
                (BindingType::UniformBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBufferReadWrite, BindingResource::Buffer(buffer)) => {
                    let access = match entry.binding_type {
                        BindingType::UniformBuffer => ComputeBufferAccess::Uniform,
                        BindingType::StorageBuffer => ComputeBufferAccess::ReadOnly,
                        _ => ComputeBufferAccess::ReadWrite,
                    };
                    buffer_bindings.push(gfx_core::ComputeBufferBinding {
                        binding: entry.binding, buffer, access,
                    });
                }
                (BindingType::SampledTexture, BindingResource::Texture(texture))
                | (BindingType::StorageTexture { .. }, BindingResource::Texture(texture)) => {
                    let id = self.texture_views.get(texture.texture_view)?.texture;
                    let writable = matches!(entry.binding_type, BindingType::StorageTexture { .. });
                    if let Some((_, earlier_writable)) = images.iter().find(|(image, _)| *image == id) {
                        if writable || *earlier_writable {
                            return Err(Error::InvalidInput(
                                "Vulkan compute storage image aliases another image binding".into()
                            ));
                        }
                    } else {
                        images.push((id, writable));
                    }
                }
                (BindingType::Sampler, BindingResource::Sampler(_)) => {}
                _ => return Err(Error::InvalidInput("Vulkan compute binding type mismatch".into())),
            }
        }
        ComputeDispatch {
            shader: dispatch.shader,
            bindings: &buffer_bindings,
            workgroups: dispatch.workgroups,
        }.validate_with_limits(self.compute.capabilities, self.buffer_binding_limits())?;

        // Read-only compute shaders do not produce incoherent storage writes.
        // Keep image layout transitions (including read-only storage images),
        // but do not emit a device-wide post-dispatch memory barrier for them.
        let writes_storage = buffer_bindings.iter().any(|binding|
            binding.access == ComputeBufferAccess::ReadWrite)
            || layout.entries.iter().any(|entry| matches!(
                entry.binding_type,
                BindingType::StorageTexture { read_only: false, .. }
            ));

        let key = (dispatch.shader, layout_id);
        let program = if let Some(program) = self.compute.resource_set_programs.get(&key) {
            program.clone()
        } else {
            let shader = self.shader_modules.get(dispatch.shader)?;
            let program = Arc::new(create_resource_set_program(
                self.device.clone(), self.pipeline_cache, shader, set_layout
            )?);
            self.compute.resource_set_programs.insert(key, program.clone());
            program
        };

        let encoder = self.create_command_encoder_with_buffer_count(1)?;
        let command = self.command_encoders.get(encoder)?.command_buffers[0];
        let query = self.command_encoders.get(encoder)?
            .gpu_trace_query_pool.filter(|_| tag.is_some());
        self.command_encoders.get_mut(encoder)?.gpu_work_tag =
            tag.filter(|_| query.is_some());
        let result = (|| -> Result<()> {
            unsafe {
                self.device.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                ).map_err(VulkanError::from)?;
                if let Some(query) = query {
                    self.device.cmd_reset_query_pool(command, query, 0, 2);
                }
            }
            let mut pre_images = Vec::with_capacity(images.len());
            let mut post_images = Vec::new();
            for &(id, writable) in &images {
                let texture = self.textures.get(id)?;
                let desired = if writable {
                    vk::ImageLayout::GENERAL
                } else {
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                };
                let range = vk::ImageSubresourceRange::default()
                    .aspect_mask(image_aspect_for_format(texture.desc.format))
                    .base_mip_level(0)
                    .level_count(texture.desc.mip_level_count)
                    .base_array_layer(0)
                    .layer_count(texture.desc.dimension.layers());
                pre_images.push(vk::ImageMemoryBarrier::default()
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .old_layout(texture.layout)
                    .new_layout(desired)
                    .image(texture.image)
                    .subresource_range(range)
                    .src_access_mask(if texture.layout == vk::ImageLayout::UNDEFINED {
                        vk::AccessFlags::empty()
                    } else {
                        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                    })
                    .dst_access_mask(vk::AccessFlags::SHADER_READ |
                        if writable { vk::AccessFlags::SHADER_WRITE } else { vk::AccessFlags::empty() }));
                // A storage-only image has no sampled-image usage. Keep it in
                // GENERAL across dispatches; the global post-dispatch memory
                // dependency below makes its writes visible without a redundant
                // layout transition. Images that can be sampled return to the
                // read-only layout required by their sampled descriptors.
                if writable && texture.desc.usage.contains(TextureUsage::SAMPLED) {
                    post_images.push(vk::ImageMemoryBarrier::default()
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .old_layout(vk::ImageLayout::GENERAL)
                        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .image(texture.image)
                        .subresource_range(range)
                        .src_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
                        .dst_access_mask(vk::AccessFlags::SHADER_READ));
                }
            }
            let before = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            // All dependencies remain GPU-side. Do not wait for the CPU to
            // observe completion between Compute and subsequent DrawIndirect.
            unsafe {
                self.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(), &[before], &[], &pre_images,
                );
                self.device.cmd_bind_pipeline(
                    command, vk::PipelineBindPoint::COMPUTE, program.pipeline,
                );
                self.device.cmd_bind_descriptor_sets(
                    command, vk::PipelineBindPoint::COMPUTE, program.layout, 0,
                    &[set_handle], &[],
                );
                if let Some(query) = query {
                    self.device.cmd_write_timestamp(
                        command, vk::PipelineStageFlags::TOP_OF_PIPE, query, 0,
                    );
                }
                self.device.cmd_dispatch(
                    command,
                    dispatch.workgroups[0], dispatch.workgroups[1], dispatch.workgroups[2],
                );
                if let Some(query) = query {
                    self.device.cmd_write_timestamp(
                        command, vk::PipelineStageFlags::BOTTOM_OF_PIPE, query, 1,
                    );
                }
                if writes_storage || !post_images.is_empty() {
                    let after = vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
                    let memory = if writes_storage {
                        std::slice::from_ref(&after)
                    } else {
                        &[]
                    };
                    self.device.cmd_pipeline_barrier(
                        command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::DependencyFlags::empty(), memory, &[], &post_images,
                    );
                }
                self.device.end_command_buffer(command).map_err(VulkanError::from)?;
            }
            self.submit_command_encoder_deferred(encoder)
        })();
        if let Err(error) = result {
            let _ = self.destroy_temporary_command_encoder_now(encoder);
            return Err(error);
        }
        for &(id, storage) in &images {
            let texture = self.textures.get_mut(id)?;
            texture.layout = if storage && !texture.desc.usage.contains(TextureUsage::SAMPLED) {
                vk::ImageLayout::GENERAL
            } else {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            };
        }
        self.poll_cleanup();
        Ok(())
    }

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
                    // A dispatch with no writable buffer cannot require a
                    // post-dispatch availability barrier. Avoid an otherwise
                    // empty ALL_COMMANDS execution dependency on the hot
                    // GPU meshing / culling batch path.
                    if !after.is_empty() {
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

fn create_resource_set_program(
    device: Arc<ash::Device>,
    cache: vk::PipelineCache,
    shader: &VulkanShaderModule,
    set_layout: vk::DescriptorSetLayout,
) -> Result<ResourceSetProgram> {
    if shader.stage != ShaderStage::Compute {
        return Err(Error::InvalidInput("resource set requires a compute shader".into()));
    }
    let entry = CString::new(shader.entry_point.as_str())
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    let mut program = ResourceSetProgram {
        device, layout: vk::PipelineLayout::null(), pipeline: vk::Pipeline::null(),
    };
    // This pipeline layout references the existing resource-set layout;
    // descriptor pools and sets remain owned by their GPU-fence-retired set.
    unsafe {
        program.layout = program.device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&[set_layout]),
            None,
        ).map_err(VulkanError::from)?;
        let info = vk::ComputePipelineCreateInfo::default()
            .layout(program.layout)
            .stage(vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(shader.module)
                .name(&entry));
        match program.device.create_compute_pipelines(cache, &[info], None) {
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
