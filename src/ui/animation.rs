use gpui::{
    Animation, AnimationDriver, AnimationProperty, AnimationSpec, App, Easing, FillMode,
    HorizontalRevealEdge, RepeatMode, SharedString, Spring, SpringPhysics, Window, point, px,
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

const TAB_TRANSITION_RESPONSE: f32 = 0.34;
const TAB_TRANSITION_DAMPING: f32 = 0.92;
const TAB_LIST_ITEM_MAX_STAGGER_SLOT: usize = 8;
const TAB_LIST_ITEM_MAX_STAGGER_MS: u64 = 112;
const TAB_LIST_STAGGER_WINDOW: Duration = Duration::from_millis(760);

fn tab_transition_spring() -> Spring {
    apple_spring(TAB_TRANSITION_RESPONSE, TAB_TRANSITION_DAMPING)
}

pub fn tab_transition_direction(from_index: usize, to_index: usize) -> f32 {
    if to_index >= from_index { 1.0 } else { -1.0 }
}

pub fn tab_toolbar_motion() -> Animation {
    Animation::from_spec(
        AnimationSpec::new(Duration::from_millis(240))
            .fill_mode(FillMode::Both)
            .ease(Easing::OutCubic),
    )
}

pub fn settled_animation() -> Animation {
    Animation::from_spec(
        AnimationSpec::new(Duration::ZERO)
            .fill_mode(FillMode::Both)
            .ease(Easing::Linear),
    )
}

/// Shared renderer-owned transition for tab/subpage content.
///
/// Main content and visible list rows intentionally use the same physical spring so their velocity
/// profile stays coherent. Rows add only a short nonlinear start delay.
pub fn tab_content_motion(from_index: usize, to_index: usize) -> Animation {
    let direction = tab_transition_direction(from_index, to_index);
    spring_motion(tab_transition_spring()).with_property(AnimationProperty::translation_opacity(
        point(px(16.0 * direction), px(0.0)),
        point(px(0.0), px(0.0)),
        0.90,
        1.0,
    ))
}

/// Short outgoing motion used only while an asset-to-asset target is loading.
///
/// The previous list moves opposite to the incoming direction and stays partially visible instead
/// of being replaced by a full-panel loading placeholder.
pub fn tab_stale_content_motion(
    _from_index: usize,
    _to_index: usize,
) -> Animation {
    Animation::from_spec(
        AnimationSpec::new(Duration::from_millis(160))
            .fill_mode(FillMode::Both)
            .ease(Easing::OutCubic),
    )
}

fn tab_list_item_delay(visible_index: usize) -> Duration {
    let slot = visible_index.min(TAB_LIST_ITEM_MAX_STAGGER_SLOT);
    if slot == 0 {
        return Duration::ZERO;
    }

    let t = slot as f32 / TAB_LIST_ITEM_MAX_STAGGER_SLOT as f32;
    // Concave ease-out spacing: 0, 26, 49, 68, 84, 96, 105, 110, 112 ms.
    // Keep the first rows distinct without leaving most of the list temporarily transparent.
    let curved = 1.0 - (1.0 - t).powi(2);
    Duration::from_millis((TAB_LIST_ITEM_MAX_STAGGER_MS as f32 * curved).round() as u64)
}

#[derive(Clone, Copy, Debug)]
struct TabListStaggerState {
    sequence: u64,
    started_at: Instant,
}

/// Keep a tab-entry stagger alive only long enough for its renderer-owned row animations.
///
/// The keyed owner survives row virtualization, so scrolling later does not restart the entrance
/// animation with the direction of an old tab switch.
pub fn tab_list_stagger_active(
    window: &mut Window,
    cx: &mut App,
    id: &'static str,
    sequence: u64,
) -> bool {
    if sequence == 0 {
        return false;
    }

    let now = window.animation_time();
    let state = window.use_keyed_state(id, cx, |_, _| TabListStaggerState {
        sequence,
        started_at: now,
    });
    let mut snapshot = *state.read(cx);
    if snapshot.sequence != sequence {
        state.update(cx, |state, _| {
            state.sequence = sequence;
            state.started_at = now;
        });
        snapshot = *state.read(cx);
    }

    now.saturating_duration_since(snapshot.started_at) <= TAB_LIST_STAGGER_WINDOW
}

/// Direction-aware row entrance timeline used by management lists.
///
/// Callers keep final row geometry and apply only a small relative paint offset. Delays are capped
/// so a long or virtualized list never turns into a long animation queue.
pub fn tab_list_item_motion(
    _from_index: usize,
    _to_index: usize,
    visible_index: usize,
) -> Animation {
    spring_motion(tab_transition_spring())
        .delay(tab_list_item_delay(visible_index))
        .fill_mode(FillMode::Both)
}

/// Direction-aware underline reveal for variable-width primary tabs.
pub fn tab_underline_motion(from_index: usize, to_index: usize) -> Animation {
    let edge = if to_index >= from_index {
        HorizontalRevealEdge::Left
    } else {
        HorizontalRevealEdge::Right
    };

    Animation::from_spec(
        AnimationSpec::new(Duration::from_millis(180))
            .fill_mode(FillMode::Both)
            .ease(Easing::OutCubic),
    )
    .with_property(AnimationProperty::horizontal_reveal(edge, 0.0, 1.0))
}

/// Staggered chart-bar timing. Final geometry stays stable and only the inner bar moves.
pub fn stat_chart_bar_motion(index: usize) -> Animation {
    let delay = Duration::from_millis(index.min(13) as u64 * 32);

    Animation::from_spec(
        AnimationSpec::new(Duration::from_millis(560))
            .delay(delay)
            .fill_mode(FillMode::Both)
            .ease(Easing::OutCubic),
    )
}

/// Stable transition key helper for state-driven tab/subpage switches.
pub fn tab_content_animation_key(scope: &str, sequence: u64) -> SharedString {
    SharedString::from(format!("{scope}-{sequence}"))
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

    // A retarget behind the current direction is a new user intent, not inertial scrolling.
    // Carrying the obsolete velocity makes the indicator continue toward the old tab for a few
    // frames and reads as "the animation cannot be interrupted". Preserve momentum only when it
    // already points toward the new target.
    if current_velocity != 0.0 && current_velocity.signum() != delta.signum() {
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
    if deadline.is_some_and(|deadline| window.animation_time() < deadline) {
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
    fn tab_list_stagger_front_loads_spacing_then_converges() {
        let delays = (0..=TAB_LIST_ITEM_MAX_STAGGER_SLOT)
            .map(|index| tab_list_item_delay(index).as_millis() as u64)
            .collect::<Vec<_>>();
        assert_eq!(delays, vec![0, 26, 49, 68, 84, 96, 105, 110, 112]);

        let gaps = delays
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>();
        assert!(
            gaps.windows(2).all(|pair| pair[0] > pair[1]),
            "stagger gaps should shrink monotonically: {gaps:?}"
        );
        assert_eq!(
            tab_list_item_delay(TAB_LIST_ITEM_MAX_STAGGER_SLOT + 20),
            Duration::from_millis(TAB_LIST_ITEM_MAX_STAGGER_MS)
        );
    }

    #[test]
    fn tab_list_stagger_window_outlives_the_last_delayed_spring() {
        let latest_delay = tab_list_item_delay(TAB_LIST_ITEM_MAX_STAGGER_SLOT);
        let remaining = TAB_LIST_STAGGER_WINDOW.saturating_sub(latest_delay);
        let sample = tab_transition_spring().sample_with_velocity(remaining.as_secs_f32(), 0.0);
        assert!(
            sample.done,
            "stagger owner must remain active until the last row spring settles"
        );
    }

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
    fn retarget_velocity_preserves_physical_velocity_and_caps_short_distance_momentum() {
        assert_eq!(responsive_retarget_velocity(4.0, -0.5), 0.0);
        assert_eq!(responsive_retarget_velocity(-4.0, 0.5), 0.0);
        assert_eq!(responsive_retarget_velocity(4.0, 0.5), 4.0);
        assert_eq!(responsive_retarget_velocity(-4.0, -0.5), -4.0);

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
