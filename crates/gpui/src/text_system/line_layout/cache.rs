use crate::{
    GpuiMemoryTrimLevel, Pixels, PlatformTextSystem, SharedString, WrappedLineLayout, px,
    record_text_layout_cache_metrics,
};
use collections::FxHashMap;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::SmallVec;
use std::{
    cell::Cell,
    collections::VecDeque,
    hash::Hash,
    mem::size_of,
    ops::Range,
    sync::Arc,
};

use super::key::{AsCacheKeyRef, CacheKey, CacheKeyRef};
use super::{FontRun, LineLayout, ShapedGlyph, ShapedRun, WrapBoundary};

pub(crate) struct LineLayoutCache {
    previous_frame: Mutex<FrameCache>,
    current_frame: RwLock<FrameCache>,
    retained: Mutex<RetainedLayoutCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    frame_metrics: LineLayoutCacheMetrics,
}

const LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
// Retained layouts are sized from the observed working set rather than a fixed line count or a
// fixed number of MiB. This keeps large editor/terminal workloads hot while allowing simple pages
// to converge to a small cache instead of reserving an arbitrary global budget.
const LINE_LAYOUT_CACHE_WORKING_SET_MULTIPLIER: usize = 3;
const LINE_LAYOUT_CACHE_WORKING_SET_DECAY_NUMERATOR: usize = 7;
const LINE_LAYOUT_CACHE_WORKING_SET_DECAY_DENOMINATOR: usize = 8;
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
    estimated_bytes: usize,
}

#[derive(Default)]
struct RetainedLayoutCache {
    lines: FxHashMap<Arc<CacheKey>, RetainedLayoutEntry<Arc<LineLayout>>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, RetainedLayoutEntry<Arc<WrappedLineLayout>>>,
    line_recency: VecDeque<(Arc<CacheKey>, u64)>,
    wrapped_line_recency: VecDeque<(Arc<CacheKey>, u64)>,
    next_stamp: u64,
    estimated_bytes: usize,
    working_set_bytes: usize,
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

        // After the swap, `prev_frame` is the frame that just finished rendering and therefore the
        // best direct observation of the active shaping working set. `curr_frame` is the older frame
        // whose entries were not reused and are now candidates for the retained LRU tier.
        let active_frame_bytes = prev_frame.estimated_bytes();
        let mut retained = self.retained.lock();
        retained.observe_working_set(active_frame_bytes);

        for (key, layout) in curr_frame.lines.drain() {
            retained.insert_line(key, layout);
        }
        for (key, layout) in curr_frame.wrapped_lines.drain() {
            retained.insert_wrapped_line(key, layout);
        }
        curr_frame.used_lines.clear();
        curr_frame.used_wrapped_lines.clear();
        retained.evict_to_working_set_budget();
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
            apply_force_width_to_layout(&mut layout, force_width);
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

// Combining marks such as Thai vowel signs and Arabic diacritics are commonly shaped at the same
// x position as their base glyph. Fixed-width positioning must not advance the cell counter for
// those zero/near-zero-advance glyphs, or the mark is displaced into the next cell. Keep the
// original within-cell offset while only advancing when shaping has moved far enough to indicate a
// new base glyph.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos = 0usize;
    let mut last_base_shaped_x = px(f32::NEG_INFINITY);
    let mut last_base_actual_x = px(0.);

    for run in layout.runs.iter_mut() {
        for glyph in run.glyphs.iter_mut() {
            let shaped_x = glyph.position.x;

            if shaped_x > last_base_shaped_x + force_width * 0.5 {
                let forced_x = glyph_pos * force_width;
                if (shaped_x - forced_x).abs() > px(1.) {
                    glyph.position.x = forced_x;
                }
                last_base_shaped_x = shaped_x;
                last_base_actual_x = glyph.position.x;
                glyph_pos += 1;
            } else {
                glyph.position.x = last_base_actual_x + (shaped_x - last_base_shaped_x);
            }
        }
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

    fn estimated_bytes(&self) -> usize {
        let line_bytes = self.lines.iter().fold(0usize, |total, (key, layout)| {
            total.saturating_add(estimate_line_entry_bytes(key, layout))
        });
        self.wrapped_lines
            .iter()
            .fold(line_bytes, |total, (key, layout)| {
                total.saturating_add(estimate_wrapped_line_entry_bytes(key, layout))
            })
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

    fn observe_working_set(&mut self, frame_bytes: usize) {
        let decayed = self
            .working_set_bytes
            .saturating_mul(LINE_LAYOUT_CACHE_WORKING_SET_DECAY_NUMERATOR)
            / LINE_LAYOUT_CACHE_WORKING_SET_DECAY_DENOMINATOR;
        self.working_set_bytes = frame_bytes.max(decayed);
    }

    fn working_set_budget(&self) -> usize {
        self.working_set_bytes
            .saturating_mul(LINE_LAYOUT_CACHE_WORKING_SET_MULTIPLIER)
    }

    fn insert_line(&mut self, key: Arc<CacheKey>, layout: Arc<LineLayout>) {
        let stamp = self.next_stamp();
        let estimated_bytes = estimate_line_entry_bytes(&key, &layout);
        if let Some(previous) = self.lines.insert(
            key.clone(),
            RetainedLayoutEntry {
                value: layout,
                stamp,
                estimated_bytes,
            },
        ) {
            self.estimated_bytes = self
                .estimated_bytes
                .saturating_sub(previous.estimated_bytes);
        }
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.line_recency.push_back((key, stamp));
        self.compact_recency_if_needed();
    }

    fn insert_wrapped_line(&mut self, key: Arc<CacheKey>, layout: Arc<WrappedLineLayout>) {
        let stamp = self.next_stamp();
        let estimated_bytes = estimate_wrapped_line_entry_bytes(&key, &layout);
        if let Some(previous) = self.wrapped_lines.insert(
            key.clone(),
            RetainedLayoutEntry {
                value: layout,
                stamp,
                estimated_bytes,
            },
        ) {
            self.estimated_bytes = self
                .estimated_bytes
                .saturating_sub(previous.estimated_bytes);
        }
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.wrapped_line_recency.push_back((key, stamp));
        self.compact_recency_if_needed();
    }

    fn take_line(
        &mut self,
        key: &dyn AsCacheKeyRef,
    ) -> Option<(Arc<CacheKey>, Arc<LineLayout>)> {
        self.lines.remove_entry(key).map(|(key, entry)| {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            (key, entry.value)
        })
    }

    fn take_wrapped_line(
        &mut self,
        key: &dyn AsCacheKeyRef,
    ) -> Option<(Arc<CacheKey>, Arc<WrappedLineLayout>)> {
        self.wrapped_lines.remove_entry(key).map(|(key, entry)| {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            (key, entry.value)
        })
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.wrapped_lines.clear();
        self.line_recency.clear();
        self.wrapped_line_recency.clear();
        self.next_stamp = 0;
        self.estimated_bytes = 0;
        self.working_set_bytes = 0;
    }

    fn compact_recency_if_needed(&mut self) {
        let live_entries = self.lines.len().saturating_add(self.wrapped_lines.len());
        let queue_entries = self
            .line_recency
            .len()
            .saturating_add(self.wrapped_line_recency.len());
        let compact_at = live_entries
            .saturating_mul(LINE_LAYOUT_CACHE_RECENCY_COMPACTION_MULTIPLIER)
            .max(live_entries.saturating_add(LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY));
        if queue_entries > compact_at {
            self.compact_recency();
        }
    }

    fn compact_recency(&mut self) {
        compact_retained_recency(&self.lines, &mut self.line_recency);
        compact_retained_recency(&self.wrapped_lines, &mut self.wrapped_line_recency);
    }

    fn evict_to_working_set_budget(&mut self) {
        self.evict_to_bytes(self.working_set_budget());
    }

    fn evict_to_bytes(&mut self, target_bytes: usize) {
        while self.estimated_bytes > target_bytes {
            let line_stamp = oldest_valid_stamp(&self.lines, &mut self.line_recency);
            let wrapped_stamp =
                oldest_valid_stamp(&self.wrapped_lines, &mut self.wrapped_line_recency);

            match (line_stamp, wrapped_stamp) {
                (Some(line_stamp), Some(wrapped_stamp)) if line_stamp <= wrapped_stamp => {
                    if !evict_oldest_retained(
                        &mut self.lines,
                        &mut self.line_recency,
                        &mut self.estimated_bytes,
                    ) {
                        break;
                    }
                }
                (Some(_), Some(_)) | (None, Some(_)) => {
                    if !evict_oldest_retained(
                        &mut self.wrapped_lines,
                        &mut self.wrapped_line_recency,
                        &mut self.estimated_bytes,
                    ) {
                        break;
                    }
                }
                (Some(_), None) => {
                    if !evict_oldest_retained(
                        &mut self.lines,
                        &mut self.line_recency,
                        &mut self.estimated_bytes,
                    ) {
                        break;
                    }
                }
                (None, None) => break,
            }
        }
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
                let divisor = match level {
                    GpuiMemoryTrimLevel::Light => 2,
                    GpuiMemoryTrimLevel::Moderate => 4,
                    GpuiMemoryTrimLevel::Aggressive => unreachable!(),
                };
                self.working_set_bytes /= divisor;
                self.evict_to_working_set_budget();
                self.compact_recency();

                let line_capacity = LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.lines.len());
                let wrapped_capacity =
                    LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY.max(self.wrapped_lines.len());
                trim_map_capacity(
                    &mut self.lines,
                    line_capacity,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_map_capacity(
                    &mut self.wrapped_lines,
                    wrapped_capacity,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_deque_capacity(
                    &mut self.line_recency,
                    line_capacity,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
                trim_deque_capacity(
                    &mut self.wrapped_line_recency,
                    wrapped_capacity,
                    LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER,
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

fn estimate_cache_key_bytes(key: &CacheKey) -> usize {
    size_of::<CacheKey>()
        .saturating_add(key.text.len())
        .saturating_add(key.runs.len().saturating_mul(size_of::<FontRun>()))
}

fn estimate_line_layout_bytes(layout: &LineLayout) -> usize {
    layout.runs.iter().fold(
        size_of::<LineLayout>()
            .saturating_add(layout.runs.capacity().saturating_mul(size_of::<ShapedRun>())),
        |total, run| {
            total.saturating_add(
                run.glyphs
                    .capacity()
                    .saturating_mul(size_of::<ShapedGlyph>()),
            )
        },
    )
}

fn estimate_wrapped_line_layout_bytes(layout: &WrappedLineLayout) -> usize {
    size_of::<WrappedLineLayout>()
        .saturating_add(
            layout
                .wrap_boundaries
                .capacity()
                .saturating_mul(size_of::<WrapBoundary>()),
        )
        // Count the shared unwrapped layout conservatively. It is often also present in the line
        // cache, but double-counting shared storage is preferable to retaining large wrapped text
        // indefinitely after its standalone line entry has already been evicted.
        .saturating_add(estimate_line_layout_bytes(&layout.unwrapped_layout))
}

fn estimate_line_entry_bytes(key: &CacheKey, layout: &LineLayout) -> usize {
    estimate_cache_key_bytes(key)
        .saturating_add(estimate_line_layout_bytes(layout))
        .saturating_add(size_of::<RetainedLayoutEntry<Arc<LineLayout>>>())
        .saturating_add(size_of::<usize>() * 2)
}

fn estimate_wrapped_line_entry_bytes(key: &CacheKey, layout: &WrappedLineLayout) -> usize {
    estimate_cache_key_bytes(key)
        .saturating_add(estimate_wrapped_line_layout_bytes(layout))
        .saturating_add(size_of::<RetainedLayoutEntry<Arc<WrappedLineLayout>>>())
        .saturating_add(size_of::<usize>() * 2)
}

fn oldest_valid_stamp<K, V>(
    map: &FxHashMap<K, RetainedLayoutEntry<V>>,
    recency: &mut VecDeque<(K, u64)>,
) -> Option<u64>
where
    K: Clone + Eq + Hash,
{
    loop {
        let (key, stamp) = recency.front()?;
        if map.get(key).is_some_and(|entry| entry.stamp == *stamp) {
            return Some(*stamp);
        }
        recency.pop_front();
    }
}

fn evict_oldest_retained<K, V>(
    map: &mut FxHashMap<K, RetainedLayoutEntry<V>>,
    recency: &mut VecDeque<(K, u64)>,
    estimated_bytes: &mut usize,
) -> bool
where
    K: Clone + Eq + Hash,
{
    while let Some((candidate, stamp)) = recency.pop_front() {
        if !map
            .get(&candidate)
            .is_some_and(|entry| entry.stamp == stamp)
        {
            continue;
        }
        if let Some(entry) = map.remove(&candidate) {
            *estimated_bytes = estimated_bytes.saturating_sub(entry.estimated_bytes);
            return true;
        }
    }
    false
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
    use crate::{
        FontId, GlyphId, GpuiMemoryTrimLevel, NoopTextSystem, Point, performance_metrics_snapshot,
        point, px,
    };

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
    fn force_width_preserves_combining_mark_offset_across_runs() {
        fn glyph(id: u32, x: f32, index: usize) -> ShapedGlyph {
            ShapedGlyph {
                id: GlyphId(id),
                position: point(px(x), px(0.)),
                render_offset: Point::default(),
                font_size: px(14.),
                index,
                is_emoji: false,
                is_cjk: false,
            }
        }

        let mut layout = LineLayout {
            font_size: px(14.),
            width: px(18.),
            ascent: px(11.),
            descent: px(3.),
            runs: vec![
                ShapedRun {
                    font_id: FontId(1),
                    glyphs: vec![glyph(1, 0., 0)],
                },
                // Simulate a combining mark shaped by a fallback face. Its one-pixel relative
                // offset belongs to the first base glyph and must not consume another cell.
                ShapedRun {
                    font_id: FontId(2),
                    glyphs: vec![glyph(2, 1., 1)],
                },
                ShapedRun {
                    font_id: FontId(1),
                    glyphs: vec![glyph(3, 8., 2)],
                },
            ],
            len: 3,
        };

        apply_force_width_to_layout(&mut layout, px(10.));

        assert_eq!(layout.runs[0].glyphs[0].position.x, px(0.));
        assert_eq!(layout.runs[1].glyphs[0].position.x, px(1.));
        assert_eq!(layout.runs[2].glyphs[0].position.x, px(10.));
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
    fn retained_budget_tracks_observed_working_set_bytes() {
        let mut retained = RetainedLayoutCache::default();
        retained.observe_working_set(1_000);
        assert_eq!(retained.working_set_budget(), 3_000);

        retained.observe_working_set(100);
        assert_eq!(retained.working_set_bytes, 875);
        assert_eq!(retained.working_set_budget(), 2_625);

        retained.observe_working_set(2_000);
        assert_eq!(retained.working_set_bytes, 2_000);
        assert_eq!(retained.working_set_budget(), 6_000);
    }

    #[test]
    fn retained_eviction_is_global_lru_across_layout_kinds() {
        let mut retained = RetainedLayoutCache::default();
        retained.working_set_bytes = 1;

        let first_key = Arc::new(CacheKey {
            text: "first".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 5,
                font_id: FontId(1),
            }]),
            wrap_width: None,
            force_width: None,
        });
        let second_key = Arc::new(CacheKey {
            text: "second".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 6,
                font_id: FontId(1),
            }]),
            wrap_width: Some(px(80.)),
            force_width: None,
        });
        let line = Arc::new(LineLayout::default());
        let wrapped = Arc::new(WrappedLineLayout {
            unwrapped_layout: line.clone(),
            wrap_boundaries: SmallVec::new(),
            wrap_width: Some(px(80.)),
        });

        retained.insert_line(first_key.clone(), line);
        retained.insert_wrapped_line(second_key.clone(), wrapped);
        retained.evict_to_bytes(0);

        assert!(retained.lines.is_empty());
        assert!(retained.wrapped_lines.is_empty());
        assert_eq!(retained.estimated_bytes, 0);
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
