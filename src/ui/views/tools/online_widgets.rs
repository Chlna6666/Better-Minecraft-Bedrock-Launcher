use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

pub(super) fn pill_button(
    colors: &ThemeColors,
    id: &'static str,
    label: SharedString,
    icon: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(10.))
        .min_w(px(0.))
        .child(themed_icon(icon, 18.0, colors.text_primary))
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_primary)
                .truncate()
                .child(label),
        )
}

pub(super) fn primary_button(
    colors: &ThemeColors,
    id: &'static str,
    label: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(18.))
        .py(px(12.))
        .rounded(px(14.))
        .bg(Hsla {
            a: 0.12,
            ..colors.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.accent
        })
        .cursor_pointer()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.accent)
        .child(label)
}

pub(super) fn icon_button(
    colors: &ThemeColors,
    id: &'static str,
    icon: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.))
        .h(px(34.))
        .rounded(px(12.))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(icon, 16.0, colors.text_primary))
}
