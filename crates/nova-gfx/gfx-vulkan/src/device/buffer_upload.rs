//! Buffer batches retain staging and commands under one submission fence.

use super::*;

#[cfg(test)]
mod tests;

impl VulkanDevice {
    pub(super) fn upload_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut copies = Vec::new();
        for write in writes {
            if write.data.is_empty() {
                continue;
            }
            let buffer = self.buffers.get(write.descriptor.buffer)?;
            if write
                .descriptor
                .offset
                .checked_add(write.data.len() as u64)
                .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "buffer write range is out of bounds".into(),
                ));
            }
            if buffer.desc.memory_location == MemoryLocation::CpuToGpu {
                self.write_buffer(write.descriptor.buffer, write.descriptor.offset, write.data)?;
            } else {
                copies.push((write, buffer.buffer));
            }
            stats.calls = stats.calls.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
        }
        if copies.is_empty() {
            return Ok(stats);
        }
        let staged = match self.stage_buffer_copies(copies) {
            Ok(staged) => staged,
            Err(error) => {
                self.upload_ring.discard_unsubmitted();
                return Err(error);
            }
        };
        let (commands, fence) = match self.submit_buffer_copies(&staged) {
            Ok(submission) => submission,
            Err(error) => {
                self.upload_ring.discard_unsubmitted();
                return Err(error);
            }
        };
        // Register without polling so a fast completion cannot destroy the fence before wait.
        self.enqueue_deferred_upload(commands, fence);
        // SAFETY: The registered fence belongs to this submission and stays live during the wait.
        unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
            .map_err(VulkanError::from)?;
        self.poll_cleanup();
        Ok(stats)
    }

    fn stage_buffer_copies<'a>(
        &mut self,
        copies: Vec<(BufferWrite<'a>, vk::Buffer)>,
    ) -> Result<Vec<StagedBufferCopy<'a>>> {
        let sizes: Vec<_> = copies
            .iter()
            .map(|(write, _)| write.data.len() as u64)
            .collect();
        let allocations = self.upload_ring.allocate_batch(&sizes)?;
        let mut staged = Vec::with_capacity(copies.len());
        for ((write, destination), allocation) in copies.into_iter().zip(allocations) {
            self.stage_upload_data(write.data, allocation)?;
            staged.push(StagedBufferCopy {
                write,
                destination,
                source: self.upload_page_buffer(allocation.page_index)?,
                allocation,
            });
        }
        Ok(staged)
    }

    fn submit_buffer_copies(
        &self,
        staged: &[StagedBufferCopy<'_>],
    ) -> Result<(VulkanUploadCommands, vk::Fence)> {
        // No timestamp queries: buffer copies must not overwrite texture-transfer timing.
        let commands =
            create_upload_commands(&self.device, self.graphics_queue_family_index, false)?;
        if let Err(error) = begin_one_time_commands(&self.device, commands.command_buffer) {
            destroy_upload_commands(&self.device, &commands);
            return Err(error);
        }
        let mut synchronized = 0;
        for (index, copy) in staged.iter().enumerate() {
            let StagedBufferCopy {
                write,
                destination,
                source,
                allocation,
            } = copy;
            if staged[synchronized..index]
                .iter()
                .any(|previous| writes_overlap(&previous.write, write))
            {
                let barrier = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                // SAFETY: The list is recording; order overlapping destinations in caller order.
                unsafe {
                    self.device.cmd_pipeline_barrier(
                        commands.command_buffer,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[barrier],
                        &[],
                        &[],
                    );
                }
                synchronized = index;
            }
            let region = vk::BufferCopy::default()
                .src_offset(allocation.offset)
                .dst_offset(write.descriptor.offset)
                .size(allocation.size);
            // SAFETY: Source staging and validated destinations remain live through the fence.
            unsafe {
                self.device.cmd_copy_buffer(
                    commands.command_buffer,
                    *source,
                    *destination,
                    &[region],
                );
            }
        }
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ);
        // SAFETY: Make completed transfer writes available to subsequent graphics consumers.
        unsafe {
            self.device.cmd_pipeline_barrier(
                commands.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
        let fence =
            match end_submit_deferred(&self.device, self.graphics_queue, commands.command_buffer) {
                Ok(fence) => fence,
                Err(error) => {
                    destroy_upload_commands(&self.device, &commands);
                    return Err(error);
                }
            };
        Ok((commands, fence))
    }
}

struct StagedBufferCopy<'a> {
    write: BufferWrite<'a>,
    destination: vk::Buffer,
    source: vk::Buffer,
    allocation: UploadAllocation,
}

fn writes_overlap(first: &BufferWrite<'_>, second: &BufferWrite<'_>) -> bool {
    first.descriptor.buffer == second.descriptor.buffer
        && first.descriptor.offset < second.descriptor.offset + second.data.len() as u64
        && second.descriptor.offset < first.descriptor.offset + first.data.len() as u64
}
