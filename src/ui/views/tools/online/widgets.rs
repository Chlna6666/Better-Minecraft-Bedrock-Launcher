use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

pub(super) fn action_button(
    colors: &ThemeColors,
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    disabled: bool,
    danger: bool,
) -> Stateful<Div> {
    let accent = if danger { colors.danger } else { colors.accent };
    div()
        .id(id)
        .min_h(px(40.))
        .px(px(14.))
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla { a: 0.28, ..accent })
        .bg(Hsla { a: 0.12, ..accent })
        .opacity(if disabled { 0.46 } else { 1.0 })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(Hsla { a: 0.20, ..accent }))
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(themed_icon(icon, 16.0, accent))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(accent)
                .child(label),
        )
}

pub(super) fn subtle_button(
    colors: &ThemeColors,
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .min_h(px(40.))
        .px(px(14.))
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.56,
            ..colors.surface
        })
        .opacity(if disabled { 0.46 } else { 1.0 })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.surface_hover))
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(themed_icon(icon, 16.0, colors.text_secondary))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .child(label),
        )
}

pub(super) fn icon_button(
    colors: &ThemeColors,
    id: &'static str,
    icon: &'static str,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(40.))
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.62,
            ..colors.surface
        })
        .opacity(if disabled { 0.46 } else { 1.0 })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.surface_hover))
        })
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(icon, 17.0, colors.text_secondary))
}
