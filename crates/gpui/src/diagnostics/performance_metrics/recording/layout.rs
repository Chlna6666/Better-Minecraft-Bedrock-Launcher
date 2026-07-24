use std::sync::atomic::Ordering;

use super::super::LayoutFrameMetrics;
use super::super::state::shared_metrics;

/// Records layout counters for diagnostics.
pub fn record_layout_frame_metrics(metrics: LayoutFrameMetrics) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .layout_nodes
        .store(metrics.nodes as u64, Ordering::Relaxed);
    shared_metrics
        .measured_layout_nodes
        .store(metrics.measured_nodes as u64, Ordering::Relaxed);
    shared_metrics
        .layout_roots
        .store(metrics.roots as u64, Ordering::Relaxed);
    shared_metrics
        .layout_bounds_cache_hits
        .store(metrics.bounds_cache_hits as u64, Ordering::Relaxed);
    shared_metrics
        .layout_bounds_cache_misses
        .store(metrics.bounds_cache_misses as u64, Ordering::Relaxed);
    shared_metrics
        .layout_cache_reused_roots
        .store(metrics.cache_reused_roots as u64, Ordering::Relaxed);
    shared_metrics
        .layout_cache_saved_roots
        .store(metrics.cache_saved_roots as u64, Ordering::Relaxed);
}

/// Records retained layout cache counters for the latest frame.
pub fn record_layout_cache_metrics(hits: usize, misses: usize) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .layout_cache_hits
        .store(hits as u64, Ordering::Relaxed);
    shared_metrics
        .layout_cache_misses
        .store(misses as u64, Ordering::Relaxed);
}

/// Records text layout cache counters for the latest frame.
pub fn record_text_layout_cache_metrics(hits: usize, reuses: usize, misses: usize) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .text_layout_hits
        .store(hits as u64, Ordering::Relaxed);
    shared_metrics
        .text_layout_reuses
        .store(reuses as u64, Ordering::Relaxed);
    shared_metrics
        .text_layout_misses
        .store(misses as u64, Ordering::Relaxed);
}

/// Records style refinement applications on hot paths.
pub fn record_style_refine(count: usize) {
    shared_metrics()
        .style_refine_count
        .fetch_add(count as u64, Ordering::Relaxed);
}

/// Records style/layout-to-Taffy conversions on hot paths.
pub fn record_layout_conversion(count: usize) {
    shared_metrics()
        .layout_conversion_count
        .fetch_add(count as u64, Ordering::Relaxed);
}

/// Records how many times the GPUI arena had to grow by allocating a new chunk.
pub fn record_arena_chunk_expansion(count: usize) {
    shared_metrics()
        .arena_chunk_expansion_count
        .fetch_add(count as u64, Ordering::Relaxed);
}
