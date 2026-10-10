//! Combine consecutive constant-buffer patches into one full native update.

use super::*;

impl Dx11Device {
    pub(super) fn upload_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut pending = None;
        let result = (|| {
            for write in writes {
                if write.data.is_empty() {
                    continue;
                }
                let id = write.descriptor.buffer;
                if pending.is_some_and(|previous| previous != id) {
                    self.flush_uniform_upload(pending.take(), &mut stats)?;
                }
                let buffer = self.buffers.get_mut(id)?;
                let offset = write.descriptor.offset;
                if offset
                    .checked_add(write.data.len() as u64)
                    .is_none_or(|end| end > buffer.desc.size)
                {
                    return Err(Error::InvalidInput(
                        "D3D11 buffer write out of range".into(),
                    ));
                }
                if let Some(bytes) = &mut buffer.uniform_bytes {
                    let offset = offset as usize;
                    bytes[offset..offset + write.data.len()].copy_from_slice(write.data);
                    pending = Some(id);
                } else {
                    ResourceDevice::write_buffer(self, id, offset, write.data)?;
                    stats.calls = stats.calls.saturating_add(1);
                    stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
                }
            }
            Ok(())
        })();
        // Keep the native buffer consistent with accepted shadow patches even on partial failure.
        self.flush_uniform_upload(pending, &mut stats)?;
        result?;
        Ok(stats)
    }

    fn flush_uniform_upload(
        &self,
        id: Option<BufferId>,
        stats: &mut BufferUploadStats,
    ) -> Result<()> {
        if let Some(id) = id {
            let buffer = self.buffers.get(id)?;
            if let Some(bytes) = &buffer.uniform_bytes {
                // SAFETY: The shadow covers the complete native constant buffer; the immediate
                // context consumes these bytes before returning, including its alignment padding.
                unsafe {
                    self.context.UpdateSubresource(
                        &buffer.native,
                        0,
                        None,
                        bytes.as_ptr().cast(),
                        0,
                        0,
                    );
                }
                stats.calls = stats.calls.saturating_add(1);
                stats.bytes = stats.bytes.saturating_add(u64::from(buffer.native_size));
            }
        }
        Ok(())
    }
}
