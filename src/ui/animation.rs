use gpui::Window;
use std::time::{Duration, Instant};

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t.powi(3)
}

pub fn ease_out_back(t: f32, overshoot: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let p = t - 1.0;
    1.0 + (overshoot + 1.0) * p.powi(3) + overshoot * p.powi(2)
}

pub fn eased_progress(now: Instant, started_at: Instant, duration: Duration) -> f32 {
    let elapsed = now.saturating_duration_since(started_at);
    let duration = duration.max(Duration::from_millis(1));
    (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

pub fn is_running(now: Instant, started_at: Option<Instant>, duration: Duration) -> bool {
    started_at.is_some_and(|t0| now.saturating_duration_since(t0) < duration)
}

pub fn request_animation_frame_if(window: &mut Window, animating: bool) {
    if animating {
        window.request_animation_frame();
    }
}

pub fn request_animation_frame_if_active(window: &mut Window, animating: bool) {
    if animating && window.is_window_active() {
        window.request_animation_frame();
    }
}

pub fn request_animation_frame_until(window: &mut Window, deadline: Option<Instant>) {
    if deadline.is_some_and(|deadline| Instant::now() < deadline) {
        window.request_animation_frame();
    }
}

pub fn request_animation_frame_until_active(window: &mut Window, deadline: Option<Instant>) {
    if window.is_window_active() {
        request_animation_frame_until(window, deadline);
    }
}
