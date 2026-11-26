use gpui::{AnyElement, Context, IntoElement, Window};

use crate::hooks::HasHooks;

pub trait HookedElement: HasHooks {
    fn cleanup_effects(&self) {
        self.cleanup_hooks();
    }
}

impl<T> HookedElement for T where T: HasHooks {}

pub trait HookedRender: HookedElement + Sized + 'static {
    fn pre_render(&self, _window: &mut Window, _cx: &mut Context<Self>) {
        self._begin_hooks();
    }

    fn post_render(&self, _window: &mut Window, _cx: &mut Context<Self>) {
        self._finish_hooks();
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}

pub fn execute_hooked_render<T: HookedRender>(
    this: &mut T,
    window: &mut Window,
    cx: &mut Context<T>,
) -> AnyElement {
    this.pre_render(window, cx);
    let element = this.render(window, cx).into_any_element();
    this.post_render(window, cx);
    element
}
