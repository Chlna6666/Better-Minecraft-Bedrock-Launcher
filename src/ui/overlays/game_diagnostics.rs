//! Minecraft game diagnostics UI. Launcher/Sentry crashes have a separate overlay.
use crate::core::minecraft::diagnostics::{self, CrashExportOptions, ExitKind, GameSessionReport};
use crate::ui::state::game_diagnostics::GameDiagnosticsState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::components::scroll::ScrollableElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::AnimationExt as _;
use gpui::*;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy)]
enum ExportField {
    Launcher,
    Game,
    Extensions,
    Windows,
    Dumps,
    Redact,
}

impl ExportField {
    fn checked(self, options: CrashExportOptions) -> bool {
        match self {
            Self::Launcher => options.include_launcher,
            Self::Game => options.include_game,
            Self::Extensions => options.include_extensions,
            Self::Windows => options.include_windows,
            Self::Dumps => options.include_dumps,
            Self::Redact => options.redact_text,
        }
    }

    fn toggle(self, options: &mut CrashExportOptions) {
        match self {
            Self::Launcher => options.include_launcher = !options.include_launcher,
            Self::Game => options.include_game = !options.include_game,
            Self::Extensions => options.include_extensions = !options.include_extensions,
            Self::Windows => options.include_windows = !options.include_windows,
            Self::Dumps => options.include_dumps = !options.include_dumps,
            Self::Redact => options.redact_text = !options.redact_text,
        }
    }
}

fn option_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    field: ExportField,
    selected: bool,
    width: Pixels,
    enabled: bool,
    colors: &ThemeColors,
) -> impl IntoElement {
    div()
        .id(id)
        .w(width)
        .min_w(px(0.))
        .flex()
        .items_start()
        .gap(px(9.))
        .p(px(10.))
        .rounded(px(9.))
        .border_1()
        .border_color(if selected { colors.accent } else { colors.border })
        .bg(if selected { colors.surface_hover } else { colors.settings_field_bg })
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .child(
            div()
                .flex_none()
                .w(px(19.))
                .h(px(19.))
                .rounded(px(5.))
                .border_1()
                .border_color(if selected { colors.accent } else { colors.border })
                .bg(if selected { colors.accent } else { colors.surface })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(colors.btn_primary_text)
                .child(if selected { "✓" } else { "" }),
        )
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div().text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .whitespace_normal()
                        .child(title),
                )
                .child(
                    div().text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(description),
                ),
        )
        .on_click(move |_, _, cx| {
            cx.update_global(|state: &mut GameDiagnosticsState, _| {
                if !enabled || state.exporting {
                    return;
                }
                field.toggle(&mut state.options);
                state.awaiting_sensitive_confirmation = false;
                state.export_status = None;
                state.export_location = None;
            });
        })
}

fn action(
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .rounded(px(8.))
        .border_1()
        .border_color(if primary { colors.accent } else { colors.border })
        .bg(if primary { colors.accent } else { colors.surface })
        .px(px(13.))
        .py(px(9.))
        .min_w(px(0.))
        .cursor_pointer()
        .hover(|this| this.bg(if primary { colors.accent_hover } else { colors.surface_hover }))
        .text_size(px(12.))
        .text_color(if !enabled {
            colors.text_muted
        } else if primary {
            colors.btn_primary_text
        } else {
            colors.text_primary
        })
        .child(label)
}

/// File picker and compression both run off the GPUI main thread.
/// Export is tracked by report identity and a monotonically increasing ticket.
fn start_export(cx: &mut App, id: String, options: CrashExportOptions) {
    let ticket = cx.update_global(|state: &mut GameDiagnosticsState, _| {
        if state.exporting || !options.includes_any()
            || state.pending.as_ref().is_none_or(|r| r.id != id)
        {
            return None;
        }
        state.exporting = true;
        state.export_generation = state.export_generation.wrapping_add(1);
        state.awaiting_sensitive_confirmation = false;
        state.export_status = Some("选择 ZIP 保存位置…".to_string());
        state.export_location = None;
        Some(state.export_generation)
    });
    let Some(ticket) = ticket else { return };

    cx.spawn(async move |cx| {
        let identity = id.clone();
        let filename = format!("BMCBL-Game-Crash-{id}.zip");
        let outcome = crate::tasks::runtime::run_io_blocking(move || {
            let chosen = crate::utils::file_picker::pick_save_path_with_filter(
                "ZIP 压缩包", &["zip"], &filename,
            );
            let Some(chosen) = chosen else {
                return Ok::<Option<String>, String>(None);
            };
            let path = PathBuf::from(chosen);
            diagnostics::export_zip_with_options(&identity, &path, options)?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }).await;

        let _ = cx.update_global(|state: &mut GameDiagnosticsState, _| {
            if state.export_generation != ticket {
                return;
            }
            state.exporting = false;
            let selected = state.pending.as_ref().is_some_and(|r| r.id == id);
            if !selected {
                return;
            }
            match outcome {
                Ok(Ok(Some(path))) => {
                    state.export_status = Some("导出成功，报告已保存到所选位置".to_string());
                    state.export_location = Some(path);
                }
                Ok(Ok(None)) => {
                    state.export_status = Some("已取消导出".to_string());
                }
                Ok(Err(error)) | Err(error) => {
                    state.export_status = Some(format!("导出失败：{error}"));
                }
            }
        });
    }).detach();
}

fn request_export(cx: &mut App, id: String) {
    let selection = cx.update_global(|state: &mut GameDiagnosticsState, _| {
        if state.exporting || state.pending.as_ref().is_none_or(|r| r.id != id) {
            return None;
        }
        if !state.options.includes_any() {
            state.export_status = Some("请选择至少一类诊断材料".to_string());
            return None;
        }
        if state.options.needs_confirmation() {
            state.awaiting_sensitive_confirmation = true;
            state.export_status = None;
            return None;
        }
        Some(state.options)
    });
    if let Some(options) = selection {
        start_export(cx, id, options);
    }
}

fn details(report: &GameSessionReport) -> String {
    let compatibility = report.compatibility_prefix.is_some();
    let pid_label = if compatibility { "Linux 兼容运行器 PID" } else { "Minecraft 原生 PID" };
    let exit_label = if compatibility { "兼容运行器退出码" } else { "游戏退出码" };
    let mut result = format!(
        "会话 ID：{}\n版本：{}\n平台：{}\n{}：{}\n开始：{}\n结束：{}\n{}：{}\n{}",
        report.id, report.version, report.platform, pid_label, report.pid,
        report.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        report.ended_at.map_or_else(
            || "未知".to_string(),
            |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        ),
        exit_label,
        report.exit_code.map_or_else(
            || "未知".to_string(), |code| format!("0x{code:08X}")
        ),
        report.summary,
    );
    if compatibility {
        result.push_str(
            "\n注意：兼容运行器 PID/退出码不等于 Wine 中的 Minecraft Windows 进程 PID/退出码。"
        );
    }
    if report.early_exit {
        result.push_str("\n在启动检测阶段提前退出");
    }
    if let Some(signal) = report.exit_signal {
        result.push_str(&format!("\nLinux 终止信号：{signal}"));
    }
    if let Some(fault) = report.compatibility_fault {
        result.push_str(&format!("\n兼容层证据：{fault:?}（未自动确认客户端崩溃）"));
    }
    if let Some(wer) = &report.wer_fault {
        result.push_str(&format!(
            "\nWindows WER：故障模块 {}，异常码 {}",
            wer.module, wer.exception_code
        ));
    }
    result
}

#[derive(Clone, Copy)]
struct DialogGeometry {
    card_width: Pixels,
    card_height: Pixels,
    compact: bool,
    inset: f32,
    available_width: Pixels,
    option_width: Pixels,
}

fn dialog_geometry(window_width: Pixels, window_height: Pixels) -> DialogGeometry {
    let card_width = (window_width - px(24.)).max(px(0.)).min(px(760.));
    let card_height = (window_height - px(24.)).max(px(0.)).min(px(690.));
    let compact = window_width < px(560.) || window_height < px(460.);
    let inset = if compact { 12. } else { 20. };
    let available_width = (card_width - px(inset * 2. + 8.)).max(px(0.));
    let option_width = if compact {
        available_width
    } else {
        (available_width - px(10.)).max(px(0.)) * 0.5
    };
    DialogGeometry {
        card_width, card_height, compact, inset, available_width, option_width,
    }
}

fn entrance_animation() -> Animation {
    Animation::from_spec(
        AnimationSpec::new(Duration::from_millis(190))
            .fill_mode(FillMode::Both)
            .ease(Easing::OutCubic),
    )
    .with_property(AnimationProperty::clipped_translation_opacity(
        point(px(0.), px(12.)),
        point(px(0.), px(0.)),
        0.92,
        1.0,
    ))
}

pub fn render_game_diagnostics_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    state: &GameDiagnosticsState,
) -> Option<AnyElement> {
    let report = state.pending.clone()?;
    // Constrain all layout dimensions to the current viewport, including tiny
    // floating windows and mid-animation resizes.
    let DialogGeometry {
        card_width, card_height, compact, inset, available_width, option_width,
    } = dialog_geometry(window_width, window_height);
    let selection = state.options;
    let exporting = state.exporting;
    let export_status = state.export_status.clone();
    let export_location = state.export_location.clone();
    let history = state.history.clone();
    let confirmation = state.awaiting_sensitive_confirmation;
    let copy_text = crate::utils::log_privacy::redact_log_text(&details(&report)).into_owned();
    let copy_path = export_location.clone();
    let id_for_export = report.id.clone();
    let id_for_confirm = report.id.clone();
    let id_for_acknowledge = report.id.clone();
    let title = match report.kind {
        ExitKind::RunnerFailure => "兼容运行器异常退出",
        ExitKind::Unknown => "游戏会话状态未知",
        ExitKind::ConfirmedCrash => "Minecraft 崩溃已确认",
        _ => "Minecraft 意外退出",
    };

    let export_warning = if !selection.redact_text && selection.include_dumps {
        "将导出未脱敏文本及原始 DMP。它们可能包含用户目录、账号令牌、世界数据或其他私密信息。"
    } else if !selection.redact_text {
        "将导出未脱敏文本，可能包含账号令牌、文件路径及其他敏感信息。"
    } else {
        "DMP 无法通过文本脱敏保护，可能包含账号凭证、聊天及世界数据。"
    };

    let body = div()
        .w(card_width)
        .h(card_height)
        .min_w(px(0.))
        .min_h(px(0.))
        .rounded(px(14.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.settings_panel_bg)
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex_none()
                .px(px(inset))
                .py(px(if compact { 12. } else { 16. }))
                .border_b_1()
                .border_color(colors.border)
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div().min_w(px(0.))
                        .text_size(px(if compact { 17. } else { 20. }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .whitespace_normal()
                        .child(title),
                )
                .child(
                    div().min_w(px(0.))
                        .text_size(px(11.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(format!(
                            "{} · {} · PID {}",
                            report.version, report.platform, report.pid,
                        )),
                ),
        )
        // The *whole* body including confirmation and actions is scrollable,
        // so even a very short window cannot clip the export confirmation.
        .child(
            div()
                .id("game-diagnostics-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .p(px(inset))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .min_w(px(0.))
                                .rounded(px(10.))
                                .bg(colors.surface)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .child(
                                    div().min_w(px(0.))
                                        .text_size(px(13.))
                                        .line_height(px(19.))
                                        .whitespace_normal()
                                        .text_color(colors.text_primary)
                                        .child(report.summary.clone()),
                                )
                                .child(
                                    div().text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .whitespace_normal()
                                        .child("此诊断仅在本地保存，退出码和故障模块不能单独证明根因。"),
                                ),
                        )
                        .when(!history.is_empty(), |panel| panel.child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("历史报告"),
                                )
                                .child(
                                    div().flex().flex_wrap().gap(px(6.))
                                        .children(history.into_iter().take(12).enumerate().map(
                                            |(index, previous)| {
                                                let text = format!(
                                                    "{} · {}",
                                                    previous.version,
                                                    previous.started_at.format("%m-%d %H:%M"),
                                                );
                                                div()
                                                    .id(("game-diagnostics-history", index))
                                                    .rounded(px(6.))
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .bg(colors.surface)
                                                    .px(px(9.))
                                                    .py(px(7.))
                                                    .cursor_pointer()
                                                    .hover(|this| this.bg(colors.surface_hover))
                                                    .text_size(px(11.))
                                                    .text_color(colors.text_secondary)
                                                    .child(text)
                                                    .on_click(move |_, _, cx| {
                                                        cx.update_global(
                                                            |state: &mut GameDiagnosticsState, _| {
                                                                state.select_report(previous.clone());
                                                            },
                                                        );
                                                    })
                                            },
                                        )),
                                ),
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("诊断详情"),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(colors.border)
                                        .bg(colors.settings_field_bg)
                                        .p(px(12.))
                                        .overflow_hidden()
                                        .text_size(px(11.))
                                        .line_height(px(18.))
                                        .whitespace_normal()
                                        .font_family("Consolas")
                                        .text_color(colors.text_primary)
                                        .child(details(&report)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("导出内容"),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .whitespace_normal()
                                        .child("选择需要打包的证据类型。仅导出当前会话的快照；不存在的文件会自动跳过。"),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .flex().flex_wrap().gap(px(10.))
                                        .child(option_row(
                                            "game-export-launcher", "启动任务日志",
                                            "当前启动任务的会话日志",
                                            ExportField::Launcher, selection.include_launcher,
                                            option_width, !exporting, colors,
                                        ))
                                        .child(option_row(
                                            "game-export-game", "Minecraft 日志",
                                            "游戏会话对应的日志快照",
                                            ExportField::Game, selection.include_game,
                                            option_width, !exporting, colors,
                                        ))
                                        .child(option_row(
                                            "game-export-extensions", "模组与运行器日志",
                                            "BLoader、LeviLamina、Proton 等",
                                            ExportField::Extensions, selection.include_extensions,
                                            option_width, !exporting, colors,
                                        ))
                                        .when(cfg!(target_os = "windows"), |panel| panel.child(option_row(
                                            "game-export-windows", "Windows 系统故障事件",
                                            "按进程身份关联的 Windows WER 证据",
                                            ExportField::Windows, selection.include_windows,
                                            option_width, !exporting, colors,
                                        )))
                                        .child(option_row(
                                            "game-export-dumps", "内存转储 DMP",
                                            "仅主动选择时导出，无法自动脱敏",
                                            ExportField::Dumps, selection.include_dumps,
                                            option_width, !exporting, colors,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("隐私保护"),
                                )
                                .child(option_row(
                                    "game-export-redact",
                                    "文本脱敏（默认开启）",
                                    "导出时过滤敏感字段；对原始 DMP 不生效",
                                    ExportField::Redact,
                                    selection.redact_text,
                                    available_width,
                                    !exporting,
                                    colors,
                                ))
                                .when(!selection.redact_text, |panel| panel.child(
                                    div().text_size(px(11.))
                                        .text_color(colors.danger)
                                        .whitespace_normal()
                                        .child("已关闭文本脱敏。导出前将再次要求确认。"),
                                )),
                        )
                        .when(confirmation, |panel| panel.child(
                            div()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(colors.danger)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(9.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.danger)
                                        .child("确认敏感材料导出"),
                                )
                                .child(
                                    div().text_size(px(12.))
                                        .line_height(px(18.))
                                        .whitespace_normal()
                                        .text_color(colors.text_primary)
                                        .child(export_warning),
                                )
                                .child(
                                    div().flex().flex_wrap().gap(px(8.))
                                        .child(
                                            action("game-export-confirm", "确认并选择保存位置", true, !exporting, colors)
                                                .on_click(move |_, _, cx| {
                                                    let options = cx.global::<GameDiagnosticsState>().options;
                                                    if options.needs_confirmation() {
                                                        start_export(cx, id_for_confirm.clone(), options);
                                                    }
                                                }),
                                        )
                                        .child(
                                            action("game-export-back", "返回修改", false, true, colors)
                                                .on_click(|_, _, cx| {
                                                    cx.update_global(
                                                        |state: &mut GameDiagnosticsState, _| {
                                                            state.awaiting_sensitive_confirmation = false;
                                                        },
                                                    );
                                                }),
                                        ),
                                ),
                        ))
                        .when_some(export_status, |panel, status| panel.child(
                            div()
                                .min_w(px(0.))
                                .rounded(px(8.))
                                .bg(colors.surface)
                                .p(px(10.))
                                .text_size(px(12.))
                                .whitespace_normal()
                                .text_color(colors.text_primary)
                                .child(status),
                        ))
                        .when_some(export_location, |panel, path| panel.child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div().min_w(px(0.)).flex_1()
                                        .text_size(px(11.))
                                        .truncate()
                                        .text_color(colors.text_secondary)
                                        .child(path.clone()),
                                )
                                .child(
                                    action("game-export-copy-path", "复制保存路径", false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                                        }),
                                ),
                        ))
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_wrap()
                                .justify_end()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    action("game-diagnostics-copy", "复制诊断详情", false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(
                                                ClipboardItem::new_string(copy_text.clone()),
                                            );
                                        }),
                                )
                                .child(
                                    action("game-diagnostics-export", "选择位置并导出 ZIP",
                                        true, !exporting && selection.includes_any(), colors,
                                    ).on_click(move |_, _, cx| {
                                        request_export(cx, id_for_export.clone());
                                    }),
                                )
                                .child(
                                    action("game-diagnostics-dismiss", "关闭", false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            let id = id_for_acknowledge.clone();
                                            cx.update_global(|state: &mut GameDiagnosticsState, _| {
                                                state.pending = None;
                                                state.awaiting_sensitive_confirmation = false;
                                                state.export_status = None;
                                                state.export_location = None;
                                            });
                                            let _ = crate::tasks::runtime::spawn_io(async move {
                                                let _ = crate::tasks::runtime::run_io_blocking(
                                                    move || diagnostics::acknowledge(&id),
                                                ).await;
                                            });
                                        }),
                                ),
                        ),
                ),
        );

    let animated: AnyElement = if crate::core::ui_prefs::reduced_motion() {
        body.into_any_element()
    } else {
        body.with_visual_animation(
            SharedString::from(format!("game-crash-entrance:{}", report.id)),
            entrance_animation(),
        )
        .expect("game diagnostics uses one retained opacity/translation animation")
        .into_any_element()
    };

    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.50))
            .child(animated)
            .into_any_element(),
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_diagnostics_layout_never_exceeds_viewport() {
        for (width, height) in [
            (0.0, 0.0),
            (180.0, 160.0),
            (320.0, 240.0),
            (450.0, 350.0),
            (760.0, 500.0),
            (972.0, 600.0),
            (2560.0, 1440.0),
        ] {
            let metrics = dialog_geometry(px(width), px(height));
            assert!(metrics.card_width >= px(0.));
            assert!(metrics.card_height >= px(0.));
            assert!(metrics.card_width <= px(width));
            assert!(metrics.card_height <= px(height));
            assert!(metrics.option_width >= px(0.));
            assert!(metrics.option_width <= metrics.available_width);
            assert!(metrics.available_width <= metrics.card_width);
        }
    }
}
