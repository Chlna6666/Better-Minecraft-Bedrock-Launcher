use super::*;
use crate::ui::animation::{ease_in_cubic, ease_out_cubic};
use crate::ui::components::icon::themed_icon;
use crate::ui::views::tasks::TaskCardMotionKind;
use crate::ui::views::tasks::{TaskCardViewModel, TaskConfirmAction, TasksPageView};
use gpui::AnimationExt;
use gpui::prelude::FluentBuilder as _;
use gpui::render_fingerprint;
use lucide_gpui::icons as lucide_icons;
use std::sync::Arc;
use std::time::Duration;

fn stable_task_id(task_id: &str) -> u64 {
    render_fingerprint(task_id)
}

fn meta_item(
    colors: &ThemeColors,
    label: &'static str,
    value: impl Into<SharedString>,
    accent: Hsla,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(task_text_secondary(colors))
                .child(label),
        )
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(accent)
                .child(value.into()),
        )
}

fn meta_separator(colors: &ThemeColors) -> Div {
    div().w(px(2.)).h(px(2.)).rounded(px(999.)).bg(Hsla {
        a: 0.16,
        ..task_text_secondary(colors)
    })
}

fn thread_label_text(worker_active: Option<u32>, worker_total: Option<u32>) -> Option<Arc<str>> {
    worker_total.map(|total| match worker_active {
        Some(active) => Arc::from(format!("{} / {} 线程", active, total)),
        None => Arc::from(format!("{} 线程", total)),
    })
}

fn status_label(model: &TaskCardViewModel) -> Arc<str> {
    match model.status.as_ref() {
        "paused" => Arc::from("已暂停"),
        "cancelling" => Arc::from("取消中..."),
        "completed" => Arc::from("已完成"),
        "cancelled" => Arc::from("已取消"),
        "error" => Arc::from("发生错误"),
        _ => model.speed_text.as_ref().cloned().unwrap_or_else(|| {
            if model.stage.as_ref().contains("安装") || model.stage.as_ref().contains("解压") {
                Arc::from("写入中...")
            } else {
                Arc::from("进行中...")
            }
        }),
    }
}

fn header_right_text(model: &TaskCardViewModel) -> Arc<str> {
    if matches!(model.status.as_ref(), "completed" | "cancelled" | "error") {
        return status_label(model);
    }

    model
        .speed_text
        .as_ref()
        .cloned()
        .unwrap_or_else(|| status_label(model))
}

pub(crate) fn render_task_card(
    colors: &ThemeColors,
    model: &TaskCardViewModel,
    motion: Option<TaskCardMotionKind>,
    cx: &mut Context<TasksPageView>,
) -> impl IntoElement {
    let task_id = model.id.clone();
    let kind = task_visual_kind(model.stage.as_ref(), model.status.as_ref());
    let accent = task_status_accent(model.status.as_ref(), kind, colors);
    let icon_path = task_visual_icon(kind);
    let progress_percent = model.percent_basis_points.map(|value| value as f64 / 100.0);
    let paused = model.status.as_ref() == "paused";

    let mut actions = div().flex().items_center().justify_center().gap(px(8.));
    if model.can_pause {
        let button_task_id = task_id.clone();
        let pause_icon = if paused {
            lucide_icons::icon_play()
        } else {
            lucide_icons::icon_pause()
        };
        actions = actions.child(
            task_icon_button(
                ("task-toggle-pause", stable_task_id(button_task_id.as_ref())),
                pause_icon,
                false,
                true,
                colors,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_pause_task(button_task_id.clone(), cx);
            })),
        );
    }
    if model.can_cancel {
        let button_task_id = task_id.clone();
        actions = actions.child(
            task_icon_button(
                ("task-cancel", stable_task_id(button_task_id.as_ref())),
                lucide_icons::icon_x(),
                true,
                true,
                colors,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.prompt_cancel_task(button_task_id.clone(), cx);
            })),
        );
    }
    if model.can_remove {
        let button_task_id = task_id.clone();
        actions = actions.child(
            task_icon_button(
                ("task-remove", stable_task_id(button_task_id.as_ref())),
                lucide_icons::icon_x(),
                true,
                true,
                colors,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.perform_confirm_action(
                    button_task_id.clone(),
                    TaskConfirmAction::RemoveTask,
                    cx,
                );
            })),
        );
    }

    let mut metrics = div()
        .flex()
        .items_start()
        .flex_wrap()
        .gap(px(10.))
        .child(meta_item(
            colors,
            "进度:",
            SharedString::from(model.amount_text.clone()),
            task_text_tertiary(colors),
        ));
    if let Some(thread_text) = thread_label_text(model.worker_active, model.worker_total) {
        metrics = metrics.child(meta_separator(colors)).child(meta_item(
            colors,
            "线程:",
            SharedString::from(thread_text),
            task_text_tertiary(colors),
        ));
    }
    if let Some(eta_text) = model.eta_text.as_ref() {
        metrics = metrics.child(meta_separator(colors)).child(meta_item(
            colors,
            "ETA:",
            SharedString::from(eta_text.clone()),
            task_text_tertiary(colors),
        ));
    }

    let header_right_value = header_right_text(model);
    let header_right_color = if paused {
        task_warning_color(colors)
    } else {
        accent
    };
    let paused_warning_color = task_warning_color(colors);
    let completed_accent = task_status_accent("completed", kind, colors);
    let card_surface = Hsla {
        a: 0.90,
        ..task_card_bg(colors)
    };
    let card_hover_surface = Hsla {
        a: 0.96,
        ..task_card_hover_bg(colors)
    };

    let message_line = model.message.as_ref().map(|message| {
        let color = if model.status.as_ref() == "error" {
            colors.danger
        } else {
            task_text_secondary(colors)
        };
        div()
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(SharedString::from(message.clone()))
            .into_any_element()
    });

    let base_card = div()
        .id(("task-card", stable_task_id(task_id.as_ref())))
        .w_full()
        .min_h(px(0.))
        .overflow_hidden()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: if paused { 0.10 } else { 0.10 },
            ..task_border_color(colors)
        })
        .bg(card_surface)
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.08,
                ..rgb(0x000000).into()
            },
            blur_radius: px(18.0),
            spread_radius: px(-10.0),
            offset: point(px(0.0), px(7.0)),
        }])
        .px(px(16.))
        .py(px(14.))
        .flex()
        .items_center()
        .gap(px(16.))
        .hover(|this| {
            this.bg(card_hover_surface).border_color(Hsla {
                a: 0.16,
                ..task_border_color(colors)
            })
        })
        .when(paused, |this| {
            this.opacity(0.72).border_color(Hsla {
                a: 0.18,
                ..task_warning_color(colors)
            })
        })
        .child(
            div()
                .w(px(46.))
                .h(px(46.))
                .rounded(px(9.))
                .flex()
                .items_center()
                .justify_center()
                .bg(Hsla { a: 0.08, ..accent })
                .child(themed_icon(icon_path, 24.0, accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .child(
                            div().flex_1().min_w(px(0.)).child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(task_text_main(colors))
                                    .child(SharedString::from(model.title.clone())),
                            ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(header_right_color)
                                .child(SharedString::from(header_right_value)),
                        ),
                )
                .child(div().w_full().child(progress_panel(
                    task_id.as_ref(),
                    kind,
                    colors,
                    model.status.as_ref(),
                    progress_percent,
                )))
                .children(message_line)
                .child(metrics),
        )
        .when(
            model.can_pause || model.can_cancel || model.can_remove,
            |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(actions),
                )
            },
        );

    if let Some(motion_kind) = motion {
        return match motion_kind {
            TaskCardMotionKind::Enter => base_card
                .with_animation(
                    ("task-card-motion-enter", stable_task_id(model.id.as_ref())),
                    Animation::new(Duration::from_millis(240)).with_easing(ease_out_cubic),
                    |card, progress| {
                        card.opacity(0.15 + progress * 0.85)
                            .relative()
                            .top(px((1.0 - progress) * 18.0))
                    },
                )
                .into_any_element(),
            TaskCardMotionKind::Complete => base_card
                .with_animation(
                    (
                        "task-card-motion-complete",
                        stable_task_id(model.id.as_ref()),
                    ),
                    Animation::new(Duration::from_millis(320)).with_easing(ease_out_cubic),
                    move |card, progress| {
                        let pulse = if progress < 0.5 {
                            progress * 2.0
                        } else {
                            (1.0 - progress) * 2.0
                        };
                        card.opacity(0.88 + pulse * 0.12)
                            .relative()
                            .top(px(-4.0 * pulse))
                            .border_color(Hsla {
                                a: 0.10 + pulse * 0.26,
                                ..completed_accent
                            })
                            .bg(Hsla {
                                a: 0.08 + pulse * 0.10,
                                ..completed_accent
                            })
                    },
                )
                .into_any_element(),
            TaskCardMotionKind::Warn => base_card
                .with_animation(
                    ("task-card-motion-warn", stable_task_id(model.id.as_ref())),
                    Animation::new(Duration::from_millis(220)).with_easing(ease_out_cubic),
                    move |card, progress| {
                        let pulse = if progress < 0.5 {
                            progress * 2.0
                        } else {
                            (1.0 - progress) * 2.0
                        };
                        card.opacity(0.92 + pulse * 0.08)
                            .border_color(Hsla {
                                a: 0.12 + pulse * 0.24,
                                ..paused_warning_color
                            })
                            .bg(Hsla {
                                a: 0.04 + pulse * 0.08,
                                ..paused_warning_color
                            })
                    },
                )
                .into_any_element(),
            TaskCardMotionKind::Exit => base_card
                .with_animation(
                    ("task-card-motion-exit", stable_task_id(model.id.as_ref())),
                    Animation::new(Duration::from_millis(220)).with_easing(ease_in_cubic),
                    move |card, progress| {
                        let fade = 1.0 - progress;
                        let collapsed = (1.0 - progress).clamp(0.0, 1.0);
                        card.opacity(fade)
                            .relative()
                            .top(px(-10.0 * progress))
                            .max_h(px(144.0 * collapsed))
                            .px(px(16.0 * collapsed))
                            .py(px(14.0 * collapsed))
                            .border_color(Hsla {
                                a: 0.06 * fade,
                                ..paused_warning_color
                            })
                    },
                )
                .into_any_element(),
        };
    }

    base_card.into_any_element()
}
