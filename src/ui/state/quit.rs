use gpui::Global;
use std::time::{Duration, Instant};

pub struct QuitState {
    started_at: Option<Instant>,
    duration: Duration,
}

impl Global for QuitState {}

impl Default for QuitState {
    fn default() -> Self {
        Self {
            started_at: None,
            duration: Duration::from_millis(140),
        }
    }
}

impl QuitState {
    pub fn request_quit(&mut self, now: Instant) -> bool {
        if self.started_at.is_some() {
            return false;
        }
        self.started_at = Some(now);
        true
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.started_at
            .is_some_and(|t0| now.saturating_duration_since(t0) < self.duration)
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn factor(&self, now: Instant) -> f32 {
        let Some(t0) = self.started_at else {
            return 0.0;
        };

        let dt = now.saturating_duration_since(t0);
        let dur = self.duration.max(Duration::from_millis(1));
        let t = (dt.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0);

        // ease_out_cubic
        1.0 - (1.0 - t).powi(3)
    }
}
