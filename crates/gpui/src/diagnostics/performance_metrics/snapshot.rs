use crate::RendererBackend;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{
    AllocatorBucketMetricsSnapshot, AnimationMetricsSnapshot, ImageRenderRecord,
    WindowMetricsSnapshot,
};

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
    /// Scene primitives encoded by the latest Nova renderer submission.
    pub encoded_scene_primitives: usize,
    /// Prepared scene batches encoded by the latest Nova renderer submission.
    pub encoded_scene_batches: usize,
    /// Bytes uploaded for quad instances by the latest Nova renderer submission.
    pub quad_upload_bytes: usize,
    /// Bytes uploaded for shadow instances by the latest Nova renderer submission.
    pub shadow_upload_bytes: usize,
    /// Bytes uploaded for path geometry and sprites by the latest Nova submission.
    pub path_upload_bytes: usize,
    /// Bytes uploaded for monochrome sprites by the latest Nova renderer submission.
    pub mono_sprite_upload_bytes: usize,
    /// Bytes uploaded for polychrome sprites by the latest Nova renderer submission.
    pub poly_sprite_upload_bytes: usize,
    /// Bytes uploaded for underline instances by the latest Nova renderer submission.
    pub underline_upload_bytes: usize,
    /// Bytes uploaded for backdrop blur descriptors by the latest Nova submission.
    pub backdrop_blur_upload_bytes: usize,
    /// Bytes uploaded for animation bindings and values by the latest Nova submission.
    pub animation_upload_bytes: usize,
    /// Bytes uploaded for custom mesh parameters by the latest Nova submission.
    pub custom_mesh_parameter_upload_bytes: usize,
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
    /// Number of presents that sampled the retained full-window target since process start.
    pub retained_present_count: usize,
    /// Number of frames presented directly to the swapchain since process start.
    pub direct_present_count: usize,
    /// Number of frames that submitted backdrop blur work since process start.
    pub backdrop_blur_frame_count: usize,
    /// Full-window pixels sampled by the latest retained present copy.
    pub retained_copy_pixels: usize,
    /// Estimated bytes read and written by the latest retained present copy.
    pub retained_copy_estimated_bytes: usize,
    /// Full drawable pixels captured by the latest backdrop blur source pass.
    pub backdrop_blur_source_pixels: usize,
    /// Aggregate pixels across active backdrop blur target levels in the latest frame.
    pub backdrop_blur_target_pixels: usize,
    /// Pixels in each active backdrop blur target level in the latest frame.
    pub backdrop_blur_level_pixels: [usize; 6],
    /// Backdrop blur primitives encoded by the latest renderer submission.
    pub backdrop_blur_primitives: usize,
    /// Time spent waiting for a GPU submission during the latest blocking wait.
    pub gpu_submission_wait_time: Option<Duration>,
    /// Number of blocking waits for GPU submissions since process start.
    pub gpu_submission_wait_count: usize,
    /// Aggregate time spent waiting for GPU submissions since process start.
    pub gpu_submission_wait_total_time: Option<Duration>,
    /// Longest GPU submission wait observed since process start.
    pub gpu_submission_wait_max_time: Option<Duration>,
    /// Number of GPU submission waits that exceeded the frame-time threshold.
    pub gpu_submission_slow_wait_count: usize,
    /// CPU time spent packing the latest scene into Nova upload buffers.
    pub scene_pack_time: Option<Duration>,
    /// Time spent queueing platform atlas upload commands during the latest reported frame.
    pub atlas_upload_time: Option<Duration>,
    /// Compressed bytes in the most recently decoded image.
    pub last_image_processing_compressed_bytes: usize,
    /// Decoded BGRA bytes in the most recently decoded image.
    pub last_image_processing_output_bytes: usize,
    /// Frame count in the most recently decoded image.
    pub last_image_processing_frames: usize,
    /// Processing time for the most recently decoded image.
    pub last_image_processing_time: Option<Duration>,
    /// Number of completed image processing operations recorded by GPUI and app decode helpers.
    pub image_processing_count: usize,
    /// Total compressed bytes processed by recorded image processing operations.
    pub image_processing_compressed_bytes: usize,
    /// Total decoded BGRA bytes produced by recorded image processing operations.
    pub image_processing_output_bytes: usize,
    /// Total decoded image frames produced by recorded image processing operations.
    pub image_processing_frames: usize,
    /// Total time spent in recorded image processing operations.
    pub image_processing_total_time: Option<Duration>,
    /// Maximum single recorded image processing time.
    pub image_processing_max_time: Option<Duration>,
    /// Number of recorded image processing operations that exceeded the supplied slow threshold.
    pub image_processing_slow_count: usize,
    /// Bounded animated-image streaming diagnostics.
    pub animation: AnimationMetricsSnapshot,
    /// Resident bytes currently retained by size-aware image assets.
    pub image_asset_resident_bytes: usize,
    /// Number of currently retained size-aware image asset variants.
    pub image_asset_retained_count: usize,
    /// Largest resident image currently retained by size-aware image assets.
    pub image_asset_largest_resident_bytes: usize,
    /// Number of entries retained by GPUI image asset caches that are visible globally.
    pub image_asset_cache_entries: usize,
    /// Compressed bytes retained by GPUI image assets.
    pub image_asset_compressed_bytes: usize,
    /// Resident image bytes retained by GPUI image assets and bounded image caches.
    pub image_asset_total_resident_bytes: usize,
    /// Estimated decoded bytes retained by render images in GPUI caches.
    pub render_image_cpu_bytes: usize,
    /// Estimated GPU texture bytes retained for render images.
    pub render_image_gpu_texture_bytes: usize,
    /// Number of entries retained by framework icon caches.
    pub icon_cache_entries: usize,
    /// Estimated decoded bytes retained by framework icon caches.
    pub icon_cache_resident_bytes: usize,
    /// Estimated bytes retained by monochrome atlas textures.
    pub atlas_monochrome_bytes: usize,
    /// Estimated bytes retained by color atlas textures.
    pub atlas_polychrome_bytes: usize,
    /// Number of live atlas keys known to renderer metrics.
    pub atlas_live_keys: usize,
    /// Estimated unused bytes inside retained atlas textures.
    pub atlas_unused_bytes: usize,
    /// Estimated bytes retained by window surface and retained-frame resources.
    pub gpu_surface_texture_bytes: usize,
    /// Aggregate GPUI-owned retained bytes visible to diagnostics.
    pub gpu_estimated_total_retained_bytes: usize,
    /// Recent size-aware image processing records.
    pub recent_image_processings: Vec<ImageRenderRecord>,
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
    /// Number of element nodes requested during layout in the latest profiled frame.
    pub element_count: usize,
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
    /// Number of retained layout cache hits in the latest profiled frame.
    pub layout_cache_hits: usize,
    /// Number of retained layout cache misses in the latest profiled frame.
    pub layout_cache_misses: usize,
    /// Number of retained layout roots directly reused in the latest profiled frame.
    pub layout_cache_reused_roots: usize,
    /// Number of layout roots whose retained traversal avoided rebuilding child roots.
    pub layout_cache_saved_roots: usize,
    /// Number of same-frame text layout cache hits in the latest profiled frame.
    pub text_layout_hits: usize,
    /// Number of previous-frame text layouts reused in the latest profiled frame.
    pub text_layout_reuses: usize,
    /// Number of text layout misses in the latest profiled frame.
    pub text_layout_misses: usize,
    /// Number of style refinement applications performed since process start.
    pub style_refine_count: usize,
    /// Number of Style/LayoutStyle to Taffy style conversions performed since process start.
    pub layout_conversion_count: usize,
    /// Number of times the GPUI arena had to allocate a new chunk.
    pub arena_chunk_expansion_count: usize,
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
    /// Number of gpu bind groups created during the latest renderer submission.
    pub bind_group_creations: usize,
    /// Number of gpu bind group cache hits during the latest renderer submission.
    pub bind_group_cache_hits: usize,
    /// Number of gpu bind group cache misses during the latest renderer submission.
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
    /// Live bytes currently retained by flushed gpu-core staging buffers.
    pub core_staging_buffer_live_bytes: usize,
    /// Peak live bytes retained by flushed gpu-core staging buffers.
    pub core_staging_buffer_peak_live_bytes: usize,
    /// Cumulative bytes created through gpu-core staging buffers.
    pub core_staging_buffer_created_bytes: usize,
    /// Bytes currently pending in gpu-core `PendingWrites` staging resources.
    pub core_staging_buffer_pending_bytes: usize,
    /// Peak pending bytes observed in gpu-core `PendingWrites`.
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

/// Global wrapper for [`PerformanceMetricsSnapshot`].
#[derive(Clone, Debug, Default)]
pub struct PerformanceMetrics(pub PerformanceMetricsSnapshot);
