use super::common::{
    settings_card_text, settings_inline_card, snapshot_from_state, spawn_persist_settings,
};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use gpui::*;

pub(super) fn tab_title(colors: &ThemeColors, title: SharedString) -> Div {
    div().w_full().pt(px(2.)).pb(px(8.)).child(
        div()
            .text_size(px(22.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.text_primary)
            .child(title),
    )
}

pub(super) fn section_placeholder(
    colors: &ThemeColors,
    title: SharedString,
    desc: SharedString,
) -> impl IntoElement {
    div()
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.98,
            ..colors.surface
        })
        .p(px(12.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(title),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(desc),
        )
}

pub(super) fn setting_dropdown_row(
    colors: &ThemeColors,
    section: SharedString,
    title: SharedString,
    desc: SharedString,
    id: &'static str,
    width: Pixels,
    label: SharedString,
    options: Vec<DropdownOption>,
    selected_index: usize,
    enabled: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let option_labels: Vec<SharedString> = options.iter().map(|opt| opt.label.clone()).collect();
    settings_inline_card(colors, id)
        .child(settings_card_text(colors, title.clone(), desc))
        .child(div().flex_shrink_0().pt(px(2.)).child(Dropdown::new(
            SharedString::from(format!("{id}-dropdown")),
            colors,
            width,
            label,
            options,
            selected_index,
            enabled,
            move |index, window, cx| {
                on_select(index, window, cx);
                // Settings page: keep toasts reserved for "save succeeded/failed" only.
            },
        )))
}

pub(super) fn setting_toggle_row(
    colors: &ThemeColors,
    section: SharedString,
    title: SharedString,
    desc: SharedString,
    enabled: bool,
    id: &'static str,
    on_toggle: fn(&mut SettingsPageState),
) -> impl IntoElement {
    settings_inline_card(colors, id)
        .child(settings_card_text(colors, title.clone(), desc))
        .child(div().flex_shrink_0().pt(px(4.)).child(ToggleSwitch::new(
            SharedString::from(format!("{id}-toggle")),
            colors,
            enabled,
            move |cx| {
                let next_enabled = !enabled;
                let snapshot = cx.update_global(|s: &mut SettingsPageState, cx| {
                    on_toggle(s);
                    snapshot_from_state(s)
                });
                spawn_persist_settings(snapshot, cx);
            },
        )))
}
