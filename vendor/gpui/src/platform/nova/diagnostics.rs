use super::*;
use std::time::{Duration, Instant};

const DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS: u128 = 50;
const DEFAULT_SLOW_FRAME_WARN_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct NovaRenderDiagnostics {
    pub(super) enabled: bool,
    pub(super) warned_unsupported: bool,
    last_slow_frame_warning_at: Option<Instant>,
}

impl NovaRenderDiagnostics {
    pub(super) fn from_env() -> Self {
        Self {
            enabled: env_flag("GPUI_NOVA_RENDER_DIAGNOSTICS"),
            warned_unsupported: false,
            last_slow_frame_warning_at: None,
        }
    }

    pub(super) fn should_log_frame_details(&self) -> bool {
        self.enabled
    }

    pub(super) fn should_warn_slow_frame(&mut self, elapsed_ms: u128) -> bool {
        if self.enabled || elapsed_ms < DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS {
            return false;
        }

        let now = Instant::now();
        let should_warn = self
            .last_slow_frame_warning_at
            .is_none_or(|last| now.duration_since(last) >= DEFAULT_SLOW_FRAME_WARN_INTERVAL);
        if should_warn {
            self.last_slow_frame_warning_at = Some(now);
        }
        should_warn
    }

    pub(super) fn should_warn_unsupported(&mut self, unsupported: UnsupportedBatchSummary) -> bool {
        if unsupported.total() == 0 {
            return false;
        }
        if self.enabled {
            return true;
        }
        if self.warned_unsupported {
            return false;
        }
        self.warned_unsupported = true;
        true
    }
}

pub(super) fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

pub(super) fn nova_power_preference(renderer_options: &RendererOptions) -> PowerPreference {
    match renderer_options.power_preference {
        crate::GpuPowerPreference::AutoLowPower => PowerPreference::LowPower,
        crate::GpuPowerPreference::HighPerformance => PowerPreference::HighPerformance,
    }
}
