use crate::ui::animation::{SpringValue, apple_spring};
use gpui::Global;
use std::time::Instant;

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
/// 保存路由与响应式标签布局目标。胶囊的双边缘弹簧由 retained presentation
/// 拥有；这里不保留或逐帧采样另一份胶囊动画。
pub struct NavState {
    pub active_index: usize,
    pub pending_route_index: Option<usize>,
    pub pill_from_index: usize,
    pub pill_to_index: usize,
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
            labels_target_visible: true,
            labels_reveal: SpringValue::new(1.0).with_spring(apple_spring(0.22, 1.0)),
        }
    }
}

impl NavState {
    pub fn visual_active_index(&self) -> usize {
        self.pending_route_index.unwrap_or(self.active_index)
    }

    /// Publish the newest visual route target; presentation retains in-flight values.
    pub fn start_pill_animation(&mut self, to_index: usize) {
        if self.pending_route_index == Some(to_index) {
            return;
        }
        if self.active_index == to_index && self.pending_route_index.is_none() {
            return;
        }
        self.pill_from_index = self.visual_active_index();
        self.pill_to_index = to_index;
        self.pending_route_index = Some(to_index);
    }

    pub fn sync_to_route(&mut self, index: usize) {
        self.active_index = index;
        self.pending_route_index = None;
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

    /// Returns true only when the responsive target actually changed.
    /// A retarget begins at the currently sampled value without snapping.
    pub fn set_labels_target(&mut self, visible: bool, now: Instant) -> bool {
        if self.labels_target_visible == visible {
            return false;
        }
        self.labels_target_visible = visible;
        self.labels_reveal
            .retarget(if visible { 1.0 } else { 0.0 }, now);
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
        let mut nav = NavState::default();

        nav.start_pill_animation(5);
        assert_eq!(nav.pending_route_index, Some(5));

        nav.confirm_route(5);

        assert_eq!(nav.active_index, 5);
        assert_eq!(nav.pending_route_index, Some(5));
        assert_eq!((nav.pill_from_index, nav.pill_to_index), (0, 5));
    }

    #[test]
    fn rapid_route_retarget_keeps_latest_target_through_confirmation() {
        let mut nav = NavState::default();

        nav.start_pill_animation(5);
        nav.confirm_route(5);
        nav.start_pill_animation(1);
        assert_eq!((nav.pill_from_index, nav.pill_to_index), (5, 1));
        assert_eq!(nav.visual_active_index(), 1);
        nav.confirm_route(1);
        assert_eq!(nav.active_index, 1);
        assert_eq!((nav.pill_from_index, nav.pill_to_index), (5, 1));
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
    fn resize_settles_active_label_spring_without_waiting_for_mouse_release() {
        let started = Instant::now();
        let mut nav = NavState::default();
        assert!(nav.set_labels_target(false, started));
        let while_animating = started + Duration::from_millis(40);
        assert!(nav.labels_animating(while_animating));

        // A live WM_SIZE settles the current responsive target immediately,
        // so held resize frames never start an independent LayoutAnimation loop.
        nav.set_labels_target_immediate(false);
        assert!(!nav.labels_animating(while_animating));
        assert_eq!(nav.labels_layout_factor(while_animating), 0.0);

        // Further breakpoint crossings change the layout once, with no spring.
        nav.set_labels_target_immediate(true);
        assert!(!nav.labels_animating(while_animating));
        assert_eq!(nav.labels_layout_factor(while_animating), 1.0);

        // Ordinary navigation after the resize still uses the existing spring.
        assert!(nav.set_labels_target(false, while_animating));
        assert!(nav.labels_animating(while_animating + Duration::from_millis(10)));
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
