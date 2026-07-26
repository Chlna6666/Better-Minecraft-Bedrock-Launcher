use crate::ui::animation::{SpringValue, apple_spring, spring_smooth};
use gpui::Global;
use std::time::Instant;

/// 顶栏导航状态。
///
/// 新版 UI 的所有导航动画均由可中断弹簧驱动（Apple 风格）：
/// - 激活胶囊的左右边缘各是一条弹簧，快弹簧领先、慢弹簧拖尾，
///   移动时自然拉伸、到位时回弹收拢；
/// - 快速连续切换 tab 时弹簧从当前位置和速度继续，不会跳变或重播。
pub struct NavState {
    pub active_index: usize,
    pub pending_route_index: Option<usize>,
    pub pill_from_index: usize,
    pub pill_to_index: usize,
    /// 领先边缘（Q 弹、响应快）。
    pill_fast: SpringValue,
    /// 拖尾边缘（更平滑、略慢）。
    pill_slow: SpringValue,
    pill_last_direction: f32,

    labels_layout: SpringValue,
    labels_opacity: SpringValue,
    pub labels_target_visible: bool,
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

            labels_layout: SpringValue::new(1.0).with_spring(spring_smooth()),
            labels_opacity: SpringValue::new(1.0).with_spring(apple_spring(0.24, 1.0)),
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
        self.pill_fast.is_animating(now)
            || self.pill_slow.is_animating(now)
            || self.labels_animating(now)
    }

    /// 胶囊左右边缘位置（以 tab 序号为单位，允许轻微过冲产生 Q 弹）。
    pub fn pill_edges(&self, now: Instant) -> (f32, f32) {
        let a = self.pill_fast.value(now);
        let b = self.pill_slow.value(now);
        (a.min(b), a.max(b))
    }

    pub fn pill_direction(&self) -> f32 {
        self.pill_last_direction
    }

    pub fn set_labels_target(&mut self, visible: bool, now: Instant) {
        if self.labels_target_visible == visible {
            return;
        }
        self.labels_target_visible = visible;
        let target = if visible { 1.0 } else { 0.0 };
        self.labels_layout.retarget(target, now);
        self.labels_opacity.retarget(target, now);
    }

    pub fn set_labels_target_immediate(&mut self, visible: bool) {
        let target = if visible { 1.0 } else { 0.0 };
        self.labels_target_visible = visible;
        self.labels_layout.snap_to(target);
        self.labels_opacity.snap_to(target);
    }

    pub fn labels_animating(&self, now: Instant) -> bool {
        self.labels_layout.is_animating(now) || self.labels_opacity.is_animating(now)
    }

    pub fn labels_layout_factor(&self, now: Instant) -> f32 {
        self.labels_layout.value(now).clamp(0.0, 1.0)
    }

    pub fn labels_opacity_factor(&self, now: Instant) -> f32 {
        self.labels_opacity.value(now).clamp(0.0, 1.0)
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
    fn pill_edges_stretch_and_settle() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(4, now);
        assert!(nav.pill_direction() > 0.0);

        // 动画早期：快弹簧领先于慢弹簧，胶囊被拉伸。
        let early = now + Duration::from_millis(90);
        let (left, right) = nav.pill_edges(early);
        assert!(right > left, "移动中胶囊应被拉伸");
        assert!(right < 4.6, "边缘不应飞出合理范围");

        // 完全稳定后：两条边缘收拢到目标 tab。
        let settled = now + Duration::from_secs(5);
        let (left, right) = nav.pill_edges(settled);
        assert!((left - 4.0).abs() < 0.01);
        assert!((right - 4.0).abs() < 0.01);
        assert!(!nav.is_animating(settled));
    }

    #[test]
    fn retargeting_mid_flight_is_continuous() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(5, now);
        let mid = now + Duration::from_millis(100);
        let (before_left, before_right) = nav.pill_edges(mid);

        // 中途改变目标：边缘位置不应跳变。
        nav.confirm_route(5);
        nav.start_pill_animation(1, mid);
        let (after_left, after_right) = nav.pill_edges(mid);
        assert!((after_left - before_left).abs() < 1e-3);
        assert!((after_right - before_right).abs() < 1e-3);
        assert!(nav.pill_direction() < 0.0);
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
