use super::*;
use crate::ui::components::button::Button;
use crate::ui::components::modal;
use std::rc::Rc;

pub fn render_tasks_overlay(
    colors: &ThemeColors,
    view: &Entity<TasksPageView>,
    cx: &App,
) -> Option<AnyElement> {
    let dialog = view.read_with(cx, |this, _| this.confirm_dialog.clone());
    dialog.map(|dialog| {
        let entity = view.downgrade();
        let dismiss_entity = entity.clone();
        let confirm_entity = entity.clone();
        let confirm_task_id = dialog.task_id.clone();
        let confirm_action = dialog.action.clone();

        modal::modal_layer_dismissible(
            div()
                .w(px(500.))
                .max_w(px(500.))
                .rounded(px(20.))
                .border_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .bg(colors.surface)
                .p(px(22.))
                .flex()
                .flex_col()
                .gap(px(16.))
                .on_mouse_down(MouseButton::Left, |_ev, _window, app| {
                    app.stop_propagation()
                })
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(dialog.title),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_secondary)
                        .child(dialog.description),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(10.))
                        .child(
                            Button::new("task-confirm-cancel")
                                .h(px(38.))
                                .px(px(16.))
                                .rounded(px(10.))
                                .bg(Hsla {
                                    a: 0.06,
                                    ..colors.text_secondary
                                })
                                .border_0()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_secondary)
                                .label("取消")
                                .on_click(move |_ev, _window, app| {
                                    let _ = dismiss_entity.update(app, |this, cx| {
                                        this.close_confirm(cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("task-confirm-ok")
                                .h(px(38.))
                                .px(px(16.))
                                .rounded(px(10.))
                                .bg(colors.accent)
                                .border_0()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.btn_primary_text)
                                .label("确认")
                                .on_click(move |_ev, _window, app| {
                                    let task_id = confirm_task_id.clone();
                                    let action = confirm_action.clone();
                                    let _ = confirm_entity.update(app, move |this, cx| {
                                        this.perform_confirm_action(task_id, action, cx);
                                    });
                                }),
                        ),
                ),
            hsla(0.0, 0.0, 0.0, 0.34),
            Rc::new(move |app| {
                let _ = entity.update(app, |this, cx| {
                    this.close_confirm(cx);
                });
            }),
        )
        .into_any_element()
    })
}
