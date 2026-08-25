use super::super::state::shared_metrics;

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
        }
        if disposition.skipped_frame {
            metrics.skip_count = metrics.skip_count.saturating_add(1);
            metrics.skipped_frame_count = metrics.skipped_frame_count.saturating_add(1);
        }
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
