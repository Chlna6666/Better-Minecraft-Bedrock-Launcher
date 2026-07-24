use super::*;
use std::{
    sync::{LazyLock, Mutex, MutexGuard},
    time::Duration,
};

static PERFORMANCE_METRICS_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn lock_performance_metrics() -> MutexGuard<'static, ()> {
    PERFORMANCE_METRICS_TEST_LOCK
        .lock()
        .expect("performance metrics test lock should not be poisoned")
}

#[test]
fn records_aggregate_image_decode_metrics() {
    let _lock = lock_performance_metrics();
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
    assert!(snapshot.image_decode_compressed_bytes >= before.image_decode_compressed_bytes + 40);
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
    assert!(snapshot.image_decode_slow_count >= before.image_decode_slow_count + 1);
}

#[test]
fn records_retained_image_asset_metrics() {
    let _lock = lock_performance_metrics();
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
    assert!(
        snapshot.recent_image_decodes.iter().any(|record| {
            record.source == "images/background.webp" && record.target_width == 960
        })
    );

    drop_image_asset_retained(key);
}

#[test]
fn records_extended_gpu_metrics() {
    let _lock = lock_performance_metrics();
    reset_frame_upload_metrics();
    let before = performance_metrics_snapshot();
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
    record_gpu_submission_wait(Duration::from_millis(2));
    record_gpu_submission_wait(Duration::from_millis(9));
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
    assert_eq!(
        snapshot.gpu_submission_wait_count,
        before.gpu_submission_wait_count + 2
    );
    assert_eq!(
        snapshot.gpu_submission_wait_time,
        Some(Duration::from_millis(9))
    );
    assert!(
        snapshot
            .gpu_submission_wait_total_time
            .is_some_and(|duration| {
                duration
                    >= before.gpu_submission_wait_total_time.unwrap_or_default()
                        + Duration::from_millis(11)
            })
    );
    assert!(
        snapshot
            .gpu_submission_wait_max_time
            .is_some_and(|duration| duration >= Duration::from_millis(9))
    );
    assert!(snapshot.gpu_submission_slow_wait_count >= before.gpu_submission_slow_wait_count + 1);
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
    let _lock = lock_performance_metrics();
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
    let before_pointer_skips = performance_metrics_snapshot().skipped_pointer_frame_count;
    record_skipped_pointer_frame();
    record_layout_cache_metrics(8, 2);
    record_text_layout_cache_metrics(3, 4, 5);
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
    assert_eq!(snapshot.layout_cache_hits, 8);
    assert_eq!(snapshot.layout_cache_misses, 2);
    assert_eq!(snapshot.text_layout_hits, 3);
    assert_eq!(snapshot.text_layout_reuses, 4);
    assert_eq!(snapshot.text_layout_misses, 5);
    assert!(snapshot.image_cache_evictions >= 2);
    assert!(snapshot.image_drop_count >= 1);
    assert!(snapshot.atlas_remove_count >= 6);
    assert_eq!(snapshot.pod_upload_bytes, 4096);
}

#[test]
fn records_window_scoped_metrics() {
    let _lock = lock_performance_metrics();
    record_window_request_redraw(10);
    record_window_frame_disposition(
        10,
        WindowFrameDisposition {
            drew_frame: true,
            presented_frame: true,
            skipped_frame: false,
        },
    );
    record_window_frame_disposition(
        10,
        WindowFrameDisposition {
            drew_frame: false,
            presented_frame: false,
            skipped_frame: true,
        },
    );
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
