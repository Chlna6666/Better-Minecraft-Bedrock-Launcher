use gpui::{
    Animation, AnimationDriver, AnimationSpec, Easing, RepeatMode, Spring, SpringPhysics, Window,
};
use std::time::{Duration, Instant};

const MIN_ANIMATION_DURATION: Duration = Duration::from_millis(1);
const MAX_RETARGET_NORMALIZED_VELOCITY: f32 = 12.0;

/// Apple 风格弹簧参数化：`response` 为周期（秒），`damping_fraction` 为阻尼比。
/// 与 SwiftUI 的 `spring(response:dampingFraction:)` 对齐：
/// stiffness = (2π / response)²，damping = 2 · ζ · √(stiffness · mass)。
pub fn apple_spring(response: f32, damping_fraction: f32) -> Spring {
    let response = response.max(0.01);
    let stiffness = (std::f32::consts::TAU / response).powi(2);
    let damping = 2.0 * damping_fraction.max(0.0) * stiffness.sqrt();
    Spring {
        physics: SpringPhysics {
            stiffness,
            damping,
            mass: 1.0,
        },
        settle_position: 0.001,
        settle_velocity: 0.001,
    }
}

/// 平滑弹簧：临界阻尼，无回弹（用于宽度/透明度等布局属性）。
pub fn spring_smooth() -> Spring {
    apple_spring(0.32, 1.0)
}

/// 干脆弹簧：轻微回弹，响应快（用于状态切换、收起方向）。
pub fn spring_snappy() -> Spring {
    apple_spring(0.30, 0.82)
}

/// Q 弹弹簧：明显回弹（用于展开、导航胶囊等重点交互）。
pub fn spring_bouncy() -> Spring {
    apple_spring(0.42, 0.62)
}

/// 一次采样得到的弹簧状态。
#[derive(Clone, Copy, Debug)]
pub struct SpringValueSample {
    pub value: f32,
    pub velocity: f32,
    pub done: bool,
}

/// 可中断、可重定向的弹簧值。
#[derive(Clone, Copy, Debug)]
pub struct SpringValue {
    from: f32,
    to: f32,
    initial_velocity: f32,
    started_at: Option<Instant>,
    spring: Spring,
}

impl SpringValue {
    pub fn new(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            initial_velocity: 0.0,
            started_at: None,
            spring: spring_smooth(),
        }
    }

    pub fn with_spring(mut self, spring: Spring) -> Self {
        self.spring = spring;
        self
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn snap_to(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.initial_velocity = 0.0;
        self.started_at = None;
    }

    pub fn retarget(&mut self, target: f32, now: Instant) {
        if (target - self.to).abs() <= f32::EPSILON {
            return;
        }
        let current = self.sample(now);
        let delta = target - current.value;
        self.from = current.value;
        self.initial_velocity = responsive_retarget_velocity(current.velocity, delta);
        self.to = target;
        if delta.abs() <= 1e-5 && current.velocity.abs() <= 1e-4 {
            self.snap_to(target);
        } else {
            self.started_at = Some(now);
        }
    }

    pub fn retarget_with_spring(&mut self, target: f32, spring: Spring, now: Instant) {
        if (target - self.to).abs() <= f32::EPSILON {
            return;
        }
        let current = self.sample(now);
        let delta = target - current.value;
        self.spring = spring;
        self.from = current.value;
        self.initial_velocity = responsive_retarget_velocity(current.velocity, delta);
        self.to = target;
        if delta.abs() <= 1e-5 && current.velocity.abs() <= 1e-4 {
            self.snap_to(target);
        } else {
            self.started_at = Some(now);
        }
    }

    pub fn sample(&self, now: Instant) -> SpringValueSample {
        let Some(started_at) = self.started_at else {
            return SpringValueSample {
                value: self.to,
                velocity: 0.0,
                done: true,
            };
        };
        let delta = self.to - self.from;
        if delta.abs() <= 1e-6 {
            return SpringValueSample {
                value: self.to,
                velocity: 0.0,
                done: true,
            };
        }
        let elapsed = now.saturating_duration_since(started_at).as_secs_f32();
        let sample = self
            .spring
            .sample_with_velocity(elapsed, self.initial_velocity / delta);
        if sample.done {
            SpringValueSample {
                value: self.to,
                velocity: 0.0,
                done: true,
            }
        } else {
            SpringValueSample {
                value: self.from + delta * sample.progress,
                velocity: delta * sample.velocity,
                done: false,
            }
        }
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.sample(now).value
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        !self.sample(now).done
    }
}

fn responsive_retarget_velocity(current_velocity: f32, delta: f32) -> f32 {
    if !current_velocity.is_finite() || !delta.is_finite() || delta.abs() <= f32::EPSILON {
        return 0.0;
    }
    if current_velocity * delta <= 0.0 {
        return 0.0;
    }

    let max_velocity = delta.abs() * MAX_RETARGET_NORMALIZED_VELOCITY;
    current_velocity.clamp(-max_velocity, max_velocity)
}

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

pub fn spring_motion(spring: Spring) -> Animation {
    Animation::spring(spring)
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

/// Request another layout-animation sample.
///
/// When called from an element lifecycle GPUI captures the current retained path and invalidates
/// only that subtree. Calls made before an element boundary exists conservatively fall back to the
/// owning view. Prefer `AnimationExt::with_layout_animation_target` for caller-sampled View code.
#[track_caller]
pub fn request_layout_animation_frame_if(window: &mut Window, animating: bool) {
    if animating {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

/// Active-window variant of [`request_layout_animation_frame_if`].
#[track_caller]
pub fn request_layout_animation_frame_if_active(window: &mut Window, animating: bool) {
    if animating && window.is_window_active() {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

/// Request layout-animation samples until the supplied deadline.
#[track_caller]
pub fn request_layout_animation_frame_until(window: &mut Window, deadline: Option<Instant>) {
    if deadline.is_some_and(|deadline| Instant::now() < deadline) {
        window.request_animation_engine_frame(AnimationDriver::Layout);
    }
}

/// Active-window variant of [`request_layout_animation_frame_until`].
#[track_caller]
pub fn request_layout_animation_frame_until_active(
    window: &mut Window,
    deadline: Option<Instant>,
) {
    if window.is_window_active() {
        request_layout_animation_frame_until(window, deadline);
    }
}

fn element_motion_from_spec(spec: AnimationSpec) -> Animation {
    let mut spec = spec;
    spec.duration = spec.duration.max(MIN_ANIMATION_DURATION);
    Animation::from_spec(spec)
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

    #[test]
    fn spring_value_settles_at_target() {
        let now = Instant::now();
        let mut spring = SpringValue::new(0.0).with_spring(spring_bouncy());
        spring.retarget(1.0, now);

        assert!(spring.is_animating(now));
        assert!(spring.value(now) < 0.1);

        let later = now + Duration::from_secs(5);
        let sample = spring.sample(later);
        assert!(sample.done);
        assert!((sample.value - 1.0).abs() < f32::EPSILON);
        assert!(!spring.is_animating(later));
    }

    #[test]
    fn spring_value_retarget_preserves_motion_continuity() {
        let now = Instant::now();
        let mut spring = SpringValue::new(0.0).with_spring(spring_smooth());
        spring.retarget(1.0, now);

        let mid = now + Duration::from_millis(120);
        let before = spring.value(mid);
        assert!(before > 0.0 && before < 1.0);
        spring.retarget(0.0, mid);
        let after = spring.value(mid);
        assert!((after - before).abs() < 1e-4);
        assert!(spring.is_animating(mid));
    }

    #[test]
    fn spring_value_reverse_retarget_moves_toward_the_new_target_immediately() {
        let now = Instant::now();
        let mut spring = SpringValue::new(0.0).with_spring(spring_bouncy());
        spring.retarget(1.0, now);

        let reverse_at = now + Duration::from_millis(40);
        let before = spring.sample(reverse_at);
        assert!(before.value > 0.0);
        assert!(before.velocity > 0.0);

        spring.retarget_with_spring(0.0, spring_snappy(), reverse_at);
        let after = spring.sample(reverse_at + Duration::from_millis(10));
        assert!(after.value < before.value);
    }

    #[test]
    fn spring_value_retarget_to_same_target_is_a_no_op() {
        let now = Instant::now();
        let mut spring = SpringValue::new(0.0).with_spring(spring_snappy());
        spring.retarget(1.0, now);

        let mid = now + Duration::from_millis(80);
        let before = spring.value(mid);
        spring.retarget(1.0, mid);
        assert!((spring.value(mid) - before).abs() < f32::EPSILON);
    }

    #[test]
    fn retarget_velocity_drops_old_direction_and_caps_short_distance_momentum() {
        assert_eq!(responsive_retarget_velocity(4.0, -0.5), 0.0);
        assert_eq!(responsive_retarget_velocity(-4.0, 0.5), 0.0);

        let capped = responsive_retarget_velocity(100.0, 0.25);
        assert!((capped - 3.0).abs() < f32::EPSILON);
        let capped_negative = responsive_retarget_velocity(-100.0, -0.25);
        assert!((capped_negative + 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn bouncy_spring_overshoots_the_target() {
        let spring = spring_bouncy();
        let mut peak = 0.0f32;
        for i in 0..200 {
            let t = i as f32 * 0.01;
            peak = peak.max(spring.sample(t));
        }
        assert!(peak > 1.01);
    }
}
