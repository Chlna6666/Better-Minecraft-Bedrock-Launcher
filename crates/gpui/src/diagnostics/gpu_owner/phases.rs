//! Exclusive wall-clock attribution for GPU owner jobs. No per-phase I/O.
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Other,
    Readiness,
    NativePacing,
    BackendLock,
    FencePoll,
    FenceWait,
    SurfaceResize,
    RenderPrepare,
    PackScene,
    ExtensionPrepare,
    ExtensionScenePrepare,
    ExtensionCameraCull,
    ExtensionResourceSync,
    ExtensionBatchBuild,
    ExtensionBufferEncode,
    ExtensionCommandEncode,
    DrawPresent,
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
    pub(crate) backend_lock: Duration,
    pub(crate) fence_poll: Duration,
    pub(crate) fence_wait: Duration,
    pub(crate) surface_resize: Duration,
    pub(crate) render_prepare: Duration,
    pub(crate) pack_scene: Duration,
    pub(crate) extension_prepare: Duration,
    pub(crate) extension_scene_prepare: Duration,
    pub(crate) extension_camera_cull: Duration,
    pub(crate) extension_resource_sync: Duration,
    pub(crate) extension_batch_build: Duration,
    pub(crate) extension_buffer_encode: Duration,
    pub(crate) extension_command_encode: Duration,
    pub(crate) draw_present: Duration,
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
            Phase::BackendLock => &mut self.durations.backend_lock,
            Phase::FencePoll => &mut self.durations.fence_poll,
            Phase::FenceWait => &mut self.durations.fence_wait,
            Phase::SurfaceResize => &mut self.durations.surface_resize,
            Phase::RenderPrepare => &mut self.durations.render_prepare,
            Phase::PackScene => &mut self.durations.pack_scene,
            Phase::ExtensionPrepare => &mut self.durations.extension_prepare,
            Phase::ExtensionScenePrepare => &mut self.durations.extension_scene_prepare,
            Phase::ExtensionCameraCull => &mut self.durations.extension_camera_cull,
            Phase::ExtensionResourceSync => &mut self.durations.extension_resource_sync,
            Phase::ExtensionBatchBuild => &mut self.durations.extension_batch_build,
            Phase::ExtensionBufferEncode => &mut self.durations.extension_buffer_encode,
            Phase::ExtensionCommandEncode => &mut self.durations.extension_command_encode,
            Phase::DrawPresent => &mut self.durations.draw_present,
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
    TRACE.with_borrow_mut(|trace| {
        *trace = Some(Trace {
            phase: Phase::Other,
            since: started_at,
            durations: Durations::default(),
        })
    });
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
