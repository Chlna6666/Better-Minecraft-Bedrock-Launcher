//! Ordered CPU-visible writes and private GPU-buffer staging uploads.
use super::*;
use gfx_core::{BufferUploadStats, BufferWrite};

const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;
const MAX_BATCH_WRITES: usize = 256;

struct PrivateWrite<'a> {
    resource: Retained<ProtocolObject<dyn MTLBuffer>>,
    offset: usize,
    data: &'a [u8],
}

impl MetalDevice {
    pub(super) fn upload_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut private = Vec::new();
        let mut private_bytes = 0usize;
        for write in writes {
            if write.data.is_empty() { continue; }
            let buffer = self.buffers.get(write.descriptor.buffer)?;
            let offset = usize::try_from(write.descriptor.offset)
                .map_err(|error| Error::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset.checked_add(write.data.len())
                .ok_or_else(|| Error::InvalidInput("buffer write range overflow".into()))?;
            if end as u64 > buffer.desc.size {
                return Err(Error::InvalidInput("buffer write range is out of bounds".into()));
            }
            let resource = buffer.resource.as_ref()
                .ok_or_else(|| Error::Backend("Metal buffer has no native resource".into()))?.clone();
            match buffer.desc.memory_location {
                MemoryLocation::CpuToGpu => {
                    if !private.is_empty() {
                        self.flush_private_writes(&private, private_bytes)?;
                        private.clear();
                        private_bytes = 0;
                    }
                    // SAFETY: the shared allocation is live and the range was
                    // validated. The caller owns CPU-write/GPU-read retirement.
                    unsafe { std::ptr::copy_nonoverlapping(write.data.as_ptr(),
                        resource.contents().as_ptr().cast::<u8>().add(offset), write.data.len()); }
                }
                MemoryLocation::GpuOnly => {
                    if offset % 4 != 0 || write.data.len() % 4 != 0 {
                        return Err(Error::InvalidInput("Metal private buffer writes require four-byte aligned offset and size".into()));
                    }
                    if !private.is_empty() && (private_bytes.saturating_add(write.data.len()) > MAX_BATCH_BYTES
                        || private.len() >= MAX_BATCH_WRITES) {
                        self.flush_private_writes(&private, private_bytes)?;
                        private.clear();
                        private_bytes = 0;
                    }
                    private_bytes = private_bytes.checked_add(write.data.len())
                        .ok_or_else(|| Error::InvalidInput("Metal private upload batch overflow".into()))?;
                    private.push(PrivateWrite { resource, offset, data: write.data });
                }
                MemoryLocation::GpuToCpu => return Err(Error::Unavailable("Metal readback buffers are not upload destinations".into())),
            }
            stats.calls = stats.calls.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
        }
        if !private.is_empty() { self.flush_private_writes(&private, private_bytes)?; }
        Ok(stats)
    }

    fn flush_private_writes(&mut self, writes: &[PrivateWrite<'_>], bytes: usize) -> Result<()> {
        // Reuse the same bounded staging pages as texture uploads. In-flight
        // pages are never overwritten, and overflow uses retained temporary
        // allocations instead of waiting on the GPU.
        let (staging, page) = self.texture_uploads.acquire(&self.device, bytes)?;
        let commands = self.command_queue.commandBuffer()
            .ok_or_else(|| Error::Backend("Metal buffer upload command buffer unavailable".into()))?;
        let blit = commands.blitCommandEncoder()
            .ok_or_else(|| Error::Backend("Metal buffer upload encoder unavailable".into()))?;
        let mut base = 0usize;
        for write in writes {
            // SAFETY: all destination ranges and four-byte alignment were
            // checked, staging has capacity for the complete batch, and
            // the native command buffer retains both buffers until completion.
            unsafe {
                std::ptr::copy_nonoverlapping(write.data.as_ptr(),
                    staging.contents().as_ptr().cast::<u8>().add(base), write.data.len());
                blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                    &staging, base, &write.resource, write.offset, write.data.len());
            }
            base += write.data.len();
        }
        blit.endEncoding();
        commands.commit();
        self.texture_uploads.submitted(page, bytes, commands);
        Ok(())
    }
}
