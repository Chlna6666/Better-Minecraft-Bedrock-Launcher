//! Direct writes into CPU-visible native shared buffers, without CPU mirrors.

use super::*;
use gfx_core::{BufferUploadStats, BufferWrite};

impl MetalDevice {
    pub(super) fn upload_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut mapped_id = None;
        let mut pointer = None;
        for write in writes {
            let id = write.descriptor.buffer;
            let buffer = self.buffers.get(id)?;
            if buffer.desc.memory_location != MemoryLocation::CpuToGpu {
                return Err(Error::Unavailable(
                    "Metal GPU-only staging upload is not enabled in this build".into(),
                ));
            }
            let offset = usize::try_from(write.descriptor.offset)
                .map_err(|error| Error::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset.checked_add(write.data.len())
                .ok_or_else(|| Error::InvalidInput("buffer write range overflow".into()))?;
            if end as u64 > buffer.desc.size {
                return Err(Error::InvalidInput("buffer write range is out of bounds".into()));
            }
            if write.data.is_empty() {
                continue;
            }
            if mapped_id != Some(id) {
                let resource = buffer.resource.as_ref()
                    .ok_or_else(|| Error::Backend("Metal buffer has no native resource".into()))?;
                pointer = Some(resource.contents().as_ptr().cast::<u8>());
                mapped_id = Some(id);
            }
            let pointer = pointer.expect("validated shared buffer");
            // SAFETY: The live native shared allocation has the validated size.
            // The caller must not reuse bytes with outstanding GPU readers;
            // the owner and frame-slot retirement contract are unchanged.
            unsafe {
                std::ptr::copy_nonoverlapping(write.data.as_ptr(), pointer.add(offset), write.data.len());
            }
            stats.calls = stats.calls.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
        }
        Ok(stats)
    }
}
