use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

use super::super::common::{
    settings_card, settings_card_header, settings_sub_input_row, settings_sub_row,
    snapshot_from_state, spawn_persist_settings,
};
use super::super::rows::setting_dropdown_row;

pub(super) fn render_download_settings(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    div()
        .w_full()
        .mt(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(download_section_title(colors, i18n))
        .child(launcher_multi_thread_row(colors, i18n, state))
        .child(launcher_auto_thread_count_row(colors, i18n, state))
        .child(launcher_max_threads_row(colors, i18n, state))
        .child(launcher_proxy_mode_row(colors, i18n, state))
        .child(launcher_curseforge_source_row(colors, i18n, state))
}

fn download_section_title(colors: &ThemeColors, i18n: &I18n) -> impl IntoElement {
    div()
        .w_full()
        .pt(px(12.))
        .pb(px(4.))
        .text_size(px(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(
            div()
                .px(px(4.))
                .py(px(2.))
                .child(i18n.t("LauncherSettings.download.title")),
        )
}

fn launcher_multi_thread_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let section = i18n.t("Settings.tabs.launcher");
    super::super::rows::setting_toggle_row(
        colors,
        section,
        i18n.t("LauncherSettings.download.multi_thread"),
        i18n.t("LauncherSettings.download.multi_thread_desc"),
        state.download_multi_thread,
        "settings-launcher-download-multi-thread",
        |settings| {
            settings.download_multi_thread = !settings.download_multi_thread;
            if settings.download_multi_thread {
                settings.download_auto_thread_count = false;
            }
        },
    )
}

fn launcher_auto_thread_count_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let section = i18n.t("Settings.tabs.launcher");
    super::super::rows::setting_toggle_row(
        colors,
        section,
        i18n.t("LauncherSettings.download.auto_thread_count"),
        i18n.t("LauncherSettings.download.auto_thread_count_desc"),
        state.download_auto_thread_count,
        "settings-launcher-download-auto-thread-count",
        |settings| {
            settings.download_auto_thread_count = !settings.download_auto_thread_count;
            if settings.download_auto_thread_count {
                settings.download_multi_thread = false;
            }
        },
    )
}

fn launcher_max_threads_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let section = i18n.t("Settings.tabs.launcher");
    let thread_values: Vec<u32> = vec![1, 2, 4, 8, 16, 32, 64, 128, 256];
    let options = thread_values
        .iter()
        .map(|value| DropdownOption::from(SharedString::from(value.to_string())))
        .collect::<Vec<_>>();
    let selected_index = thread_values
        .iter()
        .position(|value| *value == state.download_max_threads)
        .unwrap_or(3);
    setting_dropdown_row(
        colors,
        section,
        i18n.t("LauncherSettings.download.max_threads"),
        i18n.t("LauncherSettings.download.multi_thread_desc"),
        "settings-launcher-download-max-threads",
        px(180.),
        SharedString::from(state.download_max_threads.to_string()),
        options,
        selected_index,
        state.download_multi_thread,
        move |index, _window, cx| {
            let value = thread_values.get(index).copied().unwrap_or(8);
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.download_max_threads = value;
                snapshot_from_state(settings)
            });
            spawn_persist_settings(snapshot, cx);
        },
    )
}

fn launcher_proxy_mode_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let values = vec![
        SharedString::from("none"),
        SharedString::from("system"),
        SharedString::from("http"),
        SharedString::from("socks5"),
    ];
    let options = vec![
        DropdownOption::from(i18n.t("LauncherSettings.download.proxy.none")),
        DropdownOption::from(i18n.t("LauncherSettings.download.proxy.system")),
        DropdownOption::from(i18n.t("LauncherSettings.download.proxy.http")),
        DropdownOption::from(i18n.t("LauncherSettings.download.proxy.socks5")),
    ];
    let selected_index = values
        .iter()
        .position(|value| value.as_ref() == state.download_proxy_type.as_ref())
        .unwrap_or(0);
    let label = options
        .get(selected_index)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| i18n.t("LauncherSettings.download.proxy.none"));
    let mode_dropdown = Dropdown::new(
        SharedString::from("settings-launcher-download-proxy-mode-dropdown"),
        colors,
        px(180.),
        label,
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let selected = values
                .get(index)
                .cloned()
                .unwrap_or_else(|| SharedString::from("none"));
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.download_proxy_type = selected.clone();
                snapshot_from_state(settings)
            });
            spawn_persist_settings(snapshot, cx);
        },
    );

    settings_card(colors, "settings-launcher-download-proxy-mode")
        .child(
            settings_card_header(
                colors,
                i18n.t("LauncherSettings.download.proxy.mode"),
                i18n.t("LauncherSettings.download.proxy.mode_desc"),
            )
            .child(mode_dropdown),
        )
        .when(state.download_proxy_type.as_ref() == "http", |this| {
            this.child(settings_sub_input_row(
                colors,
                i18n.t("LauncherSettings.download.proxy.http_proxy_url"),
                state.download_http_proxy_url_input.as_ref(),
                "http(s)://host:port",
            ))
        })
        .when(state.download_proxy_type.as_ref() == "socks5", |this| {
            this.child(settings_sub_input_row(
                colors,
                i18n.t("LauncherSettings.download.proxy.socks_proxy_url"),
                state.download_socks_proxy_url_input.as_ref(),
                "socks5://host:port",
            ))
        })
}

fn launcher_curseforge_source_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let values = vec![
        SharedString::from("official"),
        SharedString::from("mirror"),
        SharedString::from("custom"),
    ];
    let options = vec![
        DropdownOption::from(i18n.t("LauncherSettings.download.curseforge_api_source.official")),
        DropdownOption::from(i18n.t("LauncherSettings.download.curseforge_api_source.mirror")),
        DropdownOption::from(i18n.t("LauncherSettings.download.curseforge_api_source.custom")),
    ];
    let selected_index = values
        .iter()
        .position(|value| value.as_ref() == state.download_curseforge_api_source.as_ref())
        .unwrap_or(1);
    let label = options
        .get(selected_index)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| i18n.t("LauncherSettings.download.curseforge_api_source.mirror"));
    let source_dropdown = Dropdown::new(
        SharedString::from("settings-launcher-download-cf-source-dropdown"),
        colors,
        px(180.),
        label,
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let selected = values
                .get(index)
                .cloned()
                .unwrap_or_else(|| SharedString::from("mirror"));
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.download_curseforge_api_source = selected.clone();
                snapshot_from_state(settings)
            });
            spawn_persist_settings(snapshot, cx);
        },
    );

    settings_card(colors, "settings-launcher-download-cf-source")
        .child(
            settings_card_header(
                colors,
                i18n.t("LauncherSettings.download.curseforge_api_source"),
                i18n.t("LauncherSettings.download.curseforge_api_source_desc"),
            )
            .child(source_dropdown),
        )
        .when(
            state.download_curseforge_api_source.as_ref() == "custom",
            |this| {
                this.child(settings_sub_input_row(
                    colors,
                    i18n.t("LauncherSettings.download.curseforge_api_base"),
                    state.download_curseforge_api_base_input.as_ref(),
                    "https://api.curseforge.com",
                ))
            },
        )
}
