use crate::ui::animation::{SpringValue, spring_bouncy};
use gpui::Global;
use std::time::Instant;

const PILL_SETTLE_DISTANCE: f32 = 0.006;

/// 顶栏导航状态。
///
/// 胶囊位置由一条可中断 Q 弹 spring 驱动。实际可见动画由 Nova presentation owner
/// 持有固定尺寸的 translation track；这里的 CPU mirror 只负责 retarget 连续性、
/// settled 判定和离散导航状态，不再用双边缘弹簧改变胶囊宽度。
pub struct NavState {
    pub active_index: usize,
    pub pending_route_index: Option<usize>,
    pub pill_from_index: usize,
    pub pill_to_index: usize,
    pill_motion: SpringValue,
    pill_last_direction: f32,
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
            pill_motion: SpringValue::new(0.0).with_spring(spring_bouncy()),
            pill_last_direction: 1.0,
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
        let current = self.pill_motion.value(now);
        if (target - current).abs() > f32::EPSILON {
            self.pill_last_direction = (target - current).signum();
        }
        self.pill_from_index = self.visual_active_index();
        self.pill_to_index = to_index;
        self.pill_motion.retarget(target, now);
        self.pending_route_index = Some(to_index);
    }

    pub fn sync_to_route(&mut self, index: usize) {
        self.active_index = index;
        self.pending_route_index = None;
        self.pill_from_index = index;
        self.pill_to_index = index;
        self.pill_motion.snap_to(index as f32);
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
        let motion = self.pill_motion.sample(now);
        let target = self.pill_to_index as f32;
        if motion.done || (motion.value - target).abs() <= PILL_SETTLE_DISTANCE {
            return ((target, target), false);
        }

        // Keep the compatibility edge API degenerate: the visible pill no longer stretches.
        // Overshoot comes from translation itself, so width/radius stay invariant.
        ((motion.value, motion.value), true)
    }

    /// 胶囊当前位置，以 tab 序号为单位。两个值保持相同，避免重新引入宽度拉伸。
    pub fn pill_edges(&self, now: Instant) -> (f32, f32) {
        self.pill_render_state(now).0
    }

    pub fn pill_direction(&self) -> f32 {
        self.pill_last_direction
    }

    /// Returns the discrete compositor endpoints for the current pill transition.
    ///
    /// Once the CPU mirror spring has settled, both endpoints collapse to the target. This keeps
    /// unrelated later renders from reconstructing a completed one-shot animation from stale route
    /// indices.
    pub fn pill_animation_indices(&self, now: Instant) -> (usize, usize) {
        if self.is_animating(now) {
            (self.pill_from_index, self.pill_to_index)
        } else {
            (self.pill_to_index, self.pill_to_index)
        }
    }


    pub fn set_labels_target(&mut self, visible: bool, _now: Instant) {
        self.labels_target_visible = visible;
    }

    pub fn set_labels_target_immediate(&mut self, visible: bool) {
        self.labels_target_visible = visible;
    }

    pub fn labels_layout_factor(&self, _now: Instant) -> f32 {
        if self.labels_target_visible { 1.0 } else { 0.0 }
    }

    pub fn labels_opacity_factor(&self, _now: Instant) -> f32 {
        if self.labels_target_visible { 1.0 } else { 0.0 }
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
    fn pill_translation_overshoots_without_stretch_and_settles() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.start_pill_animation(4, now);
        assert!(nav.pill_direction() > 0.0);

        let mut peak = 0.0f32;
        for step in 1..=80 {
            let sample_at = now + Duration::from_millis(step * 10);
            let (left, right) = nav.pill_edges(sample_at);
            assert!((left - right).abs() < f32::EPSILON, "固定宽度胶囊不应拉伸");
            peak = peak.max(left);
        }
        assert!(peak > 4.01, "Q 弹 translation 应轻微越过目标位置");

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
                let motion = nav.pill_motion.sample(*sample_time);
                !motion.done && (motion.value - 4.0).abs() <= PILL_SETTLE_DISTANCE
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
    fn settled_pill_animation_collapses_compositor_endpoints() {
        let now = Instant::now();
        let mut nav = NavState::default();
        nav.start_pill_animation(4, now);

        assert_eq!(nav.pill_animation_indices(now), (0, 4));
        assert_eq!(
            nav.pill_animation_indices(now + Duration::from_secs(5)),
            (4, 4),
        );
    }

    #[test]
    fn label_breakpoint_switch_is_immediate_and_does_not_drive_animation() {
        let now = Instant::now();
        let mut nav = NavState::default();

        nav.set_labels_target(false, now);

        assert_eq!(nav.labels_layout_factor(now), 0.0);
        assert_eq!(nav.labels_opacity_factor(now), 0.0);
        assert!(!nav.is_animating(now));
    }
}
