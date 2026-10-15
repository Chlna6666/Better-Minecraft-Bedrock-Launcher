use crate::ui::components::dropdown::{self, DropdownOverlayState};
use gpui::*;

pub(super) struct DropdownOverlayView {
    _subscription: Subscription,
}

impl DropdownOverlayView {
    pub(super) fn new(window_id: u64, cx: &mut Context<Self>) -> Self {
        let mut revision = cx
            .global::<DropdownOverlayState>()
            .revision_for_window(window_id);
        let subscription = cx.observe_global::<DropdownOverlayState>(move |_, cx| {
            let next = cx
                .global::<DropdownOverlayState>()
                .revision_for_window(window_id);
            if next != revision {
                revision = next;
                cx.notify();
            }
        });
        Self {
            _subscription: subscription,
        }
    }
}

impl Render for DropdownOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().absolute().inset_0().child(dropdown::render_overlay(
            window,
            cx.global::<DropdownOverlayState>(),
        ))
    }
}
