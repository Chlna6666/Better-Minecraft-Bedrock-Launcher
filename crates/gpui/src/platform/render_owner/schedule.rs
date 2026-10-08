#[cfg(any(test, not(target_os = "windows")))]
use std::time::{Duration, Instant};

/// Native display cadence for retained presentation, independent of UI dispatch.
/// Idle, hidden and notification-backed readiness waits have no deadline.
#[derive(Default)]
pub(super) struct Schedule {
    #[cfg(any(test, not(target_os = "windows")))]
    interval: Option<Duration>,
    #[cfg(any(test, not(target_os = "windows")))]
    deadline: Option<Instant>,
    #[cfg(any(test, not(target_os = "windows")))]
    native_callbacks: bool,
    #[cfg(any(test, not(target_os = "windows")))]
    native_visible: bool,
    #[cfg(any(test, not(target_os = "windows")))]
    retry_delay: Duration,
}

impl Schedule {
    #[cfg(any(test, not(target_os = "windows")))]
    pub(super) fn set_interval(&mut self, interval: Option<Duration>) {
        self.interval = interval.filter(|interval| !interval.is_zero());
        self.deadline = None;
    }

    pub(super) fn is_enabled(&self) -> bool {
        #[cfg(any(test, not(target_os = "windows")))]
        {
            self.interval.is_some() || (self.native_callbacks && self.native_visible)
        }
        #[cfg(all(not(test), target_os = "windows"))]
        {
            false
        }
    }

    #[cfg(any(test, not(target_os = "windows")))]
    pub(super) fn set_native_callbacks(&mut self) {
        self.native_callbacks = true;
    }

    #[cfg(any(test, not(target_os = "windows")))]
    pub(super) fn set_native_visible(&mut self, visible: bool) {
        self.native_visible = visible;
        self.deadline = None;
        self.retry_delay = Duration::ZERO;
    }

    #[cfg(any(test, not(target_os = "windows")))]
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    #[cfg(any(test, not(target_os = "windows")))]
    pub(super) fn after_frame(
        &mut self,
        now: Instant,
        pending: bool,
        waiting_for_ready: bool,
        presentation_deadline: Option<Instant>,
        submitted: bool,
    ) {
        if !pending || waiting_for_ready || !self.is_enabled() {
            self.deadline = None;
            self.retry_delay = Duration::ZERO;
            return;
        }
        if self.native_callbacks {
            if submitted {
                self.deadline = None;
                self.retry_delay = Duration::ZERO;
            } else if let Some(deadline) = presentation_deadline.filter(|deadline| *deadline > now)
            {
                self.deadline = Some(deadline);
            } else {
                // Vulkan can defer acquire without a readiness notification. No buffer commit
                // means no Wayland frame callback; retry only this outstanding work, with backoff.
                self.retry_delay = if self.retry_delay.is_zero() {
                    Duration::from_millis(1)
                } else {
                    (self.retry_delay * 2).min(Duration::from_millis(16))
                };
                self.deadline = Some(now + self.retry_delay);
            }
            return;
        }
        self.deadline = self.interval.map(|interval| {
            let next = self
                .deadline
                .map(|deadline| deadline + interval)
                .filter(|deadline| *deadline > now)
                .unwrap_or(now + interval);
            presentation_deadline.map_or(next, |deadline| next.max(deadline))
        });
    }
}

#[cfg(test)]
mod tests;
