use crate::{
    GpuiMemoryTrimLevel, Pixels, PlatformTextSystem, SharedString, WrappedLineLayout, px,
    record_text_layout_cache_metrics,
};
use collections::FxHashMap;
use parking_lot::{Condvar, Mutex};
use smallvec::SmallVec;
use std::{
    cell::{Cell, RefCell},
    hash::Hash,
    mem,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use super::key::{AsCacheKeyRef, CacheKey, CacheKeyRef};
use super::{FontRun, LineLayout};

pub(crate) struct LineLayoutCache {
    previous_frame: RefCell<FrameCache>,
    current_frame: RefCell<FrameCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    font_generation: Arc<AtomicU64>,
    cached_font_generation: AtomicU64,
    frame_metrics: LineLayoutCacheMetrics,
}

const LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
static LINE_LAYOUT_SINGLEFLIGHT: LazyLock<LineLayoutSingleflight> =
    LazyLock::new(LineLayoutSingleflight::default);

#[derive(Clone, Hash, Eq, PartialEq)]
struct LineLayoutFlightKey {
    platform_identity: usize,
    font_generation: u64,
    cache_key: Arc<CacheKey>,
}

#[derive(Default)]
struct LineLayoutSingleflight {
    flights: Mutex<FxHashMap<LineLayoutFlightKey, Arc<LineLayoutFlight>>>,
}

struct LineLayoutFlight {
    state: Mutex<LineLayoutFlightState>,
    ready: Condvar,
    #[cfg(test)]
    followers: std::sync::atomic::AtomicUsize,
}

impl Default for LineLayoutFlight {
    fn default() -> Self {
        Self {
            state: Mutex::new(LineLayoutFlightState::Pending),
            ready: Condvar::new(),
            #[cfg(test)]
            followers: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

enum LineLayoutFlightState {
    Pending,
    Ready(Arc<LineLayout>),
    Cancelled,
}

impl LineLayoutSingleflight {
    fn shape<F>(
        &self,
        platform_identity: usize,
        font_generation: u64,
        cache_key: Arc<CacheKey>,
        shape: F,
    ) -> Arc<LineLayout>
    where
        F: FnOnce() -> LineLayout,
    {
        let flight_key = LineLayoutFlightKey {
            platform_identity,
            font_generation,
            cache_key,
        };
        let mut shape = Some(shape);

        loop {
            let (flight, is_leader) = {
                let mut flights = self.flights.lock();
                if let Some(flight) = flights.get(&flight_key) {
                    (flight.clone(), false)
                } else {
                    let flight = Arc::new(LineLayoutFlight::default());
                    flights.insert(flight_key.clone(), flight.clone());
                    (flight, true)
                }
            };

            if is_leader {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    (shape
                        .take()
                        .expect("line layout singleflight leader missing shaper"))(
                    )
                }));

                match result {
                    Ok(layout) => {
                        let layout = Arc::new(layout);
                        {
                            let mut state = flight.state.lock();
                            *state = LineLayoutFlightState::Ready(layout.clone());
                            flight.ready.notify_all();
                        }
                        self.remove_if_current(&flight_key, &flight);
                        return layout;
                    }
                    Err(payload) => {
                        {
                            let mut state = flight.state.lock();
                            *state = LineLayoutFlightState::Cancelled;
                            flight.ready.notify_all();
                        }
                        self.remove_if_current(&flight_key, &flight);
                        resume_unwind(payload);
                    }
                }
            }

            #[cfg(test)]
            flight
                .followers
                .fetch_add(1, std::sync::atomic::Ordering::Release);

            let mut state = flight.state.lock();
            loop {
                if let LineLayoutFlightState::Ready(layout) = &*state {
                    return layout.clone();
                }
                if matches!(&*state, LineLayoutFlightState::Cancelled) {
                    break;
                }
                let _work = crate::window::element_work::begin_named(
                    "text_singleflight_wait",
                    crate::window::element_work::ElementWorkPhase::Layout,
                );
                flight.ready.wait(&mut state);
            }
            drop(state);
            // The leader panicked. Its flight has been removed and this caller still owns its
            // unused shaping closure, so retry registration and allow one follower to take over.
        }
    }

    fn remove_if_current(&self, key: &LineLayoutFlightKey, flight: &Arc<LineLayoutFlight>) {
        let mut flights = self.flights.lock();
        if flights
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            flights.remove(key);
        }
    }

    #[cfg(test)]
    fn follower_count(
        &self,
        platform_identity: usize,
        font_generation: u64,
        cache_key: &Arc<CacheKey>,
    ) -> usize {
        let key = LineLayoutFlightKey {
            platform_identity,
            font_generation,
            cache_key: cache_key.clone(),
        };
        self.flights
            .lock()
            .get(&key)
            .map(|flight| flight.followers.load(std::sync::atomic::Ordering::Acquire))
            .unwrap_or_default()
    }
}

fn platform_text_system_identity(platform: &Arc<dyn PlatformTextSystem>) -> usize {
    Arc::as_ptr(platform) as *const () as usize
}

#[derive(Default)]
struct FrameCache {
    lines: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, Arc<WrappedLineLayout>>,
    used_lines: Vec<Arc<CacheKey>>,
    used_wrapped_lines: Vec<Arc<CacheKey>>,
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

#[derive(Clone, Default, PartialEq)]
pub(crate) struct LineLayoutIndex {
    font_generation: u64,
    lines_index: usize,
    wrapped_lines_index: usize,
}

impl LineLayoutIndex {
    /// Rebase this frame-local index from one retained subtree origin to another.
    ///
    /// The operation is checked because a malformed retained range must degrade the frame instead
    /// of wrapping an index and replaying unrelated text layouts.
    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        if self.font_generation != source.font_generation
            || source.font_generation != target.font_generation
        {
            return None;
        }

        Some(Self {
            font_generation: target.font_generation,
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
    pub fn new(
        platform_text_system: Arc<dyn PlatformTextSystem>,
        font_generation: Arc<AtomicU64>,
    ) -> Self {
        let cached_font_generation = font_generation.load(Ordering::Acquire);
        Self {
            previous_frame: RefCell::default(),
            current_frame: RefCell::default(),
            platform_text_system,
            font_generation,
            cached_font_generation: AtomicU64::new(cached_font_generation),
            frame_metrics: LineLayoutCacheMetrics::default(),
        }
    }

    fn clear_if_font_generation_changed(&self) -> u64 {
        let generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) == generation {
            return generation;
        }

        // Font discovery may advance the shared generation from another thread.
        // Clear both frame maps before publishing the generation they represent.
        let mut current_frame = self.current_frame.borrow_mut();
        let mut previous_frame = self.previous_frame.borrow_mut();
        let generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) != generation {
            current_frame.clear();
            previous_frame.clear();
            self.cached_font_generation
                .store(generation, Ordering::Release);
        }
        generation
    }

    pub fn layout_index(&self) -> LineLayoutIndex {
        let font_generation = self.clear_if_font_generation_changed();
        let frame = self.current_frame.borrow();
        LineLayoutIndex {
            font_generation,
            lines_index: frame.used_lines.len(),
            wrapped_lines_index: frame.used_wrapped_lines.len(),
        }
    }

    pub fn can_reuse_layouts(&self, range: Range<LineLayoutIndex>) -> bool {
        let font_generation = self.clear_if_font_generation_changed();
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return false;
        }

        let previous_frame = self.previous_frame.borrow();
        self.font_generation.load(Ordering::Acquire) == font_generation
            && line_layout_range_is_valid(
                range.start.lines_index,
                range.end.lines_index,
                previous_frame.used_lines.len(),
            )
            && line_layout_range_is_valid(
                range.start.wrapped_lines_index,
                range.end.wrapped_lines_index,
                previous_frame.used_wrapped_lines.len(),
            )
    }

    pub fn reuse_layouts(&self, range: Range<LineLayoutIndex>) {
        let font_generation = self.clear_if_font_generation_changed();
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return;
        }

        let current_frame = &mut *self.current_frame.borrow_mut();
        let previous_frame = &mut *self.previous_frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return;
        }

        for index in range.start.lines_index..range.end.lines_index {
            let used_key = previous_frame.used_lines[index].clone();
            if let Some((key, entry)) = previous_frame.lines.remove_entry(used_key.as_ref()) {
                current_frame.lines.insert(key, entry);
                self.frame_metrics.reuse();
            }
            current_frame.used_lines.push(used_key);
        }

        for index in range.start.wrapped_lines_index..range.end.wrapped_lines_index {
            let used_key = previous_frame.used_wrapped_lines[index].clone();
            if let Some((key, entry)) = previous_frame.wrapped_lines.remove_entry(used_key.as_ref()) {
                current_frame.wrapped_lines.insert(key, entry);
                self.frame_metrics.reuse();
            }
            current_frame.used_wrapped_lines.push(used_key);
        }
    }

    pub fn truncate_layouts(&self, index: LineLayoutIndex) {
        let font_generation = self.clear_if_font_generation_changed();
        if index.font_generation != font_generation {
            return;
        }

        let current_frame = &mut *self.current_frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return;
        }
        current_frame.used_lines.truncate(index.lines_index);
        current_frame
            .used_wrapped_lines
            .truncate(index.wrapped_lines_index);
    }

    pub fn clear(&self) {
        let mut current_frame = self.current_frame.borrow_mut();
        let mut previous_frame = self.previous_frame.borrow_mut();
        previous_frame.clear();
        current_frame.clear();
    }

    pub fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        let _font_generation = self.clear_if_font_generation_changed();
        let mut current_frame = self.current_frame.borrow_mut();
        let mut previous_frame = self.previous_frame.borrow_mut();
        previous_frame.trim_retained_capacity_for_level(level);
        current_frame.trim_retained_capacity_for_level(level);
    }

    pub fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let _font_generation = self.clear_if_font_generation_changed();
        let mut curr_frame = self.current_frame.borrow_mut();
        let mut prev_frame = self.previous_frame.borrow_mut();
        carry_over_line_layouts(&mut prev_frame, &mut curr_frame);
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
        let font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
            max_lines,
            force_width: None,
        } as &dyn AsCacheKeyRef;

        let mut current_frame = self.current_frame.borrow_mut();
        if let Some(entry) = current_frame.wrapped_lines.get(key) {
            self.frame_metrics.hit();
            let layout = entry.clone();
            drop(current_frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_wrapped_line(text, font_size, runs, wrap_width, max_lines);
        }

        let previous_frame_entry = self.previous_frame.borrow_mut().wrapped_lines.remove_entry(key);
        if let Some((key, entry)) = previous_frame_entry {
            let layout = entry.clone();
            current_frame.wrapped_lines.insert(key.clone(), entry);
            current_frame.used_wrapped_lines.push(key);
            self.frame_metrics.reuse();
            drop(current_frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_wrapped_line(text, font_size, runs, wrap_width, max_lines);
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
            max_lines,
            force_width: None,
        });

        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return self.layout_wrapped_line::<&SharedString>(
                &key.text, font_size, runs, wrap_width, max_lines,
            );
        }

        let mut current_frame = self.current_frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            drop(current_frame);
            return self.layout_wrapped_line::<&SharedString>(
                &key.text, font_size, runs, wrap_width, max_lines,
            );
        }
        current_frame.wrapped_lines.insert(key.clone(), layout.clone());
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
        let font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
            max_lines: None,
            force_width,
        } as &dyn AsCacheKeyRef;

        let mut current_frame = self.current_frame.borrow_mut();
        if let Some(entry) = current_frame.lines.get(key) {
            self.frame_metrics.hit();
            let layout = entry.clone();
            drop(current_frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_line(text, font_size, runs, force_width);
        }

        let previous_frame_entry = self.previous_frame.borrow_mut().lines.remove_entry(key);
        if let Some((key, entry)) = previous_frame_entry {
            let layout = entry.clone();
            current_frame.lines.insert(key.clone(), entry);
            current_frame.used_lines.push(key);
            self.frame_metrics.reuse();
            drop(current_frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_line(text, font_size, runs, force_width);
        }

        self.frame_metrics.miss();
        drop(current_frame);
        let text = SharedString::from(text);
        let key = Arc::new(CacheKey {
            text: text.clone(),
            font_size,
            runs: SmallVec::from(runs),
            wrap_width: None,
            max_lines: None,
            force_width,
        });
        let platform_identity = platform_text_system_identity(&self.platform_text_system);
        let layout =
            LINE_LAYOUT_SINGLEFLIGHT.shape(platform_identity, font_generation, key.clone(), || {
                let mut layout = self
                    .platform_text_system
                    .layout_line(&text, font_size, runs);

                if let Some(force_width) = force_width {
                    apply_force_width_to_layout(&mut layout, force_width);
                }
                layout
            });

        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return self.layout_line::<&SharedString>(&text, font_size, runs, force_width);
        }

        let mut current_frame = self.current_frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            drop(current_frame);
            return self.layout_line::<&SharedString>(&text, font_size, runs, force_width);
        }
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

// Ownership-driven carry-over from longbridge/gpui-fast, fast/text.rs,
// revision 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
// Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
fn carry_over<K: Eq + Hash, V>(
    previous: &mut FxHashMap<Arc<K>, Arc<V>>,
    current: &mut FxHashMap<Arc<K>, Arc<V>>,
) {
    previous.retain(|_, layout| Arc::strong_count(layout) > 1);
    if previous.len() < current.len() {
        mem::swap(previous, current);
    }
    previous.extend(current.drain());
}

fn carry_over_line_layouts(previous: &mut FrameCache, current: &mut FrameCache) {
    // Wrapped layouts own their underlying line; release dead wrappers first.
    carry_over(&mut previous.wrapped_lines, &mut current.wrapped_lines);
    carry_over(&mut previous.lines, &mut current.lines);

    // Replay indices describe only this frame's access log, not all live
    // layouts. Retaining a layout does not retain the old frame's log.
    mem::swap(&mut previous.used_lines, &mut current.used_lines);
    mem::swap(&mut previous.used_wrapped_lines, &mut current.used_wrapped_lines);
    current.used_lines.clear();
    current.used_wrapped_lines.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ShapedGlyph, ShapedRun};
    use crate::{
        FontId, GlyphId, GpuiMemoryTrimLevel, NoopTextSystem, Point, snapshot,
        point, px,
    };

    #[test]
    fn line_layout_singleflight_deduplicates_concurrent_shape() {
        let singleflight = Arc::new(LineLayoutSingleflight::default());
        let key = Arc::new(CacheKey {
            text: "shared".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 6,
                font_id: FontId(1),
            }]),
            wrap_width: None,
            max_lines: None,
            force_width: None,
        });
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);

        let leader_singleflight = singleflight.clone();
        let leader_key = key.clone();
        let leader = std::thread::spawn(move || {
            leader_singleflight.shape(7, 0, leader_key, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let mut layout = LineLayout::default();
                layout.width = px(42.);
                layout
            })
        });

        started_rx.recv().unwrap();
        let follower_singleflight = singleflight.clone();
        let follower_key = key.clone();
        let follower = std::thread::spawn(move || {
            follower_singleflight.shape(7, 0, follower_key, || {
                panic!("follower must reuse the in-flight shaped layout")
            })
        });

        while singleflight.follower_count(7, 0, &key) == 0 {
            std::thread::yield_now();
        }
        release_tx.send(()).unwrap();

        let first = leader.join().unwrap();
        let second = follower.join().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.width, px(42.));
        assert!(singleflight.flights.lock().is_empty());
    }

    #[test]
    fn line_layout_singleflight_cleans_up_after_panicking_leader() {
        let singleflight = LineLayoutSingleflight::default();
        let key = Arc::new(CacheKey {
            text: "panic".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 5,
                font_id: FontId(1),
            }]),
            wrap_width: None,
            max_lines: None,
            force_width: None,
        });

        let result = catch_unwind(AssertUnwindSafe(|| {
            singleflight.shape(9, 0, key.clone(), || panic!("shape failed"))
        }));
        assert!(result.is_err());
        assert!(singleflight.flights.lock().is_empty());

        let layout = singleflight.shape(9, 0, key, LineLayout::default);
        assert_eq!(layout.width, px(0.));
        assert!(singleflight.flights.lock().is_empty());
    }

    #[test]
    fn line_layout_index_rebases_relative_offsets() {
        let source = LineLayoutIndex {
            font_generation: 7,
            lines_index: 10,
            wrapped_lines_index: 4,
        };
        let target = LineLayoutIndex {
            font_generation: 7,
            lines_index: 30,
            wrapped_lines_index: 20,
        };
        let index = LineLayoutIndex {
            font_generation: 7,
            lines_index: 17,
            wrapped_lines_index: 9,
        };
        let rebased = index.rebased_from(&source, &target).unwrap();
        assert_eq!(rebased.lines_index, 37);
        assert_eq!(rebased.wrapped_lines_index, 25);

        let different_generation = LineLayoutIndex {
            font_generation: 8,
            lines_index: 17,
            wrapped_lines_index: 9,
        };
        assert!(
            different_generation
                .rebased_from(&source, &target)
                .is_none(),
            "retained layout indices from another font generation must never be rebased"
        );
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
    fn unused_layouts_are_released_but_live_measurements_survive_idle_frames() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let live = cache.layout_line("hello", px(14.), &runs, None);
        let dead = cache.layout_line("other", px(14.), &runs, None);
        let weak = Arc::downgrade(&dead);
        drop(dead);
        cache.finish_frame();
        assert!(weak.upgrade().is_some());
        cache.finish_frame();
        assert!(weak.upgrade().is_none());
        assert_eq!(cache.previous_frame.borrow().lines.len(), 1);
        assert!(cache.previous_frame.borrow().used_lines.is_empty());
        let reused = cache.layout_line("hello", px(14.), &runs, None);
        assert!(Arc::ptr_eq(&live, &reused));
    }

    #[test]
    fn dead_wrapped_layout_does_not_keep_its_unwrapped_line_alive() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let wrapped = cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
        let weak_wrapped = Arc::downgrade(&wrapped);
        let weak_line = Arc::downgrade(&wrapped.unwrapped_layout);
        drop(wrapped);
        cache.finish_frame();
        cache.finish_frame();
        assert!(weak_wrapped.upgrade().is_none());
        assert!(weak_line.upgrade().is_none());
        assert!(cache.previous_frame.borrow().wrapped_lines.is_empty());
        assert!(cache.previous_frame.borrow().lines.is_empty());
    }

    #[test]
    fn layout_line_records_same_frame_hits() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 1);
        assert_eq!(metrics.text_layout_reuses, 0);
        assert_eq!(metrics.text_layout_misses, 1);
    }

    #[test]
    fn layout_line_records_previous_frame_reuse() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 0);
        assert_eq!(metrics.text_layout_reuses, 1);
        assert_eq!(metrics.text_layout_misses, 0);
    }

    #[test]
    fn layout_line_reuses_live_entry_after_idle_frame() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
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
    fn font_generation_invalidates_live_layouts() {
        let generation = Arc::new(AtomicU64::new(0));
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), generation.clone());
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        let stale_index = cache.layout_index();

        generation.fetch_add(1, Ordering::Release);

        assert!(!cache.can_reuse_layouts(stale_index.clone()..stale_index));
        let second = cache.layout_line("hello", px(14.), &runs, None);
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn aggressive_trim_clears_layout_cache_entries() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
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

#[cfg(test)]
mod clamp_cache_tests {
    use super::*;
    use crate::{FontId, NoopTextSystem, px};

    #[test]
    fn wrapped_cache_distinguishes_line_clamp() {
        let text = "one two three four five six";
        let runs = [FontRun { len: text.len(), font_id: FontId(1) }];
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::default());
        let full = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), None);
        let clamped = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), Some(1));
        assert!(!Arc::ptr_eq(&full, &clamped));
        assert!(full.wrap_boundaries.len() > clamped.wrap_boundaries.len());
        cache.finish_frame();
        let warm = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), Some(1));
        assert!(Arc::ptr_eq(&clamped, &warm));
    }
}
