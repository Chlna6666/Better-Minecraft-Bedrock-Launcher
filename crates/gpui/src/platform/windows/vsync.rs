#![expect(
    unsafe_code,
    reason = "the vsync scheduler owns native wait handles and callbacks"
)]

use std::{
    mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
};

use super::WindowsUserEvent;

const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_667);
const BACKGROUND_FRAME_INTERVAL: Duration = Duration::from_micros(66_667);
const EARLY_VSYNC_RETURN_THRESHOLD: Duration = Duration::from_millis(1);
const MAX_REASONABLE_VSYNC_INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct VSyncScheduler {
    active: AtomicBool,
    frame_pending: AtomicBool,
    shutdown: AtomicBool,
    thread: Mutex<Option<Thread>>,
}

impl VSyncScheduler {
    pub(super) fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            frame_pending: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            thread: Mutex::new(None),
        }
    }

    pub(super) fn request_frame(&self) -> bool {
        if !self.active.load(Ordering::Acquire) {
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
            let refresh_rate_hz = interval.as_secs_f64().recip();
            log::info!(
                "GPUI Windows DWM frame pacing enabled: refresh_rate_hz={refresh_rate_hz:.3} interval={interval:?} background_interval={BACKGROUND_FRAME_INTERVAL:?}"
            );
            let mut last_foreground_tick = None;
            let mut last_background_tick = None;
            while !thread_scheduler.shutdown.load(Ordering::Acquire) {
                if !thread_scheduler.frame_pending.swap(false, Ordering::AcqRel) {
                    std::thread::park();
                    continue;
                }

                if process_owns_foreground_window() {
                    last_background_tick = None;
                    wait_for_vsync(interval, &mut last_foreground_tick);
                } else {
                    last_foreground_tick = None;
                    wait_for_background_tick(&thread_scheduler, &mut last_background_tick);
                }

                if thread_scheduler.shutdown.load(Ordering::Acquire) {
                    break;
                }
                if event_loop_proxy
                    .send_event(WindowsUserEvent::VSync)
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

fn wait_for_background_tick(scheduler: &VSyncScheduler, last_tick: &mut Option<Instant>) {
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

fn wait_for_vsync(interval: Duration, last_tick: &mut Option<Instant>) {
    let started_at = Instant::now();
    // SAFETY: DwmFlush has no pointer parameters and only waits for the compositor.
    let waited = unsafe { DwmFlush() }.is_ok();
    if !waited || started_at.elapsed() < EARLY_VSYNC_RETURN_THRESHOLD {
        std::thread::sleep(interval);
    }
    if let Some(last_tick) = *last_tick {
        let earliest_tick = last_tick + interval;
        let now = Instant::now();
        if now < earliest_tick {
            std::thread::sleep(earliest_tick - now);
        }
    }
    *last_tick = Some(Instant::now());
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
}
