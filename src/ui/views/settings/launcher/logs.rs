use crate::config::config::LogManagementConfig;
use crate::ui::components::dropdown::DropdownOption;
use crate::ui::components::toast::{self, ToastKind};
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use crate::utils::format_bytes::format_bytes;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use tracing::warn;

use super::super::common::{
    settings_action_button, settings_card_header, settings_flat_card, settings_sub_row,
};
use super::super::rows::setting_flat_dropdown_row;

pub(super) fn render(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let actions = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            settings_action_button(
                colors,
                i18n.t("LauncherSettings.logs.refresh"),
                !state.log_storage_loading,
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                refresh_log_stats(cx);
            }),
        )
        .child(
            settings_action_button(colors, i18n.t("LauncherSettings.logs.open"), true)
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    open_log_directory(cx);
                }),
        )
        .child(
            settings_action_button(
                colors,
                i18n.t("LauncherSettings.logs.clean"),
                !state.log_cleanup_running,
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                clean_inactive_logs(cx);
            }),
        );

    let card = settings_flat_card(colors, "settings-launcher-log-management")
        .child(
            settings_card_header(
                colors,
                i18n.t("LauncherSettings.logs.title"),
                i18n.t("LauncherSettings.logs.desc"),
            )
            .child(actions),
        )
        .child(storage_summary(colors, i18n, state))
        .child(retention_row(colors, i18n, state))
        .child(advanced_toggle_row(colors, i18n, state));

    card.when(state.log_advanced_open, |card| {
        card.child(active_size_row(colors, i18n, state))
            .child(archive_count_row(colors, i18n, state))
            .child(total_size_row(colors, i18n, state))
            .child(compression_row(colors, i18n, state))
    })
}

fn storage_summary(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let oldest = if state.log_oldest_archive.is_empty() {
        i18n.t("LauncherSettings.logs.none")
    } else {
        state.log_oldest_archive.clone()
    };
    let metrics = [
        (
            i18n.t("LauncherSettings.logs.total_size"),
            SharedString::from(format_bytes(state.log_total_bytes)),
        ),
        (
            i18n.t("LauncherSettings.logs.file_count"),
            SharedString::from(state.log_file_count.to_string()),
        ),
        (
            i18n.t("LauncherSettings.logs.archive_count"),
            SharedString::from(state.log_archive_count.to_string()),
        ),
        (
            i18n.t("LauncherSettings.logs.pending_count"),
            SharedString::from(state.log_pending_count.to_string()),
        ),
        (
            i18n.t("LauncherSettings.logs.active_size"),
            SharedString::from(format_bytes(state.log_active_bytes)),
        ),
        (
            i18n.t("LauncherSettings.logs.previous_size"),
            SharedString::from(format_bytes(state.log_previous_bytes)),
        ),
        (i18n.t("LauncherSettings.logs.oldest"), oldest),
    ];

    div()
        .w_full()
        .px(px(14.))
        .pb(px(12.))
        .flex()
        .flex_wrap()
        .gap(px(8.))
        .children(metrics.into_iter().map(|(label, value)| {
            div()
                .min_w(px(150.))
                .flex_1()
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .border_1()
                .border_color(Hsla {
                    a: 0.16,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.42,
                    ..colors.surface
                })
                .px(px(10.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(colors.text_muted)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(value),
                )
        }))
}

fn retention_row(colors: &ThemeColors, i18n: &I18n, state: &SettingsPageState) -> impl IntoElement {
    let values = vec![1_u32, 3, 7, 14, 30, 90, 180, 365];
    log_setting_dropdown(
        colors,
        i18n,
        "settings-launcher-log-retention",
        (
            "LauncherSettings.logs.retention",
            "LauncherSettings.logs.retention_desc",
        ),
        state.log_retention_days,
        values,
        |state, value| state.log_retention_days = value,
    )
}

fn active_size_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    log_setting_dropdown(
        colors,
        i18n,
        "settings-launcher-log-active-size",
        (
            "LauncherSettings.logs.active_limit",
            "LauncherSettings.logs.active_limit_desc",
        ),
        state.log_active_file_size_mb,
        vec![4, 8, 16, 32, 64, 128, 256, 512],
        |state, value| state.log_active_file_size_mb = value,
    )
}

fn archive_count_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    log_setting_dropdown(
        colors,
        i18n,
        "settings-launcher-log-archive-count",
        (
            "LauncherSettings.logs.archive_limit",
            "LauncherSettings.logs.archive_limit_desc",
        ),
        state.log_max_archive_files,
        vec![8, 16, 32, 64, 128, 256, 512, 1024],
        |state, value| state.log_max_archive_files = value,
    )
}

fn total_size_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    log_setting_dropdown(
        colors,
        i18n,
        "settings-launcher-log-total-size",
        (
            "LauncherSettings.logs.total_limit",
            "LauncherSettings.logs.total_limit_desc",
        ),
        state.log_max_total_size_mb,
        vec![64, 128, 256, 512, 1024, 2048, 4096, 8192],
        |state, value| state.log_max_total_size_mb = value,
    )
}

fn compression_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let values = [1_i32, 3, 5, 7, 9];
    let options = values
        .iter()
        .map(|value| DropdownOption::from(SharedString::from(value.to_string())))
        .collect();
    let selected_index = values
        .iter()
        .position(|value| *value == state.log_compression_level)
        .unwrap_or(1);
    setting_flat_dropdown_row(
        colors,
        i18n.t("Settings.tabs.launcher"),
        i18n.t("LauncherSettings.logs.compression"),
        i18n.t("LauncherSettings.logs.compression_desc"),
        "settings-launcher-log-compression",
        px(180.),
        SharedString::from(state.log_compression_level.to_string()),
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let value = values.get(index).copied().unwrap_or(3);
            cx.update_global(|state: &mut SettingsPageState, _cx| {
                state.log_compression_level = value;
            });
            persist_log_config(cx);
        },
    )
}

fn log_setting_dropdown(
    colors: &ThemeColors,
    i18n: &I18n,
    id: &'static str,
    text_keys: (&'static str, &'static str),
    current: u32,
    values: Vec<u32>,
    update_state: fn(&mut SettingsPageState, u32),
) -> impl IntoElement {
    let options = values
        .iter()
        .map(|value| DropdownOption::from(SharedString::from(value.to_string())))
        .collect();
    let selected_index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    setting_flat_dropdown_row(
        colors,
        i18n.t("Settings.tabs.launcher"),
        i18n.t(text_keys.0),
        i18n.t(text_keys.1),
        id,
        px(180.),
        SharedString::from(current.to_string()),
        options,
        selected_index,
        true,
        move |index, _window, cx| {
            let value = values.get(index).copied().unwrap_or(current);
            cx.update_global(|state: &mut SettingsPageState, _cx| {
                update_state(state, value);
            });
            persist_log_config(cx);
        },
    )
}

fn advanced_toggle_row(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    settings_sub_row(
        colors,
        i18n.t("LauncherSettings.logs.advanced"),
        ToggleSwitch::new(
            "settings-launcher-log-advanced-toggle",
            colors,
            state.log_advanced_open,
            |cx| {
                cx.update_global(|state: &mut SettingsPageState, _cx| {
                    state.log_advanced_open = !state.log_advanced_open;
                });
            },
        ),
    )
}

fn current_config(state: &SettingsPageState) -> LogManagementConfig {
    LogManagementConfig {
        retention_days: state.log_retention_days,
        active_file_size_mb: state.log_active_file_size_mb,
        max_archive_files: state.log_max_archive_files,
        max_total_size_mb: state.log_max_total_size_mb,
        compression_level: state.log_compression_level,
    }
    .normalized()
}

fn persist_log_config(cx: &mut App) {
    let config = cx.read_global(|state: &SettingsPageState, _cx| current_config(state));
    let toast_id = toast::pending(cx, cx.global::<I18n>().t("LauncherSettings.logs.saving"));
    cx.spawn(async move |cx| {
        let config_for_storage = config.clone();
        let result = crate::tasks::runtime::run_io_blocking(move || {
            crate::config::config::update_config(|root| {
                root.launcher.log_management = config_for_storage;
            })
        })
        .await;

        match result {
            Ok(Ok(())) => {
                crate::utils::log_manager::apply_runtime_config(&config);
                toast::resolve_async(
                    cx,
                    toast_id,
                    ToastKind::Success,
                    SharedString::from("日志策略已保存"),
                );
            }
            Ok(Err(error)) => toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("保存日志策略失败：{error}")),
            ),
            Err(error) => toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("保存日志策略任务失败：{error}")),
            ),
        }
    })
    .detach();
}

pub(crate) fn refresh_log_stats(cx: &mut App) {
    let should_refresh = cx.update_global(|state: &mut SettingsPageState, _cx| {
        if state.log_storage_loading {
            false
        } else {
            state.log_storage_loading = true;
            true
        }
    });
    if !should_refresh {
        return;
    }

    cx.spawn(async move |cx| {
        let result =
            crate::tasks::runtime::run_io_blocking(crate::utils::log_manager::inspect_log_storage)
                .await;
        if let Err(error) = cx.update_global(|state: &mut SettingsPageState, _cx| {
            state.log_storage_loading = false;
            match result {
                Ok(Ok(stats)) => apply_stats(state, &stats),
                Ok(Err(error)) => warn!("inspect log storage failed: {error}"),
                Err(error) => warn!("inspect log storage task failed: {error}"),
            }
        }) {
            warn!("update log storage stats failed: {error:?}");
        }
    })
    .detach();
}

fn apply_stats(state: &mut SettingsPageState, stats: &crate::utils::log_manager::LogStorageStats) {
    state.log_file_count = stats.file_count;
    state.log_archive_count = stats.archive_count;
    state.log_pending_count = stats.pending_count;
    state.log_total_bytes = stats.total_bytes;
    state.log_active_bytes = stats.active_bytes;
    state.log_previous_bytes = stats.previous_bytes;
    state.log_oldest_archive = stats.oldest_archive.map_or_else(
        || SharedString::from(""),
        |time| {
            let time: chrono::DateTime<chrono::Local> = time.into();
            SharedString::from(time.format("%Y-%m-%d %H:%M").to_string())
        },
    );
}

fn clean_inactive_logs(cx: &mut App) {
    let should_clean = cx.update_global(|state: &mut SettingsPageState, _cx| {
        if state.log_cleanup_running {
            false
        } else {
            state.log_cleanup_running = true;
            true
        }
    });
    if !should_clean {
        return;
    }
    let toast_id = toast::pending(cx, cx.global::<I18n>().t("LauncherSettings.logs.cleaning"));
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_archive_blocking(|| {
            let report = crate::utils::log_manager::clear_inactive_logs()?;
            let stats = crate::utils::log_manager::inspect_log_storage()?;
            Ok::<_, std::io::Error>((report, stats))
        })
        .await;
        if let Err(error) = cx.update_global(|state: &mut SettingsPageState, _cx| {
            state.log_cleanup_running = false;
            if let Ok(Ok((_, stats))) = &result {
                apply_stats(state, stats);
            }
        }) {
            warn!("update log cleanup state failed: {error:?}");
        }

        match result {
            Ok(Ok((report, _))) => toast::resolve_async(
                cx,
                toast_id,
                if report.failed_files == 0 {
                    ToastKind::Success
                } else {
                    ToastKind::Error
                },
                SharedString::from(format!(
                    "已清理 {} 个日志文件，释放 {}，{} 个文件未能删除",
                    report.removed_files,
                    format_bytes(report.freed_bytes),
                    report.failed_files
                )),
            ),
            Ok(Err(error)) => toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("清理日志失败：{error}")),
            ),
            Err(error) => toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("清理日志任务失败：{error}")),
            ),
        }
    })
    .detach();
}

fn open_log_directory(cx: &mut App) {
    let path = crate::utils::file_ops::logs_dir()
        .to_string_lossy()
        .into_owned();
    cx.spawn(async move |_cx| {
        if let Err(error) = crate::utils::open_path::open_path(path).await {
            warn!("open log directory failed: {error}");
        }
    })
    .detach();
}
