use crate::tasks::task_manager;
use crate::ui::animation::repeating_linear_motion;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::{button, modal};
use crate::ui::hooks::use_linux_runtime::{
    authorize_and_install, can_authorize_install, dismiss, open_proton_gdk_settings, recheck,
};
use crate::ui::state::i18n::I18n;
use crate::ui::state::linux_runtime::{LinuxRuntimeState, LinuxRuntimeStatus};
use crate::ui::theme::colors::ThemeColors;
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Duration;

pub fn render_linux_runtime_overlay(
    state: &LinuxRuntimeState,
    colors: &ThemeColors,
    i18n: &I18n,
) -> AnyElement {
    if !state.visible {
        return Empty {}.into_any_element();
    }

    let busy = matches!(
        state.status,
        LinuxRuntimeStatus::Checking | LinuxRuntimeStatus::Installing
    );
    let is_error = state.status == LinuxRuntimeStatus::Error;
    let can_install = can_authorize_install(state);
    let check = state.check.as_ref();
    let needs_host_dependencies = check
        .and_then(|check| check.install_plan.as_ref())
        .is_some();
    let distribution = check
        .map(|check| SharedString::from(check.distribution_name.to_string()))
        .unwrap_or_else(|| t!("LinuxRuntime.detecting_distribution"));
    let status_text = match state.status {
        LinuxRuntimeStatus::Checking => t!("LinuxRuntime.checking"),
        LinuxRuntimeStatus::Installing => t!("LinuxRuntime.installing"),
        LinuxRuntimeStatus::Error => t!("LinuxRuntime.error"),
        _ if needs_host_dependencies => t!("LinuxRuntime.missing_dependencies"),
        _ => t!("LinuxRuntime.missing_runtime"),
    };

    let mut details = div().flex().flex_col().gap(px(10.));
    if let Some(reason) = check.and_then(|check| check.missing_reason.as_ref()) {
        details = details.child(info_row(
            colors,
            lucide_icons::icon_circle_alert(),
            t!("LinuxRuntime.detection_result"),
            SharedString::from(reason.to_string()),
        ));
    }
    details = details.child(info_row(
        colors,
        lucide_icons::icon_package(),
        t!("LinuxRuntime.distribution"),
        distribution,
    ));
    if let Some(plan) = check.and_then(|check| check.install_plan.as_ref()) {
        details = details.child(info_row(
            colors,
            lucide_icons::icon_terminal(),
            t!("LinuxRuntime.authorized_action"),
            plan.command_preview().into(),
        ));
    }
    if state.status == LinuxRuntimeStatus::Installing {
        let logs = state
            .install_task_id
            .as_ref()
            .map(|task_id| task_manager::task_logs(task_id.as_ref()))
            .unwrap_or_else(|| std::sync::Arc::<[std::sync::Arc<str>]>::from([]));
        let stage = state
            .install_snapshot
            .as_ref()
            .map(|snapshot| SharedString::from(snapshot.stage.to_string()))
            .unwrap_or_else(|| t!("LinuxRuntime.preparing_install"));
        let current_output = logs
            .last()
            .map(|line| SharedString::from(line.to_string()))
            .or_else(|| {
                state
                    .install_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.message.as_ref())
                    .map(|message| SharedString::from(message.to_string()))
            })
            .unwrap_or_else(|| t!("LinuxRuntime.waiting_authorization"));
        details = details.child(install_progress(
            colors,
            stage,
            current_output,
            t!("LinuxRuntime.processing"),
        ));
    }
    if let Some(error) = state.error_message.as_ref() {
        details = details.child(
            div()
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .border_1()
                .border_color(Hsla {
                    a: 0.40,
                    ..colors.danger
                })
                .bg(Hsla {
                    a: 0.10,
                    ..colors.danger
                })
                .px(px(12.))
                .py(px(10.))
                .text_size(px(12.))
                .text_color(colors.danger)
                .child(error.clone()),
        );
    }

    let manual_hint = check
        .map(|check| SharedString::from(check.manual_install_hint.to_string()))
        .unwrap_or_else(|| t!("LinuxRuntime.install_hint_pending"));
    let mut later_button =
        button::secondary_button(colors, "linux-runtime-later", t!("LinuxRuntime.later"))
            .when(busy, |button| button.opacity(0.45));
    if !busy {
        later_button = later_button.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            dismiss(cx);
        });
    }

    let action_label = match state.status {
        LinuxRuntimeStatus::Checking => t!("LinuxRuntime.checking_action"),
        LinuxRuntimeStatus::Installing => t!("LinuxRuntime.installing_action"),
        LinuxRuntimeStatus::Error if !can_install => t!("LinuxRuntime.recheck"),
        _ if can_install => t!("LinuxRuntime.authorize_install"),
        _ => t!("LinuxRuntime.open_settings"),
    };
    let action_enabled = !busy;
    let mut action_button =
        button::primary_button(colors, "linux-runtime-primary-action", action_label)
            .when(!action_enabled, |button| button.opacity(0.45));
    if action_enabled {
        action_button =
            action_button.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                if can_install {
                    authorize_and_install(cx);
                } else if is_error {
                    recheck(cx);
                } else {
                    open_proton_gdk_settings(cx);
                }
            });
    }

    let surface = modal::modal_surface(
        colors.settings_panel_bg,
        colors.border,
        px(620.),
        px(480.),
        px(18.),
    )
    .shadow_lg()
    .child(
        div()
            .px(px(24.))
            .pt(px(22.))
            .pb(px(16.))
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_start()
            .gap(px(14.))
            .child(
                div()
                    .size(px(42.))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .bg(colors.stat_orange_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(lucide_icons::icon_shield_alert())
                            .size(px(21.))
                            .text_color(colors.stat_orange_text),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary)
                            .child(if needs_host_dependencies {
                                t!("LinuxRuntime.dependencies_title")
                            } else {
                                t!("LinuxRuntime.runtime_title")
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(colors.text_secondary)
                            .child(status_text),
                    ),
            ),
    )
    .child(
        div()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scrollbar()
            .scrollbar_width(px(6.))
            .px(px(24.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(colors.text_secondary)
                    .child(if needs_host_dependencies {
                        t!("LinuxRuntime.dependencies_description")
                    } else {
                        t!("LinuxRuntime.runtime_description")
                    }),
            )
            .child(details)
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(relative(1.45))
                    .text_color(colors.text_muted)
                    .child(manual_hint),
            )
            .child(
                div()
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .bg(colors.stat_green_bg)
                    .px(px(12.))
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        svg()
                            .path(lucide_icons::icon_shield_check())
                            .size(px(16.))
                            .text_color(colors.stat_green_text),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.stat_green_text)
                            .child(if needs_host_dependencies {
                                t!("LinuxRuntime.dependencies_security")
                            } else {
                                t!("LinuxRuntime.runtime_security")
                            }),
                    ),
            ),
    )
    .child(
        div()
            .px(px(24.))
            .py(px(15.))
            .border_t_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_end()
            .gap(px(10.))
            .child(later_button)
            .child(action_button),
    );

    modal::modal_layer(surface, Hsla { a: 0.52, ..black() })
}

fn install_progress(
    colors: &ThemeColors,
    stage: SharedString,
    current_output: SharedString,
    processing: SharedString,
) -> AnyElement {
    div()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.07,
            ..colors.accent
        })
        .px(px(12.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(stage),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(colors.accent)
                        .child(processing),
                ),
        )
        .child(
            div()
                .h(px(9.))
                .w_full()
                .rounded_full()
                .relative()
                .overflow_hidden()
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .bottom(px(2.))
                        .w(relative(0.34))
                        .rounded_full()
                        .bg(linear_gradient(
                            90.0,
                            linear_color_stop(
                                Hsla {
                                    a: 0.28,
                                    ..colors.progress_fill
                                },
                                0.0,
                            ),
                            linear_color_stop(colors.progress_fill, 1.0),
                        ))
                        .with_animation(
                            "linux-runtime-install-progress",
                            repeating_linear_motion(Duration::from_millis(1200)),
                            |bar, progress| bar.left(relative(-0.34 + progress * 1.40)),
                        ),
                ),
        )
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(current_output),
        )
        .into_any_element()
}

fn info_row(
    colors: &ThemeColors,
    icon: &'static str,
    label: impl Into<SharedString>,
    value: SharedString,
) -> AnyElement {
    let label = label.into();
    div()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(colors.settings_card_bg)
        .border_1()
        .border_color(colors.border)
        .px(px(12.))
        .py(px(7.))
        .flex()
        .items_start()
        .gap(px(10.))
        .child(svg().path(icon).size(px(15.)).text_color(colors.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_muted)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_primary)
                        .child(value),
                ),
        )
        .into_any_element()
}
