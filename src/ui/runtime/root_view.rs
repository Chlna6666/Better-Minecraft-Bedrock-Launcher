use gpui::{AnyView, Context, IntoElement, ParentElement, Render, Styled, Window, div};

pub struct RootView {
    view: AnyView,
}

impl RootView {
    pub fn new(view: impl Into<AnyView>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { view: view.into() }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.view.clone())
    }
}
