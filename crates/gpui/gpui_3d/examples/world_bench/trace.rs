//! Lightweight call-trace helper for comparing the two rendering paths.
//!
//! When `--trace` is passed (or `GPUI_3D_TRACE_TIMINGS=1` is set), key API calls are printed with a
//! monotonic timestamp and a monotonic sequence number. This makes the distinct invocation flows
//! visible side by side:
//!
//! - direct path: scene build → `OffscreenRenderer::new` → `render` → `capture` → `destroy`
//! - sidecar path: scene build → `SceneView::new` → element → camera sampler ticks → GPUI Present
//!
//! For hardware-level phase traces, set `GPUI_3D_TRACE_TIMINGS=1`; the gpui_3d library then logs
//! `gpui3d gpu_hardware_phase` / `gpu_hardware_percentiles` (completed native GPU timestamps) and
//! the GPUI owner logs `native_bridge_cpu` timing. Those are emitted by the library, not here.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();

fn elapsed() -> std::time::Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// Enables or disables call tracing.
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled && std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_none() {
        println!(
            "world_bench: --trace is on; set GPUI_3D_TRACE_TIMINGS=1 for the library's native GPU phase traces"
        );
    }
}

/// Whether call tracing is active.
pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Records a trace event. `label` names the API call, `detail` is a short payload, and `note`
/// is an optional extra annotation (e.g. frame index or backend).
pub(crate) fn event(label: &'static str, detail: &str, note: Option<&str>) {
    if !enabled() {
        return;
    }
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let elapsed = elapsed();
    match note {
        Some(note) => println!(
            "[trace {:>6} #{:04}] {} :: {} ({note})",
            format_args!("{:.3}ms", elapsed.as_secs_f64() * 1000.0),
            sequence,
            label,
            detail
        ),
        None => println!(
            "[trace {:>6} #{:04}] {} :: {}",
            format_args!("{:.3}ms", elapsed.as_secs_f64() * 1000.0),
            sequence,
            label,
            detail
        ),
    }
}
