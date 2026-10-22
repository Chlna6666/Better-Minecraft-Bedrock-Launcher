#![expect(
    unsafe_code,
    reason = "the vsync scheduler owns native wait handles and callbacks"
)]

use std::{
    mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_APP,
    },
};

use super::WindowsUserEvent;

const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_667);
const BACKGROUND_FRAME_INTERVAL: Duration = Duration::from_micros(66_667);
const EARLY_VSYNC_RETURN_THRESHOLD: Duration = Duration::from_millis(1);
/// Only the final fraction of an Immediate frame is actively aligned to its
/// deadline. Parking across that last fraction produces quantized 1ms wake
/// times on some Windows timer configurations; spinning the whole interval
/// would waste a core, so keep this margin small.
const UNPACED_FINE_WAIT_MARGIN: Duration = Duration::from_micros(125);
const MAX_REASONABLE_VSYNC_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const WM_MODAL_VSYNC: u32 = WM_APP + 0x475;
pub(super) const WM_MODAL_UI_TASKS: u32 = WM_APP + 0x476;

#[derive(Clone, Copy, Debug)]
pub(crate) struct VSyncEventTiming {
    pub(super) pacing_wait: Duration,
    pub(super) reported_refresh_period: Option<Duration>,
    pub(super) reported_composition_period: Option<Duration>,
    pub(super) enqueued_at: Instant,
}

#[derive(Clone, Copy)]
struct VSyncPacing {
    elapsed: Duration,
    reported_refresh_period: Option<Duration>,
    reported_composition_period: Option<Duration>,
}

#[derive(Clone, Copy)]
struct DwmRefreshTiming {
    wait_until_refresh: Duration,
    refresh_period: Duration,
    composition_period: Option<Duration>,
}

pub(super) struct VSyncScheduler {
    refresh_interval_ns: AtomicU64,
    active: AtomicBool,
    frame_pending: AtomicBool,
    unpaced: AtomicBool,
    shutdown: AtomicBool,
    thread: Mutex<Option<Thread>>,
    modal_frame: Mutex<Option<ModalFrameTarget>>,
    modal_task_wake_pending: AtomicBool,
}

struct ModalFrameTarget {
    hwnd: isize,
    pending: Option<VSyncEventTiming>,
}

impl ModalFrameTarget {
    fn publish(&mut self, timing: VSyncEventTiming) -> bool {
        self.pending.replace(timing).is_none()
    }
}

impl VSyncScheduler {
    pub(super) fn new() -> Self {
        Self {
            refresh_interval_ns: AtomicU64::new(DEFAULT_VSYNC_INTERVAL.as_nanos() as u64),
            active: AtomicBool::new(false),
            frame_pending: AtomicBool::new(false),
            unpaced: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            thread: Mutex::new(None),
            modal_frame: Mutex::new(None),
            modal_task_wake_pending: AtomicBool::new(false),
        }
    }

    pub(super) fn set_unpaced(&self, enabled: bool) {
        self.unpaced.store(enabled, Ordering::Release);
        self.unpark();
    }

    pub(super) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) && !self.shutdown.load(Ordering::Acquire)
    }

    pub(super) fn native_unpaced_interval(&self) -> Option<Duration> {
        (self.is_active() && self.unpaced.load(Ordering::Acquire))
            .then(native_unpaced_interval)
    }

    pub(super) fn refresh_interval(&self) -> Duration {
        Duration::from_nanos(self.refresh_interval_ns.load(Ordering::Acquire))
    }

    pub(super) fn request_frame(&self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.frame_pending.store(true, Ordering::Release);
        self.unpark();
        true
    }

    pub(super) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.unpark();
    }

    pub(super) fn start_modal_loop(&self, hwnd: isize) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        *self.modal_frame.lock().expect("modal vsync lock poisoned") = Some(ModalFrameTarget {
            hwnd,
            pending: None,
        });
        self.modal_task_wake_pending.store(false, Ordering::Release);
        self.request_frame()
    }

    pub(super) fn finish_modal_loop(&self, hwnd: isize) {
        let mut target = self.modal_frame.lock().expect("modal vsync lock poisoned");
        if target.as_ref().is_some_and(|target| target.hwnd == hwnd) {
            *target = None;
            self.modal_task_wake_pending.store(false, Ordering::Release);
        }
        drop(target);
        self.request_frame();
    }

    pub(super) fn take_modal_frame(&self, hwnd: isize) -> Option<VSyncEventTiming> {
        self.modal_frame
            .lock()
            .expect("modal vsync lock poisoned")
            .as_mut()
            .filter(|target| target.hwnd == hwnd)
            .and_then(|target| target.pending.take())
    }

    /// Claim a coalesced UI-task wake for the currently active native modal HWND.
    /// A late message from an earlier drag cannot clear a later drag's wake flag.
    pub(super) fn take_modal_ui_task_wake(&self, hwnd: isize) -> bool {
        let modal = self.modal_frame.lock().expect("modal vsync lock poisoned");
        if !modal.as_ref().is_some_and(|target| target.hwnd == hwnd) {
            return false;
        }
        self.modal_task_wake_pending.swap(false, Ordering::AcqRel)
    }

    /// Wake the HWND owner to flush coalesced modal UI work. A live resize can use
    /// this to commit its latest client size before the next DWM VSync; the posted
    /// message itself does not submit a GPU frame. Multiple wakes during one drag
    /// are combined into one Win32 message. Returns true when the active native
    /// modal loop owns the wake, so callers can avoid redundant outer-loop events.
    pub(super) fn wake_modal_ui_tasks(&self) -> bool {
        let modal = self.modal_frame.lock().expect("modal vsync lock poisoned");
        let Some(target) = modal.as_ref() else {
            return false;
        };
        if self.modal_task_wake_pending.swap(true, Ordering::AcqRel) {
            return true;
        }
        if let Err(error) = unsafe {
            PostMessageW(
                Some(HWND(target.hwnd as *mut _)),
                WM_MODAL_UI_TASKS,
                WPARAM(0),
                LPARAM(0),
            )
        } {
            self.modal_task_wake_pending.store(false, Ordering::Release);
            log::warn!("failed to post GPUI modal foreground task wake: {error}");
            return false;
        }
        true
    }

    fn post_modal_frame(&self, timing: VSyncEventTiming) -> bool {
        let mut modal = self.modal_frame.lock().expect("modal vsync lock poisoned");
        let Some(target) = modal.as_mut() else {
            return false;
        };
        if !target.publish(timing) {
            return true;
        }
        // SAFETY: The native owner registers this HWND on enter and clears it on exit/destroy.
        // PostMessage does not borrow UI objects; the latest timing remains in the scheduler.
        if let Err(error) = unsafe {
            PostMessageW(
                Some(HWND(target.hwnd as *mut _)),
                WM_MODAL_VSYNC,
                WPARAM(0),
                LPARAM(0),
            )
        } {
            log::warn!("failed to post native modal vsync frame: {error}");
            *modal = None;
            return false;
        }
        true
    }

    fn unpark(&self) {
        if let Some(thread) = self
            .thread
            .lock()
            .expect("vsync thread lock poisoned")
            .as_ref()
        {
            thread.unpark();
        }
    }
}

pub(super) fn spawn_vsync_thread(
    event_loop_proxy: winit::event_loop::EventLoopProxy<WindowsUserEvent>,
    scheduler: Arc<VSyncScheduler>,
) -> std::io::Result<()> {
    let thread_scheduler = scheduler.clone();
    let join_handle = std::thread::Builder::new()
        .name("GPUI DWM VSync".to_string())
        .spawn(move || {
            let interval = dwm_refresh_interval().unwrap_or(DEFAULT_VSYNC_INTERVAL);
            thread_scheduler.refresh_interval_ns.store(interval.as_nanos() as u64, Ordering::Release);
            let refresh_rate_hz = interval.as_secs_f64().recip();
            log::info!(
                "GPUI Windows DWM frame pacing enabled: refresh_rate_hz={refresh_rate_hz:.3} interval={interval:?} background_interval={BACKGROUND_FRAME_INTERVAL:?}"
            );
            let mut last_foreground_tick = None;
            let mut last_unpaced_tick = None;
            let mut last_background_tick = None;
            while !thread_scheduler.shutdown.load(Ordering::Acquire) {
                if !thread_scheduler.frame_pending.load(Ordering::Acquire) {
                    std::thread::park();
                    continue;
                }

                if !thread_scheduler.frame_pending.swap(false, Ordering::AcqRel) {
                    continue;
                }

                let pacing = if process_owns_foreground_window() {
                    last_background_tick = None;
                    if thread_scheduler.unpaced.load(Ordering::Acquire) {
                        last_foreground_tick = None;
                        wait_for_unpaced_tick(&thread_scheduler, &mut last_unpaced_tick)
                    } else {
                        last_unpaced_tick = None;
                        wait_for_vsync(interval, &mut last_foreground_tick)
                    }
                } else {
                    last_unpaced_tick = None;
                    last_foreground_tick = None;
                    VSyncPacing {
                        elapsed: wait_for_background_tick(
                            &thread_scheduler,
                            &mut last_background_tick,
                        ),
                        reported_refresh_period: None,
                        reported_composition_period: None,
                    }
                };

                if thread_scheduler.shutdown.load(Ordering::Acquire) {
                    break;
                }
                let timing = VSyncEventTiming {
                    pacing_wait: pacing.elapsed,
                    reported_refresh_period: pacing.reported_refresh_period,
                    reported_composition_period: pacing.reported_composition_period,
                    enqueued_at: Instant::now(),
                };
                if let Some(interval) = pacing.reported_refresh_period {
                    thread_scheduler.refresh_interval_ns.store(interval.as_nanos() as u64, Ordering::Release);
                }
                // HWND messages reach Win32's nested move loop while winit's outer handler is
                // busy. Keep the same DWM cadence and a single latest-wins pending frame.
                if thread_scheduler.post_modal_frame(timing) {
                    continue;
                }
                if event_loop_proxy
                    .send_event(WindowsUserEvent::VSync(timing))
                    .is_err()
                {
                    break;
                }
            }
        })?;
    *scheduler.thread.lock().expect("vsync thread lock poisoned") =
        Some(join_handle.thread().clone());
    scheduler.active.store(true, Ordering::Release);
    if scheduler.frame_pending.load(Ordering::Acquire) {
        join_handle.thread().unpark();
    }
    Ok(())
}

// Explicit Immediate mode is clocked independently from DWM. The per-window
// FrameClock still coalesces work and enforces the requested frame cap.
// System clock in Immediate mode uses a 1ms ceiling (not a busy spin).
fn native_unpaced_interval() -> Duration {
    crate::window::default_frame_clock()
        .interval()
        .unwrap_or(Duration::from_millis(1))
}

/// Keep the native frame phase on the requested timeline, even if one native
/// callback wakes late. Resetting to Instant::now() after every late wake
/// accumulates OS timer latency and makes a 1ms cap drift toward 1.5-2ms.
fn unpaced_next_deadline(
    previous_tick: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> Instant {
    let Some(previous_tick) = previous_tick else {
        return now;
    };
    if interval.is_zero() {
        return now;
    }
    let elapsed = now.saturating_duration_since(previous_tick);
    let ticks = (elapsed.as_nanos() / interval.as_nanos()).max(1);
    let ticks = ticks.min(u128::from(u32::MAX)) as u32;
    previous_tick
        .checked_add(interval.saturating_mul(ticks))
        .unwrap_or(now)
}

fn wait_for_unpaced_tick(
    scheduler: &VSyncScheduler,
    last_tick: &mut Option<Instant>,
) -> VSyncPacing {
    let started = Instant::now();
    let interval = native_unpaced_interval();
    let deadline = unpaced_next_deadline(*last_tick, started, interval);
    loop {
        if scheduler.shutdown.load(Ordering::Acquire)
            || !scheduler.unpaced.load(Ordering::Acquire)
            || !process_owns_foreground_window()
        {
            // Do not preserve a stale clock phase when the mode changes.
            *last_tick = None;
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            *last_tick = Some(deadline);
            break;
        }
        let remaining = deadline - now;
        if remaining > UNPACED_FINE_WAIT_MARGIN {
            std::thread::park_timeout(remaining - UNPACED_FINE_WAIT_MARGIN);
        } else {
            std::hint::spin_loop();
        }
    }
    VSyncPacing {
        elapsed: started.elapsed(),
        reported_refresh_period: None,
        reported_composition_period: None,
    }
}

fn wait_for_background_tick(
    scheduler: &VSyncScheduler,
    last_tick: &mut Option<Instant>,
) -> Duration {
    let started_at = Instant::now();
    if let Some(last_tick) = *last_tick {
        let deadline = last_tick + BACKGROUND_FRAME_INTERVAL;
        loop {
            if scheduler.shutdown.load(Ordering::Acquire) || process_owns_foreground_window() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            // Unlike `sleep`, park_timeout can be interrupted by a foreground frame request,
            // so Alt-Tab/restore returns to DWM pacing without waiting for the 15 FPS deadline.
            std::thread::park_timeout(deadline - now);
        }
    }
    *last_tick = Some(Instant::now());
    started_at.elapsed()
}

fn process_owns_foreground_window() -> bool {
    // SAFETY: Both calls only inspect the current foreground HWND/process id. The PID output
    // points to stack storage for the duration of GetWindowThreadProcessId.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut process_id = 0_u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        process_id == std::process::id()
    }
}

fn wait_for_vsync(interval: Duration, last_tick: &mut Option<Instant>) -> VSyncPacing {
    let started_at = Instant::now();
    if let Some(timing) = dwm_next_refresh_wait() {
        // DwmFlush drains this process's pending DirectX updates. Used after DXGI presentation,
        // that completion barrier can consume another refresh instead of pacing the next sample.
        // Follow the current DWM vblank phase; swapchain readiness remains the backpressure gate.
        std::thread::sleep(timing.wait_until_refresh);
        *last_tick = Some(Instant::now());
        return VSyncPacing {
            elapsed: started_at.elapsed(),
            reported_refresh_period: Some(timing.refresh_period),
            reported_composition_period: timing.composition_period,
        };
    }
    // SAFETY: DwmFlush has no pointer parameters and only waits for the compositor.
    let dwm_wait_succeeded = unsafe { DwmFlush() }.is_ok();
    let dwm_wait = started_at.elapsed();

    // A normally blocking DwmFlush is the pacing authority. Do not apply the startup-time
    // refresh interval again after DWM has already released us on the compositor cadence: that
    // cached interval can become stale after a monitor/refresh-rate transition and can otherwise
    // cap a high-refresh window to the old rate. The cached interval is only a fallback for a
    // failed or suspiciously early DwmFlush.
    if !dwm_wait_succeeded || dwm_wait < EARLY_VSYNC_RETURN_THRESHOLD {
        let fallback_deadline = last_tick
            .map(|last_tick| last_tick + interval)
            .unwrap_or(started_at + interval);
        let now = Instant::now();
        if now < fallback_deadline {
            std::thread::sleep(fallback_deadline - now);
        }
    }

    *last_tick = Some(Instant::now());
    VSyncPacing {
        elapsed: started_at.elapsed(),
        reported_refresh_period: None,
        reported_composition_period: None,
    }
}

fn dwm_next_refresh_wait() -> Option<DwmRefreshTiming> {
    let mut timing = DWM_TIMING_INFO {
        cbSize: u32::try_from(mem::size_of::<DWM_TIMING_INFO>()).ok()?,
        ..Default::default()
    };
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    // SAFETY: All outputs point to initialized stack storage; null HWND requests desktop timing.
    unsafe {
        DwmGetCompositionTimingInfo(HWND::default(), &raw mut timing).ok()?;
        QueryPerformanceCounter(&raw mut counter).ok()?;
        QueryPerformanceFrequency(&raw mut frequency).ok()?;
    }
    let frequency = u64::try_from(frequency).ok().filter(|value| *value != 0)?;
    let counter = u64::try_from(counter).ok()?;
    let period = timing.qpcRefreshPeriod;
    if period > frequency {
        return None;
    }
    let ticks = ticks_until_next_refresh(counter, timing.qpcVBlank, period)?;
    if ticks > period {
        return None;
    }
    Some(DwmRefreshTiming {
        wait_until_refresh: Duration::from_secs_f64(ticks as f64 / frequency as f64),
        refresh_period: Duration::from_secs_f64(period as f64 / frequency as f64),
        composition_period: refresh_interval(
            u64::from(timing.rateCompose.uiNumerator),
            u64::from(timing.rateCompose.uiDenominator),
        ),
    })
}

fn ticks_until_next_refresh(counter: u64, vblank: u64, period: u64) -> Option<u64> {
    if period == 0 {
        return None;
    }
    Some(if counter < vblank {
        vblank - counter
    } else {
        period - (counter - vblank) % period
    })
}

fn dwm_refresh_interval() -> Option<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: u32::try_from(mem::size_of::<DWM_TIMING_INFO>()).ok()?,
        ..Default::default()
    };
    // SAFETY: timing_info is valid writable storage and a null HWND requests desktop timing.
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &raw mut timing_info) }.ok()?;
    let numerator = u64::from(timing_info.rateRefresh.uiNumerator);
    let denominator = u64::from(timing_info.rateRefresh.uiDenominator);
    refresh_interval(numerator, denominator)
}

fn refresh_interval(numerator: u64, denominator: u64) -> Option<Duration> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    let interval = Duration::from_secs_f64(denominator as f64 / numerator as f64);
    (!interval.is_zero() && interval <= MAX_REASONABLE_VSYNC_INTERVAL).then_some(interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpaced_deadline_keeps_phase_after_late_native_wake() {
        let start = Instant::now();
        let interval = Duration::from_millis(1);
        assert_eq!(unpaced_next_deadline(None, start, interval), start);
        assert_eq!(
            unpaced_next_deadline(Some(start), start + Duration::from_micros(700), interval),
            start + interval,
        );
        // A 600us late callback must not reset the next 1ms interval to
        // 2.6ms. The next slot remains on the 2ms absolute phase.
        assert_eq!(
            unpaced_next_deadline(Some(start), start + Duration::from_micros(1600), interval),
            start + interval,
        );
        assert_eq!(
            unpaced_next_deadline(
                Some(start + interval),
                start + Duration::from_micros(1750),
                interval,
            ),
            start + Duration::from_millis(2),
        );
        // Missed several slots: drop overdue samples, not the original phase.
        assert_eq!(
            unpaced_next_deadline(Some(start), start + Duration::from_micros(5200), interval),
            start + Duration::from_millis(5),
        );
    }

    #[test]
    fn refresh_phase_skips_missed_ticks_without_drifting() {
        assert_eq!(ticks_until_next_refresh(95, 100, 10), Some(5));
        assert_eq!(ticks_until_next_refresh(100, 100, 10), Some(10));
        assert_eq!(ticks_until_next_refresh(107, 100, 10), Some(3));
        assert_eq!(ticks_until_next_refresh(138, 100, 10), Some(2));
        assert_eq!(ticks_until_next_refresh(100, 100, 0), None);
    }

    #[test]
    fn refresh_interval_uses_reported_display_rate() {
        let interval = refresh_interval(180, 1).expect("180 Hz should be a valid refresh rate");
        assert!((interval.as_secs_f64() - 1.0 / 180.0).abs() < 0.000_001);
    }

    #[test]
    fn refresh_interval_rejects_invalid_rates() {
        assert_eq!(refresh_interval(0, 1), None);
        assert_eq!(refresh_interval(60, 0), None);
        assert_eq!(refresh_interval(1, 2), None);
    }

    #[test]
    fn background_frame_pacing_is_lower_than_normal_vsync() {
        assert!(BACKGROUND_FRAME_INTERVAL > DEFAULT_VSYNC_INTERVAL);
    }

    #[test]
    fn modal_frame_coalesces_to_latest_timing() {
        let first = VSyncEventTiming {
            pacing_wait: Duration::from_millis(4),
            reported_refresh_period: None,
            reported_composition_period: None,
            enqueued_at: Instant::now(),
        };
        let latest = VSyncEventTiming {
            pacing_wait: Duration::from_millis(8),
            reported_refresh_period: Some(Duration::from_micros(4_167)),
            reported_composition_period: Some(Duration::from_micros(8_333)),
            enqueued_at: first.enqueued_at + Duration::from_millis(4),
        };
        let mut target = ModalFrameTarget {
            hwnd: 1,
            pending: None,
        };
        assert!(target.publish(first));
        assert!(!target.publish(latest));
        let received = target
            .pending
            .take()
            .expect("latest frame should be pending");
        assert_eq!(received.enqueued_at, latest.enqueued_at);
        assert_eq!(received.pacing_wait, latest.pacing_wait);
        assert!(target.publish(first));
    }

    #[test]
    fn modal_frame_is_consumed_only_by_registered_window() {
        let scheduler = VSyncScheduler::new();
        scheduler.active.store(true, Ordering::Release);
        assert!(scheduler.start_modal_loop(1));
        let timing = VSyncEventTiming {
            pacing_wait: Duration::ZERO,
            reported_refresh_period: None,
            reported_composition_period: None,
            enqueued_at: Instant::now(),
        };
        scheduler
            .modal_frame
            .lock()
            .expect("modal vsync lock poisoned")
            .as_mut()
            .expect("modal target registered")
            .publish(timing);
        assert!(scheduler.take_modal_frame(2).is_none());
        scheduler.finish_modal_loop(2);
        assert!(scheduler.take_modal_frame(1).is_some());
        assert!(scheduler.take_modal_frame(1).is_none());
        scheduler.finish_modal_loop(1);
        assert!(
            scheduler
                .modal_frame
                .lock()
                .expect("modal vsync lock poisoned")
                .is_none()
        );
    }

    #[test]
    fn unavailable_vsync_keeps_timer_fallback() {
        let scheduler = VSyncScheduler::new();
        assert!(!scheduler.start_modal_loop(1));
        assert!(
            scheduler
                .modal_frame
                .lock()
                .expect("modal vsync lock poisoned")
                .is_none()
        );
    }
}
