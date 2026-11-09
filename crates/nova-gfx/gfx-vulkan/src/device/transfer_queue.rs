//! Native GPU-to-GPU transfers on one verified graphics-family physical queue.
//! Dedicated transfer-family ownership transitions are intentionally unavailable
//! until every Vulkan buffer/image release/acquire barrier can be proven.

use super::*;

impl VulkanDevice {
    /// Execute actual vkCmdCopyBuffer commands on PRIMARY or the optional
    /// second graphics-family queue. The SubmissionId always fences that
    /// physical producer, never a signal on an unrelated queue.
    pub(super) fn submit_native_buffer_copies(
        &mut self,
        queue: gfx_core::GpuQueueId,
        copies: &[gfx_core::GpuBufferCopy],
    ) -> Result<SubmissionId> {
        if copies.is_empty() {
            return Err(Error::InvalidInput("native Transfer batch is empty".into()));
        }
        let secondary = self.independent_queue(gfx_core::GpuWorkStage::Transfer);
        if queue != gfx_core::GpuQueueId::PRIMARY && secondary != Some(queue) {
            return Err(Error::Unavailable(
                "Vulkan native Transfer queue is not in an owned graphics family".into(),
            ));
        }
        // Validate every range and obtain native handles BEFORE submitting
        // anything, so partially invalid jobs cannot mutate GPU allocations.
        let mut validated = Vec::with_capacity(copies.len());
        for copy in copies {
            copy.validate()?;
            let source = self.buffers.get(copy.source)?;
            let destination = self.buffers.get(copy.destination)?;
            if !source.desc.usage.contains(BufferUsage::COPY_SRC)
                || !destination.desc.usage.contains(BufferUsage::COPY_DST)
            {
                return Err(Error::InvalidInput(
                    "Vulkan transfer requires COPY_SRC and COPY_DST usage".into(),
                ));
            }
            if copy.source_offset.checked_add(copy.bytes).is_none_or(|end| end > source.desc.size)
                || copy.destination_offset.checked_add(copy.bytes)
                    .is_none_or(|end| end > destination.desc.size)
            {
                return Err(Error::InvalidInput("Vulkan transfer range out of bounds".into()));
            }
            validated.push((
                source.buffer, destination.buffer,
                copy.source, copy.destination,
                vk::BufferCopy::default()
                    .src_offset(copy.source_offset)
                    .dst_offset(copy.destination_offset)
                    .size(copy.bytes),
            ));
        }
        let encoder = self.create_command_encoder_with_buffer_count(1)?;
        let command = self.command_encoders.get(encoder)?.command_buffers[0];
        let result = (|| -> Result<SubmissionId> {
            // SAFETY: The command buffer was allocated from this device's
            // graphics-family command pool and is not in flight.
            unsafe {
                self.device.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                ).map_err(VulkanError::from)?;
            }
            let mut unique = std::collections::HashSet::new();
            let before = validated.iter().flat_map(|(source, destination, _, _, _)| [
                *source, *destination,
            ]).filter(|buffer| unique.insert(*buffer)).map(|buffer| {
                vk::BufferMemoryBarrier::default()
                    .src_access_mask(
                        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                            | vk::AccessFlags::HOST_WRITE,
                    )
                    .dst_access_mask(
                        vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE,
                    )
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(buffer)
                    .offset(0)
                    .size(vk::WHOLE_SIZE)
            }).collect::<Vec<_>>();
            // SAFETY: Both queues belong to the same graphics family. The
            // native GPU semaphore join establishes queue-to-queue execution
            // order; barriers make prior writes available to transfer.
            unsafe {
                self.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[], &before, &[],
                );
            }
            let mut written = std::collections::HashSet::new();
            for (source, destination, source_id, destination_id, region) in &validated {
                if written.contains(source_id) || written.contains(destination_id) {
                    let dependency = vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                        .dst_access_mask(
                            vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE
                        );
                    // SAFETY: Two dependent Copy commands in one command
                    // buffer need an explicit Transfer->Transfer dependency.
                    unsafe {
                        self.device.cmd_pipeline_barrier(
                            command, vk::PipelineStageFlags::TRANSFER,
                            vk::PipelineStageFlags::TRANSFER,
                            vk::DependencyFlags::empty(),
                            &[dependency], &[], &[],
                        );
                    }
                }
                // SAFETY: Both native handles and non-overlapping regions have
                // been checked, and their allocations remain live through
                // the native command encoder's retirement fence.
                unsafe {
                    self.device.cmd_copy_buffer(command, *source, *destination, &[*region]);
                }
                written.insert(*destination_id);
            }
            let after = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(
                    vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                );
            // SAFETY: Copies finish before subsequent Graphics/Compute
            // consumers on either graphics-family queue.
            unsafe {
                self.device.cmd_pipeline_barrier(
                    command, vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(), &[after], &[], &[],
                );
                self.device.end_command_buffer(command).map_err(VulkanError::from)?;
            }
            if queue == gfx_core::GpuQueueId::PRIMARY {
                self.submit_command_encoder_deferred_tracked(encoder)
            } else {
                self.submit_command_encoder_independent(encoder, true)?
                    .ok_or_else(|| Error::Unavailable(
                        "Vulkan secondary transfer did not produce a native fence".into(),
                    ))
            }
        })();
        if result.is_err() {
            let _ = self.destroy_temporary_command_encoder_now(encoder);
        }
        result
    }
}
