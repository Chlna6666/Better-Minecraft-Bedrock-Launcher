use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::store::shared_metrics;
use super::timing::duration_micros;

/// Metrics for one rendered image.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRenderRecord {
    /// Stable source label.
    pub source: String,
    /// Original width.
    pub original_width: u32,
    /// Original height.
    pub original_height: u32,
    /// Requested width.
    pub target_width: u32,
    /// Requested height.
    pub target_height: u32,
    /// Resident pixel bytes.
    pub resident_bytes: usize,
    /// Rendering path.
    pub render_path: String,
}

/// Records aggregate image cache occupancy.
pub fn record_image_cache_metrics(cache_id: u64, items: usize, bytes: usize) {
    if let Ok(mut image_caches) = shared_metrics().image_caches.lock() {
        image_caches.insert(cache_id, (items, bytes));
    }
}

/// Removes one image cache from aggregate metrics.
pub fn drop_image_cache_metrics(cache_id: u64) {
    if let Ok(mut image_caches) = shared_metrics().image_caches.lock() {
        image_caches.remove(&cache_id);
    }
}

/// Records the latest completed image processing.
pub fn record_image_processing_metrics(
    compressed_bytes: usize,
    decoded_bytes: usize,
    frames: usize,
    duration: Duration,
) {
    record_image_processing_metrics_with_threshold(
        compressed_bytes,
        decoded_bytes,
        frames,
        duration,
        Duration::MAX,
    );
}

/// Records a completed image processing and marks it slow when it exceeds `slow_threshold`.
pub fn record_image_processing_metrics_with_threshold(
    compressed_bytes: usize,
    decoded_bytes: usize,
    frames: usize,
    duration: Duration,
    slow_threshold: Duration,
) {
    let metrics = shared_metrics();
    let duration_micros = duration_micros(duration);
    metrics
        .image_processing_compressed_bytes
        .store(compressed_bytes as u64, Ordering::Relaxed);
    metrics
        .image_processing_output_bytes
        .store(decoded_bytes as u64, Ordering::Relaxed);
    metrics
        .image_processing_frames
        .store(frames as u64, Ordering::Relaxed);
    metrics
        .image_processing_micros
        .store(duration_micros, Ordering::Relaxed);
    metrics
        .image_processing_count
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .image_processing_total_compressed_bytes
        .fetch_add(compressed_bytes as u64, Ordering::Relaxed);
    metrics
        .image_processing_total_decoded_bytes
        .fetch_add(decoded_bytes as u64, Ordering::Relaxed);
    metrics
        .image_processing_total_frames
        .fetch_add(frames as u64, Ordering::Relaxed);
    metrics
        .image_processing_total_micros
        .fetch_add(duration_micros, Ordering::Relaxed);
    metrics
        .image_processing_max_micros
        .fetch_max(duration_micros, Ordering::Relaxed);
    if duration >= slow_threshold {
        metrics
            .image_processing_slow_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Records a currently retained size-aware image asset.
pub fn record_image_asset_retained(asset_key: u64, record: ImageRenderRecord) {
    let metrics = shared_metrics();
    if let Ok(mut retained) = metrics.image_asset_retained.lock() {
        retained.insert(asset_key, record.clone());
    }
    if let Ok(mut recent) = metrics.recent_image_processings.lock() {
        recent.push_back(record);
        while recent.len() > 12 {
            recent.pop_front();
        }
    }
}

/// Removes a retained size-aware image asset from diagnostics.
pub fn drop_image_asset_retained(asset_key: u64) {
    if let Ok(mut retained) = shared_metrics().image_asset_retained.lock() {
        retained.remove(&asset_key);
    }
}

/// Records image cache entries evicted or explicitly dropped.
pub fn record_image_cache_eviction(count: usize) {
    shared_metrics()
        .image_cache_evictions
        .fetch_add(count as u64, Ordering::Relaxed);
}

/// Records render images dropped from window atlases.
pub fn record_image_drop(count: usize) {
    shared_metrics()
        .image_drop_count
        .fetch_add(count as u64, Ordering::Relaxed);
}
