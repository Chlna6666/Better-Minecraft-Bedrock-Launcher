use crate::RendererBackend;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::state::shared_metrics;
use super::{AllocatorBucketMetricsSnapshot, PerformanceMetricsSnapshot, WindowMetricsSnapshot};

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
        gpu_surface_format: gpu_surface_format,
        gpu_surface_alpha_mode: gpu_surface_alpha_mode,
        gpu_surface_present_mode: gpu_surface_present_mode,
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
        gpui_image_asset_total_retained_decoded_bytes: image_cache_bytes
            .saturating_add(image_asset_retained_decoded_bytes),
        gpui_render_image_cpu_bytes: image_cache_bytes,
        gpui_render_image_gpu_texture_bytes: shared_metrics()
            .atlas_retained_bytes
            .load(Ordering::Relaxed) as usize,
        gpui_icon_cache_entries: 0,
        gpui_icon_cache_decoded_bytes: 0,
        gpui_atlas_monochrome_bytes: 0,
        gpui_atlas_polychrome_bytes: shared_metrics()
            .atlas_retained_bytes
            .load(Ordering::Relaxed) as usize,
        gpui_atlas_live_keys: 0,
        gpui_atlas_unused_bytes: 0,
        gpui_gpu_surface_texture_bytes: shared_metrics()
            .gpu_retained_bytes
            .load(Ordering::Relaxed)
            .saturating_sub(
                shared_metrics()
                    .atlas_retained_bytes
                    .load(Ordering::Relaxed),
            ) as usize,
        gpui_gpu_estimated_total_retained_bytes: image_cache_bytes
            .saturating_add(image_asset_retained_decoded_bytes)
            .saturating_add(shared_metrics().gpu_retained_bytes.load(Ordering::Relaxed) as usize),
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
        element_count: shared_metrics().layout_nodes.load(Ordering::Relaxed) as usize,
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
        gpu_retained_bytes: shared_metrics().gpu_retained_bytes.load(Ordering::Relaxed) as usize,
        atlas_retained_bytes: shared_metrics()
            .atlas_retained_bytes
            .load(Ordering::Relaxed) as usize,
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
        layout_cache_hits: shared_metrics().layout_cache_hits.load(Ordering::Relaxed) as usize,
        layout_cache_misses: shared_metrics().layout_cache_misses.load(Ordering::Relaxed) as usize,
        layout_cache_reused_roots: shared_metrics()
            .layout_cache_reused_roots
            .load(Ordering::Relaxed) as usize,
        layout_cache_saved_roots: shared_metrics()
            .layout_cache_saved_roots
            .load(Ordering::Relaxed) as usize,
        text_layout_hits: shared_metrics().text_layout_hits.load(Ordering::Relaxed) as usize,
        text_layout_reuses: shared_metrics().text_layout_reuses.load(Ordering::Relaxed) as usize,
        text_layout_misses: shared_metrics().text_layout_misses.load(Ordering::Relaxed) as usize,
        style_refine_count: shared_metrics().style_refine_count.load(Ordering::Relaxed) as usize,
        layout_conversion_count: shared_metrics()
            .layout_conversion_count
            .load(Ordering::Relaxed) as usize,
        arena_chunk_expansion_count: shared_metrics()
            .arena_chunk_expansion_count
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
        upload_arena_uniform_capacity: shared_metrics()
            .upload_arena_uniform_capacity
            .load(Ordering::Relaxed) as usize,
        upload_arena_storage_capacity: shared_metrics()
            .upload_arena_storage_capacity
            .load(Ordering::Relaxed) as usize,
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
        hal_texture_memory_bytes: shared_metrics()
            .hal_texture_memory_bytes
            .load(Ordering::Relaxed) as usize,
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
