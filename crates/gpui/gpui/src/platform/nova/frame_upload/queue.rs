use super::atlas_encoding::{atlas_source_byte_len, encode_padded_atlas_upload};
use super::*;
mod batch;
mod damage;
#[cfg(feature = "bench-support")]
use crate::{ImageId, ImagePixelFormat, RenderImageParams, size};
pub(in crate::platform::nova) use batch::AtlasUpload;
#[cfg(test)]
use damage::atlas_upload_touches_tile;
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

/// Physically independent staging pages. The ordinary upload path can
/// append to its active page; adopted worker pages are never grown in place.
#[derive(Default)]
pub(in crate::platform::nova) struct UploadArena {
    pages: Vec<Vec<u8>>,
    active: Option<usize>,
}

impl UploadArena {
    pub(in crate::platform::nova) fn len(&self) -> usize {
        self.pages.iter().map(Vec::len).sum()
    }

    pub(in crate::platform::nova) fn capacity(&self) -> usize {
        self.pages.iter().map(Vec::capacity).sum()
    }

    fn is_empty(&self) -> bool {
        self.pages.iter().all(Vec::is_empty)
    }

    pub(in crate::platform::nova) fn adopt(&mut self, page: Vec<u8>) -> usize {
        let index = self.pages.len();
        self.pages.push(page);
        index
    }

    pub(in crate::platform::nova) fn discard_last(&mut self, index: usize) -> Vec<u8> {
        assert_eq!(index + 1, self.pages.len());
        assert_ne!(self.active, Some(index));
        self.pages.pop().expect("adopted upload page")
    }

    fn active_page(&mut self, len: usize) -> usize {
        if let Some(index) = self.active {
            return index;
        }
        let index = self.adopt(crate::assets::acquire_bitmap_buffer_capacity(len));
        self.active = Some(index);
        index
    }

    pub(in crate::platform::nova) fn release(self) {
        for page in self.pages {
            crate::assets::release_bitmap_buffer(page);
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct PendingAtlasUpload {
    pub(in crate::platform::nova) texture_id: AtlasTextureId,
    origin: Origin2d,
    size: Extent2d,
    bytes_per_row: u32,
    page: usize,
    offset: usize,
    len: usize,
}

impl PendingAtlasUpload {
    // Only merge exact rectangles whose encoded rows already have the
    // combined layout. Do not reorder writes or allocate a staging copy.
    fn append(&mut self, next: &Self) -> bool {
        if self.texture_id != next.texture_id
            || self.page != next.page
            || self.offset.checked_add(self.len) != Some(next.offset)
        {
            return false;
        }
        let width = self.size.width();
        let height = self.size.height();
        let next_width = next.size.width();
        let next_height = next.size.height();
        let bpp = atlas_bytes_per_pixel(self.texture_id.kind) as u32;
        if width.checked_mul(bpp) != Some(self.bytes_per_row)
            || next_width.checked_mul(bpp) != Some(next.bytes_per_row)
            || (self.bytes_per_row as usize).checked_mul(height as usize) != Some(self.len)
            || (next.bytes_per_row as usize).checked_mul(next_height as usize) != Some(next.len)
        {
            return false;
        }

        let (combined_width, combined_height, pitch) = if self.origin.x == next.origin.x
            && width == next_width
            && self.origin.y.checked_add(height) == Some(next.origin.y)
        {
            let Some(combined_height) = height.checked_add(next_height) else {
                return false;
            };
            (width, combined_height, self.bytes_per_row)
        } else if height == 1 && next_height == 1
            && self.origin.y == next.origin.y
            && self.origin.x.checked_add(width) == Some(next.origin.x)
        {
            let Some(combined_width) = width.checked_add(next_width) else {
                return false;
            };
            let Some(pitch) = combined_width.checked_mul(bpp) else {
                return false;
            };
            (combined_width, 1, pitch)
        } else {
            return false;
        };

        let Ok(size) = Extent2d::new(combined_width, combined_height) else {
            return false;
        };
        let Some(len) = self.len.checked_add(next.len) else {
            return false;
        };
        self.size = size;
        self.bytes_per_row = pitch;
        self.len = len;
        true
    }
}

#[derive(Default)]
pub(in crate::platform::nova) struct AtlasUploadBatch {
    bytes: UploadArena,
    uploads: Vec<PendingAtlasUpload>,
    // Captured under the SAME lock as the queued writes. New pages created
    // after this point belong to the next batch, not the current GPU upload.
    texture_infos: Vec<NovaAtlasTextureInfo>,
    content_generation: u64,
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
        AtlasUpload::new(&atlas)
            .upload(|_| Ok(TextureId::from_parts(0, 0)), |_| Ok(()))
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
        let stats = AtlasUpload::new(&self.atlas)
            .upload(|_| Ok(TextureId::from_parts(0, 0)), |_| Ok(()))
            .expect("benchmark atlas upload must succeed");
        (stats.upload_count, stats.uploaded_bytes)
    }
}

impl NovaAtlas {
    fn upload_taken_rgba_pixels(
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
            let mut pending = batch.uploads.iter().copied().peekable();
            while let Some(mut pending_upload) = pending.next() {
                while pending.peek().is_some_and(|next| pending_upload.append(next)) {
                    pending.next();
                }
                let end = pending_upload
                    .offset
                    .checked_add(pending_upload.len)
                    .ok_or_else(|| anyhow::anyhow!("nova atlas upload range overflow"))?;
                let pixels = batch.bytes.pages.get(pending_upload.page)
                    .and_then(|page| page.get(pending_upload.offset..end))
                    .ok_or_else(|| anyhow::anyhow!("nova atlas pending upload range is out of bounds"))?;
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

    fn take_pending_uploads(&self) -> AtlasUploadBatch {
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
            content_generation: state.content_generation,
        }
    }

    fn recycle_upload_batch(&self, mut batch: AtlasUploadBatch) {
        batch.uploads.clear();
        // Reuse at most the previous ordinary staging page locally. Worker
        // pages return to the existing bounded bitmap pool.
        let mut reusable = batch.bytes.active.map(|index| {
            let mut page = std::mem::take(&mut batch.bytes.pages[index]);
            page.clear();
            page
        });
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if state.upload_bytes.is_empty()
            && state.pending_uploads.is_empty()
            && reusable.as_ref().is_some_and(|page| {
                page.capacity() > state.upload_bytes.capacity()
                    && page.capacity() <= NOVA_ATLAS_RETAINED_UPLOAD_BYTES
            })
        {
            let previous = std::mem::take(&mut state.upload_bytes);
            let page = reusable.take().expect("reusable upload page");
            state.upload_bytes.active = Some(state.upload_bytes.adopt(page));
            drop(previous);
        }
        if state.pending_uploads.is_empty()
            && batch.uploads.capacity() > state.pending_uploads.capacity()
            && batch.uploads.capacity() <= NOVA_ATLAS_RETAINED_UPLOAD_COUNT
        {
            state.pending_uploads = batch.uploads;
        }
        drop(state);
        batch.bytes.release();
        if let Some(page) = reusable {
            crate::assets::release_bitmap_buffer(page);
        }
    }

    fn restore_upload_batch(&self, mut batch: AtlasUploadBatch) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        // Producers may have queued a newer value while this batch was in
        // flight. Restore older writes BEFORE those values, preserving order.
        let base_page = batch.bytes.pages.len();
        let newer_active = state.upload_bytes.active.take();
        batch.bytes.pages.append(&mut state.upload_bytes.pages);
        for upload in &mut state.pending_uploads {
            upload.page = upload.page.checked_add(base_page).expect("upload page index overflow");
        }
        if let Some(index) = newer_active {
            batch.bytes.active = Some(base_page + index);
        }
        batch.uploads.append(&mut state.pending_uploads);
        state.upload_bytes = batch.bytes;
        state.pending_uploads = batch.uploads;
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn pending_upload_bytes_for_test(&self) -> Vec<u8> {
        self.state
            .lock()
            .expect("nova atlas lock poisoned")
            .upload_bytes.pages.iter().flatten().copied().collect()
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
        state.upload_bytes = UploadArena::default();
        state.pending_uploads.clear();
    }
}

impl NovaAtlasState {
    /// Publish a range already encoded into an adopted worker page.
    pub(in crate::platform::nova) fn enqueue_encoded_tile_upload(
        &mut self,
        texture_id: AtlasTextureId,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        page: usize,
        range: std::ops::Range<usize>,
    ) -> bool {
        let padding = NOVA_ATLAS_TILE_PADDING;
        let Some(width) = (size.width.0.max(1) as u32).checked_add(2 * padding) else {
            return false;
        };
        let Some(height) = (size.height.0.max(1) as u32).checked_add(2 * padding) else {
            return false;
        };
        let Ok(extent) = Extent2d::new(width, height) else {
            return false;
        };
        let Some(bytes_per_row) = width.checked_mul(atlas_bytes_per_pixel(texture_id.kind) as u32) else {
            return false;
        };
        let Some(len) = (bytes_per_row as usize).checked_mul(height as usize) else {
            return false;
        };
        if range.len() != len || self.upload_bytes.pages.get(page)
            .and_then(|pixels| pixels.get(range.clone())).is_none()
        {
            return false;
        }
        self.pending_uploads.push(PendingAtlasUpload {
            texture_id,
            origin: Origin2d {
                x: origin.x.0.saturating_sub(padding as i32).max(0) as u32,
                y: origin.y.0.saturating_sub(padding as i32).max(0) as u32,
            },
            size: extent,
            bytes_per_row,
            page,
            offset: range.start,
            len,
        });
        self.content_generation = self.content_generation.wrapping_add(1);
        true
    }

    pub(in crate::platform::nova) fn remove_pending_uploads(&mut self, texture_id: AtlasTextureId) {
        let mut retired_pages = FxHashSet::default();
        self.pending_uploads.retain(|upload| {
            if upload.texture_id == texture_id {
                retired_pages.insert(upload.page);
                false
            } else {
                true
            }
        });
        if retired_pages.is_empty() {
            return;
        }

        // An adopted glyph page can feed several atlas textures. Only retire
        // pages with no remaining descriptors. Keep the physical page indices
        // stable: an allocation transaction may still be publishing a page.
        for upload in &self.pending_uploads {
            retired_pages.remove(&upload.page);
            if retired_pages.is_empty() {
                return;
            }
        }
        for page in retired_pages {
            if self.upload_bytes.active == Some(page) {
                self.upload_bytes.active = None;
            }
            if let Some(pixels) = self.upload_bytes.pages.get_mut(page) {
                // The producer explicitly retired this texture; do not move
                // these abandoned bytes into the global idle bitmap pool.
                drop(std::mem::take(pixels));
            }
        }
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
        self.enqueue_tile_upload_kind(texture_id, texture_kind, origin, size, bytes, padding, true)
    }

    // A fresh physical allocation must follow all earlier queued writes,
    // not search for (or overwrite) a descriptor belonging to a prior tile.
    pub(in crate::platform::nova) fn enqueue_new_tile_upload(
        &mut self,
        texture_id: AtlasTextureId,
        texture_kind: AtlasTextureKind,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        bytes: &[u8],
        padding: u32,
    ) -> bool {
        self.enqueue_tile_upload_kind(texture_id, texture_kind, origin, size, bytes, padding, false)
    }

    fn enqueue_tile_upload_kind(
        &mut self,
        texture_id: AtlasTextureId,
        texture_kind: AtlasTextureKind,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        bytes: &[u8],
        padding: u32,
        replace_existing: bool,
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
        if replace_existing && let Some(pending_upload) = self.pending_uploads.iter().rev().find(|upload| {
            upload.texture_id == texture_id
                && upload.origin == upload_origin
                && upload.size == extent
                && upload.bytes_per_row == bytes_per_row
                && upload.len == len
        }) {
            let Some(end) = pending_upload.offset.checked_add(pending_upload.len) else {
                return false;
            };
            let Some(pixels) = self.upload_bytes.pages.get_mut(pending_upload.page)
                .and_then(|page| page.get_mut(pending_upload.offset..end)) else {
                return false;
            };
            let encoded =
                encode_padded_atlas_upload(pixels, size, bytes, texture_kind, padding).is_some();
            if encoded {
                self.content_generation = self.content_generation.wrapping_add(1);
            }
            return encoded;
        }
        let page = self.upload_bytes.active_page(len);
        let pixels = &mut self.upload_bytes.pages[page];
        let offset = pixels.len();
        let Some(end) = offset.checked_add(len) else {
            return false;
        };
        pixels.resize(end, 0);
        if encode_padded_atlas_upload(
            &mut pixels[offset..end],
            size,
            bytes,
            texture_kind,
            padding,
        )
        .is_none()
        {
            pixels.truncate(offset);
            return false;
        }
        self.pending_uploads.push(PendingAtlasUpload {
            texture_id,
            origin: upload_origin,
            size: extent,
            bytes_per_row,
            page,
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
    fn retired_texture_releases_only_unreferenced_upload_pages() {
        let mut state = NovaAtlasState::default();
        let first = AtlasTextureId { index: 0, kind: AtlasTextureKind::Rgba };
        let second = AtlasTextureId { index: 1, kind: AtlasTextureKind::Rgba };
        let origin = crate::point(DevicePixels(1), DevicePixels(1));
        let size = crate::size(DevicePixels(1), DevicePixels(1));
        let shared = state.upload_bytes.adopt(vec![1; 72]);
        let exclusive = state.upload_bytes.adopt(vec![2; 36]);
        let survivor = state.upload_bytes.pages[shared].as_ptr();
        assert!(state.enqueue_encoded_tile_upload(first, origin, size, shared, 0..36));
        assert!(state.enqueue_encoded_tile_upload(second, origin, size, shared, 36..72));
        assert!(state.enqueue_encoded_tile_upload(first, origin, size, exclusive, 0..36));
        let before = state.upload_bytes.capacity();
        let exclusive_capacity = state.upload_bytes.pages[exclusive].capacity();

        state.remove_pending_uploads(first);
        assert_eq!(state.pending_uploads.len(), 1);
        assert_eq!(state.pending_uploads[0].page, shared);
        assert_eq!(state.upload_bytes.pages[shared].as_ptr(), survivor);
        assert_eq!(state.upload_bytes.pages[exclusive].capacity(), 0);
        assert_eq!(state.upload_bytes.capacity(), before - exclusive_capacity);

        state.remove_pending_uploads(second);
        assert!(state.pending_uploads.is_empty());
        assert_eq!(state.upload_bytes.capacity(), 0);
    }

    #[test]
    fn atlas_source_dependencies_distinguish_tiles_on_same_page() {
        let id = AtlasTextureId {
            index: 3,
            kind: AtlasTextureKind::Rgba,
        };
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
            page: 0,
            offset: 0,
            len: 256,
        };
        assert!(
            !atlas_upload_touches_tile(&upload, &source_tile),
            "unrelated uploads on the same page must not invalidate blur"
        );
        upload.origin = Origin2d { x: 22, y: 22 };
        assert!(
            atlas_upload_touches_tile(&upload, &source_tile),
            "tile padding may be sampled with bilinear filtering"
        );
        upload.origin = Origin2d { x: 23, y: 22 };
        assert!(
            !atlas_upload_touches_tile(&upload, &source_tile),
            "adjacent upload beyond the padded source is independent"
        );
    }

    #[test]
    fn queued_source_uploads_check_exact_tiles_and_fail_closed() {
        let atlas = NovaAtlas::new();
        let id = AtlasTextureId {
            index: 900_001,
            kind: AtlasTextureKind::Rgba,
        };
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
                page: 0,
                offset: 0,
                len: 0,
            });
        }
        let mut sources = FxHashSet::default();
        sources.insert(id);
        let mut tiles = FxHashMap::default();
        tiles.insert(id, vec![tile]);
        assert!(!AtlasUpload::new(&atlas).touches_source_tiles(&sources, &tiles));
        {
            let mut state = atlas.state.lock().expect("test atlas mutex");
            state
                .pending_uploads
                .last_mut()
                .expect("test upload")
                .origin = Origin2d { x: 21, y: 22 };
        }
        assert!(AtlasUpload::new(&atlas).touches_source_tiles(&sources, &tiles));
        tiles.clear();
        assert!(
            AtlasUpload::new(&atlas).touches_source_tiles(&sources, &tiles),
            "missing packed-sprite dependency data must preserve correctness"
        );
    }

    #[test]
    fn pending_atlas_texture_snapshot_covers_queued_uploads() {
        let atlas = NovaAtlas::new();
        let batch = atlas.take_pending_uploads();
        // Fallback tiles are already queued by NovaAtlas::new(). Every
        // pending texture must have an allocation snapshot from the same lock.
        assert!(!batch.uploads.is_empty());
        for upload in &batch.uploads {
            assert!(
                batch
                    .texture_infos()
                    .iter()
                    .any(|info| info.id == upload.texture_id)
            );
        }
        atlas.restore_upload_batch(batch);
        assert!(atlas.pending_upload_count_for_test() > 0);
    }
}
