use super::upload_encoding::{atlas_source_byte_len, encode_atlas_upload_with_padding};
use super::*;
#[cfg(feature = "bench-support")]
use crate::{ImageId, ImagePixelFormat, RenderImageParams, size};
#[cfg(feature = "bench-support")]
use std::borrow::Cow;

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
    pub(in crate::platform::nova) texture_id: AtlasTextureId,
    origin: Origin2d,
    size: Extent2d,
    bytes_per_row: u32,
    offset: usize,
    len: usize,
}

#[derive(Default)]
pub(in crate::platform::nova) struct AtlasUploadBatch {
    bytes: Vec<u8>,
    uploads: Vec<PendingAtlasUpload>,
    // Captured under the SAME lock as the queued writes. New pages created
    // after this point belong to the next batch, not the current GPU upload.
    texture_infos: Vec<NovaAtlasTextureInfo>,
}

impl AtlasUploadBatch {
    pub(in crate::platform::nova) fn texture_infos(&self) -> &[NovaAtlasTextureInfo] {
        &self.texture_infos
    }
}

#[cfg(feature = "bench-support")]
pub(crate) struct AtlasUploadBenchmarkCore {
    atlas: NovaAtlas,
}

#[cfg(feature = "bench-support")]
impl AtlasUploadBenchmarkCore {
    pub(crate) fn rgba_tiles(upload_count: usize, tile_size: u32) -> Self {
        let atlas = NovaAtlas::new();
        atlas
            .upload_pending_rgba_pixels(|_| Ok(TextureId::from_parts(0, 0)), |_| Ok(()))
            .expect("fallback atlas uploads must be valid");

        let tile_extent = i32::try_from(tile_size).expect("benchmark tile size must fit i32");
        let tile_size = usize::try_from(tile_size).expect("benchmark tile size must fit usize");
        let tile_byte_len = tile_size
            .checked_mul(tile_size)
            .and_then(|pixel_count| pixel_count.checked_mul(4))
            .expect("benchmark tile byte length must fit usize");
        for image_id in 0..upload_count {
            let pixels = vec![255; tile_byte_len];
            let key = AtlasKey::Image(RenderImageParams {
                image_id: ImageId(image_id),
                frame_slot: 0,
                pixel_format: ImagePixelFormat::Rgba8,
            });
            atlas
                .ensure_tile_with(key.clone(), &mut || {
                    Ok(Some((
                        size(DevicePixels(tile_extent), DevicePixels(tile_extent)),
                        Cow::Borrowed(&pixels),
                    )))
                })
                .expect("benchmark atlas insertion must succeed")
                .expect("benchmark atlas must retain every image");
        }
        Self { atlas }
    }

    pub(crate) fn upload(&self) -> (usize, usize) {
        let stats = self
            .atlas
            .upload_pending_rgba_pixels(|_| Ok(TextureId::from_parts(0, 0)), |_| Ok(()))
            .expect("benchmark atlas upload must succeed");
        (stats.upload_count, stats.uploaded_bytes)
    }
}

impl NovaAtlas {
    pub(in crate::platform::nova) fn upload_pending_rgba_pixels(
        &self,
        resolve_texture: impl FnMut(AtlasTextureId) -> Result<TextureId>,
        upload: impl FnMut(&[TextureWrite<'_>]) -> Result<()>,
    ) -> Result<AtlasUploadStats> {
        let batch = self.take_pending_uploads();
        self.upload_taken_rgba_pixels(batch, resolve_texture, upload)
    }

    pub(in crate::platform::nova) fn upload_taken_rgba_pixels(
        &self,
        batch: AtlasUploadBatch,
        mut resolve_texture: impl FnMut(AtlasTextureId) -> Result<TextureId>,
        mut upload: impl FnMut(&[TextureWrite<'_>]) -> Result<()>,
    ) -> Result<AtlasUploadStats> {
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
                        mip_level: 0,
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

    /// Returns whether queued Atlas uploads can change texels actually sampled by
    /// the retained backdrop source. A shared atlas page is not a dependency:
    /// only its resident, painted tile regions are. If tile bookkeeping is
    /// unavailable, conservatively invalidate rather than reuse stale pixels.
    pub(in crate::platform::nova) fn pending_uploads_touch_source_tiles(
        &self,
        texture_ids: &FxHashSet<AtlasTextureId>,
        tiles: &FxHashMap<AtlasTextureId, Vec<AtlasTile>>,
    ) -> bool {
        if texture_ids.is_empty() {
            return false;
        }
        let state = self.state.lock().expect("nova atlas lock poisoned");
        state.pending_uploads.iter().any(|upload| {
            if !texture_ids.contains(&upload.texture_id) {
                return false;
            }
            let Some(used_tiles) = tiles.get(&upload.texture_id) else {
                return true; // Unknown tile layout: do not trust the cached filter.
            };
            used_tiles.is_empty()
                || used_tiles.iter().any(|tile| {
                    tile.texture_id != upload.texture_id
                        || atlas_upload_touches_tile(upload, tile)
                })
        })
    }

    pub(in crate::platform::nova) fn take_pending_uploads(&self) -> AtlasUploadBatch {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let pending_ids: FxHashSet<_> = state
            .pending_uploads
            .iter()
            .map(|upload| upload.texture_id)
            .collect();
        let texture_infos = if pending_ids.is_empty() {
            Vec::new()
        } else {
            state
                .texture_lists
                .iter()
                .flat_map(|list| list.textures.iter().flatten())
                .filter(|texture| pending_ids.contains(&texture.id))
                .map(|texture| NovaAtlasTextureInfo {
                    id: texture.id,
                    size: texture.size,
                })
                .collect()
        };
        AtlasUploadBatch {
            bytes: std::mem::take(&mut state.upload_bytes),
            uploads: std::mem::take(&mut state.pending_uploads),
            texture_infos,
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

    pub(in crate::platform::nova) fn restore_upload_batch(&self, batch: AtlasUploadBatch) {
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

/// Texture-coordinate overlap, not screen-coordinate dirty bounds. Padding is
/// included because bilinear sampling may read the replicated edge texels.
fn atlas_upload_touches_tile(upload: &PendingAtlasUpload, tile: &AtlasTile) -> bool {
    let bounds = tile.bounds;
    let left = i64::from(bounds.origin.x.0) - i64::from(tile.padding);
    let top = i64::from(bounds.origin.y.0) - i64::from(tile.padding);
    let right = i64::from(bounds.origin.x.0)
        + i64::from(bounds.size.width.0)
        + i64::from(tile.padding);
    let bottom = i64::from(bounds.origin.y.0)
        + i64::from(bounds.size.height.0)
        + i64::from(tile.padding);
    if left >= right || top >= bottom {
        return true; // Unknown or degenerate placement requires conservative refresh.
    }
    let upload_left = i64::from(upload.origin.x);
    let upload_top = i64::from(upload.origin.y);
    let upload_right = upload_left + i64::from(upload.size.width());
    let upload_bottom = upload_top + i64::from(upload.size.height());
    upload_left < right
        && upload_right > left
        && upload_top < bottom
        && upload_bottom > top
}

impl NovaAtlasState {
    pub(in crate::platform::nova) fn remove_pending_uploads_for_texture(
        &mut self,
        texture_id: AtlasTextureId,
    ) {
        self.pending_uploads
            .retain(|upload| upload.texture_id != texture_id);
    }

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
        let Some(bytes_per_row) =
            upload_width.checked_mul(atlas_bytes_per_pixel(texture_kind) as u32)
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
            let encoded =
                encode_atlas_upload_with_padding(pixels, size, bytes, texture_kind, padding)
                    .is_some();
            if encoded {
                self.content_generation = self.content_generation.wrapping_add(1);
            }
            return encoded;
        }
        let offset = self.upload_bytes.len();
        let Some(end) = offset.checked_add(len) else {
            return false;
        };
        if self.upload_bytes.is_empty() {
            self.upload_bytes = crate::assets::acquire_bitmap_buffer_capacity(len);
        }
        self.upload_bytes.resize(end, 0);
        if encode_atlas_upload_with_padding(
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
        self.content_generation = self.content_generation.wrapping_add(1);
        true
    }
}

#[cfg(test)]
mod upload_texture_snapshot_tests {
    use super::*;

    #[test]
    fn atlas_source_dependencies_distinguish_tiles_on_same_page() {
        let id = AtlasTextureId { index: 3, kind: AtlasTextureKind::Rgba };
        let source_tile = AtlasTile {
            texture_id: id,
            tile_id: crate::TileId(1),
            padding: 1,
            bounds: crate::bounds(
                crate::point(crate::DevicePixels(10), crate::DevicePixels(20)),
                crate::size(crate::DevicePixels(12), crate::DevicePixels(8)),
            ),
        };
        let mut upload = PendingAtlasUpload {
            texture_id: id,
            origin: Origin2d { x: 200, y: 200 },
            size: Extent2d::new(8, 8).expect("valid test extent"),
            bytes_per_row: 32,
            offset: 0,
            len: 256,
        };
        assert!(!atlas_upload_touches_tile(&upload, &source_tile),
            "unrelated uploads on the same page must not invalidate blur");
        upload.origin = Origin2d { x: 22, y: 22 };
        assert!(atlas_upload_touches_tile(&upload, &source_tile),
            "tile padding may be sampled with bilinear filtering");
        upload.origin = Origin2d { x: 23, y: 22 };
        assert!(!atlas_upload_touches_tile(&upload, &source_tile),
            "adjacent upload beyond the padded source is independent");
    }

    #[test]
    fn queued_source_uploads_check_exact_tiles_and_fail_closed() {
        let atlas = NovaAtlas::new();
        let id = AtlasTextureId { index: 900_001, kind: AtlasTextureKind::Rgba };
        let tile = AtlasTile {
            texture_id: id,
            tile_id: crate::TileId(7),
            padding: 1,
            bounds: crate::bounds(
                crate::point(crate::DevicePixels(10), crate::DevicePixels(20)),
                crate::size(crate::DevicePixels(12), crate::DevicePixels(8)),
            ),
        };
        {
            let mut state = atlas.state.lock().expect("test atlas mutex");
            state.pending_uploads.push(PendingAtlasUpload {
                texture_id: id,
                origin: Origin2d { x: 300, y: 300 },
                size: Extent2d::new(8, 8).expect("valid extent"),
                bytes_per_row: 32,
                offset: 0,
                len: 0,
            });
        }
        let mut sources = FxHashSet::default();
        sources.insert(id);
        let mut tiles = FxHashMap::default();
        tiles.insert(id, vec![tile]);
        assert!(!atlas.pending_uploads_touch_source_tiles(&sources, &tiles));
        {
            let mut state = atlas.state.lock().expect("test atlas mutex");
            state.pending_uploads.last_mut().expect("test upload").origin =
                Origin2d { x: 21, y: 22 };
        }
        assert!(atlas.pending_uploads_touch_source_tiles(&sources, &tiles));
        tiles.clear();
        assert!(atlas.pending_uploads_touch_source_tiles(&sources, &tiles),
            "missing packed-sprite dependency data must preserve correctness");
    }

    #[test]
    fn pending_atlas_texture_snapshot_covers_queued_uploads() {
        let atlas = NovaAtlas::new();
        let batch = atlas.take_pending_uploads();
        // Fallback tiles are already queued by NovaAtlas::new(). Every
        // pending texture must have an allocation snapshot from the same lock.
        assert!(!batch.uploads.is_empty());
        for upload in &batch.uploads {
            assert!(batch.texture_infos().iter().any(|info| info.id == upload.texture_id));
        }
        atlas.restore_upload_batch(batch);
        assert!(atlas.pending_upload_count_for_test() > 0);
    }
}
