use gpui::{App, PerformanceMetricsSnapshot, Timer, performance_metrics_snapshot};
use std::time::Duration;

const LOG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CounterDeltas {
    frame_requests: usize,
    draws: usize,
    presents: usize,
    skips: usize,
    direct_presents: usize,
    retained_presents: usize,
    blur_frames: usize,
    partial_redraws: usize,
    full_redraw_fallbacks: usize,
}

impl CounterDeltas {
    fn between(
        previous: &PerformanceMetricsSnapshot,
        current: &PerformanceMetricsSnapshot,
    ) -> Self {
        Self {
            frame_requests: delta(previous.frame_request_count, current.frame_request_count),
            draws: delta(previous.draw_count, current.draw_count),
            presents: delta(previous.present_count, current.present_count),
            skips: delta(previous.skip_count, current.skip_count),
            direct_presents: delta(previous.direct_present_count, current.direct_present_count),
            retained_presents: delta(
                previous.retained_present_count,
                current.retained_present_count,
            ),
            blur_frames: delta(
                previous.backdrop_blur_frame_count,
                current.backdrop_blur_frame_count,
            ),
            partial_redraws: delta(previous.partial_redraw_count, current.partial_redraw_count),
            full_redraw_fallbacks: delta(
                previous.full_redraw_fallback_count,
                current.full_redraw_fallback_count,
            ),
        }
    }
}

fn delta(previous: usize, current: usize) -> usize {
    current.saturating_sub(previous)
}

fn duration_micros(duration: Option<Duration>) -> u128 {
    duration.map_or(0, |duration| duration.as_micros())
}

pub(crate) fn start(cx: &mut App) {
    let mut previous = performance_metrics_snapshot();
    cx.spawn(async move |_cx| {
        loop {
            Timer::after(LOG_INTERVAL).await;
            let current = performance_metrics_snapshot();
            emit_snapshot(&previous, &current);
            previous = current;
        }
    })
    .detach();
}

fn emit_snapshot(previous: &PerformanceMetricsSnapshot, current: &PerformanceMetricsSnapshot) {
    let counters = CounterDeltas::between(previous, current);
    emit_frame_log(current, counters);
    emit_scene_upload_log(current);
    emit_present_log(current);
}

fn emit_frame_log(metrics: &PerformanceMetricsSnapshot, counters: CounterDeltas) {
    tracing::debug!(
        target: "gpui::diagnostics",
        backend = %metrics.renderer_backend.as_str(),
        adapter = %metrics.gpu_adapter_name,
        adapter_type = %metrics.gpu_adapter_type,
        present_fps = metrics.present_fps,
        frame_requests = counters.frame_requests,
        draws = counters.draws,
        presents = counters.presents,
        skips = counters.skips,
        direct_presents = counters.direct_presents,
        retained_presents = counters.retained_presents,
        blur_frames = counters.blur_frames,
        partial_redraws = counters.partial_redraws,
        full_redraw_fallbacks = counters.full_redraw_fallbacks,
        frame_build_us = duration_micros(metrics.frame_build_time),
        layout_us = duration_micros(metrics.frame_layout_time),
        prepaint_us = duration_micros(metrics.frame_prepaint_time),
        paint_us = duration_micros(metrics.frame_paint_time),
        backend_draw_us = duration_micros(metrics.frame_backend_draw_time),
        windows = ?metrics.window_metrics,
        "GPUI frame diagnostics (5s delta)"
    );
}

fn emit_scene_upload_log(metrics: &PerformanceMetricsSnapshot) {
    tracing::debug!(
        target: "gpui::diagnostics",
        layout_nodes = metrics.layout_nodes,
        measured_layout_nodes = metrics.measured_layout_nodes,
        dirty_rects = metrics.dirty_rect_count,
        dirty_area = metrics.dirty_rect_area,
        scene_primitives = metrics.scene_primitives,
        scene_batches = metrics.scene_batches,
        replayed_primitives = metrics.scene_replayed_primitives,
        rebuilt_segments = metrics.scene_segment_rebuild_count,
        reused_segments = metrics.scene_segment_reuse_count,
        encoded_primitives = metrics.encoded_scene_primitives,
        encoded_batches = metrics.encoded_scene_batches,
        upload_bytes = metrics.upload_bytes,
        atlas_upload_bytes = metrics.atlas_upload_bytes,
        quad_bytes = metrics.quad_upload_bytes,
        shadow_bytes = metrics.shadow_upload_bytes,
        path_bytes = metrics.path_upload_bytes,
        mono_sprite_bytes = metrics.mono_sprite_upload_bytes,
        poly_sprite_bytes = metrics.poly_sprite_upload_bytes,
        underline_bytes = metrics.underline_upload_bytes,
        blur_descriptor_bytes = metrics.backdrop_blur_upload_bytes,
        animation_bytes = metrics.animation_upload_bytes,
        custom_mesh_parameter_bytes = metrics.custom_mesh_parameter_upload_bytes,
        pod_upload_bytes = metrics.pod_upload_bytes,
        "GPUI scene and upload diagnostics (latest frame)"
    );
}

fn emit_present_log(metrics: &PerformanceMetricsSnapshot) {
    tracing::debug!(
        target: "gpui::diagnostics",
        surface_format = %metrics.gpu_surface_format,
        alpha_mode = %metrics.gpu_surface_alpha_mode,
        present_mode = %metrics.gpu_surface_present_mode,
        mask_passes = metrics.mask_pass_count,
        main_passes = metrics.main_pass_count,
        composite_passes = metrics.composite_pass_count,
        retained_copy_pixels = metrics.retained_copy_pixels,
        retained_copy_estimated_bytes = metrics.retained_copy_estimated_bytes,
        has_retained_target = metrics.has_retained_frame_target,
        blur_primitives = metrics.backdrop_blur_primitives,
        blur_source_pixels = metrics.backdrop_blur_source_pixels,
        blur_target_pixels = metrics.backdrop_blur_target_pixels,
        blur_level_pixels = ?metrics.backdrop_blur_level_pixels,
        gpu_wait_us = duration_micros(metrics.gpu_submission_wait_time),
        gpu_slow_waits = metrics.gpu_submission_slow_wait_count,
        surface_reconfigures = metrics.gpu_surface_reconfigure_count,
        surface_errors = metrics.gpu_surface_error_count,
        "GPUI present and compositor diagnostics (latest frame)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_deltas_are_interval_scoped_and_saturating() {
        let previous = PerformanceMetricsSnapshot {
            frame_request_count: 10,
            draw_count: 8,
            present_count: 7,
            direct_present_count: 5,
            retained_present_count: 2,
            ..PerformanceMetricsSnapshot::default()
        };
        let current = PerformanceMetricsSnapshot {
            frame_request_count: 15,
            draw_count: 3,
            present_count: 11,
            direct_present_count: 8,
            retained_present_count: 3,
            ..PerformanceMetricsSnapshot::default()
        };

        let deltas = CounterDeltas::between(&previous, &current);

        assert_eq!(deltas.frame_requests, 5);
        assert_eq!(deltas.draws, 0);
        assert_eq!(deltas.presents, 4);
        assert_eq!(deltas.direct_presents, 3);
        assert_eq!(deltas.retained_presents, 1);
    }
}
