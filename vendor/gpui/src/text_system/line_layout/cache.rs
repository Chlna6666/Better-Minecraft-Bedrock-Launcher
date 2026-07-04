use crate::{
    GpuiMemoryTrimLevel, Pixels, PlatformTextSystem, SharedString, WrappedLineLayout, px,
    record_text_layout_cache_metrics,
};
use collections::FxHashMap;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::SmallVec;
use std::{cell::Cell, hash::Hash, ops::Range, sync::Arc};

use super::key::{AsCacheKeyRef, CacheKey, CacheKeyRef};
use super::{FontRun, LineLayout};

pub(crate) struct LineLayoutCache {
    previous_frame: Mutex<FrameCache>,
    current_frame: RwLock<FrameCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    frame_metrics: LineLayoutCacheMetrics,
}

const LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;

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

#[derive(Clone, Default)]
pub(crate) struct LineLayoutIndex {
    lines_index: usize,
    wrapped_lines_index: usize,
}

impl LineLayoutCache {
    pub fn new(platform_text_system: Arc<dyn PlatformTextSystem>) -> Self {
        Self {
            previous_frame: Mutex::default(),
            current_frame: RwLock::default(),
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
        previous_frame.clear();
        current_frame.clear();
    }

    pub fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        let mut previous_frame = self.previous_frame.lock();
        let mut current_frame = self.current_frame.write();
        previous_frame.trim_retained_capacity_for_level(level);
        current_frame.trim_retained_capacity_for_level(level);
    }

    pub fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let mut prev_frame = self.previous_frame.lock();
        let mut curr_frame = self.current_frame.write();
        std::mem::swap(&mut *prev_frame, &mut *curr_frame);
        curr_frame.lines.clear();
        curr_frame.wrapped_lines.clear();
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
            layout
        } else {
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
            layout
        } else {
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
    use crate::{FontId, GpuiMemoryTrimLevel, NoopTextSystem, performance_metrics_snapshot, px};

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
    fn aggressive_trim_clears_layout_cache_entries() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Aggressive);
        let second = cache.layout_line("hello", px(14.), &runs, None);

        assert!(!Arc::ptr_eq(&first, &second));
    }
}
