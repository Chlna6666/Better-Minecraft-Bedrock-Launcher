use super::*;
use crate::ui::components::icon::themed_icon;
use crate::ui::state::i18n::I18n;
use crate::ui::views::tasks::{
    TaskCardMotionKind, TaskCardViewModel, TasksPageRenderModel, TasksPageView,
};

fn loading_state(colors: &ThemeColors, _i18n: &I18n) -> AnyElement {
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
                .child(t!("common.loading")),
        )
        .into_any_element()
}

fn empty_state(colors: &ThemeColors, _i18n: &I18n) -> AnyElement {
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
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_gpui::icon!(inbox),
                            26.0,
                            task_visual_accent(TaskVisualKind::Download, colors),
                        )),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(task_text_main(colors))
                        .child(t!("Tasks.empty")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(task_text_secondary(colors))
                        .child(t!("Tasks.empty_hint")),
                ),
        )
        .into_any_element()
}

fn header_stat(colors: &ThemeColors, label: SharedString, value: impl ToString) -> Div {
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

fn render_task_list<'a>(
    colors: &ThemeColors,
    this: &'a TasksPageView,
    items: impl IntoIterator<Item = &'a TaskCardViewModel>,
    cx: &mut Context<TasksPageView>,
) -> Div {
    let mut list = div().w_full().flex().flex_col().gap(px(12.));
    let mut entries: Vec<(&TaskCardViewModel, Option<TaskCardMotionKind>)> = items
        .into_iter()
        .map(|item| {
            let motion = this.task_motion_kind(item.id.as_ref());
            (item, motion)
        })
        .collect();

    for transition_card in this.transition_cards() {
        if entries
            .iter()
            .any(|(item, _)| item.id == transition_card.model.id)
        {
            continue;
        }
        entries.push((&transition_card.model, Some(transition_card.motion)));
    }

    entries.sort_by(|(left, _), (right, _)| {
        left.started_at_unix
            .cmp(&right.started_at_unix)
            .then_with(|| left.id.cmp(&right.id))
    });

    for (model, motion) in entries {
        list = list.child(render_task_card(colors, model, motion, cx));
    }

    list
}

fn render_tasks_body<'a>(
    colors: &ThemeColors,
    this: &'a TasksPageView,
    render_model: &'a TasksPageRenderModel,
    i18n: &I18n,
    cx: &mut Context<TasksPageView>,
) -> AnyElement {
    if render_model.loading {
        return loading_state(colors, i18n);
    }

    if render_model.total_count == 0 {
        if !this.has_transition_cards() {
            return empty_state(colors, i18n);
        }

        return render_task_list(colors, this, std::iter::empty(), cx).into_any_element();
    }

    render_task_list(
        colors,
        this,
        render_model
            .active
            .iter()
            .chain(render_model.finished.iter()),
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
    let i18n = cx.global::<I18n>().clone();
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
                    .font_weight(FontWeight::BOLD)
                    .text_color(task_text_main(&colors))
                    .child(t!("Tasks.title")),
            ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(20.))
                .child(header_stat(
                    &colors,
                    t!("Tasks.active"),
                    this.render_model.active_total,
                ))
                .child(header_stat(
                    &colors,
                    t!("Tasks.total_threads"),
                    this.render_model.thread_total,
                )),
        );

    let list_body = render_tasks_body(&colors, this, &this.render_model, &i18n, cx);
    let body = div()
        .flex_1()
        .min_h(px(0.))
        .px(px(20.))
        .py(px(16.))
        .overflow_y_scrollbar()
        .child(div().w_full().child(list_body));

    div()
        .size_full()
        .relative()
        .child(page_shell(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(header)
                .child(body),
            &colors,
        ))
        .child(crate::ui::onboarding::anchor::observe(
            crate::ui::onboarding::state::OnboardingAnchor::TasksPage,
        ))
}
