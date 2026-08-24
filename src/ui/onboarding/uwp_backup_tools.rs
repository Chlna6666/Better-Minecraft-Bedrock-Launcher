#![cfg(target_os = "windows")]

use gpui::prelude::FluentBuilder as _;
use gpui::{AppContext as _, BorrowAppContext as _, *};
use lucide_gpui::icons as lucide_icons;
use std::path::{Path, PathBuf};

use crate::core::minecraft::uwp_backup::{
    ManualUwpBackupResult, export_user_data_backup, migration_backup_root, user_data_path,
};
use crate::core::minecraft::uwp_migration::MinecraftDataSummary;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

#[derive(Default)]
pub struct UwpBackupToolsState {
    family_name: Option<SharedString>,
    request_id: u64,
    scanning: bool,
    summary: Option<MinecraftDataSummary>,
    error: Option<SharedString>,
    exporting: bool,
    export_status: Option<SharedString>,
}

impl Global for UwpBackupToolsState {}

impl UwpBackupToolsState {
    fn reset(&mut self) {
        self.family_name = None;
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.scanning = false;
        self.summary = None;
        self.error = None;
        self.exporting = false;
        self.export_status = None;
    }

    fn begin_scan(&mut self, family_name: SharedString) -> Option<(u64, String)> {
        if self.family_name.as_ref() == Some(&family_name)
            && (self.scanning || self.summary.is_some() || self.error.is_some())
        {
            return None;
        }

        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.family_name = Some(family_name.clone());
        self.scanning = true;
        self.summary = None;
        self.error = None;
        self.exporting = false;
        self.export_status = None;
        Some((self.request_id, family_name.to_string()))
    }

    fn apply_scan(&mut self, request_id: u64, summary: MinecraftDataSummary) {
        if self.request_id != request_id {
            return;
        }
        self.scanning = false;
        self.summary = Some(summary);
        self.error = None;
    }

    fn fail_scan(&mut self, request_id: u64, error: impl Into<SharedString>) {
        if self.request_id != request_id {
            return;
        }
        self.scanning = false;
        self.summary = None;
        self.error = Some(error.into());
    }
}

pub fn sync_from_safety(cx: &mut App) {
    cx.default_global::<UwpBackupToolsState>();
    let family = cx
        .try_global::<super::uwp_safety::UwpSafetyGuideState>()
        .and_then(|state| {
            if state.visible && !state.checking {
                state
                    .system_registration
                    .as_ref()
                    .map(|registration| registration.family_name.clone())
            } else {
                None
            }
        });

    let Some(family) = family else {
        if cx.global::<UwpBackupToolsState>().family_name.is_some() {
            cx.update_global(|state: &mut UwpBackupToolsState, _cx| state.reset());
        }
        return;
    };

    let request = cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
        state.begin_scan(SharedString::from(family))
    });
    let Some((request_id, family_name)) = request else {
        return;
    };

    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            crate::core::minecraft::uwp_migration::summarize_family(&family_name)
        })
        .await;
        cx.update(|cx| match result {
            Ok(summary) => {
                cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
                    state.apply_scan(request_id, summary);
                });
            }
            Err(error) => {
                cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
                    state.fail_scan(request_id, format!("读取 UWP 用户数据失败：{error}"));
                });
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    const GIB: f64 = 1024.0 * MIB;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.2} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn open_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("目录不存在：{}", path.display()));
    }
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map_err(|error| format!("打开目录失败：{error}"))?;
    Ok(())
}

fn start_export(window: &mut Window, cx: &mut App) {
    let (family_name, has_data) = {
        let state = cx.global::<UwpBackupToolsState>();
        (
            state.family_name.as_ref().map(|value| value.to_string()),
            state
                .summary
                .as_ref()
                .is_some_and(|summary| summary.data_present && summary.file_count > 0),
        )
    };
    let Some(family_name) = family_name else {
        return;
    };
    if !has_data {
        cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
            state.export_status = Some(SharedString::from("没有检测到可导出的 UWP 用户数据"));
        });
        return;
    }

    let Some(destination) = crate::utils::file_picker::pick_directory_path_for_window(window) else {
        return;
    };
    let destination = PathBuf::from(destination);
    cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
        state.exporting = true;
        state.export_status = Some(SharedString::from("正在生成 ZIP 备份，请不要关闭 BMCBL…"));
    });

    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            export_user_data_backup(&family_name, &destination)
        })
        .await;
        cx.update(|cx| {
            cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
                state.exporting = false;
                state.export_status = Some(SharedString::from(match result {
                    Ok(Ok(ManualUwpBackupResult { archive_path, summary })) => format!(
                        "备份完成：{} · {} 个文件 · {}",
                        archive_path.display(),
                        summary.file_count,
                        format_bytes(summary.total_size)
                    ),
                    Ok(Err(error)) => format!("备份失败：{error}"),
                    Err(error) => format!("备份任务失败：{error}"),
                }));
            });
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn open_user_data(cx: &mut App) {
    let path = cx
        .global::<UwpBackupToolsState>()
        .summary
        .as_ref()
        .map(user_data_path);
    let message = match path {
        Some(path) => open_directory(&path)
            .err()
            .unwrap_or_else(|| format!("已打开：{}", path.display())),
        None => "尚未完成 UWP 数据扫描".to_string(),
    };
    cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
        state.export_status = Some(SharedString::from(message));
    });
}

fn open_migration_backups(cx: &mut App) {
    let path = migration_backup_root();
    let result = std::fs::create_dir_all(&path)
        .map_err(|error| format!("创建迁移备份目录失败：{error}"))
        .and_then(|_| open_directory(&path));
    cx.update_global(|state: &mut UwpBackupToolsState, _cx| {
        state.export_status = Some(SharedString::from(
            result.err().unwrap_or_else(|| format!("已打开：{}", path.display())),
        ));
    });
}

fn action_button(
    id: &'static str,
    label: &'static str,
    icon: SharedString,
    colors: ThemeColors,
    enabled: bool,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .h(px(34.0))
        .px(px(10.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if enabled {
            colors.text_primary
        } else {
            colors.text_muted
        })
        .child(svg().path(icon).size(px(14.0)).text_color(colors.accent))
        .child(label);

    if enabled {
        button
            .cursor_pointer()
            .hover(move |this| this.bg(colors.surface_hover))
            .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
            .on_mouse_down(MouseButton::Left, handler)
            .into_any_element()
    } else {
        button.into_any_element()
    }
}

fn compact_action_button(
    id: &'static str,
    icon: SharedString,
    colors: ThemeColors,
    enabled: bool,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .size(px(34.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(icon)
                .size(px(15.0))
                .text_color(if enabled { colors.accent } else { colors.text_muted }),
        );
    if enabled {
        button
            .cursor_pointer()
            .hover(move |this| this.bg(colors.surface_hover))
            .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
            .on_mouse_down(MouseButton::Left, handler)
            .into_any_element()
    } else {
        button.into_any_element()
    }
}

pub fn render_uwp_backup_tools(
    state: &UwpBackupToolsState,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    );
    let bounds = window.bounds().size;
    let width = bounds.width / px(1.0);
    let height = bounds.height / px(1.0);
    let safety_w = (width - 32.0).max(280.0).min(560.0);
    let safety_h = (height - 32.0).max(220.0).min(500.0);
    let safety_left = ((width - safety_w) / 2.0).max(16.0);
    let safety_top = ((height - safety_h) / 2.0).max(16.0);
    let right_space = width - (safety_left + safety_w) - 16.0;
    let attached = right_space >= 210.0;

    let data_text = if state.scanning {
        "正在统计世界、资源包和数据大小…".to_string()
    } else if let Some(summary) = state.summary.as_ref() {
        if summary.data_present && summary.file_count > 0 {
            format!(
                "{} 个文件 · {}\n世界 {} · 资源包 {} · 行为包 {}\n皮肤包 {} · 截图 {}",
                summary.file_count,
                format_bytes(summary.total_size),
                summary.worlds,
                summary.resource_packs,
                summary.behavior_packs,
                summary.skin_packs,
                summary.screenshots
            )
        } else {
            "未检测到 games/com.mojang 中的可备份文件。".to_string()
        }
    } else if let Some(error) = state.error.as_ref() {
        error.to_string()
    } else {
        "等待 UWP 数据扫描…".to_string()
    };
    let can_export = !state.scanning
        && !state.exporting
        && state
            .summary
            .as_ref()
            .is_some_and(|summary| summary.data_present && summary.file_count > 0);
    let can_open_data = state
        .summary
        .as_ref()
        .is_some_and(|summary| user_data_path(summary).is_dir());

    if !attached {
        let compact_label = state
            .summary
            .as_ref()
            .filter(|summary| summary.data_present && summary.file_count > 0)
            .map(|summary| format!("UWP 数据 · {}", format_bytes(summary.total_size)))
            .unwrap_or_else(|| {
                if state.scanning {
                    "正在检查 UWP 数据".to_string()
                } else {
                    "UWP 数据工具".to_string()
                }
            });

        return div()
            .absolute()
            .right(px(16.0))
            .top(px(16.0))
            .max_w(px((width - 32.0).max(220.0)))
            .h(px(46.0))
            .px(px(7.0))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .border_1()
            .border_color(Hsla { a: 0.32, ..colors.border })
            .bg(Hsla { a: 0.985, ..colors.bg })
            .shadow(vec![BoxShadow {
                color: Hsla { a: 0.16, ..black() },
                blur_radius: px(20.0),
                spread_radius: px(-5.0),
                offset: point(px(0.0), px(8.0)),
            }])
            .occlude()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_secondary)
                    .truncate()
                    .child(compact_label),
            )
            .child(compact_action_button(
                "uwp-manual-backup-export-compact",
                lucide_icons::icon_download(),
                colors,
                can_export,
                |_event, window, cx| start_export(window, cx),
            ))
            .child(compact_action_button(
                "uwp-manual-backup-open-data-compact",
                lucide_icons::icon_folder_open(),
                colors,
                can_open_data,
                |_event, _window, cx| open_user_data(cx),
            ))
            .child(compact_action_button(
                "uwp-manual-backup-open-migrations-compact",
                lucide_icons::icon_folder_open(),
                colors,
                true,
                |_event, _window, cx| open_migration_backups(cx),
            ));
    }

    let panel_w = right_space.clamp(210.0, 272.0);
    let actions = div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .child(action_button(
            "uwp-manual-backup-export",
            if state.exporting { "正在导出" } else { "导出备份" },
            lucide_icons::icon_download(),
            colors,
            can_export,
            |_event, window, cx| start_export(window, cx),
        ))
        .child(action_button(
            "uwp-manual-backup-open-data",
            "数据目录",
            lucide_icons::icon_folder_open(),
            colors,
            can_open_data,
            |_event, _window, cx| open_user_data(cx),
        ))
        .child(action_button(
            "uwp-manual-backup-open-migrations",
            "迁移备份",
            lucide_icons::icon_folder_open(),
            colors,
            true,
            |_event, _window, cx| open_migration_backups(cx),
        ));

    div()
        .absolute()
        .left(px(safety_left + safety_w - 1.0))
        .top(px(safety_top))
        .w(px(panel_w))
        .max_h(px(safety_h))
        .overflow_y_scrollbar()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla { a: 0.32, ..colors.border })
        .bg(Hsla { a: 0.985, ..colors.bg })
        .shadow(vec![BoxShadow {
            color: Hsla { a: 0.14, ..black() },
            blur_radius: px(20.0),
            spread_radius: px(-6.0),
            offset: point(px(6.0), px(8.0)),
        }])
        .occlude()
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .size(px(30.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla { a: 0.12, ..colors.accent })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(lucide_icons::icon_shield_check())
                                .size(px(15.0))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("数据安全工具"),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .line_height(px(14.0))
                                .text_color(colors.text_muted)
                                .child("额外手动保险，不替代自动迁移校验"),
                        ),
                ),
        )
        .child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(9.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.07, ..colors.accent })
                .text_size(px(10.0))
                .line_height(px(15.0))
                .whitespace_normal()
                .text_color(colors.text_secondary)
                .child(data_text),
        )
        .child(actions)
        .when_some(state.export_status.clone(), |this, status| {
            this.child(
                div()
                    .w_full()
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .whitespace_normal()
                    .text_color(colors.text_secondary)
                    .child(status),
            )
        })
