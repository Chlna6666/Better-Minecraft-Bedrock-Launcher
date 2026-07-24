use std::sync::atomic::Ordering;

use super::super::SceneFrameMetrics;
use super::super::state::shared_metrics;

/// Records a retained scene presentation that did not rebuild the scene.
pub fn record_retained_scene_present() {
    shared_metrics()
        .retained_present_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records platform atlas key removals.
pub fn record_atlas_remove(count: usize) {
    shared_metrics()
        .atlas_remove_count
        .fetch_add(count as u64, Ordering::Relaxed);
}

/// Records scene counters for diagnostics.
pub fn record_scene_frame_metrics(metrics: SceneFrameMetrics) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .scene_primitives
        .store(metrics.primitives as u64, Ordering::Relaxed);
    shared_metrics
        .scene_batches
        .store(metrics.batches as u64, Ordering::Relaxed);
    shared_metrics
        .scene_segments
        .store(metrics.segments as u64, Ordering::Relaxed);
    shared_metrics
        .scene_replayed_primitives
        .store(metrics.replayed_primitives as u64, Ordering::Relaxed);
    shared_metrics
        .scene_segment_rebuild_count
        .store(metrics.segment_rebuild_count as u64, Ordering::Relaxed);
    shared_metrics
        .scene_segment_reuse_count
        .store(metrics.segment_reuse_count as u64, Ordering::Relaxed);
    shared_metrics
        .scene_retained_capacity
        .store(metrics.retained_capacity as u64, Ordering::Relaxed);
}

/// Records frame-retained capacity for diagnostics.
pub fn record_frame_retained_capacity(capacity: usize) {
    shared_metrics()
        .frame_retained_capacity
        .store(capacity as u64, Ordering::Relaxed);
}

/// Records dirty transform metrics for the latest frame.
pub fn record_retained_segment_metrics(dirty_transform_count: usize) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .dirty_transform_count
        .store(dirty_transform_count as u64, Ordering::Relaxed);
}
