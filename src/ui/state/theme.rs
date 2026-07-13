use crate::ui::animation::{is_running, raw_progress};
use gpui::{App, BorrowAppContext, Global, Hsla, SharedString, Timer};
use std::time::{Duration, Instant};

const THEME_TICK_INTERVAL: Duration = Duration::from_millis(16);

pub struct ThemeState {
    pub target_dark: bool,
    pub from: f32,
    pub to: f32,
    pub started_at: Option<Instant>,
    pub duration: Duration,
    // Accent overrides derived from config `custom_style.theme_color` (hex).
    pub accent_hex: SharedString,
    pub accent: Option<Hsla>,
    tick_running: bool,
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
            tick_running: false,
        }
    }
}

impl ThemeState {
    pub fn sync_component_theme(_dark: bool, _cx: &mut App) {}

    pub fn apply_startup_config(theme_color_hex: &str, theme_mode: &str, cx: &mut App) {
        let target_dark = crate::config::config::normalize_theme_mode(theme_mode)
            == crate::config::config::THEME_MODE_DARK;
        let accent = crate::ui::theme::parse_hex_color_to_hsla(theme_color_hex);
        cx.update_global(|theme: &mut ThemeState, cx| {
            theme.target_dark = target_dark;
            theme.from = if target_dark { 1.0 } else { 0.0 };
            theme.to = theme.from;
            theme.started_at = None;
            theme.accent_hex = SharedString::from(theme_color_hex.to_string());
            theme.accent = accent;
            theme.tick_running = false;
            Self::sync_component_theme(target_dark, cx);
        });
    }

    pub fn set_accent_hex(hex: &str, cx: &mut App) {
        let accent = crate::ui::theme::parse_hex_color_to_hsla(hex);
        cx.update_global(|theme: &mut ThemeState, _cx| {
            theme.accent_hex = SharedString::from(hex.to_string());
            theme.accent = accent;
        });
    }

    pub fn toggle_global(cx: &mut App) {
        let now = Instant::now();
        let target_dark = cx.update_global(|theme: &mut ThemeState, cx| {
            let target_dark = theme.toggle(now);
            Self::sync_component_theme(target_dark, cx);
            target_dark
        });
        Self::spawn_animation_tick(cx);
        Self::persist_theme_mode(target_dark, cx);
    }

    pub fn factor(&self, now: Instant) -> f32 {
        let Some(t0) = self.started_at else {
            return if self.target_dark { 1.0 } else { 0.0 };
        };
        let t = raw_progress(now, t0, self.duration);

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

    pub fn toggle(&mut self, now: Instant) -> bool {
        let cur = self.factor(now);
        self.target_dark = !self.target_dark;
        self.from = cur;
        self.to = if self.target_dark { 1.0 } else { 0.0 };
        self.started_at = Some(now);
        self.target_dark
    }

    fn spawn_animation_tick(cx: &mut App) {
        let should_spawn = cx.update_global(|theme: &mut ThemeState, _cx| {
            let should_spawn = !theme.tick_running && theme.is_animating(Instant::now());
            if should_spawn {
                theme.tick_running = true;
            }
            should_spawn
        });
        if !should_spawn {
            return;
        }

        cx.spawn(async move |cx| {
            loop {
                Timer::after(THEME_TICK_INTERVAL).await;
                let still_animating = match cx.update_global(|theme: &mut ThemeState, _cx| {
                    let still_animating = theme.is_animating(Instant::now());
                    if !still_animating {
                        theme.from = theme.to;
                        theme.started_at = None;
                        theme.tick_running = false;
                    }
                    still_animating
                }) {
                    Ok(still_animating) => still_animating,
                    Err(error) => {
                        tracing::warn!("theme animation tick failed: {error:?}");
                        return;
                    }
                };

                if !still_animating {
                    return;
                }
            }
        })
        .detach();
    }

    fn persist_theme_mode(target_dark: bool, cx: &mut App) {
        let theme_mode = if target_dark {
            crate::config::config::THEME_MODE_DARK
        } else {
            crate::config::config::THEME_MODE_LIGHT
        }
        .to_string();

        cx.spawn(async move |_cx| {
            let result = tokio::task::spawn_blocking(move || {
                crate::config::config::update_config(|config| {
                    config.custom_style.theme_mode = theme_mode;
                })
            })
            .await;

            match result {
                Err(error) => tracing::warn!("persist theme mode join error: {error}"),
                Ok(Err(error)) => tracing::warn!("persist theme mode failed: {error}"),
                Ok(Ok(())) => {}
            }
        })
        .detach();
    }
}
