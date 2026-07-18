use gpui::{Animation, AnimationDriver, AnimationSpec, Easing, RepeatMode, Window};
use std::time::{Duration, Instant};

const MIN_ANIMATION_DURATION: Duration = Duration::from_millis(1);

pub fn ease_out_cubic(t: f32) -> f32 {
    Easing::OutCubic.sample(t)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    Easing::InCubic.sample(t)
}

pub fn ease_out_back(t: f32, overshoot: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let p = t - 1.0;
    1.0 + (overshoot + 1.0) * p.powi(3) + overshoot * p.powi(2)
}

pub fn ease_in_back(t: f32, overshoot: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * ((overshoot + 1.0) * t - overshoot)
}

pub fn ease_out_elastic(t: f32) -> f32 {
    Easing::OutElastic.sample(t)
}

pub fn raw_progress(now: Instant, started_at: Instant, duration: Duration) -> f32 {
    let elapsed = now.saturating_duration_since(started_at);
    AnimationSpec::new(duration.max(MIN_ANIMATION_DURATION))
        .sample_elapsed(elapsed)
        .raw_progress
}

pub fn eased_progress(now: Instant, started_at: Instant, duration: Duration) -> f32 {
    let elapsed = now.saturating_duration_since(started_at);
    AnimationSpec::new(duration.max(MIN_ANIMATION_DURATION))
        .ease(Easing::OutCubic)
        .sample_elapsed(elapsed)
        .eased_progress
}

pub fn is_running(now: Instant, started_at: Option<Instant>, duration: Duration) -> bool {
    started_at.is_some_and(|t0| now.saturating_duration_since(t0) < duration)
}

pub fn motion(duration: Duration, easing: Easing) -> Animation {
    element_motion_from_spec(AnimationSpec::new(duration).ease(easing))
}

pub fn repeating_motion(duration: Duration, easing: Easing) -> Animation {
    element_motion_from_spec(
        AnimationSpec::new(duration)
            .ease(easing)
            .repeat(RepeatMode::Forever),
    )
}

pub fn ease_out_cubic_motion(duration: Duration) -> Animation {
    motion(duration, Easing::OutCubic)
}

pub fn ease_in_cubic_motion(duration: Duration) -> Animation {
    motion(duration, Easing::InCubic)
}

pub fn repeating_linear_motion(duration: Duration) -> Animation {
    repeating_motion(duration, Easing::Linear)
}

#[track_caller]
pub fn request_animation_frame_if(window: &mut Window, animating: bool) {
    if animating {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

#[track_caller]
pub fn request_animation_frame_if_active(window: &mut Window, animating: bool) {
    if animating && window.is_window_active() {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

#[track_caller]
pub fn request_animation_frame_until(window: &mut Window, deadline: Option<Instant>) {
    if deadline.is_some_and(|deadline| Instant::now() < deadline) {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

#[track_caller]
pub fn request_animation_frame_until_active(window: &mut Window, deadline: Option<Instant>) {
    if window.is_window_active() {
        request_animation_frame_until(window, deadline);
    }
}

fn element_motion_from_spec(spec: AnimationSpec) -> Animation {
    let duration = spec.duration.max(MIN_ANIMATION_DURATION);
    let repeat_forever = matches!(spec.repeat, RepeatMode::Forever);
    let easing = spec.easing;
    let mut animation = Animation::new(duration).with_easing(move |t| easing.sample(t));
    if repeat_forever {
        animation = animation.repeat();
    }
    animation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eased_progress_applies_default_easing() {
        let started_at = Instant::now();
        let duration = Duration::from_millis(100);
        let now = started_at + Duration::from_millis(50);

        let raw = raw_progress(now, started_at, duration);
        let eased = eased_progress(now, started_at, duration);

        assert_eq!(raw, 0.5);
        assert!(eased > raw);
    }

    #[test]
    fn back_easing_reaches_the_target_after_overshoot() {
        assert!(ease_out_back(0.0, 0.22).abs() < f32::EPSILON);
        assert!((ease_out_back(1.0, 0.22) - 1.0).abs() < f32::EPSILON);
        assert!(ease_out_back(0.8, 0.22) > 0.8);
    }
}
