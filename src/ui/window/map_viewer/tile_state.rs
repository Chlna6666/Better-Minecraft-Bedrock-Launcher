use super::model::*;
use super::prelude::*;
use super::tile_render::*;
use super::viewport::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TileRevealState {
    pub(super) ready_batches: u64,
    pub(super) last_batch_size: usize,
}

#[derive(Clone)]
pub(super) struct ViewerTile {
    pub(super) image: Arc<RenderImage>,
    pub(super) pixel_format: Option<TilePixelFormat>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) estimated_bytes: usize,
}

#[derive(Clone)]
pub(super) struct PaintTile {
    pub(super) coord: (i32, i32),
    pub(super) image: Arc<RenderImage>,
    pub(super) pixel_format: Option<TilePixelFormat>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) estimated_bytes: usize,
}

#[derive(Clone)]
pub(super) struct ReadyTile {
    pub(super) coord: (i32, i32),
    pub(super) tile: ViewerTile,
    pub(super) source: TileReadySource,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TilePaintRect {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) right: f32,
    pub(super) bottom: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MapRenderRange {
    pub(super) min_chunk_x: i32,
    pub(super) max_chunk_x: i32,
    pub(super) min_chunk_z: i32,
    pub(super) max_chunk_z: i32,
    pub(super) render_origin_x: f32,
    pub(super) render_origin_y: f32,
    pub(super) chunk_screen_size: f32,
    pub(super) block_screen_size: f32,
    pub(super) chunks_per_tile: i32,
}

pub(super) struct TileReadyBatcher {
    pub(super) pending: Vec<ReadyTile>,
    pub(super) last_flush: Instant,
    pub(super) quick_reveal: bool,
}

impl Default for TileReadyBatcher {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            last_flush: Instant::now(),
            quick_reveal: false,
        }
    }
}

impl TileReadyBatcher {
    pub(super) fn new(quick_reveal: bool) -> Self {
        Self {
            quick_reveal,
            ..Self::default()
        }
    }

    pub(super) fn push(
        &mut self,
        sender: &Arc<Mutex<UnboundedSender<TileRenderEvent>>>,
        tile: ReadyTile,
    ) -> bool {
        let is_memory_cache = matches!(&tile.source, TileReadySource::MemoryCache);
        let is_disk_cache = matches!(
            &tile.source,
            TileReadySource::DiskCacheFresh | TileReadySource::DiskCacheStale
        );
        self.pending.push(tile);
        if is_memory_cache {
            return self.flush(sender);
        }
        let limit = if is_disk_cache {
            CACHE_READY_BATCH_LIMIT
        } else if self.quick_reveal {
            FIRST_REVEAL_READY_BATCH_LIMIT
        } else {
            TILE_READY_BATCH_LIMIT
        };
        let interval = if is_disk_cache {
            CACHE_READY_BATCH_INTERVAL
        } else if self.quick_reveal {
            FIRST_REVEAL_READY_BATCH_INTERVAL
        } else {
            TILE_READY_BATCH_INTERVAL
        };
        if self.pending.len() >= limit || self.last_flush.elapsed() >= interval {
            return self.flush(sender);
        }
        true
    }

    pub(super) fn flush(&mut self, sender: &Arc<Mutex<UnboundedSender<TileRenderEvent>>>) -> bool {
        if self.pending.is_empty() {
            return true;
        }
        let tiles = std::mem::take(&mut self.pending);
        self.last_flush = Instant::now();
        send_tile_event(sender, TileRenderEvent::ReadyBatch { tiles })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TileLoadState {
    PendingManifest,
    Queued,
    Loading,
    Loaded,
    Failed,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TileSourceStatus {
    Miss,
    DiskStale,
    Fresh,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TileSourceFreshness {
    Fresh,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum TilePriority {
    EditRefresh,
    Visible,
    Prefetch,
}

#[derive(Clone)]
pub(super) struct TileEntry {
    pub(super) state: TileLoadState,
    pub(super) source_status: TileSourceStatus,
    pub(super) image: Option<ViewerTile>,
    pub(super) priority: TilePriority,
    pub(super) sequence: u64,
    pub(super) attempts: u8,
    pub(super) retry_after: Option<Instant>,
    pub(super) last_error: Option<SharedString>,
}

impl TileEntry {
    pub(super) fn pending_manifest(priority: TilePriority, sequence: u64) -> Self {
        Self {
            state: TileLoadState::PendingManifest,
            source_status: TileSourceStatus::Miss,
            image: None,
            priority,
            sequence,
            attempts: 0,
            retry_after: None,
            last_error: None,
        }
    }

    pub(super) fn queued(priority: TilePriority, sequence: u64) -> Self {
        Self {
            state: TileLoadState::Queued,
            source_status: TileSourceStatus::Miss,
            image: None,
            priority,
            sequence,
            attempts: 0,
            retry_after: None,
            last_error: None,
        }
    }

    pub(super) fn mark_failed(&mut self, message: SharedString) -> Option<Arc<RenderImage>> {
        let dropped_image = tile_entry_take_render_image(self);
        self.state = TileLoadState::Failed;
        self.source_status = TileSourceStatus::Invalid;
        self.priority = TilePriority::Prefetch;
        self.attempts = self.attempts.saturating_add(1);
        let shift = u32::from(self.attempts.min(5));
        let retry_ms = 750_u64.saturating_mul(1_u64 << shift).min(15_000);
        self.retry_after = Some(Instant::now() + Duration::from_millis(retry_ms));
        self.last_error = Some(message);
        dropped_image
    }
}

#[derive(Default)]
pub(super) struct RegionManager {
    pub(super) entries: BTreeMap<(i32, i32), TileEntry>,
    pub(super) next_sequence: u64,
    pub(super) loaded_estimated_bytes: usize,
}

impl RegionManager {
    pub(super) fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        let dropped_images = self
            .entries
            .values_mut()
            .filter_map(tile_entry_take_render_image)
            .collect::<Vec<_>>();
        self.entries.clear();
        self.next_sequence = 0;
        self.loaded_estimated_bytes = 0;
        dropped_images
    }

    pub(super) fn ensure_tiles(&mut self, coords: &[(i32, i32)], priority: TilePriority) {
        let now = Instant::now();
        for coord in coords {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            match self.entries.get_mut(coord) {
                Some(entry) => {
                    if priority < entry.priority {
                        entry.priority = priority;
                        entry.sequence = sequence;
                    }
                    if matches!(entry.state, TileLoadState::Failed)
                        && entry
                            .retry_after
                            .is_none_or(|retry_after| retry_after <= now)
                    {
                        entry.state = TileLoadState::Queued;
                        entry.retry_after = None;
                    }
                }
                None => {
                    self.entries
                        .insert(*coord, TileEntry::queued(priority, sequence));
                }
            }
        }
    }

    pub(super) fn force_refresh_tiles(
        &mut self,
        coords: &[(i32, i32)],
        priority: TilePriority,
    ) -> Vec<Arc<RenderImage>> {
        let mut dropped_images = Vec::new();
        for coord in coords {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let mut previous = self.entries.remove(coord);
            let previous_bytes = previous
                .as_ref()
                .map_or(0, tile_entry_loaded_estimated_bytes);
            if let Some(previous) = previous.as_mut()
                && let Some(image) = tile_entry_take_render_image(previous)
            {
                dropped_images.push(image);
            }
            let mut entry = TileEntry::queued(priority, sequence);
            if let Some(previous) = previous {
                entry.attempts = previous.attempts;
            }
            self.entries.insert(*coord, entry);
            self.loaded_estimated_bytes =
                self.loaded_estimated_bytes.saturating_sub(previous_bytes);
        }
        dropped_images
    }

    pub(super) fn loaded_tile(&self, coord: (i32, i32)) -> Option<ViewerTile> {
        self.entries
            .get(&coord)
            .and_then(|entry| entry.image.as_ref())
            .cloned()
    }

    pub(super) fn ensure_pending_manifest(
        &mut self,
        coords: &[(i32, i32)],
        priority: TilePriority,
    ) {
        for coord in coords {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            match self.entries.get_mut(coord) {
                Some(entry) => {
                    if priority < entry.priority {
                        entry.priority = priority;
                        entry.sequence = sequence;
                    }
                    if matches!(entry.state, TileLoadState::Queued | TileLoadState::Failed) {
                        entry.state = TileLoadState::PendingManifest;
                        entry.retry_after = None;
                    }
                }
                None => {
                    self.entries
                        .insert(*coord, TileEntry::pending_manifest(priority, sequence));
                }
            }
        }
    }

    pub(super) fn remove_tile(&mut self, coord: (i32, i32)) -> Option<Arc<RenderImage>> {
        if let Some(mut entry) = self.entries.remove(&coord) {
            self.loaded_estimated_bytes = self
                .loaded_estimated_bytes
                .saturating_sub(tile_entry_loaded_estimated_bytes(&entry));
            tile_entry_take_render_image(&mut entry)
        } else {
            None
        }
    }

    pub(super) fn retain_tiles(
        &mut self,
        retain_tiles: &BTreeSet<(i32, i32)>,
    ) -> Vec<Arc<RenderImage>> {
        let mut dropped_images = Vec::new();
        let mut removed_bytes = 0usize;
        self.entries.retain(|coord, entry| {
            if retain_tiles.contains(coord) {
                return true;
            }
            removed_bytes = removed_bytes.saturating_add(tile_entry_loaded_estimated_bytes(entry));
            if let Some(image) = tile_entry_take_render_image(entry) {
                dropped_images.push(image);
            }
            false
        });
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(removed_bytes);
        dropped_images
    }

    pub(super) fn trim_loaded_tiles_to_budget(
        &mut self,
        visible_tiles: &BTreeSet<(i32, i32)>,
        budget: usize,
    ) -> Vec<Arc<RenderImage>> {
        let mut loaded_bytes = self.loaded_estimated_bytes;
        if loaded_bytes <= budget {
            return Vec::new();
        }
        let mut dropped_images = Vec::new();
        let mut candidates = self
            .entries
            .iter()
            .filter_map(|(coord, entry)| {
                if entry.state != TileLoadState::Loaded || visible_tiles.contains(coord) {
                    return None;
                }
                Some(Reverse((
                    trim_loaded_tile_sort_key(entry.priority, entry.sequence, entry.source_status),
                    *coord,
                    tile_entry_loaded_estimated_bytes(entry),
                )))
            })
            .collect::<BinaryHeap<_>>();
        while let Some(Reverse((_, coord, bytes))) = candidates.pop() {
            if loaded_bytes <= budget {
                break;
            }
            if let Some(entry) = self.entries.get_mut(&coord) {
                if entry.state == TileLoadState::Loaded {
                    if let Some(image) = tile_entry_take_render_image(entry) {
                        dropped_images.push(image);
                    }
                    entry.state = TileLoadState::Queued;
                    entry.source_status = TileSourceStatus::Miss;
                    loaded_bytes = loaded_bytes.saturating_sub(bytes);
                }
            }
        }
        self.loaded_estimated_bytes = loaded_bytes;
        dropped_images
    }

    pub(super) fn queued_coords(
        &self,
        center: (i32, i32),
        visible_bounds: Option<TileBounds>,
        allow_prefetch: bool,
        prioritize_center: bool,
    ) -> Vec<(i32, i32)> {
        self.queued_coords_limited(
            center,
            visible_bounds,
            allow_prefetch,
            prioritize_center,
            usize::MAX,
        )
    }

    pub(super) fn queued_coords_limited(
        &self,
        center: (i32, i32),
        visible_bounds: Option<TileBounds>,
        allow_prefetch: bool,
        prioritize_center: bool,
        limit: usize,
    ) -> Vec<(i32, i32)> {
        if limit == 0 {
            return Vec::new();
        }
        let now = Instant::now();
        let selected_priority = self
            .entries
            .iter()
            .filter_map(|(_, entry)| queued_entry_is_ready(entry, now).then_some(entry.priority))
            .min();
        let Some(selected_priority) = selected_priority else {
            return Vec::new();
        };
        if selected_priority > TilePriority::Visible && !allow_prefetch {
            return Vec::new();
        }

        if limit == usize::MAX {
            let mut candidates = self
                .entries
                .iter()
                .filter_map(|(coord, entry)| {
                    let priority_matches = selected_priority > TilePriority::Visible
                        || entry.priority == selected_priority;
                    (priority_matches && queued_entry_is_ready(entry, now)).then_some((
                        *coord,
                        queued_tile_sort_key(
                            *coord,
                            entry.priority,
                            entry.sequence,
                            entry.state,
                            center,
                            visible_bounds,
                            prioritize_center,
                        ),
                    ))
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|(_, sort_key)| *sort_key);
            return candidates.into_iter().map(|(coord, _)| coord).collect();
        }

        let mut candidates = BinaryHeap::<(QueuedTileSortKey, (i32, i32))>::with_capacity(limit);
        for (coord, entry) in &self.entries {
            let priority_matches =
                selected_priority > TilePriority::Visible || entry.priority == selected_priority;
            if !priority_matches || !queued_entry_is_ready(entry, now) {
                continue;
            }
            let sort_key = queued_tile_sort_key(
                *coord,
                entry.priority,
                entry.sequence,
                entry.state,
                center,
                visible_bounds,
                prioritize_center,
            );
            if candidates.len() < limit {
                candidates.push((sort_key, *coord));
            } else if candidates
                .peek()
                .is_some_and(|(worst_key, _)| sort_key < *worst_key)
            {
                candidates.pop();
                candidates.push((sort_key, *coord));
            }
        }
        let mut candidates = candidates.into_vec();
        candidates.sort_by_key(|(sort_key, _)| *sort_key);
        candidates.into_iter().map(|(_, coord)| coord).collect()
    }

    pub(super) fn mark_loading(&mut self, coords: &[(i32, i32)]) {
        for coord in coords {
            if let Some(entry) = self.entries.get_mut(coord) {
                entry.state = TileLoadState::Loading;
                entry.retry_after = None;
                entry.last_error = None;
            }
        }
    }

    pub(super) fn mark_manifest_ready(&mut self, coord: (i32, i32), priority: TilePriority) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let entry = self
            .entries
            .entry(coord)
            .or_insert_with(|| TileEntry::queued(priority, sequence));
        if priority < entry.priority {
            entry.priority = priority;
        }
        if matches!(
            entry.state,
            TileLoadState::PendingManifest | TileLoadState::Failed
        ) {
            entry.state = TileLoadState::Queued;
            entry.retry_after = None;
            entry.last_error = None;
        }
    }

    pub(super) fn has_visible_work(&self) -> bool {
        let now = Instant::now();
        self.entries.values().any(|entry| {
            entry.priority <= TilePriority::Visible
                && (matches!(entry.state, TileLoadState::Queued)
                    || (matches!(entry.state, TileLoadState::Failed)
                        && entry
                            .retry_after
                            .is_none_or(|retry_after| retry_after <= now)))
        })
    }

    pub(super) fn has_pending_manifest_for_tiles(&self, coords: &[(i32, i32)]) -> bool {
        coords.iter().any(|coord| {
            self.entries
                .get(coord)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::PendingManifest))
        })
    }

    pub(super) fn pending_manifest_coords_with_priority(
        &self,
        priority: TilePriority,
    ) -> Vec<(i32, i32)> {
        let mut coords = self
            .entries
            .iter()
            .filter_map(|(coord, entry)| {
                (entry.state == TileLoadState::PendingManifest && entry.priority == priority)
                    .then_some((*coord, entry.sequence))
            })
            .collect::<Vec<_>>();
        coords.sort_by_key(|(_, sequence)| *sequence);
        coords.into_iter().map(|(coord, _)| coord).collect()
    }

    pub(super) fn mark_loaded(
        &mut self,
        coord: (i32, i32),
        tile: ViewerTile,
    ) -> Option<Arc<RenderImage>> {
        let new_bytes = tile.estimated_bytes;
        let previous_bytes;
        let mut dropped_image = None;
        if let Some(entry) = self.entries.get_mut(&coord) {
            previous_bytes = tile_entry_loaded_estimated_bytes(entry);
            dropped_image = entry.image.replace(tile).map(|tile| tile.image);
            entry.state = TileLoadState::Loaded;
            entry.source_status = TileSourceStatus::Fresh;
            entry.attempts = 0;
            entry.retry_after = None;
            entry.last_error = None;
        } else {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let mut entry = TileEntry::queued(TilePriority::Prefetch, sequence);
            previous_bytes = 0;
            entry.state = TileLoadState::Loaded;
            entry.source_status = TileSourceStatus::Fresh;
            entry.image = Some(tile);
            self.entries.insert(coord, entry);
        }
        self.loaded_estimated_bytes = self
            .loaded_estimated_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(new_bytes);
        dropped_image
    }

    pub(super) fn mark_loaded_from_cache(
        &mut self,
        coord: (i32, i32),
        tile: ViewerTile,
        freshness: TileSourceFreshness,
    ) -> bool {
        self.mark_loaded_from_cache_with_eviction(coord, tile, freshness)
            .0
    }

    pub(super) fn mark_loaded_from_cache_with_eviction(
        &mut self,
        coord: (i32, i32),
        tile: ViewerTile,
        freshness: TileSourceFreshness,
    ) -> (bool, Option<Arc<RenderImage>>) {
        let Some(entry) = self.entries.get_mut(&coord) else {
            return (false, None);
        };
        if matches!(entry.state, TileLoadState::Invalid)
            || entry.source_status == TileSourceStatus::Fresh
        {
            return (false, None);
        }
        let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
        let new_bytes = tile.estimated_bytes;
        let dropped_image = entry.image.replace(tile).map(|tile| tile.image);
        entry.state = TileLoadState::Loaded;
        entry.source_status = match freshness {
            TileSourceFreshness::Fresh => TileSourceStatus::Fresh,
            TileSourceFreshness::Stale => TileSourceStatus::DiskStale,
        };
        entry.attempts = 0;
        entry.retry_after = None;
        entry.last_error = None;
        self.loaded_estimated_bytes = self
            .loaded_estimated_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(new_bytes);
        (true, dropped_image)
    }

    pub(super) fn mark_failed(
        &mut self,
        coord: (i32, i32),
        message: SharedString,
    ) -> Option<Arc<RenderImage>> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let entry = self
            .entries
            .entry(coord)
            .or_insert_with(|| TileEntry::queued(TilePriority::Prefetch, sequence));
        let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
        let dropped_image = entry.mark_failed(message);
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(previous_bytes);
        dropped_image
    }

    pub(super) fn mark_invalid(
        &mut self,
        coord: (i32, i32),
        message: SharedString,
    ) -> Option<Arc<RenderImage>> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let entry = self
            .entries
            .entry(coord)
            .or_insert_with(|| TileEntry::queued(TilePriority::Prefetch, sequence));
        let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
        let dropped_image = tile_entry_take_render_image(entry);
        entry.state = TileLoadState::Invalid;
        entry.source_status = TileSourceStatus::Invalid;
        entry.priority = TilePriority::Prefetch;
        entry.attempts = 0;
        entry.retry_after = None;
        entry.last_error = Some(message);
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(previous_bytes);
        dropped_image
    }

    pub(super) fn loaded_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == TileLoadState::Loaded)
            .count()
    }

    pub(super) fn queued_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.state, TileLoadState::Queued | TileLoadState::Failed))
            .count()
    }

    pub(super) fn pending_manifest_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == TileLoadState::PendingManifest)
            .count()
    }

    pub(super) fn loading_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == TileLoadState::Loading)
            .count()
    }

    pub(super) fn cache_miss_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.source_status == TileSourceStatus::Miss)
            .count()
    }

    pub(super) fn failed_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == TileLoadState::Failed)
            .count()
    }

    pub(super) fn invalid_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == TileLoadState::Invalid)
            .count()
    }

    pub(super) fn loaded_estimated_bytes(&self) -> usize {
        self.loaded_estimated_bytes
    }
}

pub(super) fn tile_entry_loaded_estimated_bytes(entry: &TileEntry) -> usize {
    if entry.state == TileLoadState::Loaded {
        entry.image.as_ref().map_or(0, |tile| tile.estimated_bytes)
    } else {
        0
    }
}

fn tile_entry_take_render_image(entry: &mut TileEntry) -> Option<Arc<RenderImage>> {
    entry.image.take().map(|tile| tile.image)
}

fn queued_entry_is_ready(entry: &TileEntry, now: Instant) -> bool {
    matches!(entry.state, TileLoadState::Queued)
        || (matches!(entry.state, TileLoadState::Failed)
            && entry
                .retry_after
                .is_none_or(|retry_after| retry_after <= now))
}

type TrimLoadedTileSortKey = (u8, bool, u64);

fn trim_loaded_tile_sort_key(
    priority: TilePriority,
    sequence: u64,
    source_status: TileSourceStatus,
) -> TrimLoadedTileSortKey {
    let priority_rank = match priority {
        TilePriority::Prefetch => 0_u8,
        TilePriority::Visible => 1_u8,
        TilePriority::EditRefresh => 2_u8,
    };
    (
        priority_rank,
        matches!(source_status, TileSourceStatus::Fresh),
        sequence,
    )
}

type QueuedTileSortKey = (
    TilePriority,
    bool,
    bool,
    i64,
    (i64, i64, i64, i32, i32),
    u64,
    (i64, i64, i64, i32, i32),
    u64,
    i32,
    i32,
);

fn queued_tile_sort_key(
    coord: (i32, i32),
    priority: TilePriority,
    sequence: u64,
    state: TileLoadState,
    center: (i32, i32),
    visible_bounds: Option<TileBounds>,
    prioritize_center: bool,
) -> QueuedTileSortKey {
    let distance_to_visible = visible_bounds
        .map(|bounds| squared_distance_to_tile_bounds(coord.0, coord.1, bounds))
        .unwrap_or_else(|| {
            prioritize_center
                .then_some(tile_center_distance_squared(coord, center))
                .unwrap_or(0)
        });
    let center_sort_key = tile_distance_sort_key(coord, center);
    (
        priority,
        matches!(state, TileLoadState::Failed),
        distance_to_visible > 0,
        distance_to_visible,
        prioritize_center
            .then_some(center_sort_key)
            .unwrap_or((0, 0, 0, 0, 0)),
        (!prioritize_center).then_some(sequence).unwrap_or(0),
        (!prioritize_center)
            .then_some(center_sort_key)
            .unwrap_or((0, 0, 0, 0, 0)),
        sequence,
        coord.1,
        coord.0,
    )
}
