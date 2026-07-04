use crate::ui::animation::{ease_out_back, ease_out_cubic, eased_progress, is_running};
use gpui::Global;
use std::time::{Duration, Instant};

pub struct NavState {
    pub active_index: usize,
    pub pending_route_index: Option<usize>,
    pub pill_from_steps: f32,
    pub pill_to_steps: f32,
    pub pill_started_at: Option<Instant>,
    pub pill_duration: Duration,
    pub pill_from_index: usize,
    pub pill_to_index: usize,

    pub labels_from: f32,
    pub labels_to: f32,
    pub labels_started_at: Option<Instant>,
    pub labels_duration: Duration,
    pub labels_opacity_from: f32,
    pub labels_opacity_to: f32,
    pub labels_opacity_duration: Duration,
    pub labels_opacity_delay: Duration,
    pub labels_target_visible: bool,
}

impl Global for NavState {}

impl Default for NavState {
    fn default() -> Self {
        Self {
            active_index: 0,
            pending_route_index: None,
            pill_from_steps: 0.0,
            pill_to_steps: 0.0,
            pill_started_at: None,
            pill_duration: Duration::from_millis(250),
            pill_from_index: 0,
            pill_to_index: 0,

            labels_from: 1.0,
            labels_to: 1.0,
            labels_started_at: None,
            labels_duration: Duration::from_millis(320),
            labels_opacity_from: 1.0,
            labels_opacity_to: 1.0,
            labels_opacity_duration: Duration::from_millis(180),
            labels_opacity_delay: Duration::from_millis(40),
            labels_target_visible: true,
        }
    }
}

impl NavState {
    pub fn visual_active_index(&self) -> usize {
        self.pending_route_index.unwrap_or(self.active_index)
    }

    pub fn start_pill_animation(&mut self, to_index: usize, now: Instant) {
        if self.pending_route_index == Some(to_index) {
            return;
        }
        if self.active_index == to_index && self.pending_route_index.is_none() {
            return;
        }
        let cur_steps = self.pill_steps(now);
        let cur_index = self.visual_active_index();
        self.pill_from_steps = cur_steps;
        self.pill_to_steps = to_index as f32;
        self.pill_started_at = Some(now);
        self.pill_from_index = cur_index;
        self.pill_to_index = to_index;
        self.pending_route_index = Some(to_index);
    }

    pub fn sync_to_route(&mut self, index: usize) {
        self.active_index = index;
        self.pending_route_index = None;
        self.pill_from_steps = index as f32;
        self.pill_to_steps = index as f32;
        self.pill_started_at = None;
        self.pill_from_index = index;
        self.pill_to_index = index;
    }

    pub fn confirm_route(&mut self, index: usize) {
        if self.pending_route_index == Some(index) {
            self.active_index = index;
            return;
        }

        self.sync_to_route(index);
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        is_running(now, self.pill_started_at, self.pill_duration) || self.labels_animating(now)
    }

    pub fn pill_steps(&self, now: Instant) -> f32 {
        let Some(t0) = self.pill_started_at else {
            return self.active_index as f32;
        };
        let t = eased_progress(now, t0, self.pill_duration);
        let eased = ease_out_back(t, 0.45);

        self.pill_from_steps + (self.pill_to_steps - self.pill_from_steps) * eased
    }

    pub fn pill_direction(&self) -> f32 {
        (self.pill_to_steps - self.pill_from_steps).signum()
    }

    pub fn pill_leading_progress(&self, now: Instant) -> f32 {
        let Some(t0) = self.pill_started_at else {
            return 1.0;
        };
        let t = eased_progress(now, t0, self.pill_duration);
        let advanced = (t / 0.68).clamp(0.0, 1.0);
        ease_out_back(advanced, 0.40).clamp(0.0, 1.10)
    }

    pub fn pill_trailing_progress(&self, now: Instant) -> f32 {
        let Some(t0) = self.pill_started_at else {
            return 1.0;
        };
        let t = eased_progress(now, t0, self.pill_duration);
        let delayed = ((t - 0.18) / 0.82).clamp(0.0, 1.0);
        let eased = if delayed < 0.70 {
            ease_out_cubic((delayed / 0.70).clamp(0.0, 1.0)) * 0.90
        } else {
            let rebound_t = ((delayed - 0.70) / 0.30).clamp(0.0, 1.0);
            0.90 + (ease_out_back(rebound_t, 0.22) - 1.0) * 0.10 + rebound_t * 0.10
        };
        eased.clamp(0.0, 1.04)
    }

    pub fn set_labels_target(&mut self, visible: bool, now: Instant) {
        if self.labels_target_visible == visible {
            return;
        }
        self.labels_target_visible = visible;

        let cur_layout = self.labels_layout_factor(now);
        let cur_opacity = self.labels_opacity_factor(now);
        self.labels_from = cur_layout;
        self.labels_to = if visible { 1.0 } else { 0.0 };
        self.labels_opacity_from = cur_opacity;
        self.labels_opacity_to = if visible { 1.0 } else { 0.0 };
        self.labels_duration = if visible {
            Duration::from_millis(320)
        } else {
            Duration::from_millis(220)
        };
        self.labels_opacity_duration = if visible {
            Duration::from_millis(180)
        } else {
            Duration::from_millis(120)
        };
        self.labels_opacity_delay = if visible {
            Duration::from_millis(40)
        } else {
            Duration::ZERO
        };
        self.labels_started_at = Some(now);
    }

    pub fn set_labels_target_immediate(&mut self, visible: bool) {
        let target = if visible { 1.0 } else { 0.0 };
        self.labels_target_visible = visible;
        self.labels_from = target;
        self.labels_to = target;
        self.labels_opacity_from = target;
        self.labels_opacity_to = target;
        self.labels_started_at = None;
    }

    pub fn labels_animating(&self, now: Instant) -> bool {
        self.labels_started_at.is_some_and(|t0| {
            let elapsed = now.saturating_duration_since(t0);
            elapsed < self.labels_duration
                || elapsed < self.labels_opacity_delay + self.labels_opacity_duration
        })
    }

    pub fn labels_layout_factor(&self, now: Instant) -> f32 {
        let Some(t0) = self.labels_started_at else {
            return if self.labels_target_visible { 1.0 } else { 0.0 };
        };
        let t = eased_progress(now, t0, self.labels_duration);

        // easeInOutCubic: smoother than back-easing for text width changes
        let eased = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        };

        (self.labels_from + (self.labels_to - self.labels_from) * eased).clamp(0.0, 1.0)
    }

    pub fn labels_opacity_factor(&self, now: Instant) -> f32 {
        let Some(t0) = self.labels_started_at else {
            return if self.labels_target_visible { 1.0 } else { 0.0 };
        };
        let elapsed = now.saturating_duration_since(t0);
        if elapsed <= self.labels_opacity_delay {
            return self.labels_opacity_from;
        }
        let dt = elapsed - self.labels_opacity_delay;
        let t = (dt.as_secs_f32()
            / self
                .labels_opacity_duration
                .max(Duration::from_millis(1))
                .as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = if self.labels_opacity_to > self.labels_opacity_from {
            // ease-out when showing
            ease_out_cubic(t)
        } else {
            // ease-in when hiding
            t.powi(3)
        };
        (self.labels_opacity_from + (self.labels_opacity_to - self.labels_opacity_from) * eased)
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_route_preserves_pending_pill_animation() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(5, now);
        assert_eq!(nav.pending_route_index, Some(5));
        assert!(nav.is_animating(now));

        nav.confirm_route(5);

        assert_eq!(nav.active_index, 5);
        assert_eq!(nav.pending_route_index, Some(5));
        assert!(nav.is_animating(now));
    }

    #[test]
    fn immediate_label_target_does_not_leave_animation_running() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.set_labels_target(false, now);
        assert!(nav.labels_animating(now));

        nav.set_labels_target_immediate(false);

        assert!(!nav.labels_animating(now));
        assert_eq!(nav.labels_layout_factor(now), 0.0);
        assert_eq!(nav.labels_opacity_factor(now), 0.0);
    }
}
