use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::super::FramePhaseMetrics;
use super::super::state::shared_metrics;
use super::super::timing::{duration_micros, record_once_micros};

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
pub fn record_frame_decision(should_draw: bool, should_present: bool, should_skip: bool) {
    let metrics = shared_metrics();
    if should_draw {
        metrics.draw_count.fetch_add(1, Ordering::Relaxed);
    }
    if should_present {
        metrics.present_count.fetch_add(1, Ordering::Relaxed);
    }
    if should_skip {
        metrics.skip_count.fetch_add(1, Ordering::Relaxed);
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
