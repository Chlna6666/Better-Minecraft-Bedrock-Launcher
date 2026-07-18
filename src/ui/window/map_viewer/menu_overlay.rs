use super::actions::MapViewerAction;
use gpui::{
    Context, EventEmitter, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseUpEvent, Render, ScrollWheelEvent, Styled, Window, div, px,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MapMenuOverlaySnapshot {
    pub open: bool,
}

#[derive(Default)]
pub struct MapMenuOverlayView {
    snapshot: MapMenuOverlaySnapshot,
}

impl MapMenuOverlayView {
    pub fn set_snapshot(&mut self, snapshot: MapMenuOverlaySnapshot, cx: &mut Context<Self>) {
        if self.snapshot == snapshot {
            return;
        }
        self.snapshot = snapshot;
        cx.notify();
    }
}

impl EventEmitter<MapViewerAction> for MapMenuOverlayView {}

impl Render for MapMenuOverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.snapshot.open {
            return div().absolute().w(px(0.0)).h(px(0.0)).into_any_element();
        }

        div()
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
                    cx.emit(MapViewerAction::CloseMenus);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_this, event: &MouseDownEvent, _window, cx| {
                    cx.emit(MapViewerAction::BeginRightSelectionAt(event.position));
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                    cx.emit(MapViewerAction::EndRightSelectionAt(event.position));
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                    cx.emit(MapViewerAction::EndRightSelectionAt(event.position));
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(
                cx.listener(|_this, _event: &ScrollWheelEvent, _window, cx| {
                    cx.emit(MapViewerAction::CloseMenus);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }
}
