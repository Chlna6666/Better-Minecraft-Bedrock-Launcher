//! Game-process diagnostics overlay. Launcher process/Sentry failures use diagnostics.rs.
use crate::core::minecraft::diagnostics::{self, GameSessionReport};
use crate::ui::state::game_diagnostics::GameDiagnosticsState;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

fn action(text: &'static str, colors: &ThemeColors) -> Div {
    div()
        .rounded(px(8.))
        .border_1()
        .border_color(colors.border)
        .px(px(12.))
        .py(px(9.))
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(colors.text_primary)
        .child(text)
}

fn details(report: &GameSessionReport) -> String {
    format!(
        "会话 ID：{}\n游戏版本：{}\n平台：{}\nPID：{}\n启动：{}\n结束：{}\n退出码：{}\n原因：{}",
        report.id,
        report.version,
        report.platform,
        report.pid,
        report.started_at,
        report.ended_at.map_or_else(|| "未知".to_string(), |t| t.to_string()),
        report.exit_code.map_or_else(|| "未知".to_string(), |code| format!("0x{code:08X}")),
        report.summary
    )
}

pub fn render_game_diagnostics_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    state: &GameDiagnosticsState,
) -> Option<AnyElement> {
    let report = state.pending.clone()?;
    let export_state = state.export_status.clone();
    let card_width = (window_width - px(32.)).min(px(720.)).max(px(300.));
    let card_height = (window_height - px(48.)).min(px(610.)).max(px(320.));
    let for_copy = details(&report);
    let for_acknowledge = report.id.clone();
    let for_export = report.id.clone();

    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.48))
            .child(
                div()
                    .w(card_width)
                    .max_h(card_height)
                    .rounded(px(14.))
                    .bg(colors.settings_panel_bg)
                    .border_1()
                    .border_color(colors.border)
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div().text_size(px(21.)).font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary).child("Minecraft 意外退出")
                    )
                    .child(div().text_size(px(13.)).text_color(colors.text_secondary)
                        .child("诊断记录仅保存在本机，不会自动上传到 Sentry 或 GitHub。"))
                    .child(
                        div().rounded(px(8.)).bg(colors.surface)
                            .p(px(12.)).flex().flex_col().gap(px(8.))
                            .child(div().text_size(px(14.)).text_color(colors.text_primary)
                                .child(report.summary.clone()))
                            .child(
                                div().text_size(px(12.)).text_color(colors.text_secondary)
                                    .child(format!("版本：{}   进程 PID：{}", report.version, report.pid))
                            )
                    )
                    .child(
                        div().flex_1().min_h(px(0.)).overflow_y_scrollbar()
                            .text_size(px(12.)).font_family("Consolas")
                            .text_color(colors.text_primary)
                            .child(details(&report))
                    )
                    .when_some(export_state, |div, status| div.child(
                        div().text_size(px(12.)).text_color(colors.text_secondary).child(status)
                    ))
                    .child(
                        div().flex().flex_wrap().gap(px(8.)).justify_end()
                            .child(
                                action("复制诊断", colors).id("game-diagnostics-copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(for_copy.clone()));
                                    })
                            )
                            .child(
                                action("导出 ZIP", colors).id("game-diagnostics-export")
                                    .on_click(move |_, _, cx| {
                                        let id = for_export.clone();
                                        let output = crate::utils::file_ops::state_subdir("diagnostics/game/exports")
                                            .join(format!("BMCBL-Game-Crash-{id}.zip"));
                                        cx.update_global(|state: &mut GameDiagnosticsState, _| {
                                            state.export_status = Some("正在导出诊断材料…".to_string());
                                        });
                                        cx.spawn(async move |cx| {
                                            let destination = output.clone();
                                            let result = crate::tasks::runtime::run_io_blocking(move || {
                                                diagnostics::export_zip(&id, &destination, false)
                                            }).await;
                                            let message = match result {
                                                Ok(Ok(())) => format!("报告已导出：{}", output.display()),
                                                Ok(Err(error)) => format!("导出失败：{error}"),
                                                Err(error) => format!("导出任务失败：{error}"),
                                            };
                                            let _ = cx.update_global(|state: &mut GameDiagnosticsState, _| {
                                                state.export_status = Some(message);
                                            });
                                        }).detach();
                                    })
                            )
                            .child(
                                action("关闭", colors).id("game-diagnostics-dismiss")
                                    .on_click(move |_, _, cx| {
                                        let id = for_acknowledge.clone();
                                        cx.update_global(|state: &mut GameDiagnosticsState, _| {
                                            state.pending = None;
                                            state.export_status = None;
                                        });
                                        let _ = crate::tasks::runtime::spawn_io(async move {
                                            let _ = crate::tasks::runtime::run_io_blocking(move || {
                                                diagnostics::acknowledge(&id)
                                            }).await;
                                        });
                                    })
                            )
                    )
            )
            .into_any_element()
    )
}
