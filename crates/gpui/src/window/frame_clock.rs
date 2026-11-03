use std::{num::NonZeroU32, time::Duration, time::Instant};

use anyhow::{Result, anyhow};

use super::Window;
use crate::{PlatformFrameRequest, Task};

/// Controls how often a window may submit frames.
///
/// `System` follows the native window and display cadence. `FixedFps` sets an upper bound for
/// that window's frame requests and retained-animation samples. The platform compositor may
/// present less often than the selected rate, and pending requests are merged until the next
/// eligible frame.
/// Native client-size reconciliation remains immediate during live resize so a
/// configured rate cannot leave newly exposed client pixels on an old layout.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FrameClock {
    /// Follow the native window and display cadence. This is the default.
    #[default]
    System,
    /// Limit this window to the requested maximum frame rate.
    FixedFps(NonZeroU32),
}

impl FrameClock {
    /// Creates a fixed-rate clock for one window.
    ///
    /// The requested rate is a maximum cadence. Platform VSync and compositor scheduling may
    /// make the delivered rate lower. A zero rate is invalid.
    ///
    /// # Errors
    ///
    /// Returns an error when `fps` is zero.
    pub fn fixed_fps(fps: u32) -> Result<Self> {
        NonZeroU32::new(fps)
            .map(Self::FixedFps)
            .ok_or_else(|| anyhow!("window frame rate must be greater than zero"))
    }

    /// Returns the configured fixed rate, or `None` when this window follows the system clock.
    pub const fn fps(self) -> Option<NonZeroU32> {
        match self {
            Self::System => None,
            Self::FixedFps(fps) => Some(fps),
        }
    }

    pub(crate) fn interval(self) -> Option<Duration> {
        match self {
            Self::System => None,
            Self::FixedFps(fps) => {
                let nanoseconds = 1_000_000_000_u64.div_ceil(u64::from(fps.get()));
                Some(Duration::from_nanos(nanoseconds))
            }
        }
    }
}

/// Only one owner should wait for an identical FixedFps deadline.
/// The native Immediate scheduler already paces the UI callback at that
/// interval; retain the window timer whenever the native pace differs.
fn window_frame_clock_interval(
    requested: Option<Duration>,
    native_paced: Option<Duration>,
) -> Option<Duration> {
    if requested.is_some() && requested == native_paced {
        None
    } else {
        requested
    }
}

const DEFAULT_INACTIVE_NATIVE_INTERVAL: Duration = Duration::from_micros(66_667);

fn combined_frame_interval(active: Option<Duration>, inactive: Option<Duration>) -> Option<Duration> {
    match (active, inactive) {
        (Some(active), Some(inactive)) => Some(active.max(inactive)),
        (Some(active), None) => Some(active),
        (None, Some(inactive)) => Some(inactive),
        (None, None) => None,
    }
}

impl Window {
    pub(super) fn effective_frame_clock_interval(&self) -> Option<Duration> {
        let active = self.frame_clock.interval();
        if self.active.get() {
            return window_frame_clock_interval(
                active,
                self.platform_window.native_frame_clock_interval(),
            );
        }

        let native_inactive = self.platform_window.native_inactive_frame_clock_interval();
        // A faster native scheduler is shared by all windows; a window without
        // its own opt-in must still retain the original 15 Hz background policy.
        let inactive = match self.inactive_frame_clock.get() {
            Some(FrameClock::FixedFps(fps)) => FrameClock::FixedFps(fps).interval(),
            _ => native_inactive.map(|_| DEFAULT_INACTIVE_NATIVE_INTERVAL),
        };
        window_frame_clock_interval(
            combined_frame_interval(active, inactive),
            native_inactive.or_else(|| self.platform_window.native_frame_clock_interval()),
        )
    }

    /// Returns this window's optional inactive frame pacing override.
    pub fn inactive_frame_clock(&self) -> Option<FrameClock> {
        self.inactive_frame_clock.get()
    }

    /// Changes the inactive cadence without affecting foreground frame pacing.
    /// None removes this window's override. Newly opened child windows inherit
    /// the current value unless they explicitly configure another one.
    pub fn set_inactive_frame_clock(&mut self, clock: Option<FrameClock>) {
        if self.inactive_frame_clock.get() == clock {
            return;
        }
        self.inactive_frame_clock.set(clock);
        self.platform_window
            .set_inactive_frame_interval(clock.and_then(|value| value.interval()));
        let pending_request = {
            let mut state = self.frame_clock_state.borrow_mut();
            state.cancel_timer();
            state.pending_request.take()
        };
        if let Some(request) = pending_request {
            self.request_platform_frame(request);
        }
    }

    /// Returns this window's frame pacing configuration.
    pub fn frame_clock(&self) -> FrameClock {
        self.frame_clock
    }

    /// Changes this window's frame pacing configuration.
    ///
    /// Fixed-rate deadlines are asynchronous and only enqueue a normal platform frame; they do
    /// not render from the timer task. The new pacing limit is also applied to any active
    /// renderer-owned animation on this window.
    pub fn set_frame_clock(&mut self, frame_clock: FrameClock) {
        if self.frame_clock == frame_clock {
            return;
        }

        self.frame_clock = frame_clock;
        self.platform_window
            .set_frame_interval(frame_clock.interval());
        let pending_request = {
            let mut state = self.frame_clock_state.borrow_mut();
            state.cancel_timer();
            state.pending_request.take()
        };

        if let Some(request) = pending_request {
            self.request_platform_frame(request);
        }
    }
}

/// Asynchronous request-coalescing state owned by one window's frame clock.
#[derive(Default)]
pub(super) struct FrameClockState {
    /// Cadence anchor, corrected to the deadline after small dispatch delays.
    pub(super) last_frame_started_at: Option<Instant>,
    pub(super) platform_request_pending: bool,
    pub(super) pending_request: Option<PlatformFrameRequest>,
    pub(super) timer_generation: u64,
    pub(super) timer: Option<Task<()>>,
}

pub(super) enum FrameClockRequest {
    Coalesced,
    Dispatch(PlatformFrameRequest),
    Wait { generation: u64, deadline: Instant },
}

pub(super) enum ReceivedPlatformFrame {
    Ready(PlatformFrameRequest),
    Wait { generation: u64, deadline: Instant },
}

impl FrameClockState {
    /// Even with a native display clock, only one platform callback should be
    /// outstanding. Merge additional UI/animation demands with that callback.
    pub(super) fn queue_unpaced_request(
        &mut self,
        request: PlatformFrameRequest,
    ) -> FrameClockRequest {
        self.cancel_timer();
        if self.platform_request_pending {
            self.merge_pending_request(request);
            FrameClockRequest::Coalesced
        } else {
            FrameClockRequest::Dispatch(self.take_merged_request(request))
        }
    }

    pub(super) fn queue_request(
        &mut self,
        request: PlatformFrameRequest,
        now: Instant,
        interval: Duration,
    ) -> FrameClockRequest {
        if self.platform_request_pending || self.timer.is_some() {
            self.merge_pending_request(request);
            return FrameClockRequest::Coalesced;
        }

        let deadline = self
            .last_frame_started_at
            .map_or(now, |last_frame| last_frame + interval);
        if now >= deadline {
            FrameClockRequest::Dispatch(self.take_merged_request(request))
        } else {
            self.merge_pending_request(request);
            let (generation, deadline) = self.start_timer(deadline);
            FrameClockRequest::Wait {
                generation,
                deadline,
            }
        }
    }

    pub(super) fn receive_frame(
        &mut self,
        request: PlatformFrameRequest,
        frame_started_at: Instant,
        interval: Option<Duration>,
    ) -> ReceivedPlatformFrame {
        self.platform_request_pending = false;
        let next_allowed_frame = self
            .last_frame_started_at
            .zip(interval)
            .map(|(last_frame, interval)| last_frame + interval);
        if let Some(next_allowed_frame) = next_allowed_frame
            && frame_started_at < next_allowed_frame
        {
            self.merge_pending_request(request);
            self.cancel_timer();
            let (generation, deadline) = self.start_timer(next_allowed_frame);
            return ReceivedPlatformFrame::Wait {
                generation,
                deadline,
            };
        }

        self.cancel_timer();
        // Preserve cadence across small dispatch delays. A long stall starts a
        // fresh cadence so recovery cannot produce closely spaced catch-up frames.
        self.last_frame_started_at = Some(
            next_allowed_frame
                .zip(interval)
                .filter(|(deadline, interval)| {
                    frame_started_at.saturating_duration_since(*deadline) <= *interval / 4
                })
                .map_or(frame_started_at, |(deadline, _)| deadline),
        );
        ReceivedPlatformFrame::Ready(self.take_merged_request(request))
    }

    pub(super) fn cancel_timer(&mut self) {
        self.timer_generation = self.timer_generation.wrapping_add(1);
        drop(self.timer.take());
    }

    pub(super) fn start_timer(&mut self, deadline: Instant) -> (u64, Instant) {
        self.timer_generation = self.timer_generation.wrapping_add(1);
        (self.timer_generation, deadline)
    }

    pub(super) fn merge_pending_request(&mut self, request: PlatformFrameRequest) {
        self.pending_request = Some(
            self.pending_request
                .map_or(request, |pending| pending.merge(request)),
        );
    }

    pub(super) fn take_merged_request(
        &mut self,
        request: PlatformFrameRequest,
    ) -> PlatformFrameRequest {
        self.pending_request
            .take()
            .map_or(request, |pending| pending.merge(request))
    }
}

#[cfg(test)]
mod unpaced_tests {
    use super::*;

    #[test]
    fn system_clock_merges_ui_work_into_the_in_flight_native_frame() {
        let mut state = FrameClockState::default();
        assert!(matches!(
            state.queue_unpaced_request(PlatformFrameRequest::animation_tick()),
            FrameClockRequest::Dispatch(_)
        ));
        state.platform_request_pending = true;
        assert!(matches!(
            state.queue_unpaced_request(PlatformFrameRequest::ui_commit()),
            FrameClockRequest::Coalesced
        ));
        let ReceivedPlatformFrame::Ready(merged) =
            state.receive_frame(PlatformFrameRequest::animation_tick(), Instant::now(), None)
        else {
            panic!("native callback should deliver immediately");
        };
        assert!(merged.needs_ui_commit());
        assert!(merged.needs_presentation());
        assert!(!state.platform_request_pending);
        assert!(state.pending_request.is_none());
        assert!(matches!(
            state.queue_unpaced_request(PlatformFrameRequest::animation_tick()),
            FrameClockRequest::Dispatch(_)
        ));
    }
}

#[cfg(test)]
mod native_clock_tests {
    use super::*;

    #[test]
    fn inactive_clock_never_exceeds_foreground_cap() {
        assert_eq!(
            combined_frame_interval(Some(Duration::from_millis(20)), Some(Duration::from_millis(16))),
            Some(Duration::from_millis(20))
        );
        assert_eq!(combined_frame_interval(None, None), None);
        assert_eq!(
            combined_frame_interval(None, Some(DEFAULT_INACTIVE_NATIVE_INTERVAL)),
            Some(DEFAULT_INACTIVE_NATIVE_INTERVAL)
        );
    }

    #[test]
    fn native_immediate_clock_owns_identical_fixed_fps_deadline() {
        let hz1000 = FrameClock::fixed_fps(1000).unwrap().interval();
        let hz120 = FrameClock::fixed_fps(120).unwrap().interval();
        assert_eq!(window_frame_clock_interval(hz1000, hz1000), None);
        assert_eq!(window_frame_clock_interval(hz1000, None), hz1000);
        assert_eq!(window_frame_clock_interval(hz120, hz1000), hz120);
        assert_eq!(window_frame_clock_interval(None, hz1000), None);
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;

    #[test]
    fn all_frame_request_sources_wait_for_the_same_deadline() {
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        for request in [
            PlatformFrameRequest::ui_commit(),
            PlatformFrameRequest::animation_tick(),
            PlatformFrameRequest::presentation(),
            PlatformFrameRequest::ui_commit_and_presentation(),
        ] {
            let mut state = FrameClockState::default();
            assert!(matches!(
                state.receive_frame(request, start, Some(interval)),
                ReceivedPlatformFrame::Ready(_)
            ));
            assert!(matches!(
                state.receive_frame(request, start + interval / 2, Some(interval)),
                ReceivedPlatformFrame::Wait { deadline, .. } if deadline == start + interval
            ));
            assert!(matches!(
                state.receive_frame(request, start + interval, Some(interval)),
                ReceivedPlatformFrame::Ready(merged) if merged == request
            ));
            assert!(state.pending_request.is_none());
        }
    }

    #[test]
    fn small_dispatch_delays_do_not_accumulate_cadence_drift() {
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        let request = PlatformFrameRequest::animation_tick();
        let mut state = FrameClockState::default();
        state.receive_frame(request, start, Some(interval));
        for frame in 1..=4 {
            let delayed = start + interval * frame + Duration::from_millis(2);
            assert!(matches!(
                state.receive_frame(request, delayed, Some(interval)),
                ReceivedPlatformFrame::Ready(_)
            ));
            assert!(matches!(
                state.queue_request(request, delayed, interval),
                FrameClockRequest::Wait { deadline, .. }
                    if deadline == start + interval * (frame + 1)
            ));
        }
    }

    #[test]
    fn stalls_and_unpaced_frames_reset_cadence_without_catch_up() {
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        let request = PlatformFrameRequest::animation_tick();
        let mut state = FrameClockState::default();
        state.receive_frame(request, start, Some(interval));
        let after_stall = start + interval * 2 - Duration::from_millis(1);
        assert!(matches!(
            state.receive_frame(request, after_stall, Some(interval)),
            ReceivedPlatformFrame::Ready(_)
        ));
        assert!(matches!(
            state.receive_frame(request, after_stall + interval / 2, Some(interval)),
            ReceivedPlatformFrame::Wait { deadline, .. } if deadline == after_stall + interval
        ));
        let unpaced = after_stall + interval / 2;
        assert!(matches!(
            state.receive_frame(request, unpaced, None),
            ReceivedPlatformFrame::Ready(_)
        ));
        assert!(matches!(
            state.queue_request(request, unpaced, interval),
            FrameClockRequest::Wait { deadline, .. } if deadline == unpaced + interval
        ));
    }
}
