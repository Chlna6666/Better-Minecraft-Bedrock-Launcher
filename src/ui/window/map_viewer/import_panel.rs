//! Shared chrome and task feedback for the three import editors.

use super::generator_panel::generator_choice;
use super::model::MapViewerWindowView;
use super::panels::dock_close_button;
use super::prelude::*;

impl MapViewerWindowView {
    /// Keep the close control visible while the editor's parameters scroll beneath it.
    pub(super) fn render_import_panel_header(
        &self,
        colors: &ThemeColors,
        title: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .h(px(44.0))
            .flex_none()
            .px(px(12.0))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(title),
            )
            .child(dock_close_button(colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.close_right_panel(cx)),
            ))
    }

    /// Show one owned task snapshot and cancel that same task from its feedback row.
    pub(super) fn render_import_task_status(
        &self,
        colors: &ThemeColors,
        task: &TaskSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let task_id = task.id.to_string();
        let stage = match task.stage.as_ref() {
            "map_paste" => "粘贴结构",
            "map_refresh" => "刷新地图",
            "map_write" => "写入地图",
            "map_delete" => "删除区块",
            stage => stage,
        };
        let status = match task.status.as_ref() {
            "completed" => "已完成",
            "cancelled" => "已取消",
            "error" => "失败",
            _ if task.cancel_requested => "正在取消",
            _ => "进行中",
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .child(format!(
                        "{} · {} / {} · {}",
                        stage,
                        task.done,
                        task.total
                            .map_or_else(|| "?".to_owned(), |total| total.to_string()),
                        status,
                    )),
            )
            .when(!task.is_terminal(), |panel| {
                panel
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(format!("预计剩余 {}", task.eta)),
                    )
                    .when(!task.cancel_requested, |panel| {
                        panel.child(generator_choice(colors, "取消任务", false).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _event, _window, cx| {
                                task_manager::cancel_task(&task_id);
                                cx.notify();
                            }),
                        ))
                    })
            })
            .when_some(task.message.as_ref(), |panel, message| {
                panel.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.text_primary)
                        .child(message.to_string()),
                )
            })
    }
}
