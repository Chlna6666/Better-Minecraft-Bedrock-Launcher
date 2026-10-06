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
        // Detailed frame diagnostics are explicit opt-in. In normal builds only genuinely slow
        // frames may emit a warning, and those warnings are rate-limited so pointer/scroll redraws
        // never turn the renderer hot path into synchronous log I/O.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostics(enabled: bool) -> NovaRenderDiagnostics {
        NovaRenderDiagnostics {
            enabled,
            warned_unsupported: false,
            last_slow_frame_warning_at: None,
        }
    }

    #[test]
    fn normal_fast_frames_do_not_emit_slow_frame_warnings() {
        let mut diagnostics = diagnostics(false);
        assert!(!diagnostics.should_warn_slow_frame(0));
        assert!(!diagnostics.should_warn_slow_frame(
            DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS - 1,
        ));
    }

    #[test]
    fn slow_frame_warnings_are_rate_limited() {
        let mut diagnostics = diagnostics(false);
        assert!(diagnostics.should_warn_slow_frame(
            DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS,
        ));
        assert!(!diagnostics.should_warn_slow_frame(
            DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS + 100,
        ));

        diagnostics.last_slow_frame_warning_at =
            Some(Instant::now() - DEFAULT_SLOW_FRAME_WARN_INTERVAL);
        assert!(diagnostics.should_warn_slow_frame(
            DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS + 100,
        ));
    }

    #[test]
    fn explicit_diagnostics_mode_owns_per_frame_details_without_slow_warn_spam() {
        let mut diagnostics = diagnostics(true);
        assert!(diagnostics.should_log_frame_details());
        assert!(!diagnostics.should_warn_slow_frame(
            DEFAULT_SLOW_FRAME_WARN_THRESHOLD_MS + 100,
        ));
    }
}
