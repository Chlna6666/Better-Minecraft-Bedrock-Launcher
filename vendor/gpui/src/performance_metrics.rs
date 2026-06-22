use crate::{Global, RendererBackend};
use collections::FxHashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

/// Per-process GPUI rendering and resource metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PerformanceMetricsSnapshot {
    /// Preferred renderer backend after builder/environment resolution.
    pub renderer_backend: RendererBackend,
    /// Number of images currently retained by GPUI image caches that report metrics.
    pub image_cache_items: usize,
    /// Estimated decoded image bytes retained by those caches.
    pub image_cache_bytes: usize,
    /// Number of GPUI atlas textures known to platform renderers that report metrics.
    pub atlas_textures: usize,
    /// Last reported platform draw time, when a backend reports it.
    pub last_draw_time: Option<Duration>,
    /// Exponentially-smoothed rate of platform presents reported by active renderers.
    pub present_fps: f32,
    /// Bytes uploaded into platform atlases during the latest reported frame.
    pub atlas_upload_bytes: usize,
    /// Number of atlas tiles uploaded during the latest reported frame.
    pub atlas_upload_tiles: usize,
    /// Number of prepared draw items in the latest renderer submission.
    pub prepared_command_count: usize,
    /// Active GPU surface format name.
    pub gpu_surface_format: String,
    /// Active GPU surface alpha mode.
    pub gpu_surface_alpha_mode: String,
    /// Active GPU surface present mode.
    pub gpu_surface_present_mode: String,
    /// Bytes uploaded during the latest renderer submission.
    pub upload_bytes: usize,
    /// Number of mask passes submitted by the latest reported frame.
    pub mask_pass_count: usize,
    /// Number of main 2D passes submitted by the latest reported frame.
    pub main_pass_count: usize,
    /// Number of composite passes submitted by the latest reported frame.
    pub composite_pass_count: usize,
    /// Number of GPU surface reconfigure attempts performed by the latest active renderer.
    pub gpu_surface_reconfigure_count: usize,
    /// Number of GPU surface acquire/present errors observed by the latest active renderer.
    pub gpu_surface_error_count: usize,
    /// Number of retained presents completed without rebuilding layout or paint.
    pub retained_present_count: usize,
    /// Time spent queueing platform atlas upload commands during the latest reported frame.
    pub atlas_upload_time: Option<Duration>,
    /// Compressed bytes in the most recently decoded image.
    pub last_image_decode_compressed_bytes: usize,
    /// Decoded BGRA bytes in the most recently decoded image.
    pub last_image_decode_decoded_bytes: usize,
    /// Frame count in the most recently decoded image.
    pub last_image_decode_frames: usize,
    /// Decode time for the most recently decoded image.
    pub last_image_decode_time: Option<Duration>,
    /// Number of completed image decodes recorded by GPUI and app decode helpers.
    pub image_decode_count: usize,
    /// Total compressed bytes processed by recorded image decodes.
    pub image_decode_compressed_bytes: usize,
    /// Total decoded BGRA bytes produced by recorded image decodes.
    pub image_decode_decoded_bytes: usize,
    /// Total decoded image frames produced by recorded image decodes.
    pub image_decode_frames: usize,
    /// Total time spent in recorded image decodes.
    pub image_decode_total_time: Option<Duration>,
    /// Maximum single recorded image decode time.
    pub image_decode_max_time: Option<Duration>,
    /// Number of recorded image decodes that exceeded the supplied slow threshold.
    pub image_decode_slow_count: usize,
    /// Decoded bytes currently retained by size-aware image assets.
    pub image_asset_retained_decoded_bytes: usize,
    /// Number of currently retained size-aware image asset variants.
    pub image_asset_retained_count: usize,
    /// Largest decoded image currently retained by size-aware image assets.
    pub image_asset_largest_retained_decoded_bytes: usize,
    /// Number of entries retained by GPUI image asset caches that are visible globally.
    pub gpui_image_asset_cache_entries: usize,
    /// Compressed bytes retained by GPUI image assets.
    pub gpui_image_asset_retained_compressed_bytes: usize,
    /// Decoded bytes retained by GPUI image assets and bounded image caches.
    pub gpui_image_asset_total_retained_decoded_bytes: usize,
    /// Estimated decoded bytes retained by render images in GPUI caches.
    pub gpui_render_image_cpu_bytes: usize,
    /// Estimated GPU texture bytes retained for render images.
    pub gpui_render_image_gpu_texture_bytes: usize,
    /// Number of entries retained by framework icon caches.
    pub gpui_icon_cache_entries: usize,
    /// Estimated decoded bytes retained by framework icon caches.
    pub gpui_icon_cache_decoded_bytes: usize,
    /// Estimated bytes retained by monochrome atlas textures.
    pub gpui_atlas_monochrome_bytes: usize,
    /// Estimated bytes retained by color atlas textures.
    pub gpui_atlas_polychrome_bytes: usize,
    /// Number of live atlas keys known to renderer metrics.
    pub gpui_atlas_live_keys: usize,
    /// Estimated unused bytes inside retained atlas textures.
    pub gpui_atlas_unused_bytes: usize,
    /// Estimated bytes retained by window surface and retained-frame resources.
    pub gpui_gpu_surface_texture_bytes: usize,
    /// Aggregate GPUI-owned retained bytes visible to diagnostics.
    pub gpui_gpu_estimated_total_retained_bytes: usize,
    /// Recent size-aware image decode records.
    pub recent_image_decodes: Vec<ImageDecodeRecord>,
    /// Total foreground time spent building the first frame.
    pub first_frame_build_time: Option<Duration>,
    /// Time spent in root layout for the first frame.
    pub first_frame_layout_time: Option<Duration>,
    /// Time spent in prepaint for the first frame.
    pub first_frame_prepaint_time: Option<Duration>,
    /// Time spent in paint for the first frame.
    pub first_frame_paint_time: Option<Duration>,
    /// Time spent finishing scene batches for the first frame.
    pub first_frame_scene_finish_time: Option<Duration>,
    /// Time spent in the backend draw call for the first frame.
    pub first_frame_backend_draw_time: Option<Duration>,
    /// Total foreground time spent building the latest profiled frame.
    pub frame_build_time: Option<Duration>,
    /// Time spent in root and overlay layout for the latest profiled frame.
    pub frame_layout_time: Option<Duration>,
    /// Time spent in prepaint for the latest profiled frame.
    pub frame_prepaint_time: Option<Duration>,
    /// Time spent in paint for the latest profiled frame.
    pub frame_paint_time: Option<Duration>,
    /// Time spent finishing scene batches for the latest profiled frame.
    pub frame_scene_finish_time: Option<Duration>,
    /// Time spent in the backend draw call for the latest profiled frame.
    pub frame_backend_draw_time: Option<Duration>,
    /// Number of layout nodes requested in the latest profiled frame.
    pub layout_nodes: usize,
    /// Number of measured layout nodes requested in the latest profiled frame.
    pub measured_layout_nodes: usize,
    /// Number of layout roots computed in the latest profiled frame.
    pub layout_roots: usize,
    /// Number of layout bounds cache hits in the latest profiled frame.
    pub layout_bounds_cache_hits: usize,
    /// Number of layout bounds cache misses in the latest profiled frame.
    pub layout_bounds_cache_misses: usize,
    /// Number of scene primitives emitted in the latest profiled frame.
    pub scene_primitives: usize,
    /// Number of prepared scene batches in the latest profiled frame.
    pub scene_batches: usize,
    /// Number of retained scene segments produced by scene finish.
    pub scene_segments: usize,
    /// Number of paint operations replayed from a prior frame.
    pub scene_replayed_primitives: usize,
    /// Number of retained segments rebuilt for dirty fibers in the latest profiled frame.
    pub scene_segment_rebuild_count: usize,
    /// Number of retained segments reused by clean fibers in the latest profiled frame.
    pub scene_segment_reuse_count: usize,
    /// Number of dirty transforms written in the latest profiled frame.
    pub dirty_transform_count: usize,
    /// Number of clean platform frame requests skipped by retained rendering.
    pub retained_frame_skips: usize,
    /// Number of pointer-move frames skipped before dispatch because hover state did not change.
    pub skipped_pointer_frame_count: usize,
    /// Dirty rectangles submitted by the latest frame render plan.
    pub dirty_rect_count: usize,
    /// Dirty rectangle area submitted by the latest frame render plan.
    pub dirty_rect_area: usize,
    /// Number of GPU partial redraw submissions since process start.
    pub partial_redraw_count: usize,
    /// Number of frames that fell back to full redraw since process start.
    pub full_redraw_fallback_count: usize,
    /// Estimated bytes retained by GPU window-sized targets and scratch resources.
    pub gpu_retained_bytes: usize,
    /// Estimated bytes retained by atlas textures.
    pub atlas_retained_bytes: usize,
    /// Whether a retained frame target currently exists.
    pub has_retained_frame_target: bool,
    /// Whether path intermediate textures currently exist.
    pub has_path_textures: bool,
    /// Whether backdrop source textures currently exist.
    pub has_backdrop_texture: bool,
    /// Whether depth textures currently exist.
    pub has_depth_texture: bool,
    /// Number of retained backdrop blur target groups.
    pub backdrop_blur_target_groups: usize,
    /// Number of retained GPU mesh buffers.
    pub gpu_mesh_buffers: usize,
    /// The resolved GPU adapter name reported by the active renderer.
    pub gpu_adapter_name: String,
    /// The resolved GPU adapter type reported by the active renderer.
    pub gpu_adapter_type: String,
    /// Number of redundant refresh requests coalesced before the next frame.
    pub coalesced_refresh_count: usize,
    /// Number of duplicate all-window refresh effects coalesced in an app update cycle.
    pub coalesced_refresh_effect_count: usize,
    /// Number of inactive frame callbacks that skipped presenting a retained scene.
    pub inactive_present_skip_count: usize,
    /// Number of inactive dirty frames deferred instead of rebuilding a scene.
    pub inactive_dirty_defer_count: usize,
    /// Number of retained layout cache hits in the latest profiled frame.
    pub layout_cache_hits: usize,
    /// Number of retained layout cache misses in the latest profiled frame.
    pub layout_cache_misses: usize,
    /// Number of retained layout roots directly reused in the latest profiled frame.
    pub layout_cache_reused_roots: usize,
    /// Number of layout roots whose retained traversal avoided rebuilding child roots.
    pub layout_cache_saved_roots: usize,
    /// Number of persistent Taffy nodes reused in the latest profiled frame.
    pub layout_persistent_node_reuses: usize,
    /// Number of persistent Taffy nodes created in the latest profiled frame.
    pub layout_persistent_node_creations: usize,
    /// Number of stale persistent Taffy nodes reclaimed after the latest profiled frame.
    pub layout_persistent_node_removals: usize,
    /// Number of stable view subtree prepaint cache hits since process start.
    pub view_cache_prepaint_hits: usize,
    /// Number of stable view subtree prepaint cache misses since process start.
    pub view_cache_prepaint_misses: usize,
    /// Number of stable view subtree paint cache hits since process start.
    pub view_cache_paint_hits: usize,
    /// Number of stable view subtree paint cache misses since process start.
    pub view_cache_paint_misses: usize,
    /// Number of foreground draw calls that exceeded the frame budget since process start.
    pub draw_budget_miss_count: usize,
    /// Number of times GPUI degraded frame scheduling after an over-budget draw.
    pub draw_degrade_count: usize,
    /// Number of times foreground effect flushing yielded with pending effects remaining.
    pub foreground_effect_yield_count: usize,
    /// Highest observed pending foreground effect queue depth when flushing yielded.
    pub foreground_effect_pending_max: usize,
    /// Current number of background executor tasks that have been scheduled but not completed.
    pub background_executor_queue_depth: usize,
    /// Highest observed background executor queue depth since process start.
    pub background_executor_queue_depth_max: usize,
    /// Number of same-frame text layout cache hits in the latest profiled frame.
    pub text_layout_hits: usize,
    /// Number of previous-frame text layouts reused in the latest profiled frame.
    pub text_layout_reuses: usize,
    /// Number of text layout misses in the latest profiled frame.
    pub text_layout_misses: usize,
    /// Number of text system background warm-up passes completed since process start.
    pub text_background_warmups: usize,
    /// Number of text layouts shaped by background warm-up since process start.
    pub text_background_layouts: usize,
    /// Number of image cache entries evicted or explicitly dropped.
    pub image_cache_evictions: usize,
    /// Number of render images dropped from window atlases.
    pub image_drop_count: usize,
    /// Number of atlas keys removed from platform atlases.
    pub atlas_remove_count: usize,
    /// Bytes uploaded through explicit POD/bytemuck upload paths during the latest submission.
    pub pod_upload_bytes: usize,
    /// Number of platform scheduler wakeups since process start.
    pub scheduler_wakeups: usize,
    /// Time spent with the on-demand platform scheduler asleep since process start.
    pub idle_sleep_time: Option<Duration>,
    /// Number of frame requests that reached window scheduling since process start.
    pub frame_request_count: usize,
    /// Number of frame decisions that drew a scene since process start.
    pub draw_count: usize,
    /// Number of frame decisions that presented an already prepared scene since process start.
    pub present_count: usize,
    /// Number of frame decisions skipped because no work was needed since process start.
    pub skip_count: usize,
    /// Number of resource sets created during the latest renderer submission.
    pub bind_group_creations: usize,
    /// Number of resource set cache hits during the latest renderer submission.
    pub bind_group_cache_hits: usize,
    /// Number of resource set cache misses during the latest renderer submission.
    pub bind_group_cache_misses: usize,
    /// Current uniform upload arena capacity in bytes.
    pub upload_arena_uniform_capacity: usize,
    /// Current storage upload arena capacity in bytes.
    pub upload_arena_storage_capacity: usize,
    /// Uniform upload arena bytes consumed by the latest renderer submission.
    pub upload_arena_uniform_used: usize,
    /// Storage upload arena bytes consumed by the latest renderer submission.
    pub upload_arena_storage_used: usize,
    /// Number of high-level GPU resource cache hits during the latest renderer submission.
    pub gpu_cache_hits: usize,
    /// Number of high-level GPU resource cache misses during the latest renderer submission.
    pub gpu_cache_misses: usize,
    /// Aggregate retained scene-side capacity after the latest profiled frame.
    pub scene_retained_capacity: usize,
    /// Aggregate retained frame-side capacity after the latest profiled frame.
    pub frame_retained_capacity: usize,
    /// Total bytes currently allocated through the backend allocator.
    pub allocator_allocated_bytes: usize,
    /// Total bytes currently reserved by allocator blocks.
    pub allocator_reserved_bytes: usize,
    /// Number of allocator memory blocks currently reserved.
    pub allocator_block_count: usize,
    /// Number of live allocator-tracked allocations.
    pub allocator_allocation_count: usize,
    /// Detailed bytes reserved and allocated for GPU-only allocator heaps.
    pub allocator_gpu_only: AllocatorBucketMetricsSnapshot,
    /// Detailed bytes reserved and allocated for CPU-to-GPU allocator heaps.
    pub allocator_cpu_to_gpu: AllocatorBucketMetricsSnapshot,
    /// Detailed bytes reserved and allocated for GPU-to-CPU allocator heaps.
    pub allocator_gpu_to_cpu: AllocatorBucketMetricsSnapshot,
    /// HAL-reported bytes attributed to buffers.
    pub hal_buffer_memory_bytes: usize,
    /// HAL-reported bytes attributed to textures.
    pub hal_texture_memory_bytes: usize,
    /// HAL-reported bytes attributed to acceleration structures.
    pub hal_acceleration_structure_memory_bytes: usize,
    /// HAL-reported number of memory allocations.
    pub hal_memory_allocation_count: usize,
    /// Live bytes currently retained by flushed backend staging buffers.
    pub core_staging_buffer_live_bytes: usize,
    /// Peak live bytes retained by flushed backend staging buffers.
    pub core_staging_buffer_peak_live_bytes: usize,
    /// Cumulative bytes created through backend staging buffers.
    pub core_staging_buffer_created_bytes: usize,
    /// Bytes currently pending in backend `PendingWrites` staging resources.
    pub core_staging_buffer_pending_bytes: usize,
    /// Peak pending bytes observed in backend `PendingWrites`.
    pub core_staging_buffer_peak_pending_bytes: usize,
    /// Current live flushed staging buffer count.
    pub core_staging_buffer_live_count: usize,
    /// Peak live flushed staging buffer count.
    pub core_staging_buffer_peak_live_count: usize,
    /// Current pending staging buffer count.
    pub core_staging_buffer_pending_count: usize,
    /// Peak pending staging buffer count.
    pub core_staging_buffer_peak_pending_count: usize,
    /// Window-scoped metrics captured by the latest updates.
    pub window_metrics: Vec<WindowMetricsSnapshot>,
}

/// Diagnostics for one size-aware image decode.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDecodeRecord {
    /// Stable source label used for diagnostics.
    pub source: String,
    /// Original source width, when available.
    pub original_width: u32,
    /// Original source height, when available.
    pub original_height: u32,
    /// Requested decode width in device pixels.
    pub target_width: u32,
    /// Requested decode height in device pixels.
    pub target_height: u32,
    /// Retained decoded bytes produced by the decode.
    pub retained_decoded_bytes: usize,
    /// Decode implementation used for this record.
    pub decode_mode: String,
}

/// Per-window GPUI rendering and scheduling metrics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowMetricsSnapshot {
    /// Stable platform window id.
    pub window_id: u64,
    /// Number of frame requests queued for this window.
    pub request_redraw_count: usize,
    /// Number of frames that drew for this window.
    pub draw_count: usize,
    /// Number of frames that presented for this window.
    pub present_count: usize,
    /// Number of frame decisions skipped for this window.
    pub skip_count: usize,
    /// Number of skipped frame opportunities observed for this window.
    pub skipped_frame_count: usize,
    /// Number of GPU surface reconfigures for this window.
    pub gpu_surface_reconfigure_count: usize,
    /// Number of GPU surface acquire/present errors for this window.
    pub gpu_surface_error_count: usize,
    /// Number of layout recomputes for this window.
    pub layout_recompute_count: usize,
    /// Bytes uploaded on behalf of this window during the latest renderer submission.
    pub upload_bytes: usize,
}

/// Per-memory-location allocator metrics for one backend bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocatorBucketMetricsSnapshot {
    /// Bytes currently allocated from this allocator bucket.
    pub allocated_bytes: usize,
    /// Bytes currently reserved by blocks in this allocator bucket.
    pub reserved_bytes: usize,
    /// Number of live blocks in this allocator bucket.
    pub block_count: usize,
    /// Bytes attributed to committed allocations in this allocator bucket.
    pub committed_allocated_bytes: usize,
    /// Number of committed allocations in this allocator bucket.
    pub committed_allocation_count: usize,
}

/// Global wrapper for [`PerformanceMetricsSnapshot`].
#[derive(Clone, Debug, Default)]
pub struct PerformanceMetrics(pub PerformanceMetricsSnapshot);

impl Global for PerformanceMetrics {}

#[derive(Default)]
struct SharedMetrics {
    renderer_backend: Mutex<RendererBackend>,
    gpu_adapter_name: Mutex<String>,
    gpu_adapter_type: Mutex<String>,
    image_caches: Mutex<FxHashMap<u64, (usize, usize)>>,
    render_image_live_count: AtomicU64,
    render_image_live_decoded_bytes: AtomicU64,
    atlas_textures: AtomicU64,
    atlas_monochrome_bytes: AtomicU64,
    atlas_polychrome_bytes: AtomicU64,
    atlas_live_keys: AtomicU64,
    atlas_unused_bytes: AtomicU64,
    last_draw_micros: AtomicU64,
    last_present_at: Mutex<Option<Instant>>,
    present_fps_milli: AtomicU64,
    atlas_upload_bytes: AtomicU64,
    atlas_upload_tiles: AtomicU64,
    prepared_command_count: AtomicU64,
    gpu_surface_format: Mutex<String>,
    gpu_surface_alpha_mode: Mutex<String>,
    gpu_surface_present_mode: Mutex<String>,
    upload_bytes: AtomicU64,
    mask_pass_count: AtomicU64,
    main_pass_count: AtomicU64,
    composite_pass_count: AtomicU64,
    gpu_surface_reconfigure_count: AtomicU64,
    gpu_surface_error_count: AtomicU64,
    retained_present_count: AtomicU64,
    atlas_upload_micros: AtomicU64,
    image_decode_compressed_bytes: AtomicU64,
    image_decode_decoded_bytes: AtomicU64,
    image_decode_frames: AtomicU64,
    image_decode_micros: AtomicU64,
    image_decode_count: AtomicU64,
    image_decode_total_compressed_bytes: AtomicU64,
    image_decode_total_decoded_bytes: AtomicU64,
    image_decode_total_frames: AtomicU64,
    image_decode_total_micros: AtomicU64,
    image_decode_max_micros: AtomicU64,
    image_decode_slow_count: AtomicU64,
    image_asset_retained: Mutex<FxHashMap<u64, ImageDecodeRecord>>,
    recent_image_decodes: Mutex<VecDeque<ImageDecodeRecord>>,
    first_frame_build_micros: AtomicU64,
    first_frame_layout_micros: AtomicU64,
    first_frame_prepaint_micros: AtomicU64,
    first_frame_paint_micros: AtomicU64,
    first_frame_scene_finish_micros: AtomicU64,
    first_frame_backend_draw_micros: AtomicU64,
    frame_build_micros: AtomicU64,
    frame_layout_micros: AtomicU64,
    frame_prepaint_micros: AtomicU64,
    frame_paint_micros: AtomicU64,
    frame_scene_finish_micros: AtomicU64,
    frame_backend_draw_micros: AtomicU64,
    layout_nodes: AtomicU64,
    measured_layout_nodes: AtomicU64,
    layout_roots: AtomicU64,
    layout_bounds_cache_hits: AtomicU64,
    layout_bounds_cache_misses: AtomicU64,
    scene_primitives: AtomicU64,
    scene_batches: AtomicU64,
    scene_segments: AtomicU64,
    scene_replayed_primitives: AtomicU64,
    scene_segment_rebuild_count: AtomicU64,
    scene_segment_reuse_count: AtomicU64,
    dirty_transform_count: AtomicU64,
    retained_frame_skips: AtomicU64,
    skipped_pointer_frame_count: AtomicU64,
    dirty_rect_count: AtomicU64,
    dirty_rect_area: AtomicU64,
    partial_redraw_count: AtomicU64,
    full_redraw_fallback_count: AtomicU64,
    gpu_retained_bytes: AtomicU64,
    atlas_retained_bytes: AtomicU64,
    has_retained_frame_target: AtomicU64,
    has_path_textures: AtomicU64,
    has_backdrop_texture: AtomicU64,
    has_depth_texture: AtomicU64,
    backdrop_blur_target_groups: AtomicU64,
    gpu_mesh_buffers: AtomicU64,
    coalesced_refresh_count: AtomicU64,
    coalesced_refresh_effect_count: AtomicU64,
    inactive_present_skip_count: AtomicU64,
    inactive_dirty_defer_count: AtomicU64,
    layout_cache_hits: AtomicU64,
    layout_cache_misses: AtomicU64,
    layout_cache_reused_roots: AtomicU64,
    layout_cache_saved_roots: AtomicU64,
    layout_persistent_node_reuses: AtomicU64,
    layout_persistent_node_creations: AtomicU64,
    layout_persistent_node_removals: AtomicU64,
    view_cache_prepaint_hits: AtomicU64,
    view_cache_prepaint_misses: AtomicU64,
    view_cache_paint_hits: AtomicU64,
    view_cache_paint_misses: AtomicU64,
    draw_budget_miss_count: AtomicU64,
    draw_degrade_count: AtomicU64,
    foreground_effect_yield_count: AtomicU64,
    foreground_effect_pending_max: AtomicU64,
    background_executor_queue_depth: AtomicU64,
    background_executor_queue_depth_max: AtomicU64,
    text_layout_hits: AtomicU64,
    text_layout_reuses: AtomicU64,
    text_layout_misses: AtomicU64,
    text_background_warmups: AtomicU64,
    text_background_layouts: AtomicU64,
    image_cache_evictions: AtomicU64,
    image_drop_count: AtomicU64,
    atlas_remove_count: AtomicU64,
    pod_upload_bytes: AtomicU64,
    scheduler_wakeups: AtomicU64,
    idle_sleep_micros: AtomicU64,
    frame_request_count: AtomicU64,
    draw_count: AtomicU64,
    present_count: AtomicU64,
    skip_count: AtomicU64,
    bind_group_creations: AtomicU64,
    bind_group_cache_hits: AtomicU64,
    bind_group_cache_misses: AtomicU64,
    upload_arena_uniform_capacity: AtomicU64,
    upload_arena_storage_capacity: AtomicU64,
    upload_arena_uniform_used: AtomicU64,
    upload_arena_storage_used: AtomicU64,
    gpu_cache_hits: AtomicU64,
    gpu_cache_misses: AtomicU64,
    scene_retained_capacity: AtomicU64,
    frame_retained_capacity: AtomicU64,
    allocator_allocated_bytes: AtomicU64,
    allocator_reserved_bytes: AtomicU64,
    allocator_block_count: AtomicU64,
    allocator_allocation_count: AtomicU64,
    allocator_gpu_only_allocated_bytes: AtomicU64,
    allocator_gpu_only_reserved_bytes: AtomicU64,
    allocator_gpu_only_block_count: AtomicU64,
    allocator_gpu_only_committed_allocated_bytes: AtomicU64,
    allocator_gpu_only_committed_allocation_count: AtomicU64,
    allocator_cpu_to_gpu_allocated_bytes: AtomicU64,
    allocator_cpu_to_gpu_reserved_bytes: AtomicU64,
    allocator_cpu_to_gpu_block_count: AtomicU64,
    allocator_cpu_to_gpu_committed_allocated_bytes: AtomicU64,
    allocator_cpu_to_gpu_committed_allocation_count: AtomicU64,
    allocator_gpu_to_cpu_allocated_bytes: AtomicU64,
    allocator_gpu_to_cpu_reserved_bytes: AtomicU64,
    allocator_gpu_to_cpu_block_count: AtomicU64,
    allocator_gpu_to_cpu_committed_allocated_bytes: AtomicU64,
    allocator_gpu_to_cpu_committed_allocation_count: AtomicU64,
    hal_buffer_memory_bytes: AtomicU64,
    hal_texture_memory_bytes: AtomicU64,
    hal_acceleration_structure_memory_bytes: AtomicU64,
    hal_memory_allocation_count: AtomicU64,
    core_staging_buffer_live_bytes: AtomicU64,
    core_staging_buffer_peak_live_bytes: AtomicU64,
    core_staging_buffer_created_bytes: AtomicU64,
    core_staging_buffer_pending_bytes: AtomicU64,
    core_staging_buffer_peak_pending_bytes: AtomicU64,
    core_staging_buffer_live_count: AtomicU64,
    core_staging_buffer_peak_live_count: AtomicU64,
    core_staging_buffer_pending_count: AtomicU64,
    core_staging_buffer_peak_pending_count: AtomicU64,
    window_metrics: Mutex<FxHashMap<u64, WindowMetrics>>,
}

#[derive(Default, Clone)]
struct WindowMetrics {
    request_redraw_count: u64,
    draw_count: u64,
    present_count: u64,
    skip_count: u64,
    skipped_frame_count: u64,
    gpu_surface_reconfigure_count: u64,
    gpu_surface_error_count: u64,
    layout_recompute_count: u64,
    upload_bytes: u64,
}

static SHARED_METRICS: OnceLock<Arc<SharedMetrics>> = OnceLock::new();

fn shared_metrics() -> &'static Arc<SharedMetrics> {
    SHARED_METRICS.get_or_init(|| Arc::new(SharedMetrics::default()))
}

/// Records the selected renderer backend for diagnostics.
pub fn record_renderer_backend(backend: RendererBackend) {
    if let Ok(mut current_backend) = shared_metrics().renderer_backend.lock() {
        *current_backend = backend;
    }
}

/// Records the resolved GPU adapter diagnostics for the active renderer.
pub fn record_gpu_adapter_diagnostics(name: &str, adapter_type: &str) {
    if let Ok(mut current_name) = shared_metrics().gpu_adapter_name.lock() {
        current_name.clear();
        current_name.push_str(name);
    }
    if let Ok(mut current_type) = shared_metrics().gpu_adapter_type.lock() {
        current_type.clear();
        current_type.push_str(adapter_type);
    }
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

/// Records one live [`RenderImage`](crate::RenderImage) allocation for CPU-side diagnostics.
pub(crate) fn record_render_image_created(decoded_bytes: usize) {
    let metrics = shared_metrics();
    metrics
        .render_image_live_count
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .render_image_live_decoded_bytes
        .fetch_add(decoded_bytes as u64, Ordering::Relaxed);
}

/// Records that one [`RenderImage`](crate::RenderImage) was dropped.
pub(crate) fn record_render_image_dropped(decoded_bytes: usize) {
    let metrics = shared_metrics();
    metrics
        .render_image_live_count
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_sub(1))
        })
        .ok();
    metrics
        .render_image_live_decoded_bytes
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_sub(decoded_bytes as u64))
        })
        .ok();
}

/// Records the number of live atlas textures known to the active renderer.
pub fn record_atlas_texture_count(count: usize) {
    shared_metrics()
        .atlas_textures
        .store(count as u64, Ordering::Relaxed);
}

/// Records retained bytes and live keys for platform atlas diagnostics.
pub fn record_atlas_memory_metrics(
    monochrome_bytes: usize,
    polychrome_bytes: usize,
    live_keys: usize,
    unused_bytes: usize,
) {
    let metrics = shared_metrics();
    metrics
        .atlas_monochrome_bytes
        .store(monochrome_bytes as u64, Ordering::Relaxed);
    metrics
        .atlas_polychrome_bytes
        .store(polychrome_bytes as u64, Ordering::Relaxed);
    metrics
        .atlas_live_keys
        .store(live_keys as u64, Ordering::Relaxed);
    metrics
        .atlas_unused_bytes
        .store(unused_bytes as u64, Ordering::Relaxed);
}

/// Records the number of prepared draw items submitted for the latest frame.
pub fn record_prepared_command_count(count: usize) {
    shared_metrics()
        .prepared_command_count
        .store(count as u64, Ordering::Relaxed);
}

/// Legacy compatibility shim retained during the renderer migration.
pub fn record_backdrop_blur_primitive_count(_count: usize) {}

/// Records the platform renderer draw duration.
pub fn record_draw_time(duration: Duration) {
    shared_metrics().last_draw_micros.store(
        duration.as_micros().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}

/// Records backend draw time for a profiled frame.
pub fn record_frame_backend_draw_time(duration: Duration) {
    shared_metrics().frame_backend_draw_micros.store(
        duration.as_micros().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}

/// Records a clean platform frame request skipped by retained rendering.
pub fn record_retained_frame_skip() {
    shared_metrics()
        .retained_frame_skips
        .fetch_add(1, Ordering::Relaxed);
}

/// Records a pointer frame skipped because no hover/drag/listener state changed.
pub fn record_skipped_pointer_frame() {
    shared_metrics()
        .skipped_pointer_frame_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records dirty-region diagnostics for the latest frame.
pub fn record_dirty_region_metrics(rect_count: usize, area: usize) {
    let metrics = shared_metrics();
    metrics
        .dirty_rect_count
        .store(rect_count as u64, Ordering::Relaxed);
    metrics
        .dirty_rect_area
        .store(area as u64, Ordering::Relaxed);
}

/// Records that the backend submitted a partial redraw.
pub fn record_partial_redraw() {
    shared_metrics()
        .partial_redraw_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records that the backend fell back to a full redraw.
pub fn record_full_redraw_fallback() {
    shared_metrics()
        .full_redraw_fallback_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records estimated bytes retained by GPU resources.
pub fn record_gpu_retained_bytes(bytes: usize) {
    shared_metrics()
        .gpu_retained_bytes
        .store(bytes as u64, Ordering::Relaxed);
}

/// Records a startup-facing breakdown of retained GPUI-owned GPU resources.
pub fn record_gpu_resource_breakdown(
    atlas_retained_bytes: usize,
    has_retained_frame_target: bool,
    has_path_textures: bool,
    has_backdrop_texture: bool,
    has_depth_texture: bool,
    backdrop_blur_target_groups: usize,
    gpu_mesh_buffers: usize,
) {
    let metrics = shared_metrics();
    metrics
        .atlas_retained_bytes
        .store(atlas_retained_bytes as u64, Ordering::Relaxed);
    metrics
        .has_retained_frame_target
        .store(u64::from(has_retained_frame_target), Ordering::Relaxed);
    metrics
        .has_path_textures
        .store(u64::from(has_path_textures), Ordering::Relaxed);
    metrics
        .has_backdrop_texture
        .store(u64::from(has_backdrop_texture), Ordering::Relaxed);
    metrics
        .has_depth_texture
        .store(u64::from(has_depth_texture), Ordering::Relaxed);
    metrics
        .backdrop_blur_target_groups
        .store(backdrop_blur_target_groups as u64, Ordering::Relaxed);
    metrics
        .gpu_mesh_buffers
        .store(gpu_mesh_buffers as u64, Ordering::Relaxed);
}

/// Records a redundant refresh request that was already pending for the next frame.
pub fn record_coalesced_refresh() {
    shared_metrics()
        .coalesced_refresh_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records a duplicate all-window refresh effect coalesced before effect flushing.
pub fn record_coalesced_refresh_effect() {
    shared_metrics()
        .coalesced_refresh_effect_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records an inactive frame callback that avoided presenting unchanged content.
pub fn record_inactive_present_skip() {
    shared_metrics()
        .inactive_present_skip_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records an inactive dirty frame that was deferred instead of being rebuilt immediately.
pub fn record_inactive_dirty_defer() {
    shared_metrics()
        .inactive_dirty_defer_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records a retained scene presentation that did not rebuild the scene.
pub fn record_retained_scene_present() {
    shared_metrics()
        .retained_present_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records that the active platform renderer presented one frame.
pub fn record_present() {
    let metrics = shared_metrics();
    let now = Instant::now();
    let Ok(mut last_present_at) = metrics.last_present_at.lock() else {
        return;
    };
    let Some(previous_present_at) = last_present_at.replace(now) else {
        return;
    };
    let delta = now.saturating_duration_since(previous_present_at);
    if delta < Duration::from_millis(1) || delta > Duration::from_secs(1) {
        return;
    }

    let instant_fps_milli = (1000.0 / delta.as_secs_f32()).round() as u64;
    let previous_fps_milli = metrics.present_fps_milli.load(Ordering::Relaxed);
    let next_fps_milli = if previous_fps_milli == 0 {
        instant_fps_milli
    } else {
        ((previous_fps_milli as f32 * 0.9) + (instant_fps_milli as f32 * 0.1)).round() as u64
    };
    metrics
        .present_fps_milli
        .store(next_fps_milli, Ordering::Relaxed);
}

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

/// Records one wakeup from an on-demand platform frame scheduler.
pub fn record_scheduler_wakeup() {
    shared_metrics()
        .scheduler_wakeups
        .fetch_add(1, Ordering::Relaxed);
}

/// Records time spent with an on-demand platform frame scheduler parked.
pub fn record_scheduler_idle_sleep(duration: Duration) {
    shared_metrics().idle_sleep_micros.fetch_add(
        duration.as_micros().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}

/// Records one coalesced frame request reaching the platform layer.
pub fn record_frame_request() {
    shared_metrics()
        .frame_request_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records a window frame scheduling decision.
pub fn record_frame_decision(draw: bool, present: bool, skip: bool) {
    let metrics = shared_metrics();
    if draw {
        metrics.draw_count.fetch_add(1, Ordering::Relaxed);
    }
    if present {
        metrics.present_count.fetch_add(1, Ordering::Relaxed);
    }
    if skip {
        metrics.skip_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records resource set creation count for the latest renderer submission.
pub fn record_bind_group_creations(count: usize) {
    shared_metrics()
        .bind_group_creations
        .store(count as u64, Ordering::Relaxed);
}

/// Records resource set cache hit/miss counts for the latest renderer submission.
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

/// Records that a specific window requested a redraw.
pub fn record_window_request_redraw(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.request_redraw_count = metrics.request_redraw_count.saturating_add(1);
    }
}

/// Records the outcome of a specific window frame decision.
pub fn record_window_frame_result(window_id: u64, draw: bool, present: bool, skip: bool) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        if draw {
            metrics.draw_count = metrics.draw_count.saturating_add(1);
        }
        if present {
            metrics.present_count = metrics.present_count.saturating_add(1);
        }
        if skip {
            metrics.skip_count = metrics.skip_count.saturating_add(1);
            metrics.skipped_frame_count = metrics.skipped_frame_count.saturating_add(1);
        }
    }
}

/// Records per-window GPU surface diagnostics.
pub fn record_window_gpu_surface_metrics(
    window_id: u64,
    surface_reconfigure_count: usize,
    surface_error_count: usize,
) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.gpu_surface_reconfigure_count = surface_reconfigure_count as u64;
        metrics.gpu_surface_error_count = surface_error_count as u64;
    }
}

/// Records a layout recompute for a specific window.
pub fn record_window_layout_recompute(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.layout_recompute_count = metrics.layout_recompute_count.saturating_add(1);
    }
}

/// Records bytes uploaded for a specific window during the latest renderer submission.
pub fn record_window_upload_bytes(window_id: u64, bytes: usize) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.upload_bytes = bytes as u64;
    }
}

/// Records GPU pass diagnostics for the latest frame.
pub fn record_gpu_pass_metrics(
    mask_pass_count: usize,
    main_pass_count: usize,
    composite_pass_count: usize,
) {
    let metrics = shared_metrics();
    metrics
        .mask_pass_count
        .store(mask_pass_count as u64, Ordering::Relaxed);
    metrics
        .main_pass_count
        .store(main_pass_count as u64, Ordering::Relaxed);
    metrics
        .composite_pass_count
        .store(composite_pass_count as u64, Ordering::Relaxed);
}

/// Records GPU surface diagnostics.
pub fn record_gpu_surface_metrics(
    surface_format: &str,
    surface_alpha_mode: &str,
    surface_present_mode: &str,
    surface_reconfigure_count: usize,
    surface_error_count: usize,
) {
    let metrics = shared_metrics();
    if let Ok(mut format) = metrics.gpu_surface_format.lock() {
        *format = surface_format.to_string();
    }
    if let Ok(mut alpha_mode) = metrics.gpu_surface_alpha_mode.lock() {
        *alpha_mode = surface_alpha_mode.to_string();
    }
    if let Ok(mut present_mode) = metrics.gpu_surface_present_mode.lock() {
        *present_mode = surface_present_mode.to_string();
    }
    metrics
        .gpu_surface_reconfigure_count
        .store(surface_reconfigure_count as u64, Ordering::Relaxed);
    metrics
        .gpu_surface_error_count
        .store(surface_error_count as u64, Ordering::Relaxed);
}

/// Records the number of retained presents that reused the retained target.
pub fn record_retained_present_count(count: usize) {
    shared_metrics()
        .retained_present_count
        .store(count as u64, Ordering::Relaxed);
}

/// Records the latest completed image decode.
pub fn record_image_decode_metrics(
    compressed_bytes: usize,
    decoded_bytes: usize,
    frames: usize,
    duration: Duration,
) {
    record_image_decode_metrics_with_threshold(
        compressed_bytes,
        decoded_bytes,
        frames,
        duration,
        Duration::MAX,
    );
}

/// Records a completed image decode and marks it slow when it exceeds `slow_threshold`.
pub fn record_image_decode_metrics_with_threshold(
    compressed_bytes: usize,
    decoded_bytes: usize,
    frames: usize,
    duration: Duration,
    slow_threshold: Duration,
) {
    let metrics = shared_metrics();
    let duration_micros = duration_micros(duration);
    metrics
        .image_decode_compressed_bytes
        .store(compressed_bytes as u64, Ordering::Relaxed);
    metrics
        .image_decode_decoded_bytes
        .store(decoded_bytes as u64, Ordering::Relaxed);
    metrics
        .image_decode_frames
        .store(frames as u64, Ordering::Relaxed);
    metrics
        .image_decode_micros
        .store(duration_micros, Ordering::Relaxed);
    metrics.image_decode_count.fetch_add(1, Ordering::Relaxed);
    metrics
        .image_decode_total_compressed_bytes
        .fetch_add(compressed_bytes as u64, Ordering::Relaxed);
    metrics
        .image_decode_total_decoded_bytes
        .fetch_add(decoded_bytes as u64, Ordering::Relaxed);
    metrics
        .image_decode_total_frames
        .fetch_add(frames as u64, Ordering::Relaxed);
    metrics
        .image_decode_total_micros
        .fetch_add(duration_micros, Ordering::Relaxed);
    metrics
        .image_decode_max_micros
        .fetch_max(duration_micros, Ordering::Relaxed);
    if duration >= slow_threshold {
        metrics
            .image_decode_slow_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Records a currently retained size-aware image asset.
pub fn record_image_asset_retained(asset_key: u64, record: ImageDecodeRecord) {
    let metrics = shared_metrics();
    if let Ok(mut retained) = metrics.image_asset_retained.lock() {
        retained.insert(asset_key, record.clone());
    }
    if let Ok(mut recent) = metrics.recent_image_decodes.lock() {
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

/// Records first-frame foreground build timing.
pub fn record_first_frame_build_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_build_micros, duration);
}

/// Records first-frame root layout timing.
pub fn record_first_frame_layout_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_layout_micros, duration);
}

/// Records first-frame prepaint timing.
pub fn record_first_frame_prepaint_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_prepaint_micros, duration);
}

/// Records first-frame paint timing.
pub fn record_first_frame_paint_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_paint_micros, duration);
}

/// Records first-frame scene finish timing.
pub fn record_first_frame_scene_finish_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_scene_finish_micros, duration);
}

/// Records first-frame backend draw timing.
pub fn record_first_frame_backend_draw_time(duration: Duration) {
    record_once_micros(&shared_metrics().first_frame_backend_draw_micros, duration);
}

/// Per-frame foreground timings recorded when frame profiling is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct FramePhaseMetrics {
    /// Total foreground frame build duration.
    pub build: Duration,
    /// Root and overlay layout duration.
    pub layout: Duration,
    /// Prepaint duration.
    pub prepaint: Duration,
    /// Paint duration.
    pub paint: Duration,
    /// Scene finish duration.
    pub scene_finish: Duration,
}

/// Per-frame layout counters recorded when frame profiling is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutFrameMetrics {
    /// Number of layout nodes requested this frame.
    pub nodes: usize,
    /// Number of measured layout nodes requested this frame.
    pub measured_nodes: usize,
    /// Number of layout roots computed this frame.
    pub roots: usize,
    /// Number of layout bounds cache hits this frame.
    pub bounds_cache_hits: usize,
    /// Number of layout bounds cache misses this frame.
    pub bounds_cache_misses: usize,
    /// Number of retained layout roots reused this frame.
    pub cache_reused_roots: usize,
    /// Number of child roots saved by retained layout reuse this frame.
    pub cache_saved_roots: usize,
    /// Number of persistent Taffy nodes reused this frame.
    pub persistent_node_reuses: usize,
    /// Number of persistent Taffy nodes created this frame.
    pub persistent_node_creations: usize,
    /// Number of stale persistent Taffy nodes reclaimed at the end of this frame.
    pub persistent_node_removals: usize,
}

/// Per-frame scene counters recorded when frame profiling is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct SceneFrameMetrics {
    /// Number of primitives emitted into the scene.
    pub primitives: usize,
    /// Number of prepared batches produced by scene finish.
    pub batches: usize,
    /// Number of retained scene segments produced by scene finish.
    pub segments: usize,
    /// Number of paint operations replayed from a prior frame.
    pub replayed_primitives: usize,
    /// Number of retained segments rebuilt for dirty fibers this frame.
    pub segment_rebuild_count: usize,
    /// Number of retained segments reused by clean fibers this frame.
    pub segment_reuse_count: usize,
    /// Aggregate retained scene-side capacity after finish.
    pub retained_capacity: usize,
}

/// Records foreground frame timings for diagnostics.
pub fn record_frame_phase_metrics(metrics: FramePhaseMetrics) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .frame_build_micros
        .store(duration_micros(metrics.build), Ordering::Relaxed);
    shared_metrics
        .frame_layout_micros
        .store(duration_micros(metrics.layout), Ordering::Relaxed);
    shared_metrics
        .frame_prepaint_micros
        .store(duration_micros(metrics.prepaint), Ordering::Relaxed);
    shared_metrics
        .frame_paint_micros
        .store(duration_micros(metrics.paint), Ordering::Relaxed);
    shared_metrics
        .frame_scene_finish_micros
        .store(duration_micros(metrics.scene_finish), Ordering::Relaxed);
}

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
    shared_metrics
        .layout_persistent_node_reuses
        .store(metrics.persistent_node_reuses as u64, Ordering::Relaxed);
    shared_metrics
        .layout_persistent_node_creations
        .store(metrics.persistent_node_creations as u64, Ordering::Relaxed);
    shared_metrics
        .layout_persistent_node_removals
        .store(metrics.persistent_node_removals as u64, Ordering::Relaxed);
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

/// Records whether a stable view subtree reused its cached prepaint range.
pub fn record_view_cache_prepaint_hit(hit: bool) {
    let counter = if hit {
        &shared_metrics().view_cache_prepaint_hits
    } else {
        &shared_metrics().view_cache_prepaint_misses
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Records whether a stable view subtree reused its cached paint range.
pub fn record_view_cache_paint_hit(hit: bool) {
    let counter = if hit {
        &shared_metrics().view_cache_paint_hits
    } else {
        &shared_metrics().view_cache_paint_misses
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Records an over-budget foreground draw.
pub fn record_draw_budget_miss() {
    shared_metrics()
        .draw_budget_miss_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records a frame scheduling degradation caused by an over-budget draw.
pub fn record_draw_degrade() {
    shared_metrics()
        .draw_degrade_count
        .fetch_add(1, Ordering::Relaxed);
}

/// Records that foreground effect flushing yielded to avoid monopolizing the UI thread.
pub(crate) fn record_foreground_effect_yield(pending_effects: usize) {
    let metrics = shared_metrics();
    metrics
        .foreground_effect_yield_count
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .foreground_effect_pending_max
        .fetch_max(pending_effects as u64, Ordering::Relaxed);
}

/// Records one pending background executor task.
pub(crate) fn record_background_executor_task_scheduled() {
    let metrics = shared_metrics();
    let depth = metrics
        .background_executor_queue_depth
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    metrics
        .background_executor_queue_depth_max
        .fetch_max(depth, Ordering::Relaxed);
}

/// Records one completed background executor task.
pub(crate) fn record_background_executor_task_finished() {
    shared_metrics()
        .background_executor_queue_depth
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |depth| {
            Some(depth.saturating_sub(1))
        })
        .ok();
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

/// Records text layouts that were precomputed by the background warm-up path.
pub fn record_text_background_warmup(layouts: usize) {
    let shared_metrics = shared_metrics();
    shared_metrics
        .text_background_warmups
        .fetch_add(1, Ordering::Relaxed);
    shared_metrics
        .text_background_layouts
        .fetch_add(layouts as u64, Ordering::Relaxed);
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

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn record_once_micros(metric: &AtomicU64, duration: Duration) {
    let micros = duration_micros(duration);
    let value = micros.max(1);
    if metric
        .compare_exchange(0, value, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        // Another first-frame writer won the race; preserving the first sample is intentional.
    }
}

/// Returns a point-in-time snapshot of GPUI performance metrics.
pub fn performance_metrics_snapshot() -> PerformanceMetricsSnapshot {
    let renderer_backend = shared_metrics()
        .renderer_backend
        .lock()
        .map_or(RendererBackend::Auto, |backend| *backend);
    let gpu_adapter_name = shared_metrics()
        .gpu_adapter_name
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let gpu_adapter_type = shared_metrics()
        .gpu_adapter_type
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let (image_cache_items, image_cache_bytes) = shared_metrics()
        .image_caches
        .lock()
        .map(|image_caches| {
            image_caches.values().copied().fold(
                (0usize, 0usize),
                |(total_items, total_bytes), (items, bytes)| {
                    (
                        total_items.saturating_add(items),
                        total_bytes.saturating_add(bytes),
                    )
                },
            )
        })
        .unwrap_or_default();
    let last_draw_micros = shared_metrics().last_draw_micros.load(Ordering::Relaxed);
    let atlas_upload_micros = shared_metrics().atlas_upload_micros.load(Ordering::Relaxed);
    let image_decode_micros = shared_metrics().image_decode_micros.load(Ordering::Relaxed);
    let image_decode_total_micros = shared_metrics()
        .image_decode_total_micros
        .load(Ordering::Relaxed);
    let image_decode_max_micros = shared_metrics()
        .image_decode_max_micros
        .load(Ordering::Relaxed);
    let (
        image_asset_retained_decoded_bytes,
        image_asset_retained_count,
        image_asset_largest_retained_decoded_bytes,
    ) = shared_metrics()
        .image_asset_retained
        .lock()
        .map(|retained| {
            retained.values().fold(
                (0usize, 0usize, 0usize),
                |(total, count, largest), record| {
                    (
                        total.saturating_add(record.retained_decoded_bytes),
                        count.saturating_add(1),
                        largest.max(record.retained_decoded_bytes),
                    )
                },
            )
        })
        .unwrap_or_default();
    let recent_image_decodes = shared_metrics()
        .recent_image_decodes
        .lock()
        .map(|records| records.iter().cloned().collect())
        .unwrap_or_default();
    let first_frame_build_micros = shared_metrics()
        .first_frame_build_micros
        .load(Ordering::Relaxed);
    let first_frame_layout_micros = shared_metrics()
        .first_frame_layout_micros
        .load(Ordering::Relaxed);
    let first_frame_prepaint_micros = shared_metrics()
        .first_frame_prepaint_micros
        .load(Ordering::Relaxed);
    let first_frame_paint_micros = shared_metrics()
        .first_frame_paint_micros
        .load(Ordering::Relaxed);
    let first_frame_scene_finish_micros = shared_metrics()
        .first_frame_scene_finish_micros
        .load(Ordering::Relaxed);
    let first_frame_backend_draw_micros = shared_metrics()
        .first_frame_backend_draw_micros
        .load(Ordering::Relaxed);
    let frame_build_micros = shared_metrics().frame_build_micros.load(Ordering::Relaxed);
    let frame_layout_micros = shared_metrics().frame_layout_micros.load(Ordering::Relaxed);
    let frame_prepaint_micros = shared_metrics()
        .frame_prepaint_micros
        .load(Ordering::Relaxed);
    let frame_paint_micros = shared_metrics().frame_paint_micros.load(Ordering::Relaxed);
    let frame_scene_finish_micros = shared_metrics()
        .frame_scene_finish_micros
        .load(Ordering::Relaxed);
    let frame_backend_draw_micros = shared_metrics()
        .frame_backend_draw_micros
        .load(Ordering::Relaxed);
    let idle_sleep_micros = shared_metrics().idle_sleep_micros.load(Ordering::Relaxed);
    let gpu_surface_format = shared_metrics()
        .gpu_surface_format
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let gpu_surface_alpha_mode = shared_metrics()
        .gpu_surface_alpha_mode
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let gpu_surface_present_mode = shared_metrics()
        .gpu_surface_present_mode
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let window_metrics = shared_metrics()
        .window_metrics
        .lock()
        .map(|window_metrics| {
            window_metrics
                .iter()
                .map(|(&window_id, metrics)| WindowMetricsSnapshot {
                    window_id,
                    request_redraw_count: metrics.request_redraw_count as usize,
                    draw_count: metrics.draw_count as usize,
                    present_count: metrics.present_count as usize,
                    skip_count: metrics.skip_count as usize,
                    skipped_frame_count: metrics.skipped_frame_count as usize,
                    gpu_surface_reconfigure_count: metrics.gpu_surface_reconfigure_count as usize,
                    gpu_surface_error_count: metrics.gpu_surface_error_count as usize,
                    layout_recompute_count: metrics.layout_recompute_count as usize,
                    upload_bytes: metrics.upload_bytes as usize,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let gpu_retained_bytes = shared_metrics().gpu_retained_bytes.load(Ordering::Relaxed) as usize;
    let atlas_retained_bytes = shared_metrics()
        .atlas_retained_bytes
        .load(Ordering::Relaxed) as usize;
    let atlas_monochrome_bytes = shared_metrics()
        .atlas_monochrome_bytes
        .load(Ordering::Relaxed) as usize;
    let atlas_polychrome_bytes = shared_metrics()
        .atlas_polychrome_bytes
        .load(Ordering::Relaxed) as usize;
    let atlas_live_keys = shared_metrics().atlas_live_keys.load(Ordering::Relaxed) as usize;
    let atlas_unused_bytes = shared_metrics().atlas_unused_bytes.load(Ordering::Relaxed) as usize;
    let upload_arena_uniform_capacity = shared_metrics()
        .upload_arena_uniform_capacity
        .load(Ordering::Relaxed) as usize;
    let upload_arena_storage_capacity = shared_metrics()
        .upload_arena_storage_capacity
        .load(Ordering::Relaxed) as usize;
    let hal_texture_memory_bytes = shared_metrics()
        .hal_texture_memory_bytes
        .load(Ordering::Relaxed) as usize;
    let gpui_image_asset_total_retained_decoded_bytes =
        image_cache_bytes.saturating_add(image_asset_retained_decoded_bytes);
    let gpui_render_image_cpu_bytes = shared_metrics()
        .render_image_live_decoded_bytes
        .load(Ordering::Relaxed) as usize;
    let gpui_atlas_retained_bytes = atlas_monochrome_bytes
        .saturating_add(atlas_polychrome_bytes)
        .max(atlas_retained_bytes);
    let gpui_gpu_surface_texture_bytes = gpu_retained_bytes
        .saturating_sub(gpui_atlas_retained_bytes)
        .saturating_sub(upload_arena_uniform_capacity)
        .saturating_sub(upload_arena_storage_capacity);
    let gpui_gpu_estimated_total_retained_bytes = gpui_render_image_cpu_bytes
        .max(gpui_image_asset_total_retained_decoded_bytes)
        .saturating_add(gpui_atlas_retained_bytes)
        .saturating_add(gpui_gpu_surface_texture_bytes)
        .saturating_add(upload_arena_uniform_capacity)
        .saturating_add(upload_arena_storage_capacity);

    PerformanceMetricsSnapshot {
        renderer_backend,
        image_cache_items,
        image_cache_bytes,
        atlas_textures: shared_metrics().atlas_textures.load(Ordering::Relaxed) as usize,
        last_draw_time: (last_draw_micros > 0).then(|| Duration::from_micros(last_draw_micros)),
        present_fps: shared_metrics().present_fps_milli.load(Ordering::Relaxed) as f32 / 1000.0,
        atlas_upload_bytes: shared_metrics().atlas_upload_bytes.load(Ordering::Relaxed) as usize,
        atlas_upload_tiles: shared_metrics().atlas_upload_tiles.load(Ordering::Relaxed) as usize,
        prepared_command_count: shared_metrics()
            .prepared_command_count
            .load(Ordering::Relaxed) as usize,
        gpu_surface_format,
        gpu_surface_alpha_mode,
        gpu_surface_present_mode,
        upload_bytes: shared_metrics().upload_bytes.load(Ordering::Relaxed) as usize,
        mask_pass_count: shared_metrics().mask_pass_count.load(Ordering::Relaxed) as usize,
        main_pass_count: shared_metrics().main_pass_count.load(Ordering::Relaxed) as usize,
        composite_pass_count: shared_metrics()
            .composite_pass_count
            .load(Ordering::Relaxed) as usize,
        gpu_surface_reconfigure_count: shared_metrics()
            .gpu_surface_reconfigure_count
            .load(Ordering::Relaxed) as usize,
        gpu_surface_error_count: shared_metrics()
            .gpu_surface_error_count
            .load(Ordering::Relaxed) as usize,
        retained_present_count: shared_metrics()
            .retained_present_count
            .load(Ordering::Relaxed) as usize,
        atlas_upload_time: (atlas_upload_micros > 0)
            .then(|| Duration::from_micros(atlas_upload_micros)),
        last_image_decode_compressed_bytes: shared_metrics()
            .image_decode_compressed_bytes
            .load(Ordering::Relaxed) as usize,
        last_image_decode_decoded_bytes: shared_metrics()
            .image_decode_decoded_bytes
            .load(Ordering::Relaxed) as usize,
        last_image_decode_frames: shared_metrics().image_decode_frames.load(Ordering::Relaxed)
            as usize,
        last_image_decode_time: (image_decode_micros > 0)
            .then(|| Duration::from_micros(image_decode_micros)),
        image_decode_count: shared_metrics().image_decode_count.load(Ordering::Relaxed) as usize,
        image_decode_compressed_bytes: shared_metrics()
            .image_decode_total_compressed_bytes
            .load(Ordering::Relaxed) as usize,
        image_decode_decoded_bytes: shared_metrics()
            .image_decode_total_decoded_bytes
            .load(Ordering::Relaxed) as usize,
        image_decode_frames: shared_metrics()
            .image_decode_total_frames
            .load(Ordering::Relaxed) as usize,
        image_decode_total_time: (image_decode_total_micros > 0)
            .then(|| Duration::from_micros(image_decode_total_micros)),
        image_decode_max_time: (image_decode_max_micros > 0)
            .then(|| Duration::from_micros(image_decode_max_micros)),
        image_decode_slow_count: shared_metrics()
            .image_decode_slow_count
            .load(Ordering::Relaxed) as usize,
        image_asset_retained_decoded_bytes,
        image_asset_retained_count,
        image_asset_largest_retained_decoded_bytes,
        gpui_image_asset_cache_entries: image_cache_items
            .saturating_add(image_asset_retained_count),
        gpui_image_asset_retained_compressed_bytes: 0,
        gpui_image_asset_total_retained_decoded_bytes,
        gpui_render_image_cpu_bytes,
        gpui_render_image_gpu_texture_bytes: gpui_atlas_retained_bytes
            .max(hal_texture_memory_bytes),
        gpui_icon_cache_entries: 0,
        gpui_icon_cache_decoded_bytes: 0,
        gpui_atlas_monochrome_bytes: atlas_monochrome_bytes,
        gpui_atlas_polychrome_bytes: atlas_polychrome_bytes
            .max(atlas_retained_bytes.saturating_sub(atlas_monochrome_bytes)),
        gpui_atlas_live_keys: atlas_live_keys,
        gpui_atlas_unused_bytes: atlas_unused_bytes,
        gpui_gpu_surface_texture_bytes,
        gpui_gpu_estimated_total_retained_bytes,
        recent_image_decodes,
        first_frame_build_time: (first_frame_build_micros > 0)
            .then(|| Duration::from_micros(first_frame_build_micros)),
        first_frame_layout_time: (first_frame_layout_micros > 0)
            .then(|| Duration::from_micros(first_frame_layout_micros)),
        first_frame_prepaint_time: (first_frame_prepaint_micros > 0)
            .then(|| Duration::from_micros(first_frame_prepaint_micros)),
        first_frame_paint_time: (first_frame_paint_micros > 0)
            .then(|| Duration::from_micros(first_frame_paint_micros)),
        first_frame_scene_finish_time: (first_frame_scene_finish_micros > 0)
            .then(|| Duration::from_micros(first_frame_scene_finish_micros)),
        first_frame_backend_draw_time: (first_frame_backend_draw_micros > 0)
            .then(|| Duration::from_micros(first_frame_backend_draw_micros)),
        frame_build_time: (frame_build_micros > 0)
            .then(|| Duration::from_micros(frame_build_micros)),
        frame_layout_time: (frame_layout_micros > 0)
            .then(|| Duration::from_micros(frame_layout_micros)),
        frame_prepaint_time: (frame_prepaint_micros > 0)
            .then(|| Duration::from_micros(frame_prepaint_micros)),
        frame_paint_time: (frame_paint_micros > 0)
            .then(|| Duration::from_micros(frame_paint_micros)),
        frame_scene_finish_time: (frame_scene_finish_micros > 0)
            .then(|| Duration::from_micros(frame_scene_finish_micros)),
        frame_backend_draw_time: (frame_backend_draw_micros > 0)
            .then(|| Duration::from_micros(frame_backend_draw_micros)),
        layout_nodes: shared_metrics().layout_nodes.load(Ordering::Relaxed) as usize,
        measured_layout_nodes: shared_metrics()
            .measured_layout_nodes
            .load(Ordering::Relaxed) as usize,
        layout_roots: shared_metrics().layout_roots.load(Ordering::Relaxed) as usize,
        layout_bounds_cache_hits: shared_metrics()
            .layout_bounds_cache_hits
            .load(Ordering::Relaxed) as usize,
        layout_bounds_cache_misses: shared_metrics()
            .layout_bounds_cache_misses
            .load(Ordering::Relaxed) as usize,
        scene_primitives: shared_metrics().scene_primitives.load(Ordering::Relaxed) as usize,
        scene_batches: shared_metrics().scene_batches.load(Ordering::Relaxed) as usize,
        scene_segments: shared_metrics().scene_segments.load(Ordering::Relaxed) as usize,
        scene_replayed_primitives: shared_metrics()
            .scene_replayed_primitives
            .load(Ordering::Relaxed) as usize,
        scene_segment_rebuild_count: shared_metrics()
            .scene_segment_rebuild_count
            .load(Ordering::Relaxed) as usize,
        scene_segment_reuse_count: shared_metrics()
            .scene_segment_reuse_count
            .load(Ordering::Relaxed) as usize,
        dirty_transform_count: shared_metrics()
            .dirty_transform_count
            .load(Ordering::Relaxed) as usize,
        retained_frame_skips: shared_metrics()
            .retained_frame_skips
            .load(Ordering::Relaxed) as usize,
        skipped_pointer_frame_count: shared_metrics()
            .skipped_pointer_frame_count
            .load(Ordering::Relaxed) as usize,
        dirty_rect_count: shared_metrics().dirty_rect_count.load(Ordering::Relaxed) as usize,
        dirty_rect_area: shared_metrics().dirty_rect_area.load(Ordering::Relaxed) as usize,
        partial_redraw_count: shared_metrics()
            .partial_redraw_count
            .load(Ordering::Relaxed) as usize,
        full_redraw_fallback_count: shared_metrics()
            .full_redraw_fallback_count
            .load(Ordering::Relaxed) as usize,
        gpu_retained_bytes,
        atlas_retained_bytes,
        has_retained_frame_target: shared_metrics()
            .has_retained_frame_target
            .load(Ordering::Relaxed)
            != 0,
        has_path_textures: shared_metrics().has_path_textures.load(Ordering::Relaxed) != 0,
        has_backdrop_texture: shared_metrics()
            .has_backdrop_texture
            .load(Ordering::Relaxed)
            != 0,
        has_depth_texture: shared_metrics().has_depth_texture.load(Ordering::Relaxed) != 0,
        backdrop_blur_target_groups: shared_metrics()
            .backdrop_blur_target_groups
            .load(Ordering::Relaxed) as usize,
        gpu_mesh_buffers: shared_metrics().gpu_mesh_buffers.load(Ordering::Relaxed) as usize,
        gpu_adapter_name,
        gpu_adapter_type,
        coalesced_refresh_count: shared_metrics()
            .coalesced_refresh_count
            .load(Ordering::Relaxed) as usize,
        coalesced_refresh_effect_count: shared_metrics()
            .coalesced_refresh_effect_count
            .load(Ordering::Relaxed) as usize,
        inactive_present_skip_count: shared_metrics()
            .inactive_present_skip_count
            .load(Ordering::Relaxed) as usize,
        inactive_dirty_defer_count: shared_metrics()
            .inactive_dirty_defer_count
            .load(Ordering::Relaxed) as usize,
        layout_cache_hits: shared_metrics().layout_cache_hits.load(Ordering::Relaxed) as usize,
        layout_cache_misses: shared_metrics().layout_cache_misses.load(Ordering::Relaxed) as usize,
        layout_cache_reused_roots: shared_metrics()
            .layout_cache_reused_roots
            .load(Ordering::Relaxed) as usize,
        layout_cache_saved_roots: shared_metrics()
            .layout_cache_saved_roots
            .load(Ordering::Relaxed) as usize,
        layout_persistent_node_reuses: shared_metrics()
            .layout_persistent_node_reuses
            .load(Ordering::Relaxed) as usize,
        layout_persistent_node_creations: shared_metrics()
            .layout_persistent_node_creations
            .load(Ordering::Relaxed) as usize,
        layout_persistent_node_removals: shared_metrics()
            .layout_persistent_node_removals
            .load(Ordering::Relaxed) as usize,
        view_cache_prepaint_hits: shared_metrics()
            .view_cache_prepaint_hits
            .load(Ordering::Relaxed) as usize,
        view_cache_prepaint_misses: shared_metrics()
            .view_cache_prepaint_misses
            .load(Ordering::Relaxed) as usize,
        view_cache_paint_hits: shared_metrics()
            .view_cache_paint_hits
            .load(Ordering::Relaxed) as usize,
        view_cache_paint_misses: shared_metrics()
            .view_cache_paint_misses
            .load(Ordering::Relaxed) as usize,
        draw_budget_miss_count: shared_metrics()
            .draw_budget_miss_count
            .load(Ordering::Relaxed) as usize,
        draw_degrade_count: shared_metrics().draw_degrade_count.load(Ordering::Relaxed) as usize,
        foreground_effect_yield_count: shared_metrics()
            .foreground_effect_yield_count
            .load(Ordering::Relaxed) as usize,
        foreground_effect_pending_max: shared_metrics()
            .foreground_effect_pending_max
            .load(Ordering::Relaxed) as usize,
        background_executor_queue_depth: shared_metrics()
            .background_executor_queue_depth
            .load(Ordering::Relaxed) as usize,
        background_executor_queue_depth_max: shared_metrics()
            .background_executor_queue_depth_max
            .load(Ordering::Relaxed) as usize,
        text_layout_hits: shared_metrics().text_layout_hits.load(Ordering::Relaxed) as usize,
        text_layout_reuses: shared_metrics().text_layout_reuses.load(Ordering::Relaxed) as usize,
        text_layout_misses: shared_metrics().text_layout_misses.load(Ordering::Relaxed) as usize,
        text_background_warmups: shared_metrics()
            .text_background_warmups
            .load(Ordering::Relaxed) as usize,
        text_background_layouts: shared_metrics()
            .text_background_layouts
            .load(Ordering::Relaxed) as usize,
        image_cache_evictions: shared_metrics()
            .image_cache_evictions
            .load(Ordering::Relaxed) as usize,
        image_drop_count: shared_metrics().image_drop_count.load(Ordering::Relaxed) as usize,
        atlas_remove_count: shared_metrics().atlas_remove_count.load(Ordering::Relaxed) as usize,
        pod_upload_bytes: shared_metrics().pod_upload_bytes.load(Ordering::Relaxed) as usize,
        scheduler_wakeups: shared_metrics().scheduler_wakeups.load(Ordering::Relaxed) as usize,
        idle_sleep_time: (idle_sleep_micros > 0).then(|| Duration::from_micros(idle_sleep_micros)),
        frame_request_count: shared_metrics().frame_request_count.load(Ordering::Relaxed) as usize,
        draw_count: shared_metrics().draw_count.load(Ordering::Relaxed) as usize,
        present_count: shared_metrics().present_count.load(Ordering::Relaxed) as usize,
        skip_count: shared_metrics().skip_count.load(Ordering::Relaxed) as usize,
        bind_group_creations: shared_metrics()
            .bind_group_creations
            .load(Ordering::Relaxed) as usize,
        bind_group_cache_hits: shared_metrics()
            .bind_group_cache_hits
            .load(Ordering::Relaxed) as usize,
        bind_group_cache_misses: shared_metrics()
            .bind_group_cache_misses
            .load(Ordering::Relaxed) as usize,
        upload_arena_uniform_capacity,
        upload_arena_storage_capacity,
        upload_arena_uniform_used: shared_metrics()
            .upload_arena_uniform_used
            .load(Ordering::Relaxed) as usize,
        upload_arena_storage_used: shared_metrics()
            .upload_arena_storage_used
            .load(Ordering::Relaxed) as usize,
        gpu_cache_hits: shared_metrics().gpu_cache_hits.load(Ordering::Relaxed) as usize,
        gpu_cache_misses: shared_metrics().gpu_cache_misses.load(Ordering::Relaxed) as usize,
        scene_retained_capacity: shared_metrics()
            .scene_retained_capacity
            .load(Ordering::Relaxed) as usize,
        frame_retained_capacity: shared_metrics()
            .frame_retained_capacity
            .load(Ordering::Relaxed) as usize,
        allocator_allocated_bytes: shared_metrics()
            .allocator_allocated_bytes
            .load(Ordering::Relaxed) as usize,
        allocator_reserved_bytes: shared_metrics()
            .allocator_reserved_bytes
            .load(Ordering::Relaxed) as usize,
        allocator_block_count: shared_metrics()
            .allocator_block_count
            .load(Ordering::Relaxed) as usize,
        allocator_allocation_count: shared_metrics()
            .allocator_allocation_count
            .load(Ordering::Relaxed) as usize,
        allocator_gpu_only: AllocatorBucketMetricsSnapshot {
            allocated_bytes: shared_metrics()
                .allocator_gpu_only_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            reserved_bytes: shared_metrics()
                .allocator_gpu_only_reserved_bytes
                .load(Ordering::Relaxed) as usize,
            block_count: shared_metrics()
                .allocator_gpu_only_block_count
                .load(Ordering::Relaxed) as usize,
            committed_allocated_bytes: shared_metrics()
                .allocator_gpu_only_committed_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            committed_allocation_count: shared_metrics()
                .allocator_gpu_only_committed_allocation_count
                .load(Ordering::Relaxed) as usize,
        },
        allocator_cpu_to_gpu: AllocatorBucketMetricsSnapshot {
            allocated_bytes: shared_metrics()
                .allocator_cpu_to_gpu_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            reserved_bytes: shared_metrics()
                .allocator_cpu_to_gpu_reserved_bytes
                .load(Ordering::Relaxed) as usize,
            block_count: shared_metrics()
                .allocator_cpu_to_gpu_block_count
                .load(Ordering::Relaxed) as usize,
            committed_allocated_bytes: shared_metrics()
                .allocator_cpu_to_gpu_committed_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            committed_allocation_count: shared_metrics()
                .allocator_cpu_to_gpu_committed_allocation_count
                .load(Ordering::Relaxed) as usize,
        },
        allocator_gpu_to_cpu: AllocatorBucketMetricsSnapshot {
            allocated_bytes: shared_metrics()
                .allocator_gpu_to_cpu_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            reserved_bytes: shared_metrics()
                .allocator_gpu_to_cpu_reserved_bytes
                .load(Ordering::Relaxed) as usize,
            block_count: shared_metrics()
                .allocator_gpu_to_cpu_block_count
                .load(Ordering::Relaxed) as usize,
            committed_allocated_bytes: shared_metrics()
                .allocator_gpu_to_cpu_committed_allocated_bytes
                .load(Ordering::Relaxed) as usize,
            committed_allocation_count: shared_metrics()
                .allocator_gpu_to_cpu_committed_allocation_count
                .load(Ordering::Relaxed) as usize,
        },
        hal_buffer_memory_bytes: shared_metrics()
            .hal_buffer_memory_bytes
            .load(Ordering::Relaxed) as usize,
        hal_texture_memory_bytes,
        hal_acceleration_structure_memory_bytes: shared_metrics()
            .hal_acceleration_structure_memory_bytes
            .load(Ordering::Relaxed) as usize,
        hal_memory_allocation_count: shared_metrics()
            .hal_memory_allocation_count
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_live_bytes: shared_metrics()
            .core_staging_buffer_live_bytes
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_peak_live_bytes: shared_metrics()
            .core_staging_buffer_peak_live_bytes
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_created_bytes: shared_metrics()
            .core_staging_buffer_created_bytes
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_pending_bytes: shared_metrics()
            .core_staging_buffer_pending_bytes
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_peak_pending_bytes: shared_metrics()
            .core_staging_buffer_peak_pending_bytes
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_live_count: shared_metrics()
            .core_staging_buffer_live_count
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_peak_live_count: shared_metrics()
            .core_staging_buffer_peak_live_count
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_pending_count: shared_metrics()
            .core_staging_buffer_pending_count
            .load(Ordering::Relaxed) as usize,
        core_staging_buffer_peak_pending_count: shared_metrics()
            .core_staging_buffer_peak_pending_count
            .load(Ordering::Relaxed) as usize,
        window_metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_aggregate_image_decode_metrics() {
        let before = performance_metrics_snapshot();

        record_image_decode_metrics_with_threshold(
            10,
            20,
            1,
            Duration::from_millis(2),
            Duration::from_millis(1),
        );
        record_image_decode_metrics_with_threshold(
            30,
            40,
            2,
            Duration::from_millis(3),
            Duration::from_millis(10),
        );

        let snapshot = performance_metrics_snapshot();
        assert!(snapshot.image_decode_count >= before.image_decode_count + 2);
        assert!(
            snapshot.image_decode_compressed_bytes >= before.image_decode_compressed_bytes + 40
        );
        assert!(snapshot.image_decode_decoded_bytes >= before.image_decode_decoded_bytes + 60);
        assert!(snapshot.image_decode_frames >= before.image_decode_frames + 3);
        assert!(
            snapshot
                .image_decode_total_time
                .zip(before.image_decode_total_time)
                .map_or(true, |(after, before)| after
                    >= before + Duration::from_millis(5))
        );
        assert!(
            snapshot
                .image_decode_max_time
                .is_some_and(|duration| duration >= Duration::from_millis(3))
        );
        assert!(snapshot.image_decode_slow_count > before.image_decode_slow_count);
    }

    #[test]
    fn records_retained_image_asset_metrics() {
        let key = 0xA55E7;
        drop_image_asset_retained(key);

        record_image_asset_retained(
            key,
            ImageDecodeRecord {
                source: "images/background.webp".to_string(),
                original_width: 3840,
                original_height: 2160,
                target_width: 960,
                target_height: 540,
                retained_decoded_bytes: 2_073_600,
                decode_mode: "webp_scaled".to_string(),
            },
        );

        let snapshot = performance_metrics_snapshot();
        assert!(snapshot.image_asset_retained_count >= 1);
        assert!(snapshot.image_asset_retained_decoded_bytes >= 2_073_600);
        assert!(snapshot.image_asset_largest_retained_decoded_bytes >= 2_073_600);
        assert!(snapshot.recent_image_decodes.iter().any(|record| {
            record.source == "images/background.webp" && record.target_width == 960
        }));

        drop_image_asset_retained(key);
    }

    #[test]
    fn records_extended_gpu_metrics() {
        reset_frame_upload_metrics();
        record_atlas_upload_metrics(64, 1, Duration::from_micros(2));
        record_atlas_upload_metrics(32, 2, Duration::from_micros(3));
        record_prepared_command_count(32);
        record_upload_bytes(1024);
        record_upload_bytes(256);
        record_pod_upload_bytes(512);
        record_pod_upload_bytes(128);
        record_bind_group_creations(3);
        record_bind_group_cache_metrics(5, 2);
        record_upload_arena_metrics(65_536, 1_048_576, 768, 2048);
        record_gpu_cache_metrics(7, 1);
        record_gpu_pass_metrics(1, 2, 1);
        record_gpu_surface_metrics("Bgra8Unorm", "PreMultiplied", "Mailbox", 3, 2);
        record_dirty_region_metrics(2, 4096);
        record_partial_redraw();
        record_full_redraw_fallback();
        record_gpu_retained_bytes(123_456);

        let snapshot = performance_metrics_snapshot();
        assert_eq!(snapshot.prepared_command_count, 32);
        assert_eq!(snapshot.gpu_surface_format, "Bgra8Unorm");
        assert_eq!(snapshot.gpu_surface_alpha_mode, "PreMultiplied");
        assert_eq!(snapshot.gpu_surface_present_mode, "Mailbox");
        assert_eq!(snapshot.atlas_upload_bytes, 96);
        assert_eq!(snapshot.atlas_upload_tiles, 3);
        assert_eq!(snapshot.atlas_upload_time, Some(Duration::from_micros(5)));
        assert_eq!(snapshot.upload_bytes, 1280);
        assert_eq!(snapshot.pod_upload_bytes, 640);
        assert_eq!(snapshot.bind_group_creations, 3);
        assert_eq!(snapshot.bind_group_cache_hits, 5);
        assert_eq!(snapshot.bind_group_cache_misses, 2);
        assert_eq!(snapshot.upload_arena_uniform_capacity, 65_536);
        assert_eq!(snapshot.upload_arena_storage_capacity, 1_048_576);
        assert_eq!(snapshot.upload_arena_uniform_used, 768);
        assert_eq!(snapshot.upload_arena_storage_used, 2048);
        assert_eq!(snapshot.gpu_cache_hits, 7);
        assert_eq!(snapshot.gpu_cache_misses, 1);
        assert_eq!(snapshot.mask_pass_count, 1);
        assert_eq!(snapshot.main_pass_count, 2);
        assert_eq!(snapshot.composite_pass_count, 1);
        assert_eq!(snapshot.gpu_surface_reconfigure_count, 3);
        assert_eq!(snapshot.gpu_surface_error_count, 2);
        assert_eq!(snapshot.dirty_rect_count, 2);
        assert_eq!(snapshot.dirty_rect_area, 4096);
        assert!(snapshot.partial_redraw_count > 0);
        assert!(snapshot.full_redraw_fallback_count > 0);
        assert_eq!(snapshot.gpu_retained_bytes, 123_456);
    }

    #[test]
    fn records_scene_and_refresh_metrics() {
        record_scene_frame_metrics(SceneFrameMetrics {
            primitives: 7,
            batches: 3,
            segments: 2,
            replayed_primitives: 5,
            segment_rebuild_count: 1,
            segment_reuse_count: 4,
            retained_capacity: 0,
        });
        let before = performance_metrics_snapshot().coalesced_refresh_count;
        record_coalesced_refresh();
        let before_effects = performance_metrics_snapshot().coalesced_refresh_effect_count;
        record_coalesced_refresh_effect();
        record_inactive_present_skip();
        record_inactive_dirty_defer();
        let before_pointer_skips = performance_metrics_snapshot().skipped_pointer_frame_count;
        record_skipped_pointer_frame();
        record_layout_frame_metrics(LayoutFrameMetrics {
            persistent_node_reuses: 5,
            persistent_node_creations: 2,
            persistent_node_removals: 1,
            ..LayoutFrameMetrics::default()
        });
        record_layout_cache_metrics(8, 2);
        let before_view_prepaint_hits = performance_metrics_snapshot().view_cache_prepaint_hits;
        let before_view_prepaint_misses = performance_metrics_snapshot().view_cache_prepaint_misses;
        let before_view_paint_hits = performance_metrics_snapshot().view_cache_paint_hits;
        let before_view_paint_misses = performance_metrics_snapshot().view_cache_paint_misses;
        record_view_cache_prepaint_hit(true);
        record_view_cache_prepaint_hit(false);
        record_view_cache_paint_hit(true);
        record_view_cache_paint_hit(false);
        let before_draw_budget_misses = performance_metrics_snapshot().draw_budget_miss_count;
        let before_draw_degrades = performance_metrics_snapshot().draw_degrade_count;
        record_draw_budget_miss();
        record_draw_degrade();
        let before_effect_yields = performance_metrics_snapshot().foreground_effect_yield_count;
        record_foreground_effect_yield(7);
        record_text_layout_cache_metrics(3, 4, 5);
        let before_text_warmups = performance_metrics_snapshot().text_background_warmups;
        let before_text_warmup_layouts = performance_metrics_snapshot().text_background_layouts;
        record_text_background_warmup(6);
        record_image_cache_eviction(2);
        record_image_drop(1);
        record_atlas_remove(6);
        reset_frame_upload_metrics();
        record_pod_upload_bytes(4096);

        let snapshot = performance_metrics_snapshot();
        assert_eq!(snapshot.scene_primitives, 7);
        assert_eq!(snapshot.scene_batches, 3);
        assert_eq!(snapshot.scene_segments, 2);
        assert_eq!(snapshot.scene_replayed_primitives, 5);
        assert_eq!(snapshot.scene_segment_rebuild_count, 1);
        assert_eq!(snapshot.scene_segment_reuse_count, 4);
        assert_eq!(snapshot.coalesced_refresh_count, before + 1);
        assert_eq!(snapshot.coalesced_refresh_effect_count, before_effects + 1);
        assert_eq!(
            snapshot.skipped_pointer_frame_count,
            before_pointer_skips + 1
        );
        assert!(snapshot.inactive_present_skip_count > 0);
        assert!(snapshot.inactive_dirty_defer_count > 0);
        assert_eq!(snapshot.layout_cache_hits, 8);
        assert_eq!(snapshot.layout_cache_misses, 2);
        assert_eq!(snapshot.layout_persistent_node_reuses, 5);
        assert_eq!(snapshot.layout_persistent_node_creations, 2);
        assert_eq!(snapshot.layout_persistent_node_removals, 1);
        assert_eq!(
            snapshot.view_cache_prepaint_hits,
            before_view_prepaint_hits + 1
        );
        assert_eq!(
            snapshot.view_cache_prepaint_misses,
            before_view_prepaint_misses + 1
        );
        assert_eq!(snapshot.view_cache_paint_hits, before_view_paint_hits + 1);
        assert_eq!(
            snapshot.view_cache_paint_misses,
            before_view_paint_misses + 1
        );
        assert_eq!(
            snapshot.draw_budget_miss_count,
            before_draw_budget_misses + 1
        );
        assert_eq!(snapshot.draw_degrade_count, before_draw_degrades + 1);
        assert_eq!(
            snapshot.foreground_effect_yield_count,
            before_effect_yields + 1
        );
        assert!(snapshot.foreground_effect_pending_max >= 7);
        assert_eq!(snapshot.text_layout_hits, 3);
        assert_eq!(snapshot.text_layout_reuses, 4);
        assert_eq!(snapshot.text_layout_misses, 5);
        assert_eq!(snapshot.text_background_warmups, before_text_warmups + 1);
        assert_eq!(
            snapshot.text_background_layouts,
            before_text_warmup_layouts + 6
        );
        assert!(snapshot.image_cache_evictions >= 2);
        assert!(snapshot.image_drop_count >= 1);
        assert!(snapshot.atlas_remove_count >= 6);
        assert_eq!(snapshot.pod_upload_bytes, 4096);
    }

    #[test]
    fn records_window_scoped_metrics() {
        record_window_request_redraw(10);
        record_window_frame_result(10, true, true, false);
        record_window_frame_result(10, false, false, true);
        record_window_gpu_surface_metrics(10, 4, 1);
        record_window_layout_recompute(10);
        record_window_upload_bytes(10, 2048);

        let snapshot = performance_metrics_snapshot();
        let window = snapshot
            .window_metrics
            .into_iter()
            .find(|window| window.window_id == 10)
            .expect("window metrics should be tracked");

        assert!(window.request_redraw_count >= 1);
        assert!(window.draw_count >= 1);
        assert!(window.present_count >= 1);
        assert!(window.skip_count >= 1);
        assert!(window.skipped_frame_count >= 1);
        assert_eq!(window.gpu_surface_reconfigure_count, 4);
        assert_eq!(window.gpu_surface_error_count, 1);
        assert!(window.layout_recompute_count >= 1);
        assert_eq!(window.upload_bytes, 2048);
    }
}
