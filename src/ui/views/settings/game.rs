use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{LauncherDisplayMode, SettingsPageState};
use gpui::*;

use super::common::{snapshot_from_state, spawn_persist_settings};
use super::rows::{setting_dropdown_row, setting_toggle_row, tab_title};
use crate::ui::components::dropdown::DropdownOption;

pub(super) fn render_game_tab(colors: &ThemeColors, i18n: &I18n, state: &SettingsPageState) -> Div {
    let section = i18n.t("Settings.tabs.game");

    let display_mode_options = vec![
        DropdownOption::from(i18n.t("GameSettings.visibility.minimize")),
        DropdownOption::from(i18n.t("GameSettings.visibility.close")),
        DropdownOption::from(i18n.t("GameSettings.visibility.keep")),
    ];

    let (display_mode_label, display_mode_selected) = match state.launcher_display_mode {
        LauncherDisplayMode::MinimizeOnLaunch => {
            (i18n.t("GameSettings.visibility.minimize"), 0usize)
        }
        LauncherDisplayMode::CloseOnLaunch => (i18n.t("GameSettings.visibility.close"), 1usize),
        LauncherDisplayMode::KeepVisible => (i18n.t("GameSettings.visibility.keep"), 2usize),
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(tab_title(colors, section.clone()))
        .child(setting_dropdown_row(
            colors,
            section.clone(),
            i18n.t("GameSettings.launcher_visibility"),
            i18n.t("GameSettings.launcher_visibility_desc"),
            "settings-launcher-display-mode",
            px(220.),
            display_mode_label,
            display_mode_options,
            display_mode_selected,
            true,
            move |index, _window, cx| {
                let mode = match index {
                    0 => LauncherDisplayMode::MinimizeOnLaunch,
                    1 => LauncherDisplayMode::CloseOnLaunch,
                    _ => LauncherDisplayMode::KeepVisible,
                };
                let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                    if settings.launcher_display_mode == mode {
                        return snapshot_from_state(settings);
                    }
                    settings.launcher_display_mode = mode;
                    snapshot_from_state(settings)
                });
                spawn_persist_settings(snapshot, cx);
            },
        ))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            i18n.t("GameSettings.uwp_minimize_fix"),
            i18n.t("GameSettings.uwp_minimize_fix_desc"),
            state.fix_uwp_minimize,
            "settings-uwp-minimize",
            |settings| settings.fix_uwp_minimize = !settings.fix_uwp_minimize,
        ))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            i18n.t("GameSettings.keep_downloaded_game_package"),
            i18n.t("GameSettings.keep_downloaded_game_package_desc"),
            state.keep_downloaded_packages,
            "settings-keep-package",
            |settings| settings.keep_downloaded_packages = !settings.keep_downloaded_packages,
        ))
        .child(setting_toggle_row(
            colors,
            section,
            i18n.t("GameSettings.modify_appx_manifest"),
            i18n.t("GameSettings.modify_appx_manifest_desc"),
            state.modify_appx_manifest,
            "settings-modify-manifest",
            |settings| settings.modify_appx_manifest = !settings.modify_appx_manifest,
        ))
}
