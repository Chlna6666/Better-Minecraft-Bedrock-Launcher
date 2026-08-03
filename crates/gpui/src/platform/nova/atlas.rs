use super::*;

use etagere::{AllocId, BucketedAtlasAllocator};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};

#[cfg(test)]
pub(super) use super::upload_encoding::encode_bgra_upload;
use super::upload_encoding::{atlas_kind_index, fallback_atlas_bytes};
pub(super) use super::upload_queue::AtlasUploadStats;
use super::upload_queue::PendingAtlasUpload;

pub(super) const NOVA_DEFAULT_ATLAS_SIZE: u32 = 2048;
pub(super) const NOVA_LARGE_IMAGE_ATLAS_SIZE: u32 = 4096;
pub(super) const NOVA_MAX_ATLAS_SIZE: u32 = 16_384;
pub(super) const NOVA_ATLAS_SIZE: u32 = NOVA_DEFAULT_ATLAS_SIZE;
pub(super) const NOVA_ATLAS_BYTES_PER_PIXEL: usize = 4;
pub(super) const NOVA_ATLAS_TILE_PADDING: u32 = 1;
pub(super) const NOVA_ATLAS_KIND_COUNT: usize = 4;
const NOVA_MIN_IMAGE_ATLAS_BYTES: usize = 512 * 1024 * 1024;
const NOVA_MIN_IMAGE_ATLAS_TEXTURES: usize = 128;
pub(super) const NOVA_ATLAS_TEXTURE_KINDS: [AtlasTextureKind; NOVA_ATLAS_KIND_COUNT] = [
    AtlasTextureKind::Monochrome,
    AtlasTextureKind::Bgra,
    AtlasTextureKind::Rgba,
    AtlasTextureKind::Subpixel,
];

pub(super) struct NovaAtlas {
    pub(super) state: Mutex<NovaAtlasState>,
    /// Lock-free mirror of [`NovaAtlasState::texture_set_generation`], refreshed by every atlas
    /// method that can change the texture set. The renderer polls this each frame to skip the
    /// texture sync (mutex + allocations) when nothing changed.
    texture_set_generation: AtomicU64,
    /// Lock-free mirror of whether [`NovaAtlasState::pending_removals`] is non-empty.
    pending_removals_flag: AtomicBool,
}

pub(super) struct NovaAtlasState {
    pub(super) next_tile_id: u32,
    pub(super) texture_lists: [NovaAtlasTextureList; NOVA_ATLAS_KIND_COUNT],
    tiles: FxHashMap<AtlasKey, AtlasTile>,
    pending_removals: Vec<PendingAtlasRemoval>,
    fallback_tiles: [Option<AtlasTile>; NOVA_ATLAS_KIND_COUNT],
    full_kinds_logged: FxHashSet<AtlasTextureKind>,
    #[cfg(test)]
    disabled_kinds: FxHashSet<AtlasTextureKind>,
    pub(super) upload_bytes: Vec<u8>,
    pub(super) pending_uploads: Vec<PendingAtlasUpload>,
    max_atlas_bytes: usize,
    max_atlas_textures: usize,
    /// Monotonic counter bumped whenever a texture is created or removed.
    texture_set_generation: u64,
}

impl Default for NovaAtlasState {
    fn default() -> Self {
        Self {
            next_tile_id: 0,
            texture_lists: std::array::from_fn(|_| NovaAtlasTextureList::default()),
            tiles: FxHashMap::default(),
            pending_removals: Vec::new(),
            fallback_tiles: [None; NOVA_ATLAS_KIND_COUNT],
            full_kinds_logged: FxHashSet::default(),
            #[cfg(test)]
            disabled_kinds: FxHashSet::default(),
            upload_bytes: Vec::new(),
            pending_uploads: Vec::new(),
            max_atlas_bytes: NOVA_MIN_IMAGE_ATLAS_BYTES,
            max_atlas_textures: NOVA_MIN_IMAGE_ATLAS_TEXTURES,
            texture_set_generation: 0,
        }
    }
}

#[derive(Default)]
pub(super) struct NovaAtlasTextureList {
    pub(super) textures: Vec<Option<NovaAtlasTexture>>,
    free_list: Vec<usize>,
}

pub(super) struct NovaAtlasTexture {
    pub(super) id: AtlasTextureId,
    pub(super) size: Size<DevicePixels>,
    allocator: BucketedAtlasAllocator,
    live_tile_count: usize,
}

struct PendingAtlasRemoval {
    key: AtlasKey,
    tile: AtlasTile,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NovaAtlasTextureInfo {
    pub(super) id: AtlasTextureId,
    pub(super) size: Size<DevicePixels>,
}

impl NovaAtlas {
    pub(super) fn new() -> Self {
        let state = NovaAtlasState::with_fallback_tiles();
        Self {
            texture_set_generation: AtomicU64::new(state.texture_set_generation),
            pending_removals_flag: AtomicBool::new(!state.pending_removals.is_empty()),
            state: Mutex::new(state),
        }
    }

    /// Publishes the lock-free mirrors of the locked state. Must be called before releasing the
    /// state lock by every method that can change the texture set or the pending removals list.
    fn publish_state_flags(&self, state: &NovaAtlasState) {
        self.texture_set_generation
            .store(state.texture_set_generation, AtomicOrdering::Release);
        self.pending_removals_flag
            .store(!state.pending_removals.is_empty(), AtomicOrdering::Release);
    }

    /// Monotonic counter identifying the current set of atlas textures without locking.
    pub(super) fn texture_set_generation(&self) -> u64 {
        self.texture_set_generation.load(AtomicOrdering::Acquire)
    }

    pub(super) fn trim(&self, level: GpuiMemoryTrimLevel) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        match level {
            GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
                if state.pending_uploads.is_empty() {
                    let upload_bytes = std::mem::take(&mut state.upload_bytes);
                    crate::assets::release_bitmap_buffer(upload_bytes);
                    state.pending_uploads.shrink_to(0);
                }
            }
            GpuiMemoryTrimLevel::Aggressive => {
                let previous_generation = state.texture_set_generation;
                *state = NovaAtlasState::with_fallback_tiles();
                // Keep the generation monotonic across the reset so a renderer that synced
                // before the reset can never observe a stale-but-equal value.
                state.texture_set_generation = previous_generation
                    .wrapping_add(state.texture_set_generation)
                    .wrapping_add(1);
            }
        }
        self.publish_state_flags(&state);
    }

    pub(super) fn texture_infos(&self) -> Vec<NovaAtlasTextureInfo> {
        let state = self.state.lock().expect("nova atlas lock poisoned");
        state
            .texture_lists
            .iter()
            .flat_map(|list| {
                list.textures.iter().filter_map(|texture| {
                    texture.as_ref().map(|texture| NovaAtlasTextureInfo {
                        id: texture.id,
                        size: texture.size,
                    })
                })
            })
            .collect()
    }

    pub(super) fn has_pending_removals(&self) -> bool {
        self.pending_removals_flag.load(AtomicOrdering::Acquire)
    }

    pub(super) fn apply_pending_removals(&self) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let pending_removals = std::mem::take(&mut state.pending_removals);
        for pending in pending_removals {
            state.deallocate_tile(pending.tile);
        }
        self.publish_state_flags(&state);
    }
}

impl PlatformAtlas for NovaAtlas {
    fn configure_image_budget(&self, max_bytes: usize, max_textures: usize) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        state.max_atlas_bytes = max_bytes.max(NOVA_MIN_IMAGE_ATLAS_BYTES);
        state.max_atlas_textures = max_textures.max(NOVA_MIN_IMAGE_ATLAS_TEXTURES);
        log::info!(
            "nova atlas image budget configured: bytes={} textures={} texture_size={}x{}",
            state.max_atlas_bytes,
            state.max_atlas_textures,
            NOVA_DEFAULT_ATLAS_SIZE,
            NOVA_LARGE_IMAGE_ATLAS_SIZE
        );
    }

    fn ensure_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut state = self
            .state
            .lock()
            .expect("nova placeholder atlas lock poisoned");
        if let Some(tile) = state.tiles.get(key) {
            return Ok(Some(*tile));
        }
        // Repainting an image whose removal is still queued cancels the
        // removal instead of re-decoding and re-uploading. Non-image keys
        // (glyphs cleared on DPI changes, probe-only lookups) must observe
        // removals immediately, so they never resurrect.
        if matches!(key, AtlasKey::Image(_))
            && let Some(index) = state
                .pending_removals
                .iter()
                .rposition(|pending| pending.key == *key)
        {
            let pending = state.pending_removals.swap_remove(index);
            state.tiles.insert(pending.key, pending.tile);
            self.publish_state_flags(&state);
            return Ok(Some(pending.tile));
        }
        drop(state);

        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };

        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let Some(tile) = state.allocate_and_upload(key, size, &bytes) else {
            let texture_kind = key.texture_kind();
            if state.full_kinds_logged.insert(texture_kind) {
                log::warn!(
                    concat!(
                        "nova atlas allocation deferred; atlas is full and no fallback image ",
                        "will be reported as resident: kind={:?} size={}x{}"
                    ),
                    texture_kind,
                    size.width.0.max(1),
                    size.height.0.max(1)
                );
            }
            self.publish_state_flags(&state);
            return Ok(None);
        };
        state.tiles.insert(key.clone(), tile);
        self.publish_state_flags(&state);
        Ok(Some(tile))
    }

    fn refresh_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if let Some(tile) = state.tiles.get(key).copied() {
            if tile.bounds.size == size {
                if state.enqueue_tile_upload(
                    tile.texture_id,
                    key.texture_kind(),
                    tile.bounds.origin,
                    size,
                    bytes.as_ref(),
                    tile.padding,
                ) {
                    return Ok(Some(tile));
                }
                log::warn!(
                    "nova atlas tile update failed; keeping previous tile: kind={:?}",
                    key.texture_kind()
                );
                return Ok(Some(tile));
            }
        }

        let previous = state.tiles.get(key).copied();
        let allocated = state.allocate_and_upload(key, size, bytes.as_ref());
        let Some(allocated) = allocated else {
            let texture_kind = key.texture_kind();
            if state.full_kinds_logged.insert(texture_kind) {
                log::warn!(
                    concat!(
                        "nova atlas refresh deferred; atlas is full and no fallback image ",
                        "will be reported as resident: kind={:?} size={}x{}"
                    ),
                    texture_kind,
                    size.width.0.max(1),
                    size.height.0.max(1)
                );
            }
            self.publish_state_flags(&state);
            return Ok(previous);
        };

        if let Some(previous) = state.tiles.insert(key.clone(), allocated) {
            state.pending_removals.push(PendingAtlasRemoval {
                key: key.clone(),
                tile: previous,
            });
        }
        self.publish_state_flags(&state);
        Ok(Some(allocated))
    }

    fn ensure_glyph_with(
        &self,
        params: &RenderGlyphParams,
        build: &mut dyn FnMut() -> Result<GlyphRasterization>,
    ) -> Result<Option<AtlasTile>> {
        let key = AtlasKey::from(params.clone());
        let mut build_tile = || match build()? {
            GlyphRasterization::Bitmap { size, bytes } => Ok(Some((size, Cow::Owned(bytes)))),
            GlyphRasterization::ColorLayers {
                size,
                layers,
                fallback,
            } => {
                let width = usize::try_from(size.width.0).ok();
                let height = usize::try_from(size.height.0).ok();
                let Some((width, height)) = width.zip(height) else {
                    return Ok(Some((fallback.size, Cow::Owned(fallback.bytes))));
                };
                let mut pixels = vec![[0.0f32; 4]; width.saturating_mul(height)];
                for layer in layers {
                    let layer_width = usize::try_from(layer.bounds.size.width.0).unwrap_or(0);
                    let layer_height = usize::try_from(layer.bounds.size.height.0).unwrap_or(0);
                    for layer_y in 0..layer_height {
                        let destination_y = layer.bounds.origin.y.0 + layer_y as i32;
                        if destination_y < 0 || destination_y >= size.height.0 {
                            continue;
                        }
                        for layer_x in 0..layer_width {
                            let destination_x = layer.bounds.origin.x.0 + layer_x as i32;
                            if destination_x < 0 || destination_x >= size.width.0 {
                                continue;
                            }
                            let alpha_index = layer_y * layer_width + layer_x;
                            let Some(mask) = layer.alpha.get(alpha_index) else {
                                continue;
                            };
                            let source_alpha = f32::from(*mask) / 255.0 * layer.color.a;
                            let destination_index =
                                destination_y as usize * width + destination_x as usize;
                            let destination = &mut pixels[destination_index];
                            let destination_alpha = destination[3];
                            let output_alpha =
                                source_alpha + destination_alpha * (1.0 - source_alpha);
                            if output_alpha > 0.0 {
                                let retained = destination_alpha * (1.0 - source_alpha);
                                destination[0] = (layer.color.b * source_alpha
                                    + destination[0] * retained)
                                    / output_alpha;
                                destination[1] = (layer.color.g * source_alpha
                                    + destination[1] * retained)
                                    / output_alpha;
                                destination[2] = (layer.color.r * source_alpha
                                    + destination[2] * retained)
                                    / output_alpha;
                            }
                            destination[3] = output_alpha;
                        }
                    }
                }
                let bytes = pixels
                    .into_iter()
                    .flat_map(|pixel| {
                        pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
                    })
                    .collect();
                Ok(Some((size, Cow::Owned(bytes))))
            }
        };
        self.ensure_tile_with(&key, &mut build_tile)
    }

    fn clear_glyphs(&self) {
        let keys = {
            let state = self.state.lock().expect("nova atlas lock poisoned");
            state
                .tiles
                .keys()
                .filter(|key| matches!(key, AtlasKey::Glyph(_)))
                .cloned()
                .collect::<Vec<_>>()
        };
        for key in keys {
            self.remove(&key);
        }
    }

    fn remove(&self, key: &AtlasKey) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if let Some(tile) = state.tiles.remove(key) {
            if !state.is_fallback_tile(tile) {
                state.pending_removals.push(PendingAtlasRemoval {
                    key: key.clone(),
                    tile,
                });
                self.publish_state_flags(&state);
            }
        }
    }
}

fn preferred_atlas_axis_size(content_axis: i32) -> Option<u32> {
    let padded = u32::try_from(content_axis.max(1))
        .ok()?
        .saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
    let floor = if padded > NOVA_DEFAULT_ATLAS_SIZE / 2 {
        NOVA_LARGE_IMAGE_ATLAS_SIZE
    } else {
        NOVA_DEFAULT_ATLAS_SIZE
    };
    Some(padded.max(floor).min(NOVA_MAX_ATLAS_SIZE))
}

impl NovaAtlasState {
    fn with_fallback_tiles() -> Self {
        let mut state = Self::default();
        state.initialize_fallback_tiles();
        state
    }

    fn initialize_fallback_tiles(&mut self) {
        let size = Size {
            width: DevicePixels(1),
            height: DevicePixels(1),
        };
        for texture_kind in NOVA_ATLAS_TEXTURE_KINDS {
            let bytes = fallback_atlas_bytes(texture_kind);
            self.fallback_tiles[atlas_kind_index(texture_kind)] =
                self.allocate_and_upload_kind(texture_kind, size, bytes);
        }
    }

    pub(super) fn fallback_tile(&self, texture_kind: AtlasTextureKind) -> Option<AtlasTile> {
        self.fallback_tiles[atlas_kind_index(texture_kind)]
    }

    #[cfg(test)]
    pub(super) fn disable_allocator_for_test(&mut self, texture_kind: AtlasTextureKind) {
        self.disabled_kinds.insert(texture_kind);
    }

    fn allocate_and_upload(
        &mut self,
        key: &AtlasKey,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Option<AtlasTile> {
        self.allocate_and_upload_kind(key.texture_kind(), size, bytes)
    }

    fn allocate_and_upload_kind(
        &mut self,
        texture_kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Option<AtlasTile> {
        #[cfg(test)]
        if self.disabled_kinds.contains(&texture_kind) {
            return None;
        }

        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let padded_width = width.saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
        let padded_height = height.saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
        if padded_width > NOVA_MAX_ATLAS_SIZE || padded_height > NOVA_MAX_ATLAS_SIZE {
            return None;
        }

        let allocation_size = etagere::Size::new(
            i32::try_from(padded_width).ok()?,
            i32::try_from(padded_height).ok()?,
        );
        let (texture_id, allocation_id, allocation_min_x, allocation_min_y) =
            self.allocate_in_texture(texture_kind, size, allocation_size)?;

        let origin = Point {
            x: DevicePixels(
                allocation_min_x.saturating_add(i32::try_from(NOVA_ATLAS_TILE_PADDING).ok()?),
            ),
            y: DevicePixels(
                allocation_min_y.saturating_add(i32::try_from(NOVA_ATLAS_TILE_PADDING).ok()?),
            ),
        };
        if !self.enqueue_tile_upload_kind(
            texture_id,
            texture_kind,
            origin,
            size,
            bytes,
            NOVA_ATLAS_TILE_PADDING,
        ) {
            self.deallocate_texture_allocation(texture_id, allocation_id);
            return None;
        }
        self.next_tile_id = self.next_tile_id.saturating_add(1);
        let tile = AtlasTile {
            texture_id,
            tile_id: allocation_id.into(),
            padding: NOVA_ATLAS_TILE_PADDING,
            bounds: Bounds { origin, size },
        };
        self.full_kinds_logged.remove(&texture_kind);
        Some(tile)
    }

    fn allocate_in_texture(
        &mut self,
        texture_kind: AtlasTextureKind,
        content_size: Size<DevicePixels>,
        allocation_size: etagere::Size,
    ) -> Option<(AtlasTextureId, AllocId, i32, i32)> {
        {
            let list = &mut self.texture_lists[atlas_kind_index(texture_kind)];
            for texture in list.textures.iter_mut().flatten().rev() {
                if let Some(allocation) = texture.allocator.allocate(allocation_size) {
                    texture.live_tile_count = texture.live_tile_count.saturating_add(1);
                    return Some((
                        texture.id,
                        allocation.id,
                        allocation.rectangle.min.x,
                        allocation.rectangle.min.y,
                    ));
                }
            }
        }

        let texture_count = self
            .texture_lists
            .iter()
            .flat_map(|list| list.textures.iter())
            .filter(|texture| texture.is_some())
            .count();
        if texture_count >= self.max_atlas_textures {
            return None;
        }
        let width = preferred_atlas_axis_size(content_size.width.0)?;
        let height = preferred_atlas_axis_size(content_size.height.0)?;
        let texture_bytes = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)
            .and_then(|pixels| pixels.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL))?;
        let current_bytes = self
            .texture_lists
            .iter()
            .flat_map(|list| list.textures.iter().flatten())
            .filter_map(|texture| {
                usize::try_from(texture.size.width.0)
                    .ok()?
                    .checked_mul(usize::try_from(texture.size.height.0).ok()?)
                    .and_then(|pixels| pixels.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL))
            })
            .sum::<usize>();
        if current_bytes.saturating_add(texture_bytes) > self.max_atlas_bytes {
            return None;
        }
        // Bump before pushing: a failed push only causes one spurious renderer sync, while a
        // missed bump could leave a new texture without GPU resources.
        self.texture_set_generation = self.texture_set_generation.wrapping_add(1);
        let list = &mut self.texture_lists[atlas_kind_index(texture_kind)];
        let texture = Self::push_texture(texture_kind, content_size, list)?;
        let allocation = texture.allocator.allocate(allocation_size)?;
        texture.live_tile_count = texture.live_tile_count.saturating_add(1);
        Some((
            texture.id,
            allocation.id,
            allocation.rectangle.min.x,
            allocation.rectangle.min.y,
        ))
    }

    fn push_texture(
        texture_kind: AtlasTextureKind,
        min_size: Size<DevicePixels>,
        list: &mut NovaAtlasTextureList,
    ) -> Option<&mut NovaAtlasTexture> {
        let width = preferred_atlas_axis_size(min_size.width.0)?;
        let height = preferred_atlas_axis_size(min_size.height.0)?;
        let size = Size {
            width: DevicePixels(i32::try_from(width).ok()?),
            height: DevicePixels(i32::try_from(height).ok()?),
        };
        let index = list.free_list.pop();
        let texture = NovaAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(list.textures.len()) as u32,
                kind: texture_kind,
            },
            size,
            allocator: BucketedAtlasAllocator::new(etagere::Size::new(
                i32::try_from(width).ok()?,
                i32::try_from(height).ok()?,
            )),
            live_tile_count: 0,
        };

        if let Some(index) = index {
            list.textures[index] = Some(texture);
            list.textures.get_mut(index).and_then(Option::as_mut)
        } else {
            list.textures.push(Some(texture));
            list.textures.last_mut().and_then(Option::as_mut)
        }
    }

    fn deallocate_tile(&mut self, tile: AtlasTile) {
        if self.is_fallback_tile(tile) {
            return;
        }
        self.deallocate_texture_allocation(tile.texture_id, tile.tile_id.into());
    }

    fn deallocate_texture_allocation(
        &mut self,
        texture_id: AtlasTextureId,
        allocation_id: AllocId,
    ) {
        let should_remove_pending_uploads = {
            let list = &mut self.texture_lists[atlas_kind_index(texture_id.kind)];
            let Some(index) = usize::try_from(texture_id.index).ok() else {
                return;
            };
            let Some(texture) = list.textures.get_mut(index).and_then(Option::as_mut) else {
                return;
            };
            texture.allocator.deallocate(allocation_id);
            texture.live_tile_count = texture.live_tile_count.saturating_sub(1);
            if texture.live_tile_count == 0
                && let Some(texture_slot) = list.textures.get_mut(index)
            {
                *texture_slot = None;
                list.free_list.push(index);
                true
            } else {
                false
            }
        };
        if should_remove_pending_uploads {
            self.texture_set_generation = self.texture_set_generation.wrapping_add(1);
            self.remove_pending_uploads_for_texture(texture_id);
        }
    }

    fn is_fallback_tile(&self, tile: AtlasTile) -> bool {
        self.fallback_tiles
            .iter()
            .flatten()
            .any(|fallback_tile| *fallback_tile == tile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageId, RenderImageParams, RenderImagePixelFormat, size};

    #[test]
    fn deallocating_last_texture_tile_removes_pending_uploads() {
        let mut state = NovaAtlasState::default();
        let tile = state
            .allocate_and_upload_kind(
                AtlasTextureKind::Rgba,
                Size {
                    width: DevicePixels(1),
                    height: DevicePixels(1),
                },
                &[1, 2, 3, 4],
            )
            .expect("test tile should allocate");

        assert_eq!(state.pending_uploads.len(), 1);

        state.deallocate_texture_allocation(tile.texture_id, tile.tile_id.into());

        assert!(state.pending_uploads.is_empty());
        assert!(
            state.texture_lists[atlas_kind_index(AtlasTextureKind::Rgba)]
                .textures
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn removed_tile_is_not_reused_until_pending_removals_are_applied() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let first_key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let second_key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(2),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let pixels = vec![255; 64 * 64 * NOVA_ATLAS_BYTES_PER_PIXEL];
        let first_tile = atlas
            .ensure_tile_with(&first_key, &mut || {
                Ok(Some((
                    size(DevicePixels(64), DevicePixels(64)),
                    Cow::Borrowed(pixels.as_slice()),
                )))
            })
            .expect("first tile allocation should succeed")
            .expect("first tile should exist");

        atlas.remove(&first_key);
        assert!(atlas.has_pending_removals());
        let second_tile = atlas
            .ensure_tile_with(&second_key, &mut || {
                Ok(Some((
                    size(DevicePixels(64), DevicePixels(64)),
                    Cow::Borrowed(pixels.as_slice()),
                )))
            })
            .expect("second tile allocation should succeed")
            .expect("second tile should exist");

        assert_ne!(first_tile.bounds, second_tile.bounds);
        atlas.apply_pending_removals();
        assert!(!atlas.has_pending_removals());
    }

    #[test]
    fn repainting_a_pending_image_cancels_its_removal() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(3),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let original_tile = atlas
            .ensure_tile_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[1, 2, 3, 4]),
                )))
            })
            .expect("initial tile allocation should succeed")
            .expect("initial tile should exist");
        atlas.remove(&key);

        let build_called = std::cell::Cell::new(false);
        let restored_tile = atlas
            .ensure_tile_with(&key, &mut || {
                build_called.set(true);
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[5, 6, 7, 8]),
                )))
            })
            .expect("pending tile restoration should succeed")
            .expect("pending tile should be restored");

        assert_eq!(restored_tile, original_tile);
        assert!(!build_called.get());
        assert!(!atlas.has_pending_removals());
    }

    #[test]
    fn atlas_texture_count_and_byte_budgets_prevent_growth() {
        let mut state = NovaAtlasState::default();
        state.max_atlas_textures = 1;
        let size = Size {
            width: DevicePixels(1),
            height: DevicePixels(1),
        };
        assert!(
            state
                .allocate_and_upload_kind(AtlasTextureKind::Rgba, size, &[1, 2, 3, 4])
                .is_some()
        );
        assert!(
            state
                .allocate_and_upload_kind(AtlasTextureKind::Bgra, size, &[1, 2, 3, 4])
                .is_none()
        );

        let mut byte_limited = NovaAtlasState::default();
        byte_limited.max_atlas_bytes = 1024;
        assert!(
            byte_limited
                .allocate_and_upload_kind(AtlasTextureKind::Rgba, size, &[1, 2, 3, 4])
                .is_none()
        );
    }

    #[test]
    fn full_image_atlas_returns_none_instead_of_fallback() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        atlas
            .state
            .lock()
            .expect("nova atlas lock poisoned")
            .disable_allocator_for_test(AtlasTextureKind::Rgba);
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(9),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let tile = atlas
            .ensure_tile_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[1, 2, 3, 4]),
                )))
            })
            .expect("allocation failure should not become an error");
        assert!(tile.is_none());
    }
}
