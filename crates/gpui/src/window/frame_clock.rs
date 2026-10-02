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

impl Window {
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
        self.last_frame_started_at = Some(frame_started_at);
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
