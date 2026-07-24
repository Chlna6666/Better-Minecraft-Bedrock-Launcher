use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::AllocatorBucketMetricsSnapshot;
use super::super::state::shared_metrics;

/// Records atlas upload work completed by the active renderer.
pub fn record_atlas_upload_metrics(bytes: usize, tiles: usize, duration: Duration) {
    let metrics = shared_metrics();
    metrics
        .atlas_upload_bytes
        .fetch_add(bytes as u64, Ordering::Relaxed);
    metrics
        .atlas_upload_tiles
        .fetch_add(tiles as u64, Ordering::Relaxed);
    metrics.atlas_upload_micros.fetch_add(
        duration.as_micros().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}

/// Records upload bytes for the latest frame.
pub fn record_upload_bytes(bytes: usize) {
    shared_metrics()
        .upload_bytes
        .fetch_add(bytes as u64, Ordering::Relaxed);
}

/// Records bytes uploaded by explicit POD/bytemuck upload paths.
pub fn record_pod_upload_bytes(bytes: usize) {
    shared_metrics()
        .pod_upload_bytes
        .fetch_add(bytes as u64, Ordering::Relaxed);
}

/// Clears latest-frame upload and renderer cache counters before a new submission.
pub fn reset_frame_upload_metrics() {
    let metrics = shared_metrics();
    metrics.atlas_upload_bytes.store(0, Ordering::Relaxed);
    metrics.atlas_upload_tiles.store(0, Ordering::Relaxed);
    metrics.atlas_upload_micros.store(0, Ordering::Relaxed);
    metrics.upload_bytes.store(0, Ordering::Relaxed);
    metrics.pod_upload_bytes.store(0, Ordering::Relaxed);
    metrics.bind_group_creations.store(0, Ordering::Relaxed);
    metrics.bind_group_cache_hits.store(0, Ordering::Relaxed);
    metrics.bind_group_cache_misses.store(0, Ordering::Relaxed);
    metrics.gpu_cache_hits.store(0, Ordering::Relaxed);
    metrics.gpu_cache_misses.store(0, Ordering::Relaxed);
}

/// Records gpu bind group creation count for the latest renderer submission.
pub fn record_bind_group_creations(count: usize) {
    shared_metrics()
        .bind_group_creations
        .store(count as u64, Ordering::Relaxed);
}

/// Records gpu bind group cache hit/miss counts for the latest renderer submission.
pub fn record_bind_group_cache_metrics(hits: usize, misses: usize) {
    let metrics = shared_metrics();
    metrics
        .bind_group_cache_hits
        .store(hits as u64, Ordering::Relaxed);
    metrics
        .bind_group_cache_misses
        .store(misses as u64, Ordering::Relaxed);
}

/// Records upload arena capacity and use for the latest renderer submission.
pub fn record_upload_arena_metrics(
    uniform_capacity: usize,
    storage_capacity: usize,
    uniform_used: usize,
    storage_used: usize,
) {
    let metrics = shared_metrics();
    metrics
        .upload_arena_uniform_capacity
        .store(uniform_capacity as u64, Ordering::Relaxed);
    metrics
        .upload_arena_storage_capacity
        .store(storage_capacity as u64, Ordering::Relaxed);
    metrics
        .upload_arena_uniform_used
        .store(uniform_used as u64, Ordering::Relaxed);
    metrics
        .upload_arena_storage_used
        .store(storage_used as u64, Ordering::Relaxed);
}

/// Records high-level GPU resource cache hit/miss counts for the latest renderer submission.
pub fn record_gpu_cache_metrics(hits: usize, misses: usize) {
    let metrics = shared_metrics();
    metrics.gpu_cache_hits.store(hits as u64, Ordering::Relaxed);
    metrics
        .gpu_cache_misses
        .store(misses as u64, Ordering::Relaxed);
}

/// Records allocator and HAL memory accounting for the active renderer.
pub fn record_allocator_metrics(
    allocator_allocated_bytes: usize,
    allocator_reserved_bytes: usize,
    allocator_block_count: usize,
    allocator_allocation_count: usize,
    allocator_gpu_only: AllocatorBucketMetricsSnapshot,
    allocator_cpu_to_gpu: AllocatorBucketMetricsSnapshot,
    allocator_gpu_to_cpu: AllocatorBucketMetricsSnapshot,
    hal_buffer_memory_bytes: usize,
    hal_texture_memory_bytes: usize,
    hal_acceleration_structure_memory_bytes: usize,
    hal_memory_allocation_count: usize,
    core_staging_buffer_live_bytes: usize,
    core_staging_buffer_peak_live_bytes: usize,
    core_staging_buffer_created_bytes: usize,
    core_staging_buffer_pending_bytes: usize,
    core_staging_buffer_peak_pending_bytes: usize,
    core_staging_buffer_live_count: usize,
    core_staging_buffer_peak_live_count: usize,
    core_staging_buffer_pending_count: usize,
    core_staging_buffer_peak_pending_count: usize,
) {
    let metrics = shared_metrics();
    metrics
        .allocator_allocated_bytes
        .store(allocator_allocated_bytes as u64, Ordering::Relaxed);
    metrics
        .allocator_reserved_bytes
        .store(allocator_reserved_bytes as u64, Ordering::Relaxed);
    metrics
        .allocator_block_count
        .store(allocator_block_count as u64, Ordering::Relaxed);
    metrics
        .allocator_allocation_count
        .store(allocator_allocation_count as u64, Ordering::Relaxed);
    metrics
        .allocator_gpu_only_allocated_bytes
        .store(allocator_gpu_only.allocated_bytes as u64, Ordering::Relaxed);
    metrics
        .allocator_gpu_only_reserved_bytes
        .store(allocator_gpu_only.reserved_bytes as u64, Ordering::Relaxed);
    metrics
        .allocator_gpu_only_block_count
        .store(allocator_gpu_only.block_count as u64, Ordering::Relaxed);
    metrics.allocator_gpu_only_committed_allocated_bytes.store(
        allocator_gpu_only.committed_allocated_bytes as u64,
        Ordering::Relaxed,
    );
    metrics.allocator_gpu_only_committed_allocation_count.store(
        allocator_gpu_only.committed_allocation_count as u64,
        Ordering::Relaxed,
    );
    metrics.allocator_cpu_to_gpu_allocated_bytes.store(
        allocator_cpu_to_gpu.allocated_bytes as u64,
        Ordering::Relaxed,
    );
    metrics.allocator_cpu_to_gpu_reserved_bytes.store(
        allocator_cpu_to_gpu.reserved_bytes as u64,
        Ordering::Relaxed,
    );
    metrics
        .allocator_cpu_to_gpu_block_count
        .store(allocator_cpu_to_gpu.block_count as u64, Ordering::Relaxed);
    metrics
        .allocator_cpu_to_gpu_committed_allocated_bytes
        .store(
            allocator_cpu_to_gpu.committed_allocated_bytes as u64,
            Ordering::Relaxed,
        );
    metrics
        .allocator_cpu_to_gpu_committed_allocation_count
        .store(
            allocator_cpu_to_gpu.committed_allocation_count as u64,
            Ordering::Relaxed,
        );
    metrics.allocator_gpu_to_cpu_allocated_bytes.store(
        allocator_gpu_to_cpu.allocated_bytes as u64,
        Ordering::Relaxed,
    );
    metrics.allocator_gpu_to_cpu_reserved_bytes.store(
        allocator_gpu_to_cpu.reserved_bytes as u64,
        Ordering::Relaxed,
    );
    metrics
        .allocator_gpu_to_cpu_block_count
        .store(allocator_gpu_to_cpu.block_count as u64, Ordering::Relaxed);
    metrics
        .allocator_gpu_to_cpu_committed_allocated_bytes
        .store(
            allocator_gpu_to_cpu.committed_allocated_bytes as u64,
            Ordering::Relaxed,
        );
    metrics
        .allocator_gpu_to_cpu_committed_allocation_count
        .store(
            allocator_gpu_to_cpu.committed_allocation_count as u64,
            Ordering::Relaxed,
        );
    metrics
        .hal_buffer_memory_bytes
        .store(hal_buffer_memory_bytes as u64, Ordering::Relaxed);
    metrics
        .hal_texture_memory_bytes
        .store(hal_texture_memory_bytes as u64, Ordering::Relaxed);
    metrics.hal_acceleration_structure_memory_bytes.store(
        hal_acceleration_structure_memory_bytes as u64,
        Ordering::Relaxed,
    );
    metrics
        .hal_memory_allocation_count
        .store(hal_memory_allocation_count as u64, Ordering::Relaxed);
    metrics
        .core_staging_buffer_live_bytes
        .store(core_staging_buffer_live_bytes as u64, Ordering::Relaxed);
    metrics.core_staging_buffer_peak_live_bytes.store(
        core_staging_buffer_peak_live_bytes as u64,
        Ordering::Relaxed,
    );
    metrics
        .core_staging_buffer_created_bytes
        .store(core_staging_buffer_created_bytes as u64, Ordering::Relaxed);
    metrics
        .core_staging_buffer_pending_bytes
        .store(core_staging_buffer_pending_bytes as u64, Ordering::Relaxed);
    metrics.core_staging_buffer_peak_pending_bytes.store(
        core_staging_buffer_peak_pending_bytes as u64,
        Ordering::Relaxed,
    );
    metrics
        .core_staging_buffer_live_count
        .store(core_staging_buffer_live_count as u64, Ordering::Relaxed);
    metrics.core_staging_buffer_peak_live_count.store(
        core_staging_buffer_peak_live_count as u64,
        Ordering::Relaxed,
    );
    metrics
        .core_staging_buffer_pending_count
        .store(core_staging_buffer_pending_count as u64, Ordering::Relaxed);
    metrics.core_staging_buffer_peak_pending_count.store(
        core_staging_buffer_peak_pending_count as u64,
        Ordering::Relaxed,
    );
}
