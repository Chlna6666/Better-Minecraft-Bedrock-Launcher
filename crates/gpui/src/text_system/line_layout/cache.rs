use crate::{
    GpuiMemoryTrimLevel, Pixels, PlatformTextSystem, SharedString, WrappedLineLayout, px,
    record_text_layout_cache_metrics,
};
use collections::FxHashMap;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::SmallVec;
use std::{cell::Cell, collections::VecDeque, hash::Hash, ops::Range, sync::Arc};

use super::key::{AsCacheKeyRef, CacheKey, CacheKeyRef};
use super::{FontRun, LineLayout};

pub(crate) struct LineLayoutCache {
    previous_frame: Mutex<FrameCache>,
    current_frame: RwLock<FrameCache>,
    retained: Mutex<RetainedLayoutCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    frame_metrics: LineLayoutCacheMetrics,
}

const LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
const LINE_LAYOUT_CACHE_MAX_RETAINED_LINES: usize = 2_048;
const LINE_LAYOUT_CACHE_MAX_RETAINED_WRAPPED_LINES: usize = 512;
const LINE_LAYOUT_CACHE_RECENCY_COMPACTION_MULTIPLIER: usize = 2;

#[derive(Default)]
struct FrameCache {
    lines: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, Arc<WrappedLineLayout>>,
    used_lines: Vec<Arc<CacheKey>>,
    used_wrapped_lines: Vec<Arc<CacheKey>>,
}

struct RetainedLayoutEntry<V> {
    value: V,
    stamp: u64,
}

#[derive(Default)]
struct RetainedLayoutCache {
    lines: FxHashMap<Arc<CacheKey>, RetainedLayoutEntry<Arc<LineLayout>>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, RetainedLayoutEntry<Arc<WrappedLineLayout>>>,
    line_recency: VecDeque<(Arc<CacheKey>, u64)>,
    wrapped_line_recency: VecDeque<(Arc<CacheKey>, u64)>,
    next_stamp: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LineLayoutFrameMetrics {
    pub(crate) hits: usize,
    pub(crate) reuses: usize,
    pub(crate) misses: usize,
}

#[derive(Default)]
struct LineLayoutCacheMetrics {
    hits: Cell<usize>,
    reuses: Cell<usize>,
    misses: Cell<usize>,
}

impl LineLayoutCacheMetrics {
    fn hit(&self) {
        self.hits.set(self.hits.get().saturating_add(1));
    }

    fn reuse(&self) {
        self.reuses.set(self.reuses.get().saturating_add(1));
    }

    fn miss(&self) {
        self.misses.set(self.misses.get().saturating_add(1));
    }

    fn frame_metrics(&self) -> LineLayoutFrameMetrics {
        LineLayoutFrameMetrics {
            hits: self.hits.get(),
            reuses: self.reuses.get(),
            misses: self.misses.get(),
        }
    }

    fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let metrics = self.frame_metrics();
        record_text_layout_cache_metrics(metrics.hits, metrics.reuses, metrics.misses);
        self.hits.set(0);
        self.reuses.set(0);
        self.misses.set(0);
        metrics
    }
}

#[derive(Clone, Default)]
pub(crate) struct LineLayoutIndex {
    lines_index: usize,
    wrapped_lines_index: usize,
}

impl LineLayoutIndex {
    /// Rebase this frame-local index from one retained subtree origin to another.
    ///
    /// The operation is checked because a malformed retained range must degrade the frame instead
    /// of wrapping an index and replaying unrelated text layouts.
    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        Some(Self {
            lines_index: target
                .lines_index
                .checked_add(self.lines_index.checked_sub(source.lines_index)?)?,
            wrapped_lines_index: target.wrapped_lines_index.checked_add(
                self.wrapped_lines_index
                    .checked_sub(source.wrapped_lines_index)?,
            )?,
        })
    }
}

impl LineLayoutCache {
    pub fn new(platform_text_system: Arc<dyn PlatformTextSystem>) -> Self {
        Self {
            previous_frame: Mutex::default(),
            current_frame: RwLock::default(),
            retained: Mutex::default(),
            platform_text_system,
            frame_metrics: LineLayoutCacheMetrics::default(),
        }
    }

    pub fn layout_index(&self) -> LineLayoutIndex {
        let frame = self.current_frame.read();
        LineLayoutIndex {
            lines_index: frame.used_lines.len(),
            wrapped_lines_index: frame.used_wrapped_lines.len(),
        }
    }

    pub fn can_reuse_layouts(&self, range: Range<LineLayoutIndex>) -> bool {
        let previous_frame = self.previous_frame.lock();
        line_layout_range_is_valid(
            range.start.lines_index,
            range.end.lines_index,
            previous_frame.used_lines.len(),
        ) && line_layout_range_is_valid(
            range.start.wrapped_lines_index,
            range.end.wrapped_lines_index,
            previous_frame.used_wrapped_lines.len(),
        )
    }

    pub fn reuse_layouts(&self, range: Range<LineLayoutIndex>) {
        let previous_frame = &mut *self.previous_frame.lock();
        let current_frame = &mut *self.current_frame.write();

        for key in &previous_frame.used_lines[range.start.lines_index..range.end.lines_index] {
            if let Some((key, line)) = previous_frame.lines.remove_entry(key) {
                current_frame.lines.insert(key, line);
                self.frame_metrics.reuse();
            }
            current_frame.used_lines.push(key.clone());
        }

        for key in &previous_frame.used_wrapped_lines
            [range.start.wrapped_lines_index..range.end.wrapped_lines_index]
        {
            if let Some((key, line)) = previous_frame.wrapped_lines.remove_entry(key) {
                current_frame.wrapped_lines.insert(key, line);
                self.frame_metrics.reuse();
            }
            current_frame.used_wrapped_lines.push(key.clone());
        }
    }

    pub fn truncate_layouts(&self, index: LineLayoutIndex) {
        let current_frame = &mut *self.current_frame.write();
        current_frame.used_lines.truncate(index.lines_index);
        current_frame
            .used_wrapped_lines
            .truncate(index.wrapped_lines_index);
    }

    pub fn clear(&self) {
        let mut previous_frame = self.previous_frame.lock();
        let mut current_frame = self.current_frame.write();
        let mut retained = self.retained.lock();
        previous_frame.clear();
        current_frame.clear();
        retained.clear();
    }

    pub fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        let mut previous_frame = self.previous_frame.lock();
        let mut current_frame = self.current_frame.write();
        let mut retained = self.retained.lock();
        previous_frame.trim_retained_capacity_for_level(level);
        current_frame.trim_retained_capacity_for_level(level);
        retained.trim_retained_capacity_for_level(level);
    }

    pub fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let mut prev_frame = self.previous_frame.lock();
        let mut curr_frame = self.current_frame.write();
        std::mem::swap(&mut *prev_frame, &mut *curr_frame);

        // `curr_frame` now contains the frame that has just aged out of `previous_frame`.
        // Keep its shaped layouts in a bounded secondary cache so transient invisibility,
        // progressive rendering, overlays, and page transitions do not immediately force text
        // shaping again. The hot current/previous-frame path remains unchanged.
        let mut retained = self.retained.lock();
        for (key, layout) in curr_frame.lines.drain() {
            retained.insert_line(key, layout);
        }
        for (key, layout) in curr_frame.wrapped_lines.drain() {
            retained.insert_wrapped_line(key, layout);
        }
        curr_frame.used_lines.clear();
        curr_frame.used_wrapped_lines.clear();
        self.frame_metrics.finish_frame()
    }

    pub fn layout_wrapped_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
    ) -> Arc<WrappedLineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
            force_width: None,
        } as &dyn AsCacheKeyRef;

        let current_frame = self.current_frame.upgradable_read();
        if let Some(layout) = current_frame.wrapped_lines.get(key) {
            self.frame_metrics.hit();
            return layout.clone();
        }

        let previous_frame_entry = self.previous_frame.lock().wrapped_lines.remove_entry(key);
        if let Some((key, layout)) = previous_frame_entry {
            let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
            current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            current_frame.used_wrapped_lines.push(key);
            self.frame_metrics.reuse();
            return layout;
        }

        let retained_entry = self.retained.lock().take_wrapped_line(key);
        if let Some((key, layout)) = retained_entry {
            let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
            current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            current_frame.used_wrapped_lines.push(key);
            self.frame_metrics.reuse();
            return layout;
        }

        self.frame_metrics.miss();
        drop(current_frame);
        let text = SharedString::from(text);
        let unwrapped_layout = self.layout_line::<&SharedString>(&text, font_size, runs, None);
        let wrap_boundaries = if let Some(wrap_width) = wrap_width {
            unwrapped_layout.compute_wrap_boundaries(text.as_ref(), wrap_width, max_lines)
        } else {
            SmallVec::new()
        };
        let layout = Arc::new(WrappedLineLayout {
            unwrapped_layout,
            wrap_boundaries,
            wrap_width,
        });
        let key = Arc::new(CacheKey {
            text,
            font_size,
            runs: SmallVec::from(runs),
            wrap_width,
            force_width: None,
        });

        let mut current_frame = self.current_frame.write();
        current_frame
            .wrapped_lines
            .insert(key.clone(), layout.clone());
        current_frame.used_wrapped_lines.push(key);

        layout
    }

    pub fn layout_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
            force_width,
        } as &dyn AsCacheKeyRef;

        let current_frame = self.current_frame.upgradable_read();
        if let Some(layout) = current_frame.lines.get(key) {
            self.frame_metrics.hit();
            return layout.clone();
        }

        if let Some((key, layout)) = self.previous_frame.lock().lines.remove_entry(key) {
            let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            self.frame_metrics.reuse();
            return layout;
        }

        if let Some((key, layout)) = self.retained.lock().take_line(key) {
            let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            self.frame_metrics.reuse();
            return layout;
        }

        self.frame_metrics.miss();
        drop(current_frame);
        let text = SharedString::from(text);
        let mut layout = self
            .platform_text_system
            .layout_line(&text, font_size, runs);

        if let Some(force_width) = force_width {
            let mut glyph_pos = 0;
            for run in layout.runs.iter_mut() {
                for glyph in run.glyphs.iter_mut() {
                    if (glyph.position.x - glyph_pos * force_width).abs() > px(1.) {
                        glyph.position.x = glyph_pos * force_width;
                    }
                    glyph_pos += 1;
                }
            }
        }

        let key = Arc::new(CacheKey {
            text,
            font_size,
            runs: SmallVec::from(runs),
            wrap_width: None,
            force_width,
        });
        let layout = Arc::new(layout);
        let mut current_frame = self.current_frame.write();
        current_frame.lines.insert(key.clone(), layout.clone());
        current_frame.used_lines.push(key);
        layout
    }
}

fn line_layout_range_is_valid(start: usize, end: usize, len: usize) -> bool {
    start <= end && end <= len
}

impl FrameCache {
    fn clear(&mut self) {
        self.lines.clear();
        self.wrapped_lines.clear();
        self.used_lines.clear();
        self.used_wrapped_lines.clear();
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => self.trim_retained_capacity(),
            GpuiMemoryTrimLevel::Moderate => {
                self.shrink_retained_capacity(LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY)
            }
            GpuiMemoryTrimLevel::Aggressive => {
                self.clear();
                self.shrink_retained_capacity(0);
            }
        }
    }

    fn trim_retained_capacity(&mut self) {
        trim_map_capacity(
            &mut self.lines,
            LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
            LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_map_capacity(
            &mut self.wrapped_lines,
            LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
            LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.used_lines,
            LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
            LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.used_wrapped_lines,
            LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
            LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
        );
    }

    fn shrink_retained_capacity(&mut self, floor: usize) {
        self.lines.shrink_to(floor);
        self.wrapped_lines.shrink_to(floor);
        self.used_lines.shrink_to(floor);
        self.used_wrapped_lines.shrink_to(floor);
    }
}

impl RetainedLayoutCache {
    fn next_stamp(&mut self) -> u64 {
        self.next_stamp = self.next_stamp.wrapping_add(1);
        self.next_stamp
    }

    fn insert_line(&mut self, key: Arc<CacheKey>, layout: Arc<LineLayout>) {
        let stamp = self.next_stamp();
        insert_retained_bounded(
            &mut self.lines,
            &mut self.line_recency,
            key,
            layout,
            stamp,
            LINE_LAYOUT_CACHE_MAX_RETAINED_LINES,
        );
    }

    fn insert_wrapped_line(&mut self, key: Arc<CacheKey>, layout: Arc<WrappedLineLayout>) {
        let stamp = self.next_stamp();
        insert_retained_bounded(
            &mut self.wrapped_lines,
            &mut self.wrapped_line_recency,
            key,
            layout,
            stamp,
            LINE_LAYOUT_CACHE_MAX_RETAINED_WRAPPED_LINES,
        );
    }

    fn take_line(
        &mut self,
        key: &dyn AsCacheKeyRef,
    ) -> Option<(Arc<CacheKey>, Arc<LineLayout>)> {
        self.lines
            .remove_entry(key)
            .map(|(key, entry)| (key, entry.value))
    }

    fn take_wrapped_line(
        &mut self,
        key: &dyn AsCacheKeyRef,
    ) -> Option<(Arc<CacheKey>, Arc<WrappedLineLayout>)> {
        self.wrapped_lines
            .remove_entry(key)
            .map(|(key, entry)| (key, entry.value))
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.wrapped_lines.clear();
        self.line_recency.clear();
        self.wrapped_line_recency.clear();
        self.next_stamp = 0;
    }

    fn compact_recency(&mut self) {
        compact_retained_recency(&self.lines, &mut self.line_recency);
        compact_retained_recency(&self.wrapped_lines, &mut self.wrapped_line_recency);
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => {
                self.compact_recency();
                trim_map_capacity(
                    &mut self.lines,
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_map_capacity(
                    &mut self.wrapped_lines,
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_deque_capacity(
                    &mut self.line_recency,
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_deque_capacity(
                    &mut self.wrapped_line_recency,
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
            }
            GpuiMemoryTrimLevel::Moderate => {
                self.compact_recency();
                self.lines
                    .shrink_to(LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.lines.len()));
                self.wrapped_lines.shrink_to(
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.wrapped_lines.len()),
                );
                self.line_recency.shrink_to(
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.line_recency.len()),
                );
                self.wrapped_line_recency.shrink_to(
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.wrapped_line_recency.len()),
                );
            }
            GpuiMemoryTrimLevel::Aggressive => {
                self.clear();
                self.lines.shrink_to(0);
                self.wrapped_lines.shrink_to(0);
                self.line_recency.shrink_to(0);
                self.wrapped_line_recency.shrink_to(0);
            }
        }
    }
}

fn insert_retained_bounded<K, V>(
    map: &mut FxHashMap<K, RetainedLayoutEntry<V>>,
    recency: &mut VecDeque<(K, u64)>,
    key: K,
    value: V,
    stamp: u64,
    max_entries: usize,
) where
    K: Clone + Eq + Hash,
{
    if max_entries == 0 {
        return;
    }

    if map.len() >= max_entries && !map.contains_key(&key) {
        evict_oldest_retained(map, recency);
    }

    map.insert(key.clone(), RetainedLayoutEntry { value, stamp });
    recency.push_back((key, stamp));

    let compact_at = max_entries
        .saturating_mul(LINE_LAYOUT_CACHE_RECENCY_COMPACTION_MULTIPLIER)
        .max(max_entries.saturating_add(1));
    if recency.len() > compact_at {
        compact_retained_recency(map, recency);
    }
}

fn evict_oldest_retained<K, V>(
    map: &mut FxHashMap<K, RetainedLayoutEntry<V>>,
    recency: &mut VecDeque<(K, u64)>,
) where
    K: Clone + Eq + Hash,
{
    while let Some((candidate, stamp)) = recency.pop_front() {
        if map
            .get(&candidate)
            .is_some_and(|entry| entry.stamp == stamp)
        {
            map.remove(&candidate);
            return;
        }
    }

    // Queue/map divergence should not normally happen, but keep the hard bound sound even after a
    // future representation change instead of allowing an accidental unbounded cache.
    if let Some(candidate) = map.keys().next().cloned() {
        map.remove(&candidate);
    }
}

fn compact_retained_recency<K, V>(
    map: &FxHashMap<K, RetainedLayoutEntry<V>>,
    recency: &mut VecDeque<(K, u64)>,
) where
    K: Eq + Hash,
{
    recency.retain(|(key, stamp)| {
        map.get(key)
            .is_some_and(|entry| entry.stamp == *stamp)
    });
}

fn trim_map_capacity<K, V>(map: &mut FxHashMap<K, V>, floor: usize, multiplier: usize)
where
    K: Eq + Hash,
{
    let target = floor.max(map.len());
    if map.capacity() > target.saturating_mul(multiplier) {
        map.shrink_to(target);
    }
}

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(vec.len());
    if vec.capacity() > target.saturating_mul(multiplier) {
        vec.shrink_to(target);
    }
}

fn trim_deque_capacity<T>(deque: &mut VecDeque<T>, floor: usize, multiplier: usize) {
    let target = floor.max(deque.len());
    if deque.capacity() > target.saturating_mul(multiplier) {
        deque.shrink_to(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GpuiMemoryTrimLevel, NoopTextSystem, performance_metrics_snapshot, px};

    #[test]
    fn line_layout_index_rebases_relative_offsets() {
        let source = LineLayoutIndex {
            lines_index: 10,
            wrapped_lines_index: 4,
        };
        let target = LineLayoutIndex {
            lines_index: 30,
            wrapped_lines_index: 20,
        };
        let index = LineLayoutIndex {
            lines_index: 17,
            wrapped_lines_index: 9,
        };
        let rebased = index.rebased_from(&source, &target).unwrap();
        assert_eq!(rebased.lines_index, 37);
        assert_eq!(rebased.wrapped_lines_index, 25);
    }

    #[test]
    fn layout_line_records_same_frame_hits() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = performance_metrics_snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 1);
        assert_eq!(metrics.text_layout_reuses, 0);
        assert_eq!(metrics.text_layout_misses, 1);
    }

    #[test]
    fn layout_line_records_previous_frame_reuse() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = performance_metrics_snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 0);
        assert_eq!(metrics.text_layout_reuses, 1);
        assert_eq!(metrics.text_layout_misses, 0);
    }

    #[test]
    fn layout_line_reuses_retained_entry_after_idle_frame() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        let second = cache.layout_line("hello", px(14.), &runs, None);

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn retained_layout_cache_is_bounded() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 1,
            font_id: FontId(1),
        }];

        for index in 0..LINE_LAYOUT_CACHE_MAX_RETAINED_LINES + 64 {
            cache.layout_line(format!("{index}"), px(14.), &runs, None);
        }
        cache.finish_frame();
        cache.finish_frame();

        assert!(cache.retained.lock().lines.len() <= LINE_LAYOUT_CACHE_MAX_RETAINED_LINES);
    }

    #[test]
    fn retained_eviction_uses_current_generation_order() {
        let mut map = FxHashMap::default();
        let mut recency = VecDeque::new();

        insert_retained_bounded(&mut map, &mut recency, 1_u32, 10_u32, 1, 2);
        insert_retained_bounded(&mut map, &mut recency, 2_u32, 20_u32, 2, 2);

        // Simulate a retained hit: the map entry leaves the retained tier, while its queue record is
        // intentionally left stale. Reinserting it gives it a newer generation.
        map.remove(&1);
        insert_retained_bounded(&mut map, &mut recency, 1_u32, 11_u32, 3, 2);
        insert_retained_bounded(&mut map, &mut recency, 3_u32, 30_u32, 4, 2);

        assert!(map.contains_key(&1));
        assert!(!map.contains_key(&2));
        assert!(map.contains_key(&3));
    }

    #[test]
    fn aggressive_trim_clears_layout_cache_entries() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        cache.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Aggressive);
        let second = cache.layout_line("hello", px(14.), &runs, None);

        assert!(!Arc::ptr_eq(&first, &second));
    }
}
