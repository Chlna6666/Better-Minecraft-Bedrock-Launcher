//! Shared-storage buffer batches preserve CPU shadows without repeated contents queries.

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
            if write.data.is_empty() {
                continue;
            }
            let id = write.descriptor.buffer;
            let buffer = self.buffers.get_mut(id)?;
            let storage = buffer.data.as_mut().ok_or_else(|| {
                Error::Unavailable(
                    "Metal GPU-only staging upload is not enabled in this build".into(),
                )
            })?;
            let offset = usize::try_from(write.descriptor.offset)
                .map_err(|error| Error::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(write.data.len())
                .ok_or_else(|| Error::InvalidInput("buffer write range overflow".into()))?;
            let target = storage
                .get_mut(offset..end)
                .ok_or_else(|| Error::InvalidInput("buffer write range is out of bounds".into()))?;
            target.copy_from_slice(write.data);
            if mapped_id != Some(id) {
                pointer = buffer
                    .resource
                    .as_ref()
                    .map(|resource| resource.contents().as_ptr().cast::<u8>());
                mapped_id = Some(id);
            }
            if let Some(pointer) = pointer {
                // SAFETY: Shared storage stays live throughout this owner-thread call; the shadow
                // range check covers the identically sized native buffer, with no in-flight reuse.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        write.data.as_ptr(),
                        pointer.add(offset),
                        write.data.len(),
                    );
                }
                stats.calls = stats.calls.saturating_add(1);
                stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
            }
        }
        Ok(stats)
    }
}
