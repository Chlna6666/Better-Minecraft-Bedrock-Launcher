use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::StatefulInteractiveElement as _;
use gpui::*;

#[cfg(target_os = "linux")]
use super::proton_gdk;
use super::{about, customization, game, launcher, plugins};

pub(super) fn render_settings_content(
    colors: &ThemeColors,
    window_width: Pixels,
    render_engine: SharedString,
    i18n: &I18n,
    state: &SettingsPageState,
    plugin_model: &plugins::PluginSettingsModel,
    update: &UpdateState,
    system_font_names: &[String],
) -> impl IntoElement {
    let panel: AnyElement = match state.tab {
        SettingsTab::Game => game::render_game_tab(colors, i18n, state).into_any_element(),
        SettingsTab::Launcher => {
            launcher::render_launcher_tab(colors, i18n, state).into_any_element()
        }
        #[cfg(target_os = "linux")]
        SettingsTab::ProtonGdk => proton_gdk::render(colors).into_any_element(),
        SettingsTab::Customization => {
            customization::render_customization_tab(colors, i18n, state, system_font_names)
                .into_any_element()
        }
        SettingsTab::Plugins => {
            plugins::render_plugins_tab(colors, i18n, state, plugin_model).into_any_element()
        }
        SettingsTab::About => {
            about::render_about_tab(colors, window_width, render_engine, i18n, state, update)
                .into_any_element()
        }
    };

    let scroll_area = div()
        .id("settings-content-scroll")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .max_w(px(960.))
                    .pt(px(6.))
                    .pb(px(24.))
                    .child(panel),
            ),
        );

    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(scroll_area)
}
