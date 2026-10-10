//! Synchronous buffer batches: reuse mappings and submit device-local copies together.

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
        let staging_desc = BufferDescriptor {
            label: Some("nova-gfx DX12 buffer batch staging".into()),
            size: staging_size,
            usage: BufferUsage::COPY_SRC,
            memory_location: MemoryLocation::CpuToGpu,
        };
        let staging = create_buffer_resource(&self.device, &staging_desc)?;
        let staging_map = MappedBuffer::new(staging.clone())?;
        for (write, _, _, offset) in &copies {
            let offset = usize::try_from(*offset).map_err(|error| {
                Error::InvalidInput(format!("staging offset overflow: {error}"))
            })?;
            staging_map.write(offset, write.data);
        }
        drop(staging_map);

        let allocator = create_command_allocator(&self.device)?;
        let commands = create_command_list(&self.device, &allocator)?;
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
            // SAFETY: All ranges were validated; staging and destinations live through the wait.
            unsafe {
                commands.CopyBufferRegion(
                    destination,
                    write.descriptor.offset,
                    &staging,
                    *source_offset,
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
        // Own the submitted resources before Execute/Signal, including failed fence waits.
        self.deferred_releases.retire(
            fence_value,
            DeferredDx12Release::Buffer(Dx12Buffer {
                desc: staging_desc,
                resource: Some(staging),
                state: D3D12_RESOURCE_STATE_GENERIC_READ,
            }),
        );
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
        self.wait_for_fence_value(fence_value)?;
        self.poll_cleanup();
        Ok(stats)
    }
}
