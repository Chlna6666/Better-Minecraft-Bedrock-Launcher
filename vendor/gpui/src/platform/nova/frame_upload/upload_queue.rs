use super::upload_encoding::{atlas_source_byte_len, encode_bgra_upload_with_padding};
use super::*;

const NOVA_ATLAS_RETAINED_UPLOAD_BYTES: usize = 32 * 1024 * 1024;
const NOVA_ATLAS_RETAINED_UPLOAD_COUNT: usize = 4096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::nova) struct AtlasUploadStats {
    pub(in crate::platform::nova) uploaded_bytes: usize,
    pub(in crate::platform::nova) upload_count: usize,
    pub(in crate::platform::nova) arena_used_bytes: usize,
    pub(in crate::platform::nova) arena_capacity: usize,
}

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct PendingAtlasUpload {
    texture_id: AtlasTextureId,
    origin: Origin2d,
    size: Extent2d,
    bytes_per_row: u32,
    offset: usize,
    len: usize,
}

#[derive(Default)]
struct AtlasUploadBatch {
    bytes: Vec<u8>,
    uploads: Vec<PendingAtlasUpload>,
}

impl NovaAtlas {
    pub(in crate::platform::nova) fn upload_pending_rgba_pixels(
        &self,
        mut resolve_texture: impl FnMut(AtlasTextureId) -> Result<TextureId>,
        mut upload: impl FnMut(&[TextureWrite<'_>]) -> Result<()>,
    ) -> Result<AtlasUploadStats> {
        let batch = self.take_pending_uploads();
        let mut stats = AtlasUploadStats {
            arena_used_bytes: batch.bytes.len(),
            arena_capacity: batch.bytes.capacity(),
            ..AtlasUploadStats::default()
        };
        if batch.uploads.is_empty() {
            self.recycle_upload_batch(batch);
            return Ok(stats);
        }
        let result = (|| {
            let mut writes = Vec::with_capacity(batch.uploads.len());
            for pending_upload in &batch.uploads {
                let end = pending_upload
                    .offset
                    .checked_add(pending_upload.len)
                    .ok_or_else(|| anyhow::anyhow!("nova atlas upload range overflow"))?;
                let pixels = batch.bytes.get(pending_upload.offset..end).ok_or_else(|| {
                    anyhow::anyhow!("nova atlas pending upload range is out of bounds")
                })?;
                writes.push(TextureWrite {
                    descriptor: TextureWriteDescriptor {
                        texture: resolve_texture(pending_upload.texture_id)?,
                        layout: TextureDataLayout::new(
                            0,
                            pending_upload.bytes_per_row,
                            pending_upload.size.height(),
                        )?,
                        origin: pending_upload.origin,
                        size: pending_upload.size,
                    },
                    data: pixels,
                });
                stats.uploaded_bytes = stats.uploaded_bytes.saturating_add(pixels.len());
                stats.upload_count = stats.upload_count.saturating_add(1);
            }
            upload(&writes)?;
            Ok(())
        })();
        if result.is_ok() {
            self.recycle_upload_batch(batch);
        } else {
            self.restore_upload_batch(batch);
        }
        result.map(|()| stats)
    }

    fn take_pending_uploads(&self) -> AtlasUploadBatch {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        AtlasUploadBatch {
            bytes: std::mem::take(&mut state.upload_bytes),
            uploads: std::mem::take(&mut state.pending_uploads),
        }
    }

    fn recycle_upload_batch(&self, mut batch: AtlasUploadBatch) {
        batch.bytes.clear();
        batch.uploads.clear();
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if state.upload_bytes.is_empty()
            && state.pending_uploads.is_empty()
            && batch.bytes.capacity() > state.upload_bytes.capacity()
            && batch.bytes.capacity() <= NOVA_ATLAS_RETAINED_UPLOAD_BYTES
        {
            state.upload_bytes = batch.bytes;
        }
        if state.pending_uploads.is_empty()
            && batch.uploads.capacity() > state.pending_uploads.capacity()
            && batch.uploads.capacity() <= NOVA_ATLAS_RETAINED_UPLOAD_COUNT
        {
            state.pending_uploads = batch.uploads;
        }
    }

    fn restore_upload_batch(&self, batch: AtlasUploadBatch) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let base_offset = state.upload_bytes.len();
        state.upload_bytes.extend_from_slice(&batch.bytes);
        state.pending_uploads.reserve(batch.uploads.len());
        for mut upload in batch.uploads {
            upload.offset = upload.offset.saturating_add(base_offset);
            state.pending_uploads.push(upload);
        }
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn pending_upload_bytes_for_test(&self) -> Vec<u8> {
        self.state
            .lock()
            .expect("nova atlas lock poisoned")
            .upload_bytes
            .clone()
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn pending_upload_count_for_test(&self) -> usize {
        self.state
            .lock()
            .expect("nova atlas lock poisoned")
            .pending_uploads
            .len()
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn clear_pending_uploads_for_test(&self) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        state.upload_bytes.clear();
        state.pending_uploads.clear();
    }
}

impl NovaAtlasState {
    pub(in crate::platform::nova) fn enqueue_tile_upload(
        &mut self,
        texture_id: AtlasTextureId,
        texture_kind: AtlasTextureKind,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        bytes: &[u8],
        padding: u32,
    ) -> bool {
        self.enqueue_tile_upload_kind(texture_id, texture_kind, origin, size, bytes, padding)
    }

    pub(in crate::platform::nova) fn enqueue_tile_upload_kind(
        &mut self,
        texture_id: AtlasTextureId,
        texture_kind: AtlasTextureKind,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        bytes: &[u8],
        padding: u32,
    ) -> bool {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let upload_width = width.saturating_add(padding.saturating_mul(2));
        let upload_height = height.saturating_add(padding.saturating_mul(2));
        let Ok(extent) = Extent2d::new(upload_width, upload_height) else {
            return false;
        };
        let Some(bytes_per_row) = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL as u32)
        else {
            return false;
        };
        let Some(source_len) = atlas_source_byte_len(size, texture_kind) else {
            return false;
        };
        if bytes.len() < source_len {
            return false;
        }
        let Some(len) = bytes_per_row
            .checked_mul(upload_height)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let upload_origin = Origin2d {
            x: origin
                .x
                .0
                .saturating_sub(i32::try_from(padding).unwrap_or(0))
                .max(0) as u32,
            y: origin
                .y
                .0
                .saturating_sub(i32::try_from(padding).unwrap_or(0))
                .max(0) as u32,
        };
        if let Some(pending_upload) = self.pending_uploads.iter().rev().find(|upload| {
            upload.texture_id == texture_id
                && upload.origin == upload_origin
                && upload.size == extent
                && upload.bytes_per_row == bytes_per_row
                && upload.len == len
        }) {
            let Some(end) = pending_upload.offset.checked_add(pending_upload.len) else {
                return false;
            };
            let Some(pixels) = self.upload_bytes.get_mut(pending_upload.offset..end) else {
                return false;
            };
            return encode_bgra_upload_with_padding(pixels, size, bytes, texture_kind, padding)
                .is_some();
        }
        let offset = self.upload_bytes.len();
        let Some(end) = offset.checked_add(len) else {
            return false;
        };
        self.upload_bytes.resize(end, 0);
        if encode_bgra_upload_with_padding(
            &mut self.upload_bytes[offset..end],
            size,
            bytes,
            texture_kind,
            padding,
        )
        .is_none()
        {
            self.upload_bytes.truncate(offset);
            return false;
        }
        self.pending_uploads.push(PendingAtlasUpload {
            texture_id,
            origin: upload_origin,
            size: extent,
            bytes_per_row,
            offset,
            len,
        });
        true
    }
}
