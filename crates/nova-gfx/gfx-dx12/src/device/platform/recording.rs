//! Opt-in host command-recording diagnostics; these are not GPU execution timings.
use std::{sync::OnceLock, time::Instant};

pub(super) fn start() -> Option<Instant> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    ENABLED
        .get_or_init(|| std::env::var("GFX_DX12_TRACE_TIMINGS").as_deref() == Ok("1"))
        .then(Instant::now)
}

pub(super) fn finish(start: Option<Instant>, staging_ms: f64, commands: usize) {
    if let Some(start) = start {
        eprintln!(
            "gfx-dx12 record commands={commands} indirect_stage_ms={staging_ms:.3} draw_commands_ms={:.3}",
            (start.elapsed().as_secs_f64() * 1000. - staging_ms).max(0.)
        );
    }
}

pub(super) fn debug_layer_enabled(value: Option<&str>, debug_build: bool) -> bool {
    match value {
        Some("0") => false,
        Some("1") => true,
        _ => debug_build,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn debug_layer_override_preserves_build_defaults() {
        assert!(debug_layer_enabled(None, true));
        assert!(!debug_layer_enabled(None, false));
        assert!(!debug_layer_enabled(Some("0"), true));
        assert!(debug_layer_enabled(Some("1"), false));
        assert!(debug_layer_enabled(Some("invalid"), true));
    }
}
