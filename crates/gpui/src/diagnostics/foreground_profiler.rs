//! Opt-in foreground interval profiler and hang detector.
//!
//! This profiler is intentionally separate from the always-on aggregate performance metrics.
//! Enabling the `profiler` feature records completed foreground work on the GPUI thread and seals
//! intervals at newly presented frames, true foreground idle, or power transitions. Hidden windows
//! and sleep/wake boundaries invalidate frame samples instead of turning background time into fake
//! latency.
//!
//! The implementation is single-threaded by design: GPUI foreground work already has thread
//! affinity, so recording requires no mutex or atomic traffic on the hot path. Detectors must be
//! polled on the same foreground thread.

use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    marker::PhantomData,
    rc::Rc,
    time::{Duration, Instant},
};

const TASK_POLL_FLOOR: Duration = Duration::from_micros(100);
const PENDING_FRAME_DEADLINE: Duration = Duration::from_secs(1);
const MAX_INTERVAL_EVENTS: usize = 16 * 1024;
const SNAPSHOT_CAPACITY: usize = 512;

/// Default release threshold for one foreground operation to qualify as a hang.
pub const DEFAULT_HANG_THRESHOLD: Duration = if cfg!(debug_assertions) {
    if cfg!(target_os = "windows") {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(5)
    }
} else {
    Duration::from_millis(100)
};

/// Default cumulative foreground-work budget.
///
/// Release builds use 24ms, matching the upstream semantic: multiple individually-small pieces of
/// work can still miss a frame. Debug builds use a wider budget to avoid profiling unoptimized
/// work as a continuous hang.
pub const DEFAULT_FRAME_BUDGET: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(100)
} else {
    Duration::from_millis(24)
};

/// Why a foreground interval qualified as a hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HangTrigger {
    /// At least one individual operation exceeded the detector threshold.
    Threshold,
    /// No individual operation crossed the threshold, but cumulative foreground occupancy exceeded
    /// the interval budget.
    Budget,
}

/// One category of foreground work retained by the profiler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundWorkKind {
    /// One poll of a GPUI foreground task.
    TaskPoll {
        /// Source file where the task was spawned.
        file: &'static str,
        /// Source line where the task was spawned.
        line: u32,
    },
    /// Top-level GPUI action dispatch.
    Action {
        /// Registered action name.
        name: &'static str,
    },
    /// One platform-input dispatch.
    Input {
        /// Input variant name.
        kind: &'static str,
        /// Window receiving the input.
        window_id: u64,
    },
    /// CPU work that generated a new GPUI frame.
    Draw {
        /// Window being generated.
        window_id: u64,
    },
    /// Platform renderer submission work.
    Submit {
        /// Window being submitted.
        window_id: u64,
    },
}

/// One completed foreground operation.
#[derive(Clone, Copy, Debug)]
pub struct ForegroundWorkSample {
    /// Operation category.
    pub kind: ForegroundWorkKind,
    /// Start timestamp on the foreground clock.
    pub started_at: Instant,
    /// Completion timestamp on the foreground clock.
    pub ended_at: Instant,
}

impl ForegroundWorkSample {
    /// Duration spent in this foreground operation.
    pub fn duration(&self) -> Duration {
        self.ended_at.saturating_duration_since(self.started_at)
    }
}

/// End-to-end timing for one newly drawn, successfully submitted visible frame.
///
/// "present" is GPUI's platform presentation boundary: the renderer submit returned successfully
/// and the platform frame lifecycle completed. It is deliberately not claimed to be a hardware
/// scan-out timestamp.
#[derive(Clone, Copy, Debug)]
pub struct FramePipelineSample {
    /// Window that produced the frame.
    pub window_id: u64,
    /// Timestamp at GPUI's successful platform presentation boundary.
    pub presented_at: Instant,
    /// Time from first dirty edge to the first platform frame request.
    pub dirty_to_request: Option<Duration>,
    /// Time from frame request until CPU frame generation began.
    pub request_to_draw: Option<Duration>,
    /// CPU frame generation duration.
    pub draw: Option<Duration>,
    /// Delay between CPU generation completion and renderer submission beginning.
    pub draw_to_submit: Option<Duration>,
    /// Renderer/platform submission duration.
    pub submit: Option<Duration>,
    /// Delay from the renderer submission return to GPUI's completed presentation boundary.
    pub submit_to_present: Option<Duration>,
    /// Time from first dirty edge through the GPUI presentation boundary.
    pub dirty_to_present: Option<Duration>,
    /// Time from platform request through the GPUI presentation boundary.
    pub request_to_present: Option<Duration>,
}

/// Semantic boundary that sealed a foreground activity interval.
#[derive(Clone, Copy, Debug)]
pub enum ForegroundIntervalBoundary {
    /// A newly drawn frame reached GPUI's platform presentation boundary.
    Presented(FramePipelineSample),
    /// The foreground returned to idle with no visible frame pending.
    Idle {
        /// When the foreground became idle.
        ended_at: Instant,
    },
    /// Sleep or wake interrupted measurement.
    PowerTransition {
        /// When the transition was observed.
        ended_at: Instant,
    },
}

/// One completed foreground activity interval.
#[derive(Clone, Debug)]
pub struct ForegroundIntervalSnapshot {
    /// Start of retained activity in this interval.
    pub started_at: Instant,
    /// Boundary that sealed the interval.
    pub boundary: ForegroundIntervalBoundary,
    /// Individually retained foreground operations.
    pub events: Vec<ForegroundWorkSample>,
    /// Number of sub-100us task polls folded out of the event vector.
    pub small_poll_count: u64,
    /// Exact total duration of folded task polls.
    pub small_poll_total: Duration,
    /// Events dropped because the per-interval cap was reached.
    pub dropped_events: u64,
}

impl ForegroundIntervalSnapshot {
    /// End timestamp of this interval.
    pub fn ended_at(&self) -> Instant {
        match self.boundary {
            ForegroundIntervalBoundary::Presented(sample) => sample.presented_at,
            ForegroundIntervalBoundary::Idle { ended_at }
            | ForegroundIntervalBoundary::PowerTransition { ended_at } => ended_at,
        }
    }

    /// Approximate foreground occupancy without double-counting nested retained spans.
    pub fn foreground_spend(&self) -> Duration {
        let mut spans = self
            .events
            .iter()
            .map(|event| (event.started_at, event.ended_at))
            .collect::<Vec<_>>();
        spans.sort_unstable_by_key(|(start, _)| *start);

        let mut occupied = Duration::ZERO;
        let mut merged_until: Option<Instant> = None;
        for (start, end) in spans {
            let effective_start = merged_until.map_or(start, |until| start.max(until));
            if end > effective_start {
                occupied += end.saturating_duration_since(effective_start);
            }
            merged_until = Some(merged_until.map_or(end, |until| until.max(end)));
        }
        let occupied = occupied + self.small_poll_total;
        let interval = self.ended_at().saturating_duration_since(self.started_at);
        occupied.min(interval)
    }
}

/// Intervals returned by one collector drain.
#[derive(Clone, Debug, Default)]
pub struct CollectedForegroundIntervals {
    /// Sealed intervals observed since the previous drain.
    pub intervals: Vec<ForegroundIntervalSnapshot>,
    /// Number of intervals overwritten before this collector observed them.
    pub lost: u64,
}

/// Independent cursor over visibility- and power-aware foreground intervals.
///
/// Creating a collector starts at the current tail; it does not replay historical intervals.
pub struct ForegroundIntervalCollector {
    next_sequence: u64,
    _not_send: PhantomData<Rc<()>>,
}

impl ForegroundIntervalCollector {
    /// Creates a collector that observes intervals sealed after this call.
    pub fn new() -> Self {
        Self {
            next_sequence: with_state(|state| state.next_snapshot_sequence),
            _not_send: PhantomData,
        }
    }

    /// Returns intervals sealed since the previous drain.
    pub fn collect_unseen(&mut self) -> CollectedForegroundIntervals {
        with_state(|state| state.snapshots_since(&mut self.next_sequence))
    }
}

impl Default for ForegroundIntervalCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// One detected foreground hang.
#[derive(Clone, Debug)]
pub struct HangIncident {
    /// Detection rule that qualified the interval.
    pub trigger: HangTrigger,
    /// Full interval snapshot.
    pub snapshot: ForegroundIntervalSnapshot,
    /// Longest contributors first. Budget-only incidents contain every retained event.
    pub contributors: Vec<ForegroundWorkSample>,
    /// Total foreground occupancy used by the budget rule.
    pub foreground_spend: Duration,
}

impl HangIncident {
    fn detect(
        snapshot: ForegroundIntervalSnapshot,
        threshold: Duration,
        frame_budget: Duration,
    ) -> Option<Self> {
        let foreground_spend = snapshot.foreground_spend();
        let mut contributors = snapshot
            .events
            .iter()
            .copied()
            .filter(|event| event.duration() >= threshold)
            .collect::<Vec<_>>();

        let trigger = if contributors.is_empty() {
            // Cumulative inference is not reliable once the interval event cap has dropped work.
            if snapshot.dropped_events != 0 || foreground_spend < frame_budget {
                return None;
            }
            contributors = snapshot.events.clone();
            HangTrigger::Budget
        } else {
            HangTrigger::Threshold
        };

        contributors.sort_unstable_by_key(|event| std::cmp::Reverse(event.duration()));
        Some(Self {
            trigger,
            snapshot,
            contributors,
            foreground_spend,
        })
    }
}

/// Polls sealed foreground intervals for threshold- and budget-qualified hangs.
///
/// A detector is bound to the foreground thread on which it is created.
pub struct HangDetector {
    collector: ForegroundIntervalCollector,
    threshold: Duration,
    frame_budget: Duration,
}

impl HangDetector {
    /// Creates a detector using explicit per-event and cumulative interval budgets.
    pub fn new(threshold: Duration, frame_budget: Duration) -> Self {
        assert!(
            threshold >= TASK_POLL_FLOOR,
            "hang threshold must be at least the foreground task-poll floor"
        );
        assert!(!frame_budget.is_zero(), "frame budget must be non-zero");
        Self {
            collector: ForegroundIntervalCollector::new(),
            threshold,
            frame_budget,
        }
    }

    /// Creates a detector with BMCBL's release/debug defaults.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_HANG_THRESHOLD, DEFAULT_FRAME_BUDGET)
    }

    /// Returns incidents sealed since the previous poll.
    pub fn poll(&mut self) -> Vec<HangIncident> {
        self.collector
            .collect_unseen()
            .intervals
            .into_iter()
            .filter_map(|snapshot| {
                HangIncident::detect(snapshot, self.threshold, self.frame_budget)
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct WorkValidity {
    power_epoch: u64,
    window: Option<(u64, u64)>,
}

/// RAII timing span used by GPUI foreground hot paths.
pub(crate) struct ForegroundWorkSpan {
    kind: ForegroundWorkKind,
    started_at: Instant,
    validity: WorkValidity,
    seal_idle_on_drop: bool,
}

impl ForegroundWorkSpan {
    pub(crate) fn task_poll(location: &'static std::panic::Location<'static>) -> Self {
        Self::new(
            ForegroundWorkKind::TaskPoll {
                file: location.file(),
                line: location.line(),
            },
            None,
            false,
        )
    }

    pub(crate) fn action(name: &'static str) -> Self {
        Self::new(ForegroundWorkKind::Action { name }, None, true)
    }

    pub(crate) fn input(kind: &'static str, window_id: u64) -> Self {
        Self::new(
            ForegroundWorkKind::Input { kind, window_id },
            Some(window_id),
            true,
        )
    }

    pub(crate) fn draw(window_id: u64) -> Self {
        let span = Self::new(
            ForegroundWorkKind::Draw { window_id },
            Some(window_id),
            false,
        );
        record_draw_started(window_id, span.started_at);
        span
    }

    pub(crate) fn submit(window_id: u64) -> Self {
        let span = Self::new(
            ForegroundWorkKind::Submit { window_id },
            Some(window_id),
            false,
        );
        record_submit_started(window_id, span.started_at);
        span
    }

    fn new(
        kind: ForegroundWorkKind,
        window_id: Option<u64>,
        seal_idle_on_drop: bool,
    ) -> Self {
        let started_at = Instant::now();
        let validity = with_state(|state| {
            state.active_work_depth = state.active_work_depth.saturating_add(1);
            WorkValidity {
                power_epoch: state.power_epoch,
                window: window_id.map(|id| {
                    let window = state.windows.entry(id).or_default();
                    (id, window.visibility_epoch)
                }),
            }
        });
        Self {
            kind,
            started_at,
            validity,
            seal_idle_on_drop,
        }
    }
}

impl Drop for ForegroundWorkSpan {
    fn drop(&mut self) {
        let ended_at = Instant::now();
        with_state(|state| {
            let valid = state.sample_is_valid(self.validity);
            if valid {
                match self.kind {
                    ForegroundWorkKind::Draw { window_id } => {
                        state.record_draw_finished(window_id, ended_at);
                    }
                    ForegroundWorkKind::Submit { window_id } => {
                        state.record_submit_finished(window_id, ended_at);
                    }
                    _ => {}
                }

                state.record_work(ForegroundWorkSample {
                    kind: self.kind,
                    started_at: self.started_at,
                    ended_at,
                });
            }

            state.active_work_depth = state.active_work_depth.saturating_sub(1);
            if self.seal_idle_on_drop && state.active_work_depth == 0 {
                state.seal_idle_if_ready(ended_at);
            }
        });
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingFrame {
    dirty_at: Instant,
    request_at: Instant,
    draw_start: Option<Instant>,
    draw_end: Option<Instant>,
    submit_start: Option<Instant>,
    submit_end: Option<Instant>,
    power_epoch: u64,
    visibility_epoch: u64,
}

#[derive(Clone, Copy, Debug)]
struct WindowProfileState {
    visible: bool,
    visibility_epoch: u64,
    pending_frame: Option<PendingFrame>,
}

impl Default for WindowProfileState {
    fn default() -> Self {
        Self {
            visible: true,
            visibility_epoch: 0,
            pending_frame: None,
        }
    }
}

#[derive(Default)]
struct IntervalBuilder {
    started_at: Option<Instant>,
    events: Vec<ForegroundWorkSample>,
    small_poll_count: u64,
    small_poll_total: Duration,
    dropped_events: u64,
}

impl IntervalBuilder {
    fn is_empty(&self) -> bool {
        self.events.is_empty() && self.small_poll_count == 0 && self.dropped_events == 0
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

struct ProfilerState {
    awake: bool,
    power_epoch: u64,
    active_work_depth: usize,
    windows: HashMap<u64, WindowProfileState>,
    interval: IntervalBuilder,
    snapshots: VecDeque<(u64, ForegroundIntervalSnapshot)>,
    next_snapshot_sequence: u64,
}

impl Default for ProfilerState {
    fn default() -> Self {
        Self {
            awake: true,
            power_epoch: 0,
            active_work_depth: 0,
            windows: HashMap::new(),
            interval: IntervalBuilder::default(),
            snapshots: VecDeque::with_capacity(SNAPSHOT_CAPACITY),
            next_snapshot_sequence: 0,
        }
    }
}

impl ProfilerState {
    fn sample_is_valid(&self, validity: WorkValidity) -> bool {
        if !self.awake || validity.power_epoch != self.power_epoch {
            return false;
        }
        validity.window.is_none_or(|(id, epoch)| {
            self.windows
                .get(&id)
                .is_some_and(|window| window.visible && window.visibility_epoch == epoch)
        })
    }

    fn record_work(&mut self, event: ForegroundWorkSample) {
        self.interval.started_at.get_or_insert(event.started_at);

        let duration = event.duration();
        if matches!(event.kind, ForegroundWorkKind::TaskPoll { .. })
            && duration < TASK_POLL_FLOOR
        {
            self.interval.small_poll_count = self.interval.small_poll_count.saturating_add(1);
            self.interval.small_poll_total += duration;
            return;
        }

        if self.interval.events.len() >= MAX_INTERVAL_EVENTS {
            self.interval.dropped_events = self.interval.dropped_events.saturating_add(1);
        } else {
            self.interval.events.push(event);
        }
    }

    fn record_draw_finished(&mut self, window_id: u64, at: Instant) {
        if let Some(pending) = self
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.pending_frame.as_mut())
        {
            pending.draw_end = Some(at);
        }
    }

    fn record_submit_finished(&mut self, window_id: u64, at: Instant) {
        if let Some(pending) = self
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.pending_frame.as_mut())
        {
            pending.submit_end = Some(at);
        }
    }

    fn seal_idle_if_ready(&mut self, now: Instant) {
        if self.active_work_depth != 0 || self.has_unexpired_pending_frame(now) {
            return;
        }
        // Folded-only wakeups are discarded so sparse sub-100us timers cannot accumulate toward a
        // later unrelated budget incident.
        if self.interval.events.is_empty() {
            self.interval.clear();
            return;
        }
        self.seal(ForegroundIntervalBoundary::Idle { ended_at: now }, now);
    }

    fn has_unexpired_pending_frame(&mut self, now: Instant) -> bool {
        for window in self.windows.values_mut() {
            if window
                .pending_frame
                .is_some_and(|frame| now.saturating_duration_since(frame.request_at) >= PENDING_FRAME_DEADLINE)
            {
                window.pending_frame = None;
            }
        }
        self.awake
            && self
                .windows
                .values()
                .any(|window| window.visible && window.pending_frame.is_some())
    }

    fn seal(&mut self, boundary: ForegroundIntervalBoundary, ended_at: Instant) {
        if self.interval.is_empty() {
            self.interval.clear();
            return;
        }
        let started_at = self.interval.started_at.unwrap_or(ended_at);
        let snapshot = ForegroundIntervalSnapshot {
            started_at,
            boundary,
            events: std::mem::take(&mut self.interval.events),
            small_poll_count: std::mem::take(&mut self.interval.small_poll_count),
            small_poll_total: std::mem::take(&mut self.interval.small_poll_total),
            dropped_events: std::mem::take(&mut self.interval.dropped_events),
        };
        self.interval.started_at = None;

        let sequence = self.next_snapshot_sequence;
        self.next_snapshot_sequence = self.next_snapshot_sequence.wrapping_add(1);
        if self.snapshots.len() == SNAPSHOT_CAPACITY {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back((sequence, snapshot));
    }

    fn snapshots_since(
        &self,
        next_sequence: &mut u64,
    ) -> CollectedForegroundIntervals {
        let first_retained = self
            .snapshots
            .front()
            .map(|(sequence, _)| *sequence)
            .unwrap_or(self.next_snapshot_sequence);
        let lost = first_retained.saturating_sub(*next_sequence);
        *next_sequence = (*next_sequence).max(first_retained);
        let intervals = self
            .snapshots
            .iter()
            .filter(|(sequence, _)| *sequence >= *next_sequence)
            .map(|(_, snapshot)| snapshot.clone())
            .collect::<Vec<_>>();
        *next_sequence = self.next_snapshot_sequence;
        CollectedForegroundIntervals { intervals, lost }
    }
}

thread_local! {
    static PROFILER: RefCell<ProfilerState> = RefCell::new(ProfilerState::default());
}

fn with_state<R>(f: impl FnOnce(&mut ProfilerState) -> R) -> R {
    PROFILER.with(|state| f(&mut state.borrow_mut()))
}

/// Records the first platform frame request for a dirty generation.
pub(crate) fn record_frame_request(window_id: u64, dirty_at: Option<Instant>) {
    let Some(dirty_at) = dirty_at else {
        return;
    };
    let now = Instant::now();
    with_state(|state| {
        if !state.awake {
            return;
        }
        let window = state.windows.entry(window_id).or_default();
        if !window.visible {
            return;
        }

        match window.pending_frame {
            Some(ref mut pending)
                if pending.power_epoch == state.power_epoch
                    && pending.visibility_epoch == window.visibility_epoch =>
            {
                pending.dirty_at = pending.dirty_at.min(dirty_at);
                pending.request_at = pending.request_at.min(now);
            }
            _ => {
                window.pending_frame = Some(PendingFrame {
                    dirty_at,
                    request_at: now,
                    draw_start: None,
                    draw_end: None,
                    submit_start: None,
                    submit_end: None,
                    power_epoch: state.power_epoch,
                    visibility_epoch: window.visibility_epoch,
                });
            }
        }
    });
}

fn record_draw_started(window_id: u64, at: Instant) {
    with_state(|state| {
        if let Some(pending) = state
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.pending_frame.as_mut())
        {
            // A new draw attempt supersedes the stage timing of any earlier failed attempt. Those
            // attempts remain visible as ordinary Draw/Submit work contributors in the interval.
            pending.draw_start = Some(at);
            pending.draw_end = None;
            pending.submit_start = None;
            pending.submit_end = None;
        }
    });
}

fn record_submit_started(window_id: u64, at: Instant) {
    with_state(|state| {
        if let Some(pending) = state
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.pending_frame.as_mut())
        {
            pending.submit_start = Some(at);
            pending.submit_end = None;
        }
    });
}

/// Records the presentation boundary for a newly drawn frame.
pub(crate) fn record_frame_presented(window_id: u64, presented_at: Instant) {
    with_state(|state| {
        let pending = {
            let Some(window) = state.windows.get_mut(&window_id) else {
                return;
            };
            let Some(pending) = window.pending_frame else {
                return;
            };
            if !state.awake
                || !window.visible
                || pending.power_epoch != state.power_epoch
                || pending.visibility_epoch != window.visibility_epoch
                || pending.draw_start.is_none()
            {
                return;
            }
            window
                .pending_frame
                .take()
                .expect("pending frame was checked above")
        };

        let between = |start: Option<Instant>, end: Option<Instant>| {
            start.zip(end)
                .map(|(start, end)| end.saturating_duration_since(start))
        };
        let sample = FramePipelineSample {
            window_id,
            presented_at,
            dirty_to_request: Some(
                pending
                    .request_at
                    .saturating_duration_since(pending.dirty_at),
            ),
            request_to_draw: pending
                .draw_start
                .map(|draw| draw.saturating_duration_since(pending.request_at)),
            draw: between(pending.draw_start, pending.draw_end),
            draw_to_submit: between(pending.draw_end, pending.submit_start),
            submit: between(pending.submit_start, pending.submit_end),
            submit_to_present: pending
                .submit_end
                .map(|submit_end| presented_at.saturating_duration_since(submit_end)),
            dirty_to_present: Some(presented_at.saturating_duration_since(pending.dirty_at)),
            request_to_present: Some(presented_at.saturating_duration_since(pending.request_at)),
        };
        state.seal(ForegroundIntervalBoundary::Presented(sample), presented_at);
    });
}

/// Updates profiler visibility state and invalidates any in-flight sample for the old epoch.
pub(crate) fn record_window_visibility(window_id: u64, visible: bool) {
    with_state(|state| {
        let window = state.windows.entry(window_id).or_default();
        if window.visible != visible {
            window.visible = visible;
            window.visibility_epoch = window.visibility_epoch.wrapping_add(1);
            window.pending_frame = None;
        }
    });
}

/// Removes a closed window from pending-frame accounting.
pub(crate) fn record_window_closed(window_id: u64) {
    with_state(|state| {
        state.windows.remove(&window_id);
    });
}

/// Records sleep/wake and cuts the current measurement epoch at the transition.
pub(crate) fn record_power_state(awake: bool) {
    let now = Instant::now();
    with_state(|state| {
        if state.awake == awake {
            return;
        }
        state.seal(
            ForegroundIntervalBoundary::PowerTransition { ended_at: now },
            now,
        );
        state.awake = awake;
        state.power_epoch = state.power_epoch.wrapping_add(1);
        for window in state.windows.values_mut() {
            window.pending_frame = None;
        }
    });
}

/// Seals foreground work when the platform task pump is truly idle and no visible frame is pending.
pub(crate) fn record_foreground_idle() {
    let now = Instant::now();
    with_state(|state| state.seal_idle_if_ready(now));
}

/// Clears an in-flight frame that finished generating without producing content to present.
pub(crate) fn record_frame_no_present(window_id: u64) {
    with_state(|state| {
        if let Some(window) = state.windows.get_mut(&window_id) {
            window.pending_frame = None;
        }
    });
}


#[cfg(test)]
mod tests {
    use super::*;

    fn input_event(start: Instant, duration: Duration) -> ForegroundWorkSample {
        ForegroundWorkSample {
            kind: ForegroundWorkKind::Input {
                kind: "test",
                window_id: 1,
            },
            started_at: start,
            ended_at: start + duration,
        }
    }

    fn snapshot(events: Vec<ForegroundWorkSample>, dropped_events: u64) -> ForegroundIntervalSnapshot {
        let started_at = events
            .first()
            .map(|event| event.started_at)
            .unwrap_or_else(Instant::now);
        let ended_at = events
            .last()
            .map(|event| event.ended_at)
            .unwrap_or(started_at);
        ForegroundIntervalSnapshot {
            started_at,
            boundary: ForegroundIntervalBoundary::Idle { ended_at },
            events,
            small_poll_count: 0,
            small_poll_total: Duration::ZERO,
            dropped_events,
        }
    }

    #[test]
    fn threshold_trigger_wins_when_one_event_crosses_threshold() {
        let start = Instant::now();
        let incident = HangIncident::detect(
            snapshot(
                vec![
                    input_event(start, Duration::from_millis(4)),
                    input_event(start + Duration::from_millis(5), Duration::from_millis(12)),
                ],
                0,
            ),
            Duration::from_millis(10),
            Duration::from_millis(8),
        )
        .expect("threshold-qualified interval");

        assert_eq!(incident.trigger, HangTrigger::Threshold);
        assert_eq!(incident.contributors.len(), 1);
        assert_eq!(incident.contributors[0].duration(), Duration::from_millis(12));
    }

    #[test]
    fn budget_trigger_accumulates_individually_small_work() {
        let start = Instant::now();
        let incident = HangIncident::detect(
            snapshot(
                vec![
                    input_event(start, Duration::from_millis(5)),
                    input_event(start + Duration::from_millis(6), Duration::from_millis(5)),
                    input_event(start + Duration::from_millis(12), Duration::from_millis(5)),
                ],
                0,
            ),
            Duration::from_millis(10),
            Duration::from_millis(15),
        )
        .expect("budget-qualified interval");

        assert_eq!(incident.trigger, HangTrigger::Budget);
        assert_eq!(incident.contributors.len(), 3);
        assert_eq!(incident.foreground_spend, Duration::from_millis(15));
    }

    #[test]
    fn dropped_events_suppress_budget_inference_but_not_threshold_detection() {
        let start = Instant::now();
        let budget_only = HangIncident::detect(
            snapshot(
                vec![
                    input_event(start, Duration::from_millis(5)),
                    input_event(start + Duration::from_millis(6), Duration::from_millis(5)),
                ],
                1,
            ),
            Duration::from_millis(20),
            Duration::from_millis(10),
        );
        assert!(budget_only.is_none());

        let threshold = HangIncident::detect(
            snapshot(vec![input_event(start, Duration::from_millis(25))], 1),
            Duration::from_millis(20),
            Duration::from_millis(10),
        )
        .expect("directly observed threshold hang remains valid");
        assert_eq!(threshold.trigger, HangTrigger::Threshold);
    }

    #[test]
    fn nested_work_is_not_double_counted_in_budget_occupancy() {
        let start = Instant::now();
        let outer = input_event(start, Duration::from_millis(10));
        let inner = ForegroundWorkSample {
            kind: ForegroundWorkKind::Action { name: "nested" },
            started_at: start + Duration::from_millis(2),
            ended_at: start + Duration::from_millis(8),
        };
        let sample = snapshot(vec![outer, inner], 0);
        assert_eq!(sample.foreground_spend(), Duration::from_millis(10));
    }

    #[test]
    fn visibility_epoch_discards_window_work_spanning_hide() {
        with_state(|state| *state = ProfilerState::default());
        record_window_visibility(7, true);

        let span = ForegroundWorkSpan::input("mouse_move", 7);
        record_window_visibility(7, false);
        drop(span);

        with_state(|state| {
            assert!(state.interval.is_empty());
            assert!(state.snapshots.is_empty());
            assert_eq!(state.active_work_depth, 0);
        });
    }

    #[test]
    fn power_epoch_discards_work_spanning_sleep_wake() {
        with_state(|state| *state = ProfilerState::default());

        let span = ForegroundWorkSpan::action("test::Action");
        record_power_state(false);
        record_power_state(true);
        drop(span);

        with_state(|state| {
            assert!(state.interval.is_empty());
            assert!(state.snapshots.is_empty());
            assert_eq!(state.active_work_depth, 0);
            assert!(state.awake);
        });
    }
}
