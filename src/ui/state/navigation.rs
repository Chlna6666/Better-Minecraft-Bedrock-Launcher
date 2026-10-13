use crate::ui::animation::{SpringValue, apple_spring};
use gpui::Global;
use std::time::Instant;

const PILL_EDGE_SETTLE_DISTANCE: f32 = 0.006;

/// Show and hide have separate thresholds so native Resize jitter around the
/// breakpoint cannot repeatedly retarget the navigation's layout animation.
pub(crate) const NAV_LABEL_BREAKPOINT: f32 = 1180.0;
const NAV_LABEL_BREAKPOINT_HYSTERESIS: f32 = 16.0;

pub(crate) fn nav_labels_visible_for_width(width: f32, currently_visible: bool) -> bool {
    if !width.is_finite() {
        return currently_visible;
    }
    if currently_visible {
        width >= NAV_LABEL_BREAKPOINT - NAV_LABEL_BREAKPOINT_HYSTERESIS
    } else {
        width >= NAV_LABEL_BREAKPOINT + NAV_LABEL_BREAKPOINT_HYSTERESIS
    }
}

/// 顶栏导航状态。
///
/// 胶囊左右边缘由两条可中断弹簧驱动：前沿响应更快、后沿带一点拖尾，移动时形成
/// 类似流体的拉伸/回收，而不是整个固定矩形做刚体平移。动画只改变 pill 自身 geometry，
/// 周围 icon、label 与 hit target 保持静态 retained。
pub struct NavState {
    pub active_index: usize,
    pub pending_route_index: Option<usize>,
    pub pill_from_index: usize,
    pub pill_to_index: usize,
    pill_fast: SpringValue,
    pill_slow: SpringValue,
    pill_last_direction: f32,
    pub labels_target_visible: bool,
    labels_reveal: SpringValue,
}

impl Global for NavState {}

impl Default for NavState {
    fn default() -> Self {
        Self {
            active_index: 0,
            pending_route_index: None,
            pill_from_index: 0,
            pill_to_index: 0,
            pill_fast: SpringValue::new(0.0).with_spring(apple_spring(0.34, 0.60)),
            pill_slow: SpringValue::new(0.0).with_spring(apple_spring(0.42, 0.80)),
            pill_last_direction: 1.0,
            labels_target_visible: true,
            labels_reveal: SpringValue::new(1.0).with_spring(apple_spring(0.22, 1.0)),
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
        let target = to_index as f32;
        let current = self.pill_fast.value(now);
        if (target - current).abs() > f32::EPSILON {
            self.pill_last_direction = (target - current).signum();
        }
        self.pill_from_index = self.visual_active_index();
        self.pill_to_index = to_index;
        self.pill_fast.retarget(target, now);
        self.pill_slow.retarget(target, now);
        self.pending_route_index = Some(to_index);
    }

    pub fn sync_to_route(&mut self, index: usize) {
        self.active_index = index;
        self.pending_route_index = None;
        self.pill_from_index = index;
        self.pill_to_index = index;
        self.pill_fast.snap_to(index as f32);
        self.pill_slow.snap_to(index as f32);
    }

    pub fn confirm_route(&mut self, index: usize) {
        if self.pending_route_index == Some(index) {
            self.active_index = index;
            return;
        }

        self.sync_to_route(index);
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.pill_render_state(now).1
    }

    pub(crate) fn pill_render_state(&self, now: Instant) -> ((f32, f32), bool) {
        let fast = self.pill_fast.sample(now);
        let slow = self.pill_slow.sample(now);
        let target = self.pill_to_index as f32;
        let settled = (fast.value - target).abs() <= PILL_EDGE_SETTLE_DISTANCE
            && (slow.value - target).abs() <= PILL_EDGE_SETTLE_DISTANCE;
        if settled {
            return ((target, target), false);
        }

        (
            (fast.value.min(slow.value), fast.value.max(slow.value)),
            !fast.done || !slow.done,
        )
    }

    /// 胶囊左右边缘位置（以 tab 序号为单位）；允许短暂拉伸和轻微过冲形成流体感。
    pub fn pill_edges(&self, now: Instant) -> (f32, f32) {
        self.pill_render_state(now).0
    }

    pub fn pill_direction(&self) -> f32 {
        self.pill_last_direction
    }

    /// Returns true only when the responsive target actually changed.
    /// A retarget begins at the currently sampled value without snapping.
    pub fn set_labels_target(&mut self, visible: bool, now: Instant) -> bool {
        if self.labels_target_visible == visible {
            return false;
        }
        self.labels_target_visible = visible;
        self.labels_reveal.retarget(if visible { 1.0 } else { 0.0 }, now);
        true
    }

    pub fn set_labels_target_immediate(&mut self, visible: bool) {
        self.labels_target_visible = visible;
        self.labels_reveal.snap_to(if visible { 1.0 } else { 0.0 });
    }

    pub fn labels_animating(&self, now: Instant) -> bool {
        self.labels_reveal.is_animating(now)
    }

    pub fn labels_layout_factor(&self, now: Instant) -> f32 {
        self.labels_reveal.value(now).clamp(0.0, 1.0)
    }

    pub fn labels_opacity_factor(&self, now: Instant) -> f32 {
        // Do not paint partially clipped glyphs at the first few pixels of
        // expansion. Width continues smoothly while text starts fully hidden.
        ((self.labels_layout_factor(now) - 0.18) / 0.82).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
    fn pill_edges_stretch_like_fluid_and_settle() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(4, now);
        assert!(nav.pill_direction() > 0.0);

        let early = now + Duration::from_millis(90);
        let (left, right) = nav.pill_edges(early);
        assert!(right > left, "前后沿不同步时胶囊应产生流体拉伸");
        assert!(right < 4.6, "边缘不应飞出合理范围");

        let settled = now + Duration::from_secs(5);
        let (left, right) = nav.pill_edges(settled);
        assert!((left - 4.0).abs() < 0.01);
        assert!((right - 4.0).abs() < 0.01);
        assert!(!nav.is_animating(settled));
    }

    #[test]
    fn pill_edges_snap_subpixel_tail_motion_and_stop_cadence() {
        let now = Instant::now();
        let mut nav = NavState::default();
        nav.start_pill_animation(4, now);

        let settling_time = (1..=500)
            .map(|step| now + Duration::from_millis(step * 10))
            .find(|sample_time| {
                let fast = nav.pill_fast.sample(*sample_time);
                let slow = nav.pill_slow.sample(*sample_time);
                !fast.done
                    && !slow.done
                    && (fast.value - 4.0).abs() <= PILL_EDGE_SETTLE_DISTANCE
                    && (slow.value - 4.0).abs() <= PILL_EDGE_SETTLE_DISTANCE
            })
            .expect("弹簧应在完成前进入亚像素收敛区间");

        assert_eq!(nav.pill_edges(settling_time), (4.0, 4.0));
        assert!(!nav.is_animating(settling_time));
    }

    #[test]
    fn retargeting_mid_flight_is_continuous() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(5, now);
        let mid = now + Duration::from_millis(100);
        let (before_left, before_right) = nav.pill_edges(mid);

        nav.confirm_route(5);
        nav.start_pill_animation(1, mid);
        let (after_left, after_right) = nav.pill_edges(mid);
        assert!((after_left - before_left).abs() < 1e-3);
        assert!((after_right - before_right).abs() < 1e-3);
        assert!(nav.pill_direction() < 0.0);
    }

    #[test]
    fn label_breakpoint_hysteresis_rejects_native_resize_jitter() {
        assert!(nav_labels_visible_for_width(1180.0, true));
        assert!(nav_labels_visible_for_width(1170.0, true));
        assert!(!nav_labels_visible_for_width(1163.0, true));
        assert!(!nav_labels_visible_for_width(1180.0, false));
        assert!(!nav_labels_visible_for_width(1195.0, false));
        assert!(nav_labels_visible_for_width(1196.0, false));
        assert!(nav_labels_visible_for_width(f32::NAN, true));
        assert!(!nav_labels_visible_for_width(f32::NAN, false));
    }

    #[test]
    fn responsive_label_reveal_interpolates_and_retargets_continuously() {
        let now = Instant::now();
        let mut nav = NavState::default();
        assert!(nav.set_labels_target(false, now));
        assert!(!nav.set_labels_target(false, now + Duration::from_millis(10)));
        assert_eq!(nav.labels_layout_factor(now), 1.0);
        let mid = now + Duration::from_millis(100);
        let intermediate = nav.labels_layout_factor(mid);
        assert!(intermediate > 0.0 && intermediate < 1.0);
        assert!(nav.labels_animating(mid));
        assert!(nav.set_labels_target(true, mid));
        assert!((nav.labels_layout_factor(mid) - intermediate).abs() <= 1e-4);
        assert_eq!(nav.labels_layout_factor(now + Duration::from_secs(5)), 1.0);
        assert!(!nav.labels_animating(now + Duration::from_secs(5)));
        nav.set_labels_target_immediate(false);
        assert_eq!(nav.labels_layout_factor(now), 0.0);
        assert_eq!(nav.labels_opacity_factor(now), 0.0);
    }
}
