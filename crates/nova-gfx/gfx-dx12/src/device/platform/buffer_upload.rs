//! Ordered buffer batches retain staging and commands until their submission fence completes.

use std::collections::HashSet;

use super::*;

#[cfg(test)]
mod tests;

struct MappedBuffer {
    resource: ID3D12Resource,
    pointer: *mut u8,
}

impl MappedBuffer {
    fn new(resource: ID3D12Resource) -> Result<Self> {
        let mut pointer = ptr::null_mut();
        let read_range = D3D12_RANGE { Begin: 0, End: 0 };
        // SAFETY: Only upload heap resources enter this helper; the CPU never reads them.
        unsafe { resource.Map(0, Some(&read_range), Some(&raw mut pointer)) }
            .map_err(|error| Error::Backend(error.to_string()))?;
        Ok(Self {
            resource,
            pointer: pointer.cast(),
        })
    }

    fn write(&self, offset: usize, data: &[u8]) {
        // SAFETY: Callers validate the entire range against the live mapped resource.
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), self.pointer.add(offset), data.len());
        }
    }
}

impl Drop for MappedBuffer {
    fn drop(&mut self) {
        // SAFETY: This object owns exactly one successful Map, including on error paths.
        unsafe { self.resource.Unmap(0, None) };
    }
}

impl Dx12Device {
    pub(super) fn upload_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut mapped = None;
        let mut mapped_id = None;
        let mut copies = Vec::new();
        let mut staging_size = 0_u64;
        for write in writes {
            if write.data.is_empty() {
                continue;
            }
            let id = write.descriptor.buffer;
            let buffer = self.buffers.get(id)?;
            let length = write.data.len() as u64;
            if write
                .descriptor
                .offset
                .checked_add(length)
                .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "buffer write range is out of bounds".into(),
                ));
            }
            let resource = buffer
                .resource
                .clone()
                .ok_or_else(|| Error::Backend("DX12 buffer has no native resource".into()))?;
            match buffer.desc.memory_location {
                MemoryLocation::CpuToGpu => {
                    if mapped_id != Some(id) {
                        mapped = Some(MappedBuffer::new(resource)?);
                        mapped_id = Some(id);
                    }
                    let offset = usize::try_from(write.descriptor.offset).map_err(|error| {
                        Error::InvalidInput(format!("offset overflow: {error}"))
                    })?;
                    if let Some(mapped) = &mapped {
                        mapped.write(offset, write.data);
                    }
                }
                MemoryLocation::GpuToCpu => {
                    return Err(Error::Unavailable(
                        "DX12 readback buffers are written by the GPU, not the CPU".into(),
                    ));
                }
                MemoryLocation::GpuOnly => {
                    copies.push((write, resource, buffer.state, staging_size));
                    staging_size = staging_size.checked_add(length).ok_or_else(|| {
                        Error::InvalidInput("buffer staging size overflow".into())
                    })?;
                }
            }
            stats.calls = stats.calls.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(length);
        }
        drop(mapped);
        if copies.is_empty() {
            return Ok(stats);
        }
        self.poll_cleanup();
        let result = (|| {
            let allocation = self.upload_ring.allocate(staging_size)?;
            self.ensure_upload_page(allocation.page_index)?;
            let page = self
                .upload_pages
                .get(allocation.page_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| Error::Backend("missing DX12 buffer upload page".into()))?;
            for (write, _, _, offset) in &copies {
                let offset = usize::try_from(allocation.offset + offset).map_err(|error| {
                    Error::InvalidInput(format!("buffer upload offset overflow: {error}"))
                })?;
                // SAFETY: the mapped page owns this checked ring allocation, which is not reused until its fence completes.
                unsafe {
                    ptr::copy_nonoverlapping(
                        write.data.as_ptr(),
                        page.mapped.as_ptr().add(offset),
                        write.data.len(),
                    );
                }
            }
            let staging = self.upload_page_resource(allocation.page_index)?;
            let (allocator, commands) = if let Some(index) = self
                .upload_command_pool
                .iter()
                .position(|commands| commands.timestamps.is_none())
            {
                let recycled = self.upload_command_pool.swap_remove(index);
                // SAFETY: upload commands enter the pool only after their fence completes.
                unsafe {
                    recycled
                        .allocator
                        .Reset()
                        .map_err(|e| Error::Backend(e.to_string()))?;
                    recycled
                        .graphics_command_list
                        .Reset(&recycled.allocator, None)
                        .map_err(|e| Error::Backend(e.to_string()))?;
                }
                (recycled.allocator, recycled.graphics_command_list)
            } else {
                let allocator = create_command_allocator(&self.device)?;
                let commands = create_command_list(&self.device, &allocator)?;
                (allocator, commands)
            };
            let mut touched = HashSet::new();
            for (write, destination, initial_state, source_offset) in &copies {
                let state = if touched.insert(write.descriptor.buffer) {
                    *initial_state
                } else {
                    D3D12_RESOURCE_STATE_GENERIC_READ
                };
                if state != D3D12_RESOURCE_STATE_COPY_DEST {
                    record_transition_barrier(
                        &commands,
                        destination,
                        state,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    );
                }
                // Upload heaps remain GENERIC_READ, which already includes COPY_SOURCE.
                // SAFETY: Validated ranges remain live until the submission fence retires.
                unsafe {
                    commands.CopyBufferRegion(
                        destination,
                        write.descriptor.offset,
                        &staging,
                        allocation.offset + *source_offset,
                        write.data.len() as u64,
                    );
                }
                // This transition also orders overlapping writes to the same destination.
                record_transition_barrier(
                    &commands,
                    destination,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                );
            }
            // SAFETY: The list is recording and all resource dependencies are live.
            unsafe { commands.Close() }.map_err(|error| Error::Backend(error.to_string()))?;
            let executable: ID3D12CommandList = commands
                .cast()
                .map_err(|error| Error::Backend(error.to_string()))?;
            let fence_value = self.next_fence_value;
            // Own submitted resources before Execute/Signal, including a failed queue signal.
            self.upload_ring.retire_used_pages(fence_value);
            self.pending_texture_uploads.retire(
                fence_value,
                Dx12SubmittedCommandList {
                    commands: Dx12UploadCommands {
                        allocator,
                        graphics_command_list: commands,
                        timestamps: None,
                    },
                    _command_list: executable.clone(),
                },
            );
            // SAFETY: The closed list and its resources are retained until this fence completes.
            unsafe { self.graphics_queue.ExecuteCommandLists(&[Some(executable)]) };
            for id in touched {
                self.buffers.get_mut(id)?.state = D3D12_RESOURCE_STATE_GENERIC_READ;
            }
            self.signal_frame()?;
            self.poll_cleanup();
            Ok(())
        })();
        if result.is_err() {
            self.upload_ring.discard_unsubmitted();
        }
        result.map(|()| stats)
    }
}
