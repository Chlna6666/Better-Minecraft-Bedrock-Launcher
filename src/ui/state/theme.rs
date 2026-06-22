use crate::ui::animation::{eased_progress, is_running};
use gpui::{App, Global, Hsla, SharedString};
use std::time::{Duration, Instant};

pub struct ThemeState {
    pub target_dark: bool,
    pub from: f32,
    pub to: f32,
    pub started_at: Option<Instant>,
    pub duration: Duration,
    // Accent overrides derived from config `custom_style.theme_color` (hex).
    pub accent_hex: SharedString,
    pub accent: Option<Hsla>,
}

impl Global for ThemeState {}

impl Default for ThemeState {
    fn default() -> Self {
        Self {
            target_dark: false,
            from: 0.0,
            to: 0.0,
            started_at: None,
            duration: Duration::from_millis(260),
            accent_hex: SharedString::from(""),
            accent: None,
        }
    }
}

impl ThemeState {
    pub fn sync_component_theme(_dark: bool, _cx: &mut App) {}

    pub fn factor(&self, now: Instant) -> f32 {
        let Some(t0) = self.started_at else {
            return if self.target_dark { 1.0 } else { 0.0 };
        };
        let t = eased_progress(now, t0, self.duration);

        // easeInOutQuad
        let eased = if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
        };

        (self.from + (self.to - self.from) * eased).clamp(0.0, 1.0)
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        is_running(now, self.started_at, self.duration)
    }

    pub fn toggle(&mut self, now: Instant) {
        let cur = self.factor(now);
        self.target_dark = !self.target_dark;
        self.from = cur;
        self.to = if self.target_dark { 1.0 } else { 0.0 };
        self.started_at = Some(now);
    }
}
