use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use super::store::shared_metrics;

/// Metrics for one window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowMetricsSnapshot {
    /// Platform window identifier.
    pub window_id: u64,
    /// Per-window present rate in milli-FPS. This is never shared across windows.
    pub present_fps_milli: usize,
    /// Logical drawable width in milli logical-pixels.
    pub logical_width_milli: usize,
    /// Logical drawable height in milli logical-pixels.
    pub logical_height_milli: usize,
    /// Current drawable width in physical pixels.
    pub physical_width_px: usize,
    /// Current drawable height in physical pixels.
    pub physical_height_px: usize,
    /// Current display scale factor in milli-units.
    pub scale_factor_milli: usize,
    /// Whether this window is the OS-active window.
    pub active: bool,
    /// Whether this window is minimized.
    pub minimized: bool,
    /// Redraw requests.
    pub request_redraw_count: usize,
    /// Drawn frames.
    pub draw_count: usize,
    /// Presented frames.
    pub present_count: usize,
    /// Skipped frame decisions.
    pub skip_count: usize,
    /// Skipped frame opportunities.
    pub skipped_frame_count: usize,
    /// Surface reconfigurations.
    pub gpu_surface_reconfigure_count: usize,
    /// Surface errors.
    pub gpu_surface_error_count: usize,
    /// Layout recomputes.
    pub layout_recompute_count: usize,
    /// Uploaded bytes.
    pub upload_bytes: usize,
}

/// Per-window frame accounting for one frame decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowFrameDisposition {
    /// Whether the frame produced freshly drawn scene content.
    pub drew_frame: bool,
    /// Whether the frame submitted visible content for presentation.
    pub presented_frame: bool,
    /// Whether the frame decision skipped visible work entirely.
    pub skipped_frame: bool,
}

/// Records that a specific window requested a redraw.
pub fn record_window_request_redraw(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.request_redraw_count = metrics.request_redraw_count.saturating_add(1);
    }
}

/// Records the disposition of a specific window frame.
pub fn record_window_frame_disposition(window_id: u64, disposition: WindowFrameDisposition) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        if disposition.drew_frame {
            metrics.draw_count = metrics.draw_count.saturating_add(1);
        }
        if disposition.presented_frame {
            metrics.present_count = metrics.present_count.saturating_add(1);

            let now = Instant::now();
            if let Some(previous_present_at) = metrics.last_present_at.replace(now) {
                let delta = now.saturating_duration_since(previous_present_at);
                if delta >= Duration::from_micros(250) && delta <= Duration::from_secs(1) {
                    let instant_fps_milli =
                        (1000.0 / delta.as_secs_f32()).round().max(0.0) as u64;
                    metrics.present_fps_milli = if metrics.present_fps_milli == 0 {
                        instant_fps_milli
                    } else {
                        ((metrics.present_fps_milli as f32 * 0.85)
                            + (instant_fps_milli as f32 * 0.15))
                            .round() as u64
                    };
                }
            }
        }
        if disposition.skipped_frame {
            metrics.skip_count = metrics.skip_count.saturating_add(1);
            metrics.skipped_frame_count = metrics.skipped_frame_count.saturating_add(1);
        }
    }
}

/// Records current per-window geometry and activation state.
pub fn record_window_runtime_state(
    window_id: u64,
    logical_width: f32,
    logical_height: f32,
    scale_factor: f32,
    active: bool,
    minimized: bool,
) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let logical_width = logical_width.max(0.0);
        let logical_height = logical_height.max(0.0);

        metrics.logical_width_milli = (logical_width * 1000.0).round() as u64;
        metrics.logical_height_milli = (logical_height * 1000.0).round() as u64;
        metrics.physical_width_px = (logical_width * scale_factor).round() as u64;
        metrics.physical_height_px = (logical_height * scale_factor).round() as u64;
        metrics.scale_factor_milli = (scale_factor * 1000.0).round() as u64;
        metrics.active = active;
        metrics.minimized = minimized;
    }
}

/// Records per-window gpu surface diagnostics.
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
