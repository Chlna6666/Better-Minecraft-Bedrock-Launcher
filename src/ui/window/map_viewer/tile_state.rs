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
    pub(super) chunk_positions: Option<TileChunkPositions>,
}

pub(super) type ActiveRenderTiles = BTreeMap<(i32, i32), usize>;

pub(super) fn track_active_render_tiles(
    active_tiles: &mut ActiveRenderTiles,
    requested_tiles: &[(i32, i32)],
) {
    for coord in requested_tiles {
        let count = active_tiles.entry(*coord).or_insert(0);
        *count = count.saturating_add(1);
    }
}

pub(super) fn finish_active_render_tiles(
    active_tiles: &mut ActiveRenderTiles,
    requested_tiles: &[(i32, i32)],
) {
    for coord in requested_tiles {
        let Some(count) = active_tiles.get_mut(coord) else {
            continue;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            active_tiles.remove(coord);
        }
    }
}

pub(super) fn requeue_active_render_tiles_after_cancel(
    tile_manager: &mut RegionManager,
    active_tiles: &mut ActiveRenderTiles,
) {
    let active_coords = active_tiles.keys().copied().collect::<Vec<_>>();
    tile_manager.requeue_cancelled_loading(&active_coords);
    active_tiles.clear();
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
    pub(super) center_tile: (i32, i32),
}

impl Default for TileReadyBatcher {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            last_flush: Instant::now(),
            quick_reveal: false,
            center_tile: (0, 0),
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

    pub(super) fn with_center(quick_reveal: bool, center_tile: (i32, i32)) -> Self {
        Self {
            quick_reveal,
            center_tile,
            ..Self::default()
        }
    }

    pub(super) fn push(&mut self, tile: ReadyTile) -> Option<Vec<ReadyTile>> {
        let cache_hit = matches!(
            tile.source,
            TileReadySource::MemoryCache
                | TileReadySource::DiskCacheFresh
                | TileReadySource::DiskCacheStale
        );
        self.pending.push(tile);
        let limit = if cache_hit {
            if self.quick_reveal {
                FIRST_REVEAL_READY_BATCH_LIMIT
            } else {
                TILE_READY_BATCH_LIMIT
            }
        } else if self.quick_reveal {
            FIRST_REVEAL_READY_BATCH_LIMIT
        } else {
            TILE_READY_BATCH_LIMIT
        };
        let interval = if self.quick_reveal {
            FIRST_REVEAL_READY_BATCH_INTERVAL
        } else {
            TILE_READY_BATCH_INTERVAL
        };
        if self.pending.len() >= limit || self.last_flush.elapsed() >= interval {
            return self.flush();
        }
        None
    }

    pub(super) fn flush(&mut self) -> Option<Vec<ReadyTile>> {
        if self.pending.is_empty() {
            return None;
        }
        self.pending
            .sort_unstable_by_key(|tile| tile_distance_sort_key(tile.coord, self.center_tile));
        let tiles = std::mem::take(&mut self.pending);
        self.last_flush = Instant::now();
        Some(tiles)
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TileLoadStateCounts {
    pub(super) pending_manifest: usize,
    pub(super) queued: usize,
    pub(super) loading: usize,
    pub(super) loaded: usize,
    pub(super) failed: usize,
    pub(super) invalid: usize,
}

impl TileLoadStateCounts {
    fn increment(&mut self, state: TileLoadState) {
        match state {
            TileLoadState::PendingManifest => {
                self.pending_manifest = self.pending_manifest.saturating_add(1);
            }
            TileLoadState::Queued => {
                self.queued = self.queued.saturating_add(1);
            }
            TileLoadState::Loading => {
                self.loading = self.loading.saturating_add(1);
            }
            TileLoadState::Loaded => {
                self.loaded = self.loaded.saturating_add(1);
            }
            TileLoadState::Failed => {
                self.failed = self.failed.saturating_add(1);
            }
            TileLoadState::Invalid => {
                self.invalid = self.invalid.saturating_add(1);
            }
        }
    }

    fn decrement(&mut self, state: TileLoadState) {
        match state {
            TileLoadState::PendingManifest => {
                self.pending_manifest = self.pending_manifest.saturating_sub(1);
            }
            TileLoadState::Queued => {
                self.queued = self.queued.saturating_sub(1);
            }
            TileLoadState::Loading => {
                self.loading = self.loading.saturating_sub(1);
            }
            TileLoadState::Loaded => {
                self.loaded = self.loaded.saturating_sub(1);
            }
            TileLoadState::Failed => {
                self.failed = self.failed.saturating_sub(1);
            }
            TileLoadState::Invalid => {
                self.invalid = self.invalid.saturating_sub(1);
            }
        }
    }

    fn transition(&mut self, old_state: TileLoadState, new_state: TileLoadState) {
        if old_state == new_state {
            return;
        }
        self.decrement(old_state);
        self.increment(new_state);
    }

    fn subtract(&mut self, removed: TileLoadStateCounts) {
        self.pending_manifest = self
            .pending_manifest
            .saturating_sub(removed.pending_manifest);
        self.queued = self.queued.saturating_sub(removed.queued);
        self.loading = self.loading.saturating_sub(removed.loading);
        self.loaded = self.loaded.saturating_sub(removed.loaded);
        self.failed = self.failed.saturating_sub(removed.failed);
        self.invalid = self.invalid.saturating_sub(removed.invalid);
    }
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
    pub(super) last_access: u64,
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
            last_access: sequence,
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
            last_access: sequence,
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
    pub(super) next_access: u64,
    pub(super) loaded_estimated_bytes: usize,
    pub(super) state_counts: TileLoadStateCounts,
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
        self.next_access = 0;
        self.loaded_estimated_bytes = 0;
        self.state_counts = TileLoadStateCounts::default();
        dropped_images
    }

    pub(super) fn ensure_tiles(&mut self, coords: &[(i32, i32)], priority: TilePriority) {
        let now = Instant::now();
        for coord in coords {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let last_access = self.allocate_access_stamp();
            match self.entries.get_mut(coord) {
                Some(entry) => {
                    entry.last_access = last_access;
                    if priority < entry.priority {
                        entry.priority = priority;
                        entry.sequence = sequence;
                    }
                    if matches!(entry.state, TileLoadState::Failed)
                        && entry
                            .retry_after
                            .is_none_or(|retry_after| retry_after <= now)
                    {
                        self.state_counts
                            .transition(entry.state, TileLoadState::Queued);
                        entry.state = TileLoadState::Queued;
                        entry.retry_after = None;
                    } else if entry.state == TileLoadState::Loaded && entry.image.is_none() {
                        // A GPU/LRU eviction can leave the manifest entry intact while the
                        // render image is gone. Treat it as a cold queue item immediately.
                        self.state_counts
                            .transition(entry.state, TileLoadState::Queued);
                        entry.state = TileLoadState::Queued;
                        entry.source_status = TileSourceStatus::Miss;
                        entry.retry_after = None;
                        entry.last_error = None;
                    }
                }
                None => {
                    self.state_counts.increment(TileLoadState::Queued);
                    let mut entry = TileEntry::queued(priority, sequence);
                    entry.last_access = last_access;
                    self.entries.insert(*coord, entry);
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
            let last_access = self.allocate_access_stamp();
            let mut previous = self.entries.remove(coord);
            let previous_bytes = previous
                .as_ref()
                .map_or(0, tile_entry_loaded_estimated_bytes);
            if let Some(previous) = previous.as_ref() {
                self.state_counts.decrement(previous.state);
            }
            if let Some(previous) = previous.as_mut()
                && let Some(image) = tile_entry_take_render_image(previous)
            {
                dropped_images.push(image);
            }
            let mut entry = TileEntry::queued(priority, sequence);
            entry.last_access = last_access;
            if let Some(previous) = previous {
                entry.attempts = previous.attempts;
            }
            self.state_counts.increment(entry.state);
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
    ) -> bool {
        let mut needs_cache_bypass = false;
        for coord in coords {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let last_access = self.allocate_access_stamp();
            match self.entries.get_mut(coord) {
                Some(entry) => {
                    entry.last_access = last_access;
                    if priority < entry.priority {
                        entry.priority = priority;
                        entry.sequence = sequence;
                    }
                    if matches!(entry.state, TileLoadState::Queued | TileLoadState::Failed) {
                        self.state_counts
                            .transition(entry.state, TileLoadState::PendingManifest);
                        entry.state = TileLoadState::PendingManifest;
                        entry.retry_after = None;
                    } else if entry.state == TileLoadState::Loaded && entry.image.is_none() {
                        self.state_counts
                            .transition(entry.state, TileLoadState::PendingManifest);
                        entry.state = TileLoadState::PendingManifest;
                        entry.source_status = TileSourceStatus::Miss;
                        entry.retry_after = None;
                        entry.last_error = None;
                        needs_cache_bypass = true;
                    }
                }
                None => {
                    self.state_counts.increment(TileLoadState::PendingManifest);
                    let mut entry = TileEntry::pending_manifest(priority, sequence);
                    entry.last_access = last_access;
                    self.entries.insert(*coord, entry);
                }
            }
        }
        needs_cache_bypass
    }

    pub(super) fn remove_tile(&mut self, coord: (i32, i32)) -> Option<Arc<RenderImage>> {
        if let Some(mut entry) = self.entries.remove(&coord) {
            self.state_counts.decrement(entry.state);
            self.loaded_estimated_bytes = self
                .loaded_estimated_bytes
                .saturating_sub(tile_entry_loaded_estimated_bytes(&entry));
            tile_entry_take_render_image(&mut entry)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(super) fn retain_tiles(
        &mut self,
        retain_tiles: &BTreeSet<(i32, i32)>,
    ) -> Vec<Arc<RenderImage>> {
        self.retain_tiles_by(|coord| retain_tiles.contains(&coord))
    }

    pub(super) fn retain_tiles_by(
        &mut self,
        mut should_retain: impl FnMut((i32, i32)) -> bool,
    ) -> Vec<Arc<RenderImage>> {
        let mut dropped_images = Vec::new();
        let mut removed_bytes = 0usize;
        let mut removed_counts = TileLoadStateCounts::default();
        self.entries.retain(|coord, entry| {
            if should_retain(*coord) {
                return true;
            }
            removed_counts.increment(entry.state);
            removed_bytes = removed_bytes.saturating_add(tile_entry_loaded_estimated_bytes(entry));
            if let Some(image) = tile_entry_take_render_image(entry) {
                dropped_images.push(image);
            }
            false
        });
        self.state_counts.subtract(removed_counts);
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(removed_bytes);
        dropped_images
    }

    pub(super) fn trim_entries_to_capacity_by(
        &mut self,
        mut should_retain: impl FnMut((i32, i32)) -> bool,
        capacity: usize,
    ) -> Vec<Arc<RenderImage>> {
        if self.entries.len() <= capacity {
            return Vec::new();
        }

        // Keep active render ownership until its completion event has updated the state
        // machine. Pending manifest entries are cheap and can be probed again after eviction.
        let mut candidates = self
            .entries
            .iter()
            .filter_map(|(coord, entry)| {
                (!should_retain(*coord) && entry.state != TileLoadState::Loading)
                    .then_some((entry.last_access, *coord))
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable();

        let mut dropped_images = Vec::new();
        for (_, coord) in candidates {
            if self.entries.len() <= capacity {
                break;
            }
            if let Some(mut entry) = self.entries.remove(&coord) {
                self.state_counts.decrement(entry.state);
                self.loaded_estimated_bytes = self
                    .loaded_estimated_bytes
                    .saturating_sub(tile_entry_loaded_estimated_bytes(&entry));
                if let Some(image) = tile_entry_take_render_image(&mut entry) {
                    dropped_images.push(image);
                }
            }
        }
        dropped_images
    }

    #[cfg(test)]
    pub(super) fn trim_loaded_tiles_to_budget(
        &mut self,
        visible_tiles: &BTreeSet<(i32, i32)>,
        budget: usize,
    ) -> Vec<Arc<RenderImage>> {
        self.trim_loaded_tiles_to_budget_by(|coord| visible_tiles.contains(&coord), budget)
    }

    pub(super) fn trim_loaded_tiles_to_budget_by(
        &mut self,
        mut should_retain: impl FnMut((i32, i32)) -> bool,
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
                if entry.image.is_none() || should_retain(*coord) {
                    return None;
                }
                Some(Reverse((
                    trim_loaded_tile_sort_key(
                        entry.last_access,
                        entry.priority,
                        entry.sequence,
                        entry.source_status,
                    ),
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
                if entry.image.is_some() {
                    if let Some(image) = tile_entry_take_render_image(entry) {
                        dropped_images.push(image);
                    }
                    if entry.state == TileLoadState::Loaded {
                        self.state_counts
                            .transition(entry.state, TileLoadState::Queued);
                        entry.state = TileLoadState::Queued;
                    }
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

        if limit == usize::MAX {
            let mut selected_priority = None;
            let mut candidates = Vec::new();
            for (coord, entry) in &self.entries {
                if !queued_entry_is_ready(entry, now) {
                    continue;
                }
                if queued_candidate_changes_priority(&mut selected_priority, entry.priority) {
                    candidates.clear();
                }
                if !queued_candidate_priority_matches(selected_priority, entry.priority) {
                    continue;
                }
                candidates.push((
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
                ));
            }
            let Some(selected_priority) = selected_priority else {
                return Vec::new();
            };
            if selected_priority > TilePriority::Visible && !allow_prefetch {
                return Vec::new();
            }
            candidates.sort_by_key(|(_, sort_key)| *sort_key);
            return candidates.into_iter().map(|(coord, _)| coord).collect();
        }

        let mut selected_priority = None;
        let mut candidates = BinaryHeap::<(QueuedTileSortKey, (i32, i32))>::with_capacity(limit);
        for (coord, entry) in &self.entries {
            if !queued_entry_is_ready(entry, now) {
                continue;
            }
            if queued_candidate_changes_priority(&mut selected_priority, entry.priority) {
                candidates.clear();
            }
            if !queued_candidate_priority_matches(selected_priority, entry.priority) {
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
        let Some(selected_priority) = selected_priority else {
            return Vec::new();
        };
        if selected_priority > TilePriority::Visible && !allow_prefetch {
            return Vec::new();
        }
        let mut candidates = candidates.into_vec();
        candidates.sort_by_key(|(sort_key, _)| *sort_key);
        candidates.into_iter().map(|(_, coord)| coord).collect()
    }

    pub(super) fn queued_visible_coords_limited(
        &self,
        visible_tiles: &[(i32, i32)],
        center: (i32, i32),
        limit: usize,
    ) -> Vec<(i32, i32)> {
        if limit == 0 {
            return Vec::new();
        }
        if tiles_are_sorted_center_first(visible_tiles, center) {
            return self.queued_visible_coords_limited_ordered(visible_tiles, limit);
        }
        let now = Instant::now();
        let mut selected_priority = None;
        let mut candidates = BinaryHeap::<(QueuedTileSortKey, (i32, i32))>::with_capacity(limit);
        for coord in visible_tiles {
            let Some(entry) = self.entries.get(coord) else {
                continue;
            };
            if entry.priority > TilePriority::Visible || !queued_entry_is_ready(entry, now) {
                continue;
            }
            if queued_candidate_changes_priority(&mut selected_priority, entry.priority) {
                candidates.clear();
            }
            if !queued_candidate_priority_matches(selected_priority, entry.priority) {
                continue;
            }
            let sort_key = queued_tile_sort_key(
                *coord,
                entry.priority,
                entry.sequence,
                entry.state,
                center,
                None,
                true,
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

    pub(super) fn queued_visible_coords_limited_ordered(
        &self,
        visible_tiles: &[(i32, i32)],
        limit: usize,
    ) -> Vec<(i32, i32)> {
        if limit == 0 {
            return Vec::new();
        }
        let now = Instant::now();
        let mut coords = Vec::with_capacity(limit.min(visible_tiles.len()));
        for priority in [TilePriority::EditRefresh, TilePriority::Visible] {
            for include_failed in [false, true] {
                for coord in visible_tiles {
                    if coords.len() >= limit {
                        return coords;
                    }
                    let Some(entry) = self.entries.get(coord) else {
                        continue;
                    };
                    if entry.priority != priority
                        || matches!(entry.state, TileLoadState::Failed) != include_failed
                        || !queued_entry_is_ready(entry, now)
                    {
                        continue;
                    }
                    coords.push(*coord);
                }
            }
        }
        coords
    }

    pub(super) fn mark_loading(&mut self, coords: &[(i32, i32)]) {
        for coord in coords {
            if let Some(entry) = self.entries.get_mut(coord) {
                self.state_counts
                    .transition(entry.state, TileLoadState::Loading);
                entry.state = TileLoadState::Loading;
                entry.retry_after = None;
                entry.last_error = None;
            }
        }
    }

    pub(super) fn requeue_cancelled_loading(&mut self, coords: &[(i32, i32)]) {
        for coord in coords {
            if let Some(entry) = self.entries.get_mut(coord)
                && matches!(entry.state, TileLoadState::Loading)
            {
                self.state_counts
                    .transition(entry.state, TileLoadState::Queued);
                entry.state = TileLoadState::Queued;
                entry.retry_after = None;
                entry.last_error = None;
            }
        }
    }

    pub(super) fn mark_manifest_ready(&mut self, coord: (i32, i32), priority: TilePriority) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        match self.entries.get_mut(&coord) {
            Some(entry) => {
                if priority < entry.priority {
                    entry.priority = priority;
                }
                if matches!(
                    entry.state,
                    TileLoadState::PendingManifest | TileLoadState::Failed
                ) {
                    self.state_counts
                        .transition(entry.state, TileLoadState::Queued);
                    entry.state = TileLoadState::Queued;
                    entry.retry_after = None;
                    entry.last_error = None;
                }
            }
            None => {
                self.state_counts.increment(TileLoadState::Queued);
                self.entries
                    .insert(coord, TileEntry::queued(priority, sequence));
            }
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

    pub(super) fn is_pending_manifest(&self, coord: (i32, i32)) -> bool {
        self.entries
            .get(&coord)
            .is_some_and(|entry| matches!(entry.state, TileLoadState::PendingManifest))
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
        let last_access = self.allocate_access_stamp();
        if let Some(entry) = self.entries.get_mut(&coord) {
            previous_bytes = tile_entry_loaded_estimated_bytes(entry);
            dropped_image = entry.image.replace(tile).map(|tile| tile.image);
            entry.last_access = last_access;
            self.state_counts
                .transition(entry.state, TileLoadState::Loaded);
            entry.state = TileLoadState::Loaded;
            entry.source_status = TileSourceStatus::Fresh;
            entry.attempts = 0;
            entry.retry_after = None;
            entry.last_error = None;
        } else {
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let mut entry = TileEntry::queued(TilePriority::Prefetch, sequence);
            entry.last_access = last_access;
            previous_bytes = 0;
            entry.state = TileLoadState::Loaded;
            entry.source_status = TileSourceStatus::Fresh;
            entry.image = Some(tile);
            self.state_counts.increment(entry.state);
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
        let last_access = self.allocate_access_stamp();
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
        entry.last_access = last_access;
        self.state_counts
            .transition(entry.state, TileLoadState::Loaded);
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

    pub(super) fn requeue_stale_cache_for_refresh(&mut self, coord: (i32, i32)) -> bool {
        let Some(entry) = self.entries.get_mut(&coord) else {
            return false;
        };
        if entry.source_status != TileSourceStatus::DiskStale || entry.image.is_none() {
            return false;
        }
        if entry.state != TileLoadState::Queued {
            self.state_counts
                .transition(entry.state, TileLoadState::Queued);
            entry.state = TileLoadState::Queued;
        }
        entry.retry_after = None;
        entry.last_error = None;
        true
    }

    pub(super) fn mark_failed(
        &mut self,
        coord: (i32, i32),
        message: SharedString,
    ) -> Option<Arc<RenderImage>> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let (previous_bytes, dropped_image) = if let Some(entry) = self.entries.get_mut(&coord) {
            let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
            let previous_state = entry.state;
            let dropped_image = entry.mark_failed(message);
            self.state_counts
                .transition(previous_state, TileLoadState::Failed);
            (previous_bytes, dropped_image)
        } else {
            let mut entry = TileEntry::queued(TilePriority::Prefetch, sequence);
            let dropped_image = entry.mark_failed(message);
            self.state_counts.increment(entry.state);
            self.entries.insert(coord, entry);
            (0, dropped_image)
        };
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
        let (previous_bytes, dropped_image) = if let Some(entry) = self.entries.get_mut(&coord) {
            let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
            let dropped_image = tile_entry_take_render_image(entry);
            self.state_counts
                .transition(entry.state, TileLoadState::Invalid);
            entry.state = TileLoadState::Invalid;
            entry.source_status = TileSourceStatus::Invalid;
            entry.priority = TilePriority::Prefetch;
            entry.attempts = 0;
            entry.retry_after = None;
            entry.last_error = Some(message);
            (previous_bytes, dropped_image)
        } else {
            let mut entry = TileEntry::queued(TilePriority::Prefetch, sequence);
            entry.state = TileLoadState::Invalid;
            entry.source_status = TileSourceStatus::Invalid;
            entry.priority = TilePriority::Prefetch;
            entry.attempts = 0;
            entry.retry_after = None;
            entry.last_error = Some(message);
            self.state_counts.increment(entry.state);
            self.entries.insert(coord, entry);
            (0, None)
        };
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(previous_bytes);
        dropped_image
    }

    pub(super) fn loaded_count(&self) -> usize {
        self.state_counts.loaded
    }

    pub(super) fn queued_count(&self) -> usize {
        self.state_counts
            .queued
            .saturating_add(self.state_counts.failed)
    }

    pub(super) fn pending_manifest_count(&self) -> usize {
        self.state_counts.pending_manifest
    }

    pub(super) fn loading_count(&self) -> usize {
        self.state_counts.loading
    }

    pub(super) fn cache_miss_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.source_status == TileSourceStatus::Miss)
            .count()
    }

    pub(super) fn failed_count(&self) -> usize {
        self.state_counts.failed
    }

    pub(super) fn invalid_count(&self) -> usize {
        self.state_counts.invalid
    }

    pub(super) fn loaded_estimated_bytes(&self) -> usize {
        self.loaded_estimated_bytes
    }

    fn allocate_access_stamp(&mut self) -> u64 {
        let access = self.next_access;
        self.next_access = self.next_access.saturating_add(1);
        access
    }
}

pub(super) fn tile_entry_loaded_estimated_bytes(entry: &TileEntry) -> usize {
    entry.image.as_ref().map_or(0, |tile| tile.estimated_bytes)
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

fn queued_candidate_changes_priority(
    selected_priority: &mut Option<TilePriority>,
    candidate_priority: TilePriority,
) -> bool {
    match *selected_priority {
        Some(selected) if candidate_priority >= selected => false,
        _ => {
            *selected_priority = Some(candidate_priority);
            true
        }
    }
}

fn queued_candidate_priority_matches(
    selected_priority: Option<TilePriority>,
    candidate_priority: TilePriority,
) -> bool {
    let Some(selected_priority) = selected_priority else {
        return false;
    };
    selected_priority > TilePriority::Visible || candidate_priority == selected_priority
}

type TrimLoadedTileSortKey = (u64, u8, bool, u64);

fn trim_loaded_tile_sort_key(
    last_access: u64,
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
        last_access,
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
