use super::*;

impl Window {
    pub(in crate::window::input) fn window_control_area_under_mouse(
        &self,
    ) -> Option<WindowControlArea> {
        self.rendered_frame
            .window_control_hitboxes
            .iter()
            .rev()
            .find_map(|(area, hitbox)| hitbox.is_hovered(self).then_some(*area))
            .or_else(|| self.transparent_caption_area_under_mouse())
    }

    fn transparent_caption_area_under_mouse(&self) -> Option<WindowControlArea> {
        if !self.transparent_caption_enabled && self.transparent_caption_height.is_none() {
            return None;
        }

        let height = self
            .transparent_caption_height
            .or(self.observed_caption_height)
            .unwrap_or(px(32.0));
        if self.mouse_position.x < px(0.)
            || self.mouse_position.y < px(0.)
            || self.mouse_position.x > self.viewport_size.width
            || self.mouse_position.y > height
        {
            return None;
        }

        self.mouse_hit_test
            .ids
            .is_empty()
            .then_some(WindowControlArea::Drag)
    }

    pub(in crate::window) fn observe_caption_height(&mut self) {
        self.observed_caption_height = self
            .rendered_frame
            .window_control_hitboxes
            .iter()
            .filter_map(|(area, hitbox)| {
                (*area == WindowControlArea::Drag).then_some(hitbox.bounds.size.height)
            })
            .max_by(|left, right| left.partial_cmp(right).unwrap_or(cmp::Ordering::Equal));
    }

    pub(in crate::window::input) fn dispatch_window_control_mouse_event(
        &mut self,
        event: &dyn Any,
        cx: &mut App,
    ) {
        if let Some(mouse_down) = event
            .downcast_ref::<MouseDownEvent>()
            .filter(|mouse_down| mouse_down.button == MouseButton::Left)
        {
            if self.window_control_area_under_mouse() == Some(WindowControlArea::Drag) {
                let mut gesture = mem::take(&mut self.window_control_drag_gesture);
                if gesture.mouse_down(mouse_down, Instant::now()) {
                    self.titlebar_double_click();
                    self.refresh();
                    self.reset_cursor_style(cx);
                }
                self.window_control_drag_gesture = gesture;
                cx.propagate_event = false;
                self.default_prevented = true;
            }
            return;
        }

        if let Some(mouse_move) = event
            .downcast_ref::<MouseMoveEvent>()
            .filter(|mouse_move| mouse_move.pressed_button == Some(MouseButton::Left))
        {
            let mut gesture = mem::take(&mut self.window_control_drag_gesture);
            if gesture.should_start_drag(mouse_move) {
                gesture.disarm();
                self.start_window_move();
                cx.propagate_event = false;
                self.default_prevented = true;
            }
            self.window_control_drag_gesture = gesture;
            return;
        }

        if event.downcast_ref::<MouseUpEvent>().is_some() {
            self.window_control_drag_gesture.handle_mouse_up();
        }
    }
}
