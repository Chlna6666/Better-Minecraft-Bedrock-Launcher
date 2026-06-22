use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use gpui::*;

use super::rows::{setting_toggle_row, tab_title};

mod background;
mod font;
mod theme_color;

pub(super) fn render_customization_tab(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    system_font_names: &[String],
) -> Div {
    let section = i18n.t("Settings.tabs.customization");

    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(tab_title(colors, section.clone()))
        .child(setting_toggle_row(
            colors,
            section,
            i18n.t("CustomizationSettings.launch_animation"),
            i18n.t("CustomizationSettings.launch_animation_desc"),
            state.show_launch_animation,
            "settings-launch-anim",
            |settings| settings.show_launch_animation = !settings.show_launch_animation,
        ))
        .child(theme_color::render_theme_color_card(colors, i18n, state))
        .child(font::render_font_card(
            colors,
            i18n,
            state,
            system_font_names,
        ))
        .child(background::render_background_card(colors, i18n, state))
}
