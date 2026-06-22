use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use gpui::*;

pub fn page_shell(content: impl IntoElement, colors: &ThemeColors) -> Div {
    let _ = colors;
    div()
        .absolute()
        .left(px(22.))
        .right(px(22.))
        .top(px(92.))
        .bottom(px(20.))
        .child(content)
}

pub fn panel_shell(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(18.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
}

pub fn icon_action(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    icon_path: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(38.))
        .h(px(38.))
        .rounded(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .child(themed_icon(icon_path, 18.0, colors.text_secondary))
}

pub fn secondary_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
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
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(label.into()),
        )
}

pub fn ghost_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(8.))
        .py(px(6.))
        .rounded(px(10.))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .child(label.into()),
        )
}

pub fn primary_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(16.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(colors.accent)
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(label.into()),
        )
}

pub fn tonal_badge(colors: &ThemeColors, label: impl Into<SharedString>, accent: Hsla) -> Div {
    div()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(Hsla { a: 0.12, ..accent })
        .text_size(px(11.))
        .text_color(accent)
        .child(label.into())
}

pub fn subtle_badge(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(colors.surface_hover)
        .text_size(px(11.))
        .text_color(colors.text_secondary)
        .child(label.into())
}

pub fn empty_state(
    colors: &ThemeColors,
    icon_path: &'static str,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> Div {
    div()
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(themed_icon(icon_path, 42.0, colors.text_muted))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_secondary)
                        .child(description.into()),
                ),
        )
}

pub fn card_title(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(text.into())
}
