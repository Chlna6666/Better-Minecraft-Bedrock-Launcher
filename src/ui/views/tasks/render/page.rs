use super::*;
use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::views::tasks::{
    TaskCardMotionKind, TaskCardViewModel, TasksPageRenderModel, TasksPageView,
};
use gpui::prelude::FluentBuilder as _;
use lucide_gpui::icons as lucide_icons;
use std::sync::Arc;

fn loading_state(colors: &ThemeColors) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(task_text_secondary(colors))
                .child("任务数据整理中..."),
        )
        .into_any_element()
}

fn empty_state(colors: &ThemeColors) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .size(px(54.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_icons::icon_inbox(),
                            26.0,
                            task_visual_accent(TaskVisualKind::Download, colors),
                        )),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(task_text_main(colors))
                        .child("暂无任务"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(task_text_secondary(colors))
                        .child("下载、安装和更新开始后，任务会在这里按紧凑列表展示。"),
                ),
        )
        .into_any_element()
}

fn header_stat(colors: &ThemeColors, label: &'static str, value: impl ToString) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(task_text_secondary(colors))
                .child(label),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(task_text_main(colors))
                .child(SharedString::from(value.to_string())),
        )
}

fn render_task_list(
    colors: &ThemeColors,
    this: &TasksPageView,
    items: impl IntoIterator<Item = TaskCardViewModel>,
    cx: &mut Context<TasksPageView>,
) -> Div {
    let mut list = div().w_full().flex().flex_col().gap(px(12.));
    let mut entries: Vec<(u64, Arc<str>, TaskCardViewModel, Option<TaskCardMotionKind>)> = items
        .into_iter()
        .map(|item| {
            let sort_key = item.started_at_unix;
            let motion = this.task_motion_kind(item.id.as_ref());
            (sort_key, item.id.clone(), item, motion)
        })
        .collect();

    for transition_card in this.transition_cards() {
        if this
            .render_model
            .active
            .iter()
            .chain(this.render_model.finished.iter())
            .any(|item| item.id == transition_card.model.id)
        {
            continue;
        }
        entries.push((
            transition_card.model.started_at_unix,
            transition_card.model.id.clone(),
            transition_card.model,
            Some(transition_card.motion),
        ));
    }

    entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    for (_, _, model, motion) in entries {
        list = list.child(render_task_card(colors, &model, motion, cx));
    }

    list
}

fn render_tasks_body(
    colors: &ThemeColors,
    this: &TasksPageView,
    render_model: &TasksPageRenderModel,
    cx: &mut Context<TasksPageView>,
) -> AnyElement {
    if render_model.loading {
        return loading_state(colors);
    }

    if render_model.total_count == 0 {
        if this.transition_cards().is_empty() {
            return empty_state(colors);
        }

        return render_task_list(colors, this, Vec::new(), cx).into_any_element();
    }

    render_task_list(
        colors,
        this,
        render_model
            .active
            .iter()
            .cloned()
            .chain(render_model.finished.iter().cloned())
            .collect::<Vec<_>>(),
        cx,
    )
    .into_any_element()
}

pub(super) fn render_tasks_page(
    colors: ThemeColors,
    this: &TasksPageView,
    _window: &mut Window,
    cx: &mut Context<TasksPageView>,
) -> impl IntoElement {
    let header = div()
        .w_full()
        .px(px(24.))
        .py(px(16.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.08,
            ..task_border_color(&colors)
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(20.))
        .child(
            div().flex().flex_col().gap(px(4.)).child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(task_text_main(&colors))
                    .child("任务管理器"),
            ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(20.))
                .child(header_stat(
                    &colors,
                    "活动任务:",
                    this.render_model.active_total,
                ))
                .child(header_stat(
                    &colors,
                    "总线程:",
                    this.render_model.thread_total,
                )),
        );

    let list_body = render_tasks_body(&colors, this, &this.render_model, cx);
    let body = div()
        .flex_1()
        .min_h(px(0.))
        .px(px(20.))
        .py(px(16.))
        .overflow_y_scrollbar()
        .child(div().w_full().child(list_body));

    page_shell(
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body),
        &colors,
    )
}
