use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use std::{cell::RefCell, rc::Rc};

const SLIDER_HEIGHT: f32 = 30.0;
const TRACK_HEIGHT: f32 = 4.0;
const TRACK_INSET: f32 = 8.0;
const HANDLE_SIZE: f32 = 16.0;

#[derive(Clone)]
pub struct SliderDrag {
    id: String,
    min: f32,
    max: f32,
    on_change: Rc<dyn Fn(f32, &mut App)>,
}

#[derive(Default)]
struct SliderState {
    active_drag_id: Option<String>,
}

impl Global for SliderState {}

#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    colors: ThemeColors,
    min: f32,
    max: f32,
    value: f32,
    width: Pixels,
    on_change: Rc<dyn Fn(f32, &mut App)>,
    on_commit: Option<Rc<dyn Fn(f32, &mut App)>>,
}

impl Slider {
    pub fn new(
        id: impl Into<ElementId>,
        colors: &ThemeColors,
        min: f32,
        max: f32,
        value: f32,
        on_change: impl Fn(f32, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            colors: colors.clone(),
            min,
            max,
            value,
            width: px(280.0),
            on_change: Rc::new(on_change),
            on_commit: None,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn on_commit(mut self, on_commit: impl Fn(f32, &mut App) + 'static) -> Self {
        self.on_commit = Some(Rc::new(on_commit));
        self
    }

    fn normalized_value(&self) -> f32 {
        if !self.min.is_finite()
            || !self.max.is_finite()
            || !self.value.is_finite()
            || (self.max - self.min).abs() <= f32::EPSILON
        {
            return 0.0;
        }

        ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    fn value_from_position(min: f32, max: f32, position_x: Pixels, bounds: Bounds<Pixels>) -> f32 {
        let width = (bounds.size.width / px(1.0) - TRACK_INSET * 2.0).max(1.0);
        let offset = (position_x - bounds.left()) / px(1.0) - TRACK_INSET;
        let ratio = (offset / width).clamp(0.0, 1.0);
        min + (max - min) * ratio
    }

    fn is_active_drag(id: &str, cx: &App) -> bool {
        cx.try_global::<SliderState>()
            .is_some_and(|state| state.active_drag_id.as_deref() == Some(id))
    }

    fn begin_active_drag(id: &str, cx: &mut App) {
        if !cx.has_global::<SliderState>() {
            cx.set_global(SliderState::default());
        }
        cx.update_global(|state: &mut SliderState, _cx| {
            state.active_drag_id = Some(id.to_string());
        });
    }

    fn end_active_drag(id: &str, cx: &mut App) -> bool {
        if !Self::is_active_drag(id, cx) {
            return false;
        }
        cx.update_global(|state: &mut SliderState, _cx| {
            state.active_drag_id = None;
        });
        true
    }
}

impl RenderOnce for Slider {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let id = self.id.to_string();
        let ratio = self.normalized_value();
        let width_px = self.width / px(1.0);
        let track_width = (width_px - TRACK_INSET * 2.0).max(1.0);
        let fill_width = track_width * ratio;
        let handle_left = TRACK_INSET + fill_width - HANDLE_SIZE / 2.0;
        let track_top = (SLIDER_HEIGHT - TRACK_HEIGHT) / 2.0;
        let handle_top = (SLIDER_HEIGHT - HANDLE_SIZE) / 2.0;
        let colors = self.colors;
        let min = self.min;
        let max = self.max;
        let on_change = self.on_change.clone();
        let on_commit = self.on_commit.clone();
        let on_click_change = self.on_change.clone();
        let slider_bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::default();
        let slider_bounds_for_prepaint = slider_bounds.clone();
        let slider_bounds_for_click = slider_bounds.clone();
        let slider_bounds_for_commit = slider_bounds.clone();
        let slider_bounds_for_commit_out = slider_bounds.clone();
        let on_commit_inside = on_commit.clone();
        let on_commit_outside = on_commit.clone();
        let drag = SliderDrag {
            id: id.clone(),
            min,
            max,
            on_change: on_change.clone(),
        };
        let id_for_drag = id.clone();
        let id_for_mouse_down = id.clone();
        let id_for_mouse_down_out = id.clone();
        let id_for_commit = id.clone();
        let id_for_commit_out = id.clone();

        div()
            .on_children_prepainted(move |children_bounds, _window, _cx| {
                *slider_bounds_for_prepaint.borrow_mut() = children_bounds.first().copied();
            })
            .id(self.id)
            .relative()
            .w(self.width)
            .h(px(SLIDER_HEIGHT))
            .flex_shrink_0()
            .cursor_pointer()
            .on_drag(drag, |_: &SliderDrag, _, _, cx| cx.new(|_| Empty))
            .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                Self::begin_active_drag(&id_for_mouse_down, cx);
                if let Some(bounds) = *slider_bounds_for_click.borrow() {
                    let value = Self::value_from_position(min, max, event.position.x, bounds);
                    (on_click_change)(value, cx);
                }
            })
            .on_mouse_down_out(move |_event, _window, cx| {
                let _ = Self::end_active_drag(&id_for_mouse_down_out, cx);
            })
            .on_mouse_up(MouseButton::Left, move |event, _window, cx| {
                if !Self::end_active_drag(&id_for_commit, cx) {
                    return;
                }
                let Some(on_commit) = on_commit_inside.as_ref() else {
                    return;
                };
                if let Some(bounds) = *slider_bounds_for_commit.borrow() {
                    let value = Self::value_from_position(min, max, event.position.x, bounds);
                    on_commit(value, cx);
                }
            })
            .on_mouse_up_out(MouseButton::Left, move |event, _window, cx| {
                if !Self::end_active_drag(&id_for_commit_out, cx) {
                    return;
                }
                let Some(on_commit) = on_commit_outside.as_ref() else {
                    return;
                };
                if let Some(bounds) = *slider_bounds_for_commit_out.borrow() {
                    let value = Self::value_from_position(min, max, event.position.x, bounds);
                    on_commit(value, cx);
                }
            })
            .on_drag_move::<SliderDrag>(move |event, _window, cx| {
                let drag = event.drag(cx);
                if drag.id != id_for_drag {
                    return;
                }
                let on_change = drag.on_change.clone();
                let min = drag.min;
                let max = drag.max;
                let value =
                    Self::value_from_position(min, max, event.event.position.x, event.bounds);
                (on_change)(value, cx);
            })
            .child(div().w(self.width).h(px(SLIDER_HEIGHT)))
            .child(
                div()
                    .absolute()
                    .left(px(TRACK_INSET))
                    .top(px(track_top))
                    .w(px(track_width))
                    .h(px(TRACK_HEIGHT))
                    .rounded_full()
                    .bg(Hsla {
                        a: 0.72,
                        ..colors.settings_field_bg
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(TRACK_INSET))
                    .top(px(track_top))
                    .w(px(fill_width))
                    .h(px(TRACK_HEIGHT))
                    .rounded_full()
                    .bg(Hsla {
                        a: 0.9,
                        ..colors.accent
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(handle_left))
                    .top(px(handle_top))
                    .w(px(HANDLE_SIZE))
                    .h(px(HANDLE_SIZE))
                    .rounded_full()
                    .bg(colors.accent)
                    .border_2()
                    .border_color(Hsla {
                        a: 0.86,
                        ..colors.surface
                    }),
            )
    }
}
