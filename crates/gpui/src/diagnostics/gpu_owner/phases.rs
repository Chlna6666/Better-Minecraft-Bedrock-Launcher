//! Exclusive wall-clock attribution for opt-in owner traces. No per-phase I/O.
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Other,
    Readiness,
    NativePacing,
    SurfaceResize,
    RenderPrepare,
    BackendSubmit,
    Publication,
    Diagnostics,
    LogOutput,
    Report,
}

#[derive(Debug, Default)]
pub(crate) struct Durations {
    pub(crate) other: Duration,
    pub(crate) readiness: Duration,
    pub(crate) native_pacing: Duration,
    pub(crate) surface_resize: Duration,
    pub(crate) render_prepare: Duration,
    pub(crate) backend_submit: Duration,
    pub(crate) publication: Duration,
    pub(crate) diagnostics: Duration,
    pub(crate) log_output: Duration,
    pub(crate) report: Duration,
}

struct Trace {
    phase: Phase,
    since: Instant,
    durations: Durations,
}

impl Trace {
    fn switch(&mut self, phase: Phase, now: Instant) -> Phase {
        let elapsed = now.saturating_duration_since(self.since);
        let total = match self.phase {
            Phase::Other => &mut self.durations.other,
            Phase::Readiness => &mut self.durations.readiness,
            Phase::NativePacing => &mut self.durations.native_pacing,
            Phase::SurfaceResize => &mut self.durations.surface_resize,
            Phase::RenderPrepare => &mut self.durations.render_prepare,
            Phase::BackendSubmit => &mut self.durations.backend_submit,
            Phase::Publication => &mut self.durations.publication,
            Phase::Diagnostics => &mut self.durations.diagnostics,
            Phase::LogOutput => &mut self.durations.log_output,
            Phase::Report => &mut self.durations.report,
        };
        *total += elapsed;
        self.since = now;
        std::mem::replace(&mut self.phase, phase)
    }
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

pub(crate) fn start(started_at: Instant) {
    #[cfg(target_os = "windows")]
    if super::resize_trace_enabled() {
        TRACE.with_borrow_mut(|trace| {
            *trace = Some(Trace {
                phase: Phase::Other,
                since: started_at,
                durations: Durations::default(),
            })
        });
    }
    let _ = started_at;
}

/// Restores the outer phase on every exit, including errors and early returns.
pub(crate) struct Scope(Option<Phase>);

pub(crate) fn enter(phase: Phase) -> Scope {
    Scope(TRACE.with_borrow_mut(|trace| {
        trace
            .as_mut()
            .map(|trace| trace.switch(phase, Instant::now()))
    }))
}

// Keep formatting, both logger sinks and their locks inside the measured scope.
macro_rules! timed_log {
    ($level:ident, $($args:tt)*) => {{
        let _scope = $crate::diagnostics::gpu_owner::phases::enter(
            $crate::diagnostics::gpu_owner::phases::Phase::LogOutput,
        );
        log::$level!($($args)*);
    }};
}
pub(crate) use timed_log;

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            TRACE.with_borrow_mut(|trace| {
                if let Some(trace) = trace {
                    trace.switch(previous, Instant::now());
                }
            });
        }
    }
}

pub(crate) fn finish(completed_at: Instant) -> Option<Durations> {
    TRACE
        .with_borrow_mut(|trace| trace.take())
        .map(|mut trace| {
            trace.switch(Phase::Other, completed_at);
            trace.durations
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_phases_partition_wall_time_without_double_counting() {
        let start = Instant::now();
        let mut trace = Trace {
            phase: Phase::Other,
            since: start,
            durations: Durations::default(),
        };
        let at = |ms| start + Duration::from_millis(ms);
        let outer = trace.switch(Phase::Publication, at(2));
        let nested = trace.switch(Phase::Diagnostics, at(5));
        trace.switch(nested, at(12));
        trace.switch(outer, at(16));
        trace.switch(Phase::Other, at(20));
        assert_eq!(trace.durations.publication, Duration::from_millis(7));
        assert_eq!(trace.durations.diagnostics, Duration::from_millis(7));
        assert_eq!(trace.durations.other, Duration::from_millis(6));
    }

    #[test]
    fn scope_restores_outer_phase_on_early_return() {
        TRACE.with_borrow_mut(|trace| {
            *trace = Some(Trace {
                phase: Phase::RenderPrepare,
                since: Instant::now(),
                durations: Durations::default(),
            })
        });
        let attempt = || -> Result<(), ()> {
            let _scope = enter(Phase::BackendSubmit);
            Err(())?;
            Ok(())
        };
        assert!(attempt().is_err());
        TRACE.with_borrow(|trace| {
            assert!(matches!(
                trace.as_ref().map(|trace| trace.phase),
                Some(Phase::RenderPrepare)
            ))
        });
        assert!(finish(Instant::now()).is_some());
        assert!(finish(Instant::now()).is_none());
    }
}
