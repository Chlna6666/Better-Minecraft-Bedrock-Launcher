use crate::i18n::Locale;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::components::{dropdown::Dropdown, dropdown::DropdownOption};
use crate::ui::state::i18n::{I18n, LanguageController, LanguagePreference, LanguageRequest};
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use tracing::warn;

use super::common::{
    settings_action_button, settings_card, settings_card_header, settings_sub_row,
    snapshot_from_state, spawn_persist_settings,
};
use super::rows::{setting_dropdown_row, setting_toggle_row, tab_title};
use crate::ui::components::toast::{self, ToastKind};

mod connectivity;
mod download;
pub(super) mod logs;

pub(super) fn render_launcher_tab(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> Div {
    let section = t!("Settings.tabs.launcher");

    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(tab_title(colors, section.clone()))
        .child(launcher_language_row(colors, i18n, state))
        .child(launcher_render_engine_row(colors, i18n, state))
        .child(launcher_gpu_adapter_row(colors, i18n, state))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            t!("LauncherSettings.debug"),
            t!("LauncherSettings.debug_desc"),
            state.debug,
            "settings-launcher-debug",
            |settings| settings.debug = !settings.debug,
        ))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            t!("LauncherSettings.stats_upload"),
            t!("LauncherSettings.stats_upload_desc"),
            state.stats_upload,
            "settings-launcher-stats",
            |settings| settings.stats_upload = !settings.stats_upload,
        ))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            t!("LauncherSettings.error_report_sentry"),
            t!("LauncherSettings.error_report_sentry_desc"),
            state.error_report_sentry_enabled,
            "settings-launcher-error-report-sentry",
            |settings| settings.error_report_sentry_enabled = !settings.error_report_sentry_enabled,
        ))
        .child(setting_toggle_row(
            colors,
            section.clone(),
            t!("LauncherSettings.error_report_sentry_auto"),
            t!("LauncherSettings.error_report_sentry_auto_desc"),
            state.error_report_sentry_auto,
            "settings-launcher-error-report-sentry-auto",
            |settings| settings.error_report_sentry_auto = !settings.error_report_sentry_auto,
        ))
        .child(launcher_sentry_test_row(colors, i18n, state))
        .child(logs::render(colors, i18n, state))
        .child(connectivity::launcher_connectivity_row(
            colors,
            i18n,
            state.launcher_connectivity_running,
        ))
        .child(launcher_auto_update_group(colors, i18n, state))
        .child(download::render_download_settings(colors, i18n, state))
}

pub(super) fn render_connectivity_modal(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    i18n: &I18n,
    state: &SettingsPageState,
) -> Option<AnyElement> {
    connectivity::render_connectivity_modal(colors, i18n, state, window_width, window_height)
}

fn launcher_sentry_test_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let enabled = state.error_report_sentry_enabled;

    settings_card(colors, "settings-launcher-error-report-sentry-test")
        .px(px(16.))
        .py(px(16.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(22.))
        .child(super::common::settings_card_text(
            colors,
            t!("LauncherSettings.error_report_sentry_test"),
            t!("LauncherSettings.error_report_sentry_test_desc"),
        ))
        .child(
            settings_action_button(
                colors,
                t!("LauncherSettings.error_report_sentry_test_send"),
                enabled,
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                if !enabled {
                    toast::error(cx, t!("Diagnostics.toast.sentry_unconfigured"));
                    return;
                }

                spawn_sentry_test_log(cx);
            }),
        )
}

fn spawn_sentry_test_log(cx: &mut App) {
    let toast_id = toast::pending(cx, t!("LauncherSettings.error_report_sentry_test_sending"));
    let success_message = t!("LauncherSettings.error_report_sentry_test_sent");

    cx.spawn(async move |cx| {
        let result = cx
            .background_spawn(async move {
                let config = crate::config::config::read_config()?;
                let Some(dsn) =
                    crate::config::config::resolved_error_report_sentry_dsn(&config.launcher)
                else {
                    anyhow::bail!("sentry dsn is not configured");
                };

                crate::utils::diagnostics::send_sentry_test_log(&dsn)
            })
            .await;

        match result {
            Ok(()) => toast::resolve_async(cx, toast_id, ToastKind::Success, success_message),
            Err(error) => toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("Sentry test log failed: {error:#}")),
            ),
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn resolve_locale_for_code(code: &str) -> Locale {
    if code.trim().eq_ignore_ascii_case("auto") {
        let system_language = crate::utils::system_info::get_system_language();
        Locale::from_code(&system_language).unwrap_or(Locale::EnUs)
    } else {
        Locale::from_code(code).unwrap_or(Locale::EnUs)
    }
}

fn submit_language(request: LanguageRequest, cx: &mut App) {
    let requested_locale = request.locale;
    let request_to_start =
        cx.update_global(|controller: &mut LanguageController, _cx| controller.submit(request));

    if cx.global::<I18n>().locale() != requested_locale {
        cx.update_global(|i18n: &mut I18n, _cx| {
            i18n.set_locale(requested_locale);
        });
    }

    let Some((request_id, request)) = request_to_start else {
        return;
    };

    let toast_id = toast::pending(
        cx,
        cx.global::<I18n>()
            .t_key(crate::i18n_key!("LauncherSettings.language_saving")),
    );
    cx.spawn(async move |cx| {
        let mut current = (request_id, request);
        loop {
            let (id, request) = current;
            let save_result = crate::tasks::runtime::run_io_blocking({
                let code = request.preference.code().to_string();
                move || crate::config::config::persist_language(&code)
            })
            .await;
            let error_text = match save_result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => {
                    warn!("persist language failed: {error}");
                    Some(error.to_string())
                }
                Err(error) => {
                    warn!("persist language join error: {error}");
                    Some(error.to_string())
                }
            };

            let completion = match cx.update_global(|controller: &mut LanguageController, _cx| {
                controller.complete(id, error_text.is_none())
            }) {
                Ok(completion) => completion,
                Err(error) => {
                    warn!("complete language request failed: {error:?}");
                    break;
                }
            };

            if let Some(rollback) = completion.rollback {
                let current_locale = cx
                    .read_global(|i18n: &I18n, _cx| i18n.locale())
                    .unwrap_or(rollback.locale);
                if current_locale != rollback.locale
                    && let Err(error) = cx.update_global(|i18n: &mut I18n, _cx| {
                        i18n.set_locale(rollback.locale);
                    })
                {
                    warn!("rollback language failed: {error:?}");
                }
                if let Err(error) = cx.update_global(|settings: &mut SettingsPageState, _cx| {
                    settings.language = SharedString::from(rollback.preference.code());
                }) {
                    warn!("rollback language setting failed: {error:?}");
                }
                let error = error_text.unwrap_or_else(|| "unknown error".to_string());
                let message = cx
                    .read_global(|i18n: &I18n, _cx| {
                        t!("LauncherSettings.language_save_failed", error = error)
                    })
                    .unwrap_or_else(|_| SharedString::from("Language save failed"));
                toast::resolve_async(cx, toast_id, ToastKind::Error, message);
                break;
            }

            if let Some(next) = completion.next {
                current = next;
                continue;
            }

            let message = cx
                .read_global(|i18n: &I18n, _cx| {
                    i18n.t_key(crate::i18n_key!("LauncherSettings.language_saved"))
                })
                .unwrap_or_else(|_| SharedString::from("Language saved"));
            toast::resolve_async(cx, toast_id, ToastKind::Success, message);
            break;
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn launcher_language_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    fn language_label(i18n: &I18n, code: &str) -> SharedString {
        let code = code.trim();
        if code.eq_ignore_ascii_case("auto") {
            return t!("LauncherSettings.lang_options.auto");
        }
        match Locale::from_code(code) {
            Some(Locale::ZhCn) => SharedString::from("简体中文"),
            Some(Locale::ZhTw) => SharedString::from("繁体中文"),
            Some(Locale::EnUs) => SharedString::from("English"),
            Some(Locale::JaJp) => SharedString::from("日本語"),
            Some(Locale::KoKr) => SharedString::from("한국어"),
            None => SharedString::from(code.to_string()),
        }
    }

    let section = t!("Settings.tabs.launcher");

    let codes: Vec<SharedString> = vec![
        SharedString::from("auto"),
        SharedString::from("zh-CN"),
        SharedString::from("zh-TW"),
        SharedString::from("en-US"),
        SharedString::from("ja-JP"),
        SharedString::from("ko-KR"),
    ];

    let mut options = Vec::with_capacity(codes.len());
    for code in &codes {
        options.push(DropdownOption::from(language_label(i18n, code.as_ref())));
    }

    let selected_index = codes
        .iter()
        .position(|c| {
            c.as_ref()
                .eq_ignore_ascii_case(state.language.as_ref().trim())
        })
        .unwrap_or(0);

    let display = language_label(i18n, state.language.as_ref());

    setting_dropdown_row(
        colors,
        section,
        t!("LauncherSettings.language"),
        t!("LauncherSettings.language_desc"),
        "settings-language",
        px(180.),
        display,
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let new_code = codes
                .get(index)
                .cloned()
                .unwrap_or_else(|| SharedString::from("auto"))
                .to_string();
            let resolved = resolve_locale_for_code(&new_code);
            let preference = if new_code.eq_ignore_ascii_case("auto") {
                LanguagePreference::Auto
            } else {
                LanguagePreference::Explicit(resolved)
            };
            cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.language = SharedString::from(new_code.clone());
            });
            submit_language(
                LanguageRequest {
                    preference,
                    locale: resolved,
                },
                cx,
            );
        },
    )
}

fn render_engine_label(i18n: &I18n, renderer_backend: &SharedString) -> SharedString {
    match crate::config::config::normalize_renderer_backend(renderer_backend.as_ref()).as_str() {
        "vulkan" => t!("LauncherSettings.render_engine.vulkan"),
        "dx12" => t!("LauncherSettings.render_engine.dx12"),
        _ => t!("LauncherSettings.render_engine.auto"),
    }
}

fn launcher_render_engine_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let section = t!("Settings.tabs.launcher");

    let values = if cfg!(target_os = "linux") {
        vec![SharedString::from("auto"), SharedString::from("vulkan")]
    } else {
        vec![
            SharedString::from("auto"),
            SharedString::from("vulkan"),
            SharedString::from("dx12"),
        ]
    };

    let current =
        crate::config::config::normalize_renderer_backend(state.renderer_backend.as_ref());
    let options = values
        .iter()
        .map(|value| DropdownOption::from(render_engine_label(i18n, value)))
        .collect::<Vec<_>>();
    let selected_index = values
        .iter()
        .position(|value| value.as_ref().eq_ignore_ascii_case(&current))
        .unwrap_or(0);
    let display = values
        .get(selected_index)
        .map(|value| render_engine_label(i18n, value))
        .unwrap_or_else(|| t!("LauncherSettings.render_engine.auto"));

    setting_dropdown_row(
        colors,
        section,
        t!("LauncherSettings.render_engine"),
        t!("LauncherSettings.render_engine_desc"),
        "settings-render-engine",
        px(180.),
        display,
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let renderer_backend = values
                .get(index)
                .cloned()
                .unwrap_or_else(|| SharedString::from("auto"))
                .to_string();
            let selected_renderer_backend = renderer_backend.clone();
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.renderer_backend = SharedString::from(renderer_backend);
                settings.gpu_adapter_name =
                    SharedString::from(crate::config::config::default_gpu_adapter_name());
                settings.gpu_adapter_options.clear();
                snapshot_from_state(settings)
            });
            super::tabs::refresh_gpu_adapters_for_backend(selected_renderer_backend, cx);
            spawn_persist_settings(snapshot, cx);
        },
    )
}

fn gpu_adapter_options(state: &SettingsPageState) -> Vec<SharedString> {
    let mut values = Vec::with_capacity(state.gpu_adapter_options.len() + 2);
    values.push(SharedString::from(
        crate::config::config::default_gpu_adapter_name(),
    ));
    for adapter_name in &state.gpu_adapter_options {
        if adapter_name.as_ref().trim().is_empty()
            || values.iter().any(|value| {
                value
                    .as_ref()
                    .eq_ignore_ascii_case(adapter_name.as_ref().trim())
            })
        {
            continue;
        }
        values.push(adapter_name.clone());
    }
    let current = state.gpu_adapter_name.as_ref().trim();
    if !current.is_empty()
        && !current.eq_ignore_ascii_case("auto")
        && !values
            .iter()
            .any(|value| value.as_ref().eq_ignore_ascii_case(current))
    {
        values.push(SharedString::from(current.to_string()));
    }
    values
}

fn gpu_adapter_label(i18n: &I18n, adapter_name: &SharedString) -> SharedString {
    if adapter_name.as_ref().eq_ignore_ascii_case("auto") {
        t!("LauncherSettings.gpu_adapter.auto")
    } else {
        adapter_name.clone()
    }
}

fn launcher_gpu_adapter_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let section = t!("Settings.tabs.launcher");
    let values = gpu_adapter_options(state);
    let current =
        crate::config::config::normalize_gpu_adapter_name(state.gpu_adapter_name.as_ref());

    let options = values
        .iter()
        .map(|value| DropdownOption::from(gpu_adapter_label(i18n, value)))
        .collect::<Vec<_>>();
    let selected_index = values
        .iter()
        .position(|value| value.as_ref().eq_ignore_ascii_case(&current))
        .unwrap_or(0);
    let display = values
        .get(selected_index)
        .map(|value| gpu_adapter_label(i18n, value))
        .unwrap_or_else(|| t!("LauncherSettings.gpu_adapter.auto"));

    setting_dropdown_row(
        colors,
        section,
        t!("LauncherSettings.gpu_adapter"),
        t!("LauncherSettings.gpu_adapter_desc"),
        "settings-gpu-adapter",
        px(260.),
        display,
        options,
        selected_index,
        values.len() > 1,
        move |index, _window, cx| {
            let gpu_adapter_name = values
                .get(index)
                .cloned()
                .unwrap_or_else(|| SharedString::from("auto"))
                .to_string();
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.gpu_adapter_name = SharedString::from(gpu_adapter_name);
                snapshot_from_state(settings)
            });
            spawn_persist_settings(snapshot, cx);
        },
    )
}

fn launcher_update_channel_dropdown(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let options = vec![
        DropdownOption::from(t!("LauncherSettings.update_channel.stable")),
        DropdownOption::from(t!("LauncherSettings.update_channel.nightly")),
    ];

    let (label, selected_index) = if state.update_channel_nightly {
        (t!("LauncherSettings.update_channel.nightly"), 1usize)
    } else {
        (t!("LauncherSettings.update_channel.stable"), 0usize)
    };

    Dropdown::new(
        SharedString::from("settings-update-channel-dropdown"),
        colors,
        px(180.),
        label,
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let nightly = index == 1;
            let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                settings.update_channel_nightly = nightly;
                snapshot_from_state(settings)
            });
            spawn_persist_settings(snapshot, cx);
        },
    )
}

fn launcher_auto_update_group(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    settings_card(colors, "settings-launcher-auto-update")
        .child(
            settings_card_header(
                colors,
                t!("LauncherSettings.auto_check_updates"),
                t!("LauncherSettings.auto_check_updates_desc"),
            )
            .child(ToggleSwitch::new(
                SharedString::from("settings-launcher-autoupdate-toggle"),
                colors,
                state.auto_check_updates,
                move |cx| {
                    let snapshot = cx.update_global(|settings: &mut SettingsPageState, cx| {
                        settings.auto_check_updates = !settings.auto_check_updates;
                        snapshot_from_state(settings)
                    });
                    spawn_persist_settings(snapshot, cx);
                },
            )),
        )
        .when(state.auto_check_updates, |this| {
            this.child(settings_sub_row(
                colors,
                t!("LauncherSettings.update_channel"),
                launcher_update_channel_dropdown(colors, i18n, state),
            ))
        })
}
