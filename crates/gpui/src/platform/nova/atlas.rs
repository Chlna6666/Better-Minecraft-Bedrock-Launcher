use super::*;

use etagere::{AllocId, BucketedAtlasAllocator};
use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
};

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
const NOVA_DEDICATED_IMAGE_AXIS_THRESHOLD: u32 = 1536;
const NOVA_DEDICATED_IMAGE_AREA_DIVISOR: u64 = 4;
pub(super) const NOVA_ATLAS_TEXTURE_KINDS: [AtlasTextureKind; NOVA_ATLAS_KIND_COUNT] = [
    AtlasTextureKind::Monochrome,
    AtlasTextureKind::Bgra,
    AtlasTextureKind::Rgba,
    AtlasTextureKind::Subpixel,
];

pub(super) struct NovaAtlas {
    pub(super) state: Mutex<NovaAtlasState>,
    /// Per-key ownership gates for cache-miss construction. Expensive builders run while holding
    /// only their key's gate, never the global atlas mutex, so unrelated misses stay parallel while
    /// duplicate misses for one key share a single build.
    build_entries: Mutex<FxHashMap<AtlasKey, Weak<AtlasBuildEntry>>>,
    /// Lock-free mirror of [`NovaAtlasState::texture_set_generation`], refreshed by every atlas
    /// method that can change the texture set. The renderer polls this each frame to skip the
    /// texture sync (mutex + allocations) when nothing changed.
    texture_set_generation: AtomicU64,
    /// Lock-free mirror of atlas pixel content changes. Blur source caching uses this to avoid
    /// reusing a filtered texture after an image or glyph upload changed sampled pixels.
    content_generation: AtomicU64,
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
    /// Monotonic counter bumped whenever a texture is created or removed.
    texture_set_generation: u64,
    /// Monotonic counter bumped after every successfully encoded tile upload.
    pub(super) content_generation: u64,
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
            texture_set_generation: 0,
            content_generation: 0,
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

#[derive(Default)]
struct AtlasBuildEntry {
    build: Mutex<()>,
    generation: AtomicU64,
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
            build_entries: Mutex::new(FxHashMap::default()),
            texture_set_generation: AtomicU64::new(state.texture_set_generation),
            content_generation: AtomicU64::new(state.content_generation),
            pending_removals_flag: AtomicBool::new(!state.pending_removals.is_empty()),
            state: Mutex::new(state),
        }
    }

    /// Publishes the lock-free mirrors of the locked state. Must be called before releasing the
    /// state lock by every method that can change the texture set or the pending removals list.
    fn publish_state_flags(&self, state: &NovaAtlasState) {
        self.texture_set_generation
            .store(state.texture_set_generation, AtomicOrdering::Release);
        self.content_generation
            .store(state.content_generation, AtomicOrdering::Release);
        self.pending_removals_flag
            .store(!state.pending_removals.is_empty(), AtomicOrdering::Release);
    }

    fn build_entry(&self, key: &AtlasKey) -> Arc<AtlasBuildEntry> {
        let mut entries = self
            .build_entries
            .lock()
            .expect("nova atlas build-entry lock poisoned");
        if let Some(entry) = entries.get(key).and_then(Weak::upgrade) {
            return entry;
        }

        entries.retain(|_, entry| entry.strong_count() != 0);
        let entry = Arc::new(AtlasBuildEntry::default());
        entries.insert(key.clone(), Arc::downgrade(&entry));
        entry
    }

    fn invalidate_build(&self, key: &AtlasKey) {
        let entry = self
            .build_entries
            .lock()
            .expect("nova atlas build-entry lock poisoned")
            .get(key)
            .and_then(Weak::upgrade);
        if let Some(entry) = entry {
            entry.generation.fetch_add(1, AtomicOrdering::AcqRel);
        }
    }

    fn invalidate_builds_matching(&self, mut matches: impl FnMut(&AtlasKey) -> bool) {
        let mut entries = self
            .build_entries
            .lock()
            .expect("nova atlas build-entry lock poisoned");
        entries.retain(|key, weak| {
            let Some(entry) = weak.upgrade() else {
                return false;
            };
            if matches(key) {
                entry.generation.fetch_add(1, AtomicOrdering::AcqRel);
            }
            true
        });
    }

    fn lookup_or_restore_tile(&self, key: &AtlasKey) -> Option<AtlasTile> {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if let Some(tile) = state.tiles.get(&key) {
            return Some(*tile);
        }

        if matches!(key, AtlasKey::Image(_))
            && let Some(index) = state
                .pending_removals
                .iter()
                .rposition(|pending| pending.key == *key)
        {
            let pending = state.pending_removals.swap_remove(index);
            state.tiles.insert(pending.key, pending.tile);
            self.publish_state_flags(&state);
            return Some(pending.tile);
        }

        None
    }

    /// Monotonic counter identifying the current set of atlas textures without locking.
    pub(super) fn texture_set_generation(&self) -> u64 {
        self.texture_set_generation.load(AtomicOrdering::Acquire)
    }

    pub(super) fn content_generation(&self) -> u64 {
        self.content_generation.load(AtomicOrdering::Acquire)
    }

    pub(super) fn trim(&self, level: GpuiMemoryTrimLevel) {
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.invalidate_builds_matching(|_| true);
        }

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
                let previous_content_generation = state.content_generation;
                *state = NovaAtlasState::with_fallback_tiles();
                // Keep the generation monotonic across the reset so a renderer that synced
                // before the reset can never observe a stale-but-equal value.
                state.texture_set_generation = previous_generation
                    .wrapping_add(state.texture_set_generation)
                    .wrapping_add(1);
                state.content_generation = previous_content_generation
                    .wrapping_add(state.content_generation)
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

    /// Returns whether at least one queued atlas retirement is no longer referenced by the Scene
    /// that is about to be submitted.
    pub(super) fn has_retirable_pending_removals(
        &self,
        live_tiles: &FxHashSet<(AtlasTextureId, u32)>,
    ) -> bool {
        let state = self.state.lock().expect("nova atlas lock poisoned");
        state.pending_removals.iter().any(|pending| {
            !live_tiles.contains(&(pending.tile.texture_id, pending.tile.tile_id.0))
        })
    }

    /// Applies queued atlas retirements except for allocations referenced by the Scene that is
    /// about to be submitted.
    ///
    /// remove/remove_image remove keys from the CPU lookup immediately, but the retained Scene
    /// stores AtlasTile identities directly. Deallocating such a tile before encoding that Scene
    /// would make a valid PolychromeSprite sample freed or reused atlas memory. Protected removals
    /// stay queued and can either be resurrected by ensure_tile_with or retired once a later Scene
    /// no longer references them.
    pub(super) fn apply_pending_removals_except(
        &self,
        live_tiles: &FxHashSet<(AtlasTextureId, u32)>,
    ) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let pending_removals = std::mem::take(&mut state.pending_removals);
        for pending in pending_removals {
            if live_tiles.contains(&(pending.tile.texture_id, pending.tile.tile_id.0)) {
                state.pending_removals.push(pending);
            } else {
                state.deallocate_tile(pending.tile);
            }
        }
        self.publish_state_flags(&state);
    }

    pub(super) fn apply_pending_removals(&self) {
        self.apply_pending_removals_except(&FxHashSet::default());
    }
}

impl PlatformAtlas for NovaAtlas {
    fn ensure_tile_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        if let Some(tile) = self.lookup_or_restore_tile(&key) {
            return Ok(Some(tile));
        }

        let build_entry = self.build_entry(&key);
        let _build_guard = build_entry
            .build
            .lock()
            .expect("nova atlas per-key build lock poisoned");

        // Another caller for this exact key may have completed while we waited on the per-key
        // gate. Re-check before doing any expensive rasterization/decoding.
        if let Some(tile) = self.lookup_or_restore_tile(&key) {
            return Ok(Some(tile));
        }

        let generation = build_entry.generation.load(AtomicOrdering::Acquire);
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };

        let mut state = self.state.lock().expect("nova atlas lock poisoned");

        // remove/clear/trim invalidates in-flight ownership before mutating atlas state. If that
        // happened while the expensive builder was running, its output belongs to the old
        // generation and must never repopulate the atlas after removal.
        if build_entry.generation.load(AtomicOrdering::Acquire) != generation {
            return Ok(state.tiles.get(&key).copied());
        }

        if let Some(tile) = state.tiles.get(&key) {
            return Ok(Some(*tile));
        }

        let Some(tile) = state.allocate_and_upload(&key, size, &bytes) else {
            let texture_kind = key.texture_kind();
            if state.full_kinds_logged.insert(texture_kind) {
                log::warn!(
                    concat!(
                        "nova atlas allocation deferred; atlas allocation is unavailable and the shared fallback ",
                        "tile will be used: kind={:?} size={}x{}"
                    ),
                    texture_kind,
                    size.width.0.max(1),
                    size.height.0.max(1)
                );
            }
            let fallback = state.fallback_tile(texture_kind);
            self.publish_state_flags(&state);
            return Ok(fallback);
        };
        state.tiles.insert(key, tile);
        self.publish_state_flags(&state);
        Ok(Some(tile))
    }

    fn refresh_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let build_entry = self.build_entry(key);
        let _build_guard = build_entry
            .build
            .lock()
            .expect("nova atlas per-key build lock poisoned");
        let generation = build_entry.generation.load(AtomicOrdering::Acquire);

        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if build_entry.generation.load(AtomicOrdering::Acquire) != generation {
            return Ok(state.tiles.get(key).copied());
        }
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
                    self.publish_state_flags(&state);
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
                        "nova atlas refresh deferred; atlas allocation is unavailable and no fallback image ",
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
        self.ensure_tile_with(key, &mut build_tile)
    }

    fn clear_glyphs(&self) {
        self.invalidate_builds_matching(|key| matches!(key, AtlasKey::Glyph(_)));
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
        self.invalidate_build(key);
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

    fn remove_image(&self, image_id: ImageId) {
        self.invalidate_builds_matching(
            |key| matches!(key, AtlasKey::Image(params) if params.image_id == image_id),
        );
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let keys = state
            .tiles
            .keys()
            .filter(|key| matches!(key, AtlasKey::Image(params) if params.image_id == image_id))
            .cloned()
            .collect::<Vec<_>>();
        let mut queued_removal = false;
        for key in keys {
            if let Some(tile) = state.tiles.remove(&key) {
                if state.is_fallback_tile(tile) {
                    continue;
                }
                state
                    .pending_removals
                    .push(PendingAtlasRemoval { key, tile });
                queued_removal = true;
            }
        }
        if queued_removal {
            self.publish_state_flags(&state);
        }
    }
}

fn preferred_atlas_axis_size(content_axis: i32) -> Option<u32> {
    let padded = u32::try_from(content_axis.max(1))
        .ok()?
        .saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
    let floor = if padded > NOVA_DEFAULT_ATLAS_SIZE {
        NOVA_LARGE_IMAGE_ATLAS_SIZE
    } else {
        NOVA_DEFAULT_ATLAS_SIZE
    };
    Some(padded.max(floor).min(NOVA_MAX_ATLAS_SIZE))
}

fn padded_content_texture_size(content_size: Size<DevicePixels>) -> Option<(u32, u32)> {
    let width = u32::try_from(content_size.width.0.max(1))
        .ok()?
        .saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
    let height = u32::try_from(content_size.height.0.max(1))
        .ok()?
        .saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
    (width <= NOVA_MAX_ATLAS_SIZE && height <= NOVA_MAX_ATLAS_SIZE).then_some((width, height))
}

fn image_prefers_dedicated_texture(content_size: Size<DevicePixels>) -> bool {
    let width = u32::try_from(content_size.width.0.max(1)).unwrap_or(u32::MAX);
    let height = u32::try_from(content_size.height.0.max(1)).unwrap_or(u32::MAX);
    let area = u64::from(width).saturating_mul(u64::from(height));
    let shared_page_area =
        u64::from(NOVA_DEFAULT_ATLAS_SIZE).saturating_mul(u64::from(NOVA_DEFAULT_ATLAS_SIZE));
    area >= shared_page_area / NOVA_DEDICATED_IMAGE_AREA_DIVISOR
        || width > NOVA_DEDICATED_IMAGE_AXIS_THRESHOLD
        || height > NOVA_DEDICATED_IMAGE_AXIS_THRESHOLD
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
        let dedicated = matches!(key, AtlasKey::Image(_)) && image_prefers_dedicated_texture(size);
        self.allocate_and_upload_kind_with_placement(key.texture_kind(), size, bytes, dedicated)
    }

    fn allocate_and_upload_kind(
        &mut self,
        texture_kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Option<AtlasTile> {
        self.allocate_and_upload_kind_with_placement(texture_kind, size, bytes, false)
    }

    fn allocate_and_upload_kind_with_placement(
        &mut self,
        texture_kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
        dedicated: bool,
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
            self.allocate_in_texture(texture_kind, size, allocation_size, dedicated)?;

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
        dedicated: bool,
    ) -> Option<(AtlasTextureId, AllocId, i32, i32)> {
        if !dedicated {
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

        let (width, height) = if dedicated {
            padded_content_texture_size(content_size)?
        } else {
            (
                preferred_atlas_axis_size(content_size.width.0)?,
                preferred_atlas_axis_size(content_size.height.0)?,
            )
        };
        // Atlas growth is governed by live resource ownership and the backend's maximum texture
        // dimension, not a process-global byte or texture-count ceiling. Empty textures are
        // retired when their last tile is deallocated.
        self.texture_set_generation = self.texture_set_generation.wrapping_add(1);
        let list = &mut self.texture_lists[atlas_kind_index(texture_kind)];
        let texture = Self::push_texture_with_size(texture_kind, width, height, list)?;
        let allocation = texture.allocator.allocate(allocation_size)?;
        texture.live_tile_count = texture.live_tile_count.saturating_add(1);
        Some((
            texture.id,
            allocation.id,
            allocation.rectangle.min.x,
            allocation.rectangle.min.y,
        ))
    }

    fn push_texture_with_size(
        texture_kind: AtlasTextureKind,
        width: u32,
        height: u32,
        list: &mut NovaAtlasTextureList,
    ) -> Option<&mut NovaAtlasTexture> {
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
    use crate::{ImageId, ImagePixelFormat, RenderImageParams, size};

    #[test]
    fn medium_axis_uses_default_atlas_size() {
        assert_eq!(
            preferred_atlas_axis_size(1296),
            Some(NOVA_DEFAULT_ATLAS_SIZE)
        );
    }

    #[test]
    fn axis_larger_than_default_atlas_uses_large_atlas_size() {
        assert_eq!(
            preferred_atlas_axis_size(2304),
            Some(NOVA_LARGE_IMAGE_ATLAS_SIZE)
        );
    }

    #[test]
    fn fullscreen_image_prefers_exact_dedicated_texture() {
        let image_size = size(DevicePixels(2304), DevicePixels(1296));
        assert!(image_prefers_dedicated_texture(image_size));
        assert_eq!(padded_content_texture_size(image_size), Some((2306, 1298)));
    }

    #[test]
    fn map_tile_remains_shared_atlas_candidate() {
        assert!(!image_prefers_dedicated_texture(size(
            DevicePixels(512),
            DevicePixels(512)
        )));
    }

    #[test]
    fn queued_tile_upload_advances_atlas_content_generation() {
        let mut state = NovaAtlasState::default();
        let generation = state.content_generation;

        assert!(state.enqueue_tile_upload(
            AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Rgba,
            },
            AtlasTextureKind::Rgba,
            Point {
                x: DevicePixels(0),
                y: DevicePixels(0),
            },
            size(DevicePixels(1), DevicePixels(1)),
            &[0, 0, 0, 255],
            0,
        ));
        assert_ne!(state.content_generation, generation);
    }

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
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let second_key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(2),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let pixels = vec![255; 64 * 64 * NOVA_ATLAS_BYTES_PER_PIXEL];
        let first_tile = atlas
            .ensure_tile_with(first_key.clone(), &mut || {
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
            .ensure_tile_with(second_key.clone(), &mut || {
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
    fn scene_live_tile_blocks_pending_retirement_until_no_longer_referenced() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(29),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let tile = atlas
            .ensure_tile_with(key.clone(), &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[1, 2, 3, 4]),
                )))
            })
            .expect("test image should allocate")
            .expect("test image should have a tile");

        atlas.remove(&key);
        assert!(atlas.has_pending_removals());

        let mut live_tiles = FxHashSet::default();
        live_tiles.insert((tile.texture_id, tile.tile_id.0));
        assert!(!atlas.has_retirable_pending_removals(&live_tiles));
        atlas.apply_pending_removals_except(&live_tiles);
        assert!(atlas.has_pending_removals());

        let build_called = std::cell::Cell::new(false);
        let restored = atlas
            .ensure_tile_with(key.clone(), &mut || {
                build_called.set(true);
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[5, 6, 7, 8]),
                )))
            })
            .expect("scene-live pending image should be restorable")
            .expect("scene-live pending image should keep its tile");
        assert_eq!(restored, tile);
        assert!(!build_called.get());
        assert!(!atlas.has_pending_removals());

        atlas.remove(&key);
        live_tiles.clear();
        assert!(atlas.has_retirable_pending_removals(&live_tiles));
        atlas.apply_pending_removals_except(&live_tiles);
        assert!(!atlas.has_pending_removals());
    }

    #[test]
    fn removing_image_retires_all_frame_slots() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let image_id = ImageId(30);
        let pixels = vec![255; 64 * 64 * NOVA_ATLAS_BYTES_PER_PIXEL];
        for frame_slot in 0..2 {
            let key = AtlasKey::Image(RenderImageParams {
                image_id,
                frame_slot,
                pixel_format: ImagePixelFormat::Rgba8,
            });
            atlas
                .ensure_tile_with(key.clone(), &mut || {
                    Ok(Some((
                        size(DevicePixels(64), DevicePixels(64)),
                        Cow::Borrowed(pixels.as_slice()),
                    )))
                })
                .expect("image frame should allocate")
                .expect("image frame should have a tile");
        }

        atlas.remove_image(image_id);
        let state = atlas.state.lock().expect("nova atlas lock poisoned");
        assert!(
            state.tiles.keys().all(|key| {
                !matches!(key, AtlasKey::Image(params) if params.image_id == image_id)
            })
        );
        assert_eq!(state.pending_removals.len(), 2);
    }

    #[test]
    fn repainting_a_pending_image_cancels_its_removal() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(3),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let original_tile = atlas
            .ensure_tile_with(key.clone(), &mut || {
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
            .ensure_tile_with(key.clone(), &mut || {
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
    fn same_key_build_requests_share_one_gate() {
        let atlas = NovaAtlas::new();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(40),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });

        let first = atlas.build_entry(&key);
        let second = atlas.build_entry(&key);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn different_keys_keep_independent_build_gates() {
        let atlas = NovaAtlas::new();
        let first = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(41),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let second = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(42),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });

        assert!(!Arc::ptr_eq(
            &atlas.build_entry(&first),
            &atlas.build_entry(&second)
        ));
    }

    #[test]
    fn concurrent_same_key_miss_runs_builder_once() {
        use std::{
            sync::{
                Arc as StdArc,
                atomic::{AtomicUsize, Ordering},
                mpsc,
            },
            time::Duration,
        };

        let atlas = StdArc::new(NovaAtlas::new());
        atlas.clear_pending_uploads_for_test();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(44),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let build_calls = StdArc::new(AtomicUsize::new(0));

        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let atlas_first = atlas.clone();
        let key_first = key.clone();
        let build_calls_first = build_calls.clone();
        let first = std::thread::spawn(move || {
            atlas_first
                .ensure_tile_with(key_first.clone(), &mut || {
                    build_calls_first.fetch_add(1, Ordering::SeqCst);
                    first_entered_tx
                        .send(())
                        .expect("test should observe the first builder");
                    release_first_rx
                        .recv()
                        .expect("test should release the first builder");
                    Ok(Some((
                        size(DevicePixels(1), DevicePixels(1)),
                        Cow::Owned(vec![1, 2, 3, 4]),
                    )))
                })
                .expect("first concurrent atlas request should succeed")
                .expect("first concurrent atlas request should produce a tile")
        });

        first_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first builder should enter");

        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (duplicate_builder_tx, duplicate_builder_rx) = mpsc::channel();
        let atlas_second = atlas.clone();
        let key_second = key.clone();
        let build_calls_second = build_calls.clone();
        let second = std::thread::spawn(move || {
            second_started_tx
                .send(())
                .expect("test should observe the second request");
            atlas_second
                .ensure_tile_with(key_second.clone(), &mut || {
                    build_calls_second.fetch_add(1, Ordering::SeqCst);
                    duplicate_builder_tx
                        .send(())
                        .expect("duplicate builder observation channel should be live");
                    Ok(Some((
                        size(DevicePixels(1), DevicePixels(1)),
                        Cow::Owned(vec![5, 6, 7, 8]),
                    )))
                })
                .expect("second concurrent atlas request should succeed")
                .expect("second concurrent atlas request should produce a tile")
        });

        second_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second request should start");
        let duplicate_builder_started = duplicate_builder_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();

        release_first_tx
            .send(())
            .expect("first builder should still be waiting");

        let first_tile = first.join().expect("first atlas thread should not panic");
        let second_tile = second.join().expect("second atlas thread should not panic");
        assert!(
            !duplicate_builder_started,
            "same-key request must wait for the in-flight builder instead of starting another one"
        );
        assert_eq!(first_tile, second_tile);
        assert_eq!(build_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn removal_during_build_does_not_repopulate_the_key() {
        use std::{
            sync::{Arc as StdArc, mpsc},
            time::Duration,
        };

        let atlas = StdArc::new(NovaAtlas::new());
        atlas.clear_pending_uploads_for_test();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(45),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_atlas = atlas.clone();
        let worker_key = key.clone();
        let worker = std::thread::spawn(move || {
            worker_atlas
                .ensure_tile_with(worker_key.clone(), &mut || {
                    entered_tx
                        .send(())
                        .expect("test should observe the in-flight builder");
                    release_rx
                        .recv()
                        .expect("test should release the in-flight builder");
                    Ok(Some((
                        size(DevicePixels(1), DevicePixels(1)),
                        Cow::Owned(vec![1, 2, 3, 4]),
                    )))
                })
                .expect("invalidated build should not become an atlas error")
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("builder should enter before removal");
        atlas.remove(&key);
        release_tx
            .send(())
            .expect("builder should still be waiting");

        assert_eq!(
            worker.join().expect("atlas build thread should not panic"),
            None,
            "a build invalidated by removal must not publish a tile"
        );
        assert!(
            !atlas
                .state
                .lock()
                .expect("nova atlas lock poisoned")
                .tiles
                .contains_key(&key),
            "removed key must stay absent after the stale builder completes"
        );
    }

    #[test]
    fn removing_key_invalidates_in_flight_build_generation() {
        let atlas = NovaAtlas::new();
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(43),
            frame_slot: 0,
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let entry = atlas.build_entry(&key);
        let generation = entry.generation.load(AtomicOrdering::Acquire);

        atlas.remove(&key);

        assert_ne!(
            entry.generation.load(AtomicOrdering::Acquire),
            generation,
            "removal must invalidate a builder that could otherwise repopulate the removed key"
        );
    }

    #[test]
    fn atlas_growth_is_not_gated_by_byte_or_texture_count_budgets() {
        let mut state = NovaAtlasState::default();
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
                .is_some()
        );
        let texture_count = state
            .texture_lists
            .iter()
            .flat_map(|list| list.textures.iter())
            .filter(|texture| texture.is_some())
            .count();
        assert_eq!(texture_count, 2);
    }

    #[test]
    fn full_image_atlas_returns_shared_fallback() {
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
            pixel_format: ImagePixelFormat::Rgba8,
        });
        let tile = atlas
            .ensure_tile_with(key.clone(), &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[1, 2, 3, 4]),
                )))
            })
            .expect("allocation failure should not become an error");
        let fallback = atlas
            .state
            .lock()
            .expect("nova atlas lock poisoned")
            .fallback_tile(AtlasTextureKind::Rgba);
        assert_eq!(tile, fallback);
    }
}
