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
            duration: Duration::from_millis(360),
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

    pub fn progress(&self, now: Instant) -> f32 {
        let Some(t0) = self.started_at else {
            return 0.0;
        };

        let dt = now.saturating_duration_since(t0);
        let dur = self.duration.max(Duration::from_millis(1));
        (dt.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_progress_is_linear_and_clamped() {
        let started_at = Instant::now();
        let mut state = QuitState::default();
        assert!(state.request_quit(started_at));

        assert_eq!(state.progress(started_at), 0.0);
        assert!((state.progress(started_at + state.duration() / 2) - 0.5).abs() < 0.001);
        assert_eq!(state.progress(started_at + state.duration()), 1.0);
        assert_eq!(
            state.progress(started_at + state.duration() + Duration::from_secs(1)),
            1.0
        );
    }

    #[test]
    fn duplicate_quit_request_does_not_restart_animation() {
        let started_at = Instant::now();
        let mut state = QuitState::default();

        assert!(state.request_quit(started_at));
        assert!(!state.request_quit(started_at + Duration::from_millis(40)));
        assert!((state.progress(started_at + state.duration() / 2) - 0.5).abs() < 0.001);
    }
}
