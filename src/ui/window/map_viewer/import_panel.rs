//! Shared chrome and task feedback for the three import editors.

use super::model::View;
use super::panels::dock_close_button;
use super::prelude::*;

impl View {
    /// Keep the close control visible while the editor's parameters scroll beneath it.
    pub(super) fn render_import_panel_header(
        &self,
        colors: &ThemeColors,
        title: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .h(px(48.0))
            .flex_none()
            .px(px(14.0))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(title.into()),
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
        let stage = crate::ui::views::tasks::task_stage_text(&task.stage);
        let status = match task.status.as_ref() {
            "completed" => t!("Tasks.status.completed"),
            "cancelled" => t!("Tasks.status.cancelled"),
            "error" => t!("Tasks.status.error"),
            _ if task.cancel_requested => t!("Tasks.status.cancelling"),
            _ => t!("Tasks.status.running"),
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
                            .child(t!("Tasks.eta_remaining", eta = task.eta.to_string())),
                    )
                    .when(!task.cancel_requested, |panel| {
                        panel.child(choice::chip(colors, t!("Tasks.cancel_task"), false).on_mouse_down(
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

