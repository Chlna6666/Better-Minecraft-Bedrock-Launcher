use crate::ui::components::icon::themed_icon;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    base: gpui::Div,
    label: Option<SharedString>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            base: div(),
            label: None,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut button = self
            .base
            .id(self.id)
            .px(px(12.))
            .py(px(8.))
            .rounded(px(10.))
            .border_1()
            .cursor_pointer()
            .child(self.label.unwrap_or_default());

        if let Some(on_click) = self.on_click {
            button = button.on_click(move |ev, window, cx| {
                (on_click)(ev, window, cx);
            });
        }

        button
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    base: gpui::Div,
    icon_path: &'static str,
    icon_size: f32,
    icon_color: Option<gpui::Hsla>,
    label: Option<SharedString>,
    disabled: bool,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon_path: &'static str) -> Self {
        Self {
            id: id.into(),
            base: div(),
            icon_path,
            icon_size: 18.0,
            icon_color: None,
            label: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn icon_size(mut self, icon_size: f32) -> Self {
        self.icon_size = icon_size;
        self
    }

    pub fn icon_color(mut self, icon_color: gpui::Hsla) -> Self {
        self.icon_color = Some(icon_color);
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for IconButton {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon_color = self.icon_color.unwrap_or(gpui::transparent_black());
        let mut button = self
            .base
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .child(themed_icon(self.icon_path, self.icon_size, icon_color));

        if self.disabled {
            button = button.opacity(0.45);
        } else {
            button = button.cursor_pointer();
        }

        if let Some(label) = self.label {
            button = button.child(label);
        }

        if let Some(on_click) = self.on_click.filter(|_| !self.disabled) {
            button = button.on_click(move |event, window, cx| {
                (on_click)(event, window, cx);
            });
        }

        button
    }
}

pub fn secondary_button(
    colors: &crate::ui::theme::ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(16.))
        .py(px(10.))
        .rounded(px(12.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(label.into()),
        )
}

pub fn ghost_button(
    colors: &crate::ui::theme::ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(8.))
        .py(px(6.))
        .rounded(px(10.))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .child(label.into()),
        )
}

pub fn primary_button(
    colors: &crate::ui::theme::ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(16.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(colors.accent)
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(label.into()),
        )
}
