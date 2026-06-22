use std::time::Instant;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::core::minecraft::launcher::preflight::{LaunchPlatform, detect_launch_platform};
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::hooks::use_launcher::{
    cancel_launch_prereq, dismiss_launch_prereq, enable_launch_prereq_developer_mode,
    install_launch_prereq_game_input, install_launch_prereq_uwp_dependencies,
    open_launch_prereq_developer_settings, recheck_launch_prereq,
};
use crate::ui::state::i18n::I18n;
use crate::ui::state::launch_prereq::{LaunchPrereqOperation, LaunchPrereqState};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::utils::mc_dependency::{GAMEINPUT_RELEASES_URL, GameInputInstallerSource};

pub fn render_launch_prereq_overlay(
    state: &LaunchPrereqState,
    _window: &mut Window,
    cx: &App,
) -> AnyElement {
    if !state.visible {
        return div().into_any_element();
    }

    let now = Instant::now();
    let theme_state = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme_state.factor(now),
        theme_state.accent,
    );
    let i18n = cx.global::<I18n>();
    let version = state.version.as_ref();
    let platform = state
        .check
        .as_ref()
        .map(|check| check.platform)
        .or_else(|| version.map(|version| detect_launch_platform(version.kind.as_ref())))
        .unwrap_or(LaunchPlatform::Uwp);
    let platform_label = match platform {
        LaunchPlatform::Uwp => i18n.t("common.uwp"),
        LaunchPlatform::Gdk => i18n.t("common.gdk"),
    };
    let subtitle = version
        .map(|version| {
            i18n.t_args(
                "LaunchPrereq.subtitle",
                crate::i18n_args![
                    ("name", &version.name),
                    ("folder", &version.folder),
                    ("version", &version.version)
                ],
            )
        })
        .unwrap_or_else(|| i18n.t("LaunchPrereq.waitingAction"));
    let operation_text = state
        .operation
        .map(|operation| operation_label(operation, i18n))
        .unwrap_or_else(|| i18n.t("McDeps.waitingAction"));
    let progress_percent = state
        .progress_percent
        .unwrap_or(match state.operation {
            Some(LaunchPrereqOperation::Checking) => 12,
            Some(LaunchPrereqOperation::OpeningDeveloperSettings) => 18,
            _ => 0,
        })
        .min(100);
    let progress_ratio = (progress_percent as f32 / 100.0).clamp(0.0, 1.0);
    let action_enabled = !state.is_busy();

    let mut content = div()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scrollbar()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .gap(px(12.));

    if let Some(admin_notice) = state.admin_notice.as_ref() {
        content = content.child(status_banner(
            &colors,
            lucide_icons::icon_shield_alert(),
            colors.stat_orange_bg,
            colors.stat_orange_text,
            admin_notice.clone(),
        ));
    }
    if let Some(error_message) = state.error_message.as_ref() {
        content = content.child(status_banner(
            &colors,
            lucide_icons::icon_circle_x(),
            Hsla {
                a: 0.14,
                ..colors.danger
            },
            colors.danger,
            error_message.clone(),
        ));
    }

    content = content.child(render_progress_section(
        state,
        &colors,
        now,
        operation_text,
        progress_percent,
        progress_ratio,
    ));
    content = content.child(render_issue_sections(state, &colors, action_enabled, i18n));
    content = content.child(render_logs_section(state, &colors, i18n));

    let mut recheck_button =
        secondary_button(&colors, i18n.t("LaunchPrereq.recheck"), action_enabled);
    if action_enabled {
        recheck_button = recheck_button.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            recheck_launch_prereq(cx);
        });
    }
    let mut close_button = secondary_button(&colors, i18n.t("common.cancel"), true);
    close_button = close_button.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
        if cx.global::<LaunchPrereqState>().is_busy() {
            cancel_launch_prereq(cx);
        } else {
            dismiss_launch_prereq(cx);
        }
    });

    modal::modal_layer(
        render_shell(
            &colors,
            i18n.t("LaunchPrereq.title"),
            subtitle,
            platform,
            platform_label,
            i18n.t("LaunchPrereq.footerHint"),
            content,
            recheck_button,
            close_button,
        ),
        Hsla {
            a: 0.50,
            ..rgb(0x020617).into()
        },
    )
    .into_any_element()
}

fn render_progress_section(
    state: &LaunchPrereqState,
    colors: &ThemeColors,
    now: Instant,
    operation_text: SharedString,
    progress_percent: u32,
    progress_ratio: f32,
) -> AnyElement {
    let show = state.operation.is_some() || state.check.is_none();
    if !show {
        return Empty {}.into_any_element();
    }

    let progress_target = state.progress_target.clone();
    let progress_stage = if state.progress_stage.is_empty() {
        operation_text.clone()
    } else {
        state.progress_stage.clone()
    };

    section_card(colors)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(spinning_icon_shell(
                            colors,
                            lucide_icons::icon_loader_circle(),
                            colors.accent,
                            state.busy_animation_rotation(now),
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(operation_text),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(progress_stage),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child(format!("{progress_percent}%")),
                ),
        )
        .child(progress_bar(colors, progress_ratio))
        .when_some(progress_target, |this, target| {
            this.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_muted)
                    .child(target),
            )
        })
        .into_any_element()
}

fn render_issue_sections(
    state: &LaunchPrereqState,
    colors: &ThemeColors,
    close_enabled: bool,
    i18n: &I18n,
) -> AnyElement {
    let Some(check) = state.check.as_ref() else {
        return Empty {}.into_any_element();
    };

    let mut sections = div().flex().flex_col().gap(px(12.));

    if check.developer_mode_required {
        let mut open_settings_button = secondary_button(
            colors,
            i18n.t("LaunchPrereq.issueDeveloperMode.openSettings"),
            close_enabled,
        );
        if close_enabled {
            open_settings_button = open_settings_button
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    open_launch_prereq_developer_settings(cx)
                });
        }
        let mut modify_registry_button = primary_button(
            colors,
            i18n.t("LaunchPrereq.issueDeveloperMode.modifyRegistry"),
            close_enabled,
        );
        if close_enabled {
            modify_registry_button = modify_registry_button
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    enable_launch_prereq_developer_mode(cx)
                });
        }

        sections = sections.child(issue_card(
            colors,
            lucide_icons::icon_wrench(),
            colors.stat_orange_text,
            i18n.t("LaunchPrereq.issueDeveloperMode.title"),
            i18n.t("LaunchPrereq.issueDeveloperMode.description"),
            vec![developer_mode_actions(
                open_settings_button,
                modify_registry_button,
            )],
        ));
    }

    if !check.missing_uwp_dependencies.is_empty() {
        let mut install_button = primary_button(
            colors,
            i18n.t("LaunchPrereq.issueUwpDependencies.install"),
            close_enabled,
        );
        if close_enabled {
            install_button = install_button
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    install_launch_prereq_uwp_dependencies(cx)
                });
        }

        let issue_rows = check
            .missing_uwp_dependencies
            .iter()
            .map(|dependency| render_uwp_dependency_issue_row(colors, dependency, i18n))
            .collect::<Vec<_>>();

        sections = sections.child(
            section_card(colors).child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .gap(px(12.))
                            .items_start()
                            .child(icon_shell(
                                colors,
                                lucide_icons::icon_package_plus(),
                                colors.accent,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text_primary)
                                            .child(
                                                i18n.t("LaunchPrereq.issueUwpDependencies.title"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .line_height(relative(1.45))
                                            .text_color(colors.text_secondary)
                                            .child(i18n.t(
                                                "LaunchPrereq.issueUwpDependencies.description",
                                            )),
                                    )
                                    .child(
                                        div().flex().flex_col().gap(px(8.)).children(issue_rows),
                                    ),
                            ),
                    )
                    .child(install_button),
            ),
        );
    }

    if let Some(plan) = check.game_input_plan.as_ref() {
        let mut install_button = primary_button(
            colors,
            i18n.t("LaunchPrereq.issueGameInput.install"),
            close_enabled,
        );
        if close_enabled {
            install_button = install_button
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    install_launch_prereq_game_input(cx)
                });
        }

        let description = match plan.source {
            GameInputInstallerSource::Local => {
                i18n.t("LaunchPrereq.issueGameInput.descriptionLocal")
            }
            GameInputInstallerSource::Download => {
                i18n.t("LaunchPrereq.issueGameInput.descriptionDownload")
            }
        };
        let source_label = match plan.source {
            GameInputInstallerSource::Local => {
                SharedString::from(plan.installer_path.display().to_string())
            }
            GameInputInstallerSource::Download => SharedString::from(GAMEINPUT_RELEASES_URL),
        };

        sections = sections.child(
            section_card(colors).child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .items_start()
                            .gap(px(12.))
                            .child(icon_shell(
                                colors,
                                lucide_icons::icon_gamepad_2(),
                                colors.stat_green_text,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text_primary)
                                            .child(i18n.t("LaunchPrereq.issueGameInput.title")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .line_height(relative(1.45))
                                            .text_color(colors.text_secondary)
                                            .child(description),
                                    )
                                    .child(metadata_chip(colors, source_label)),
                            ),
                    )
                    .child(install_button),
            ),
        );
    }

    sections.into_any_element()
}

fn render_logs_section(state: &LaunchPrereqState, colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    let logs = state.log_lines();
    div()
        .rounded(px(16.))
        .bg(launch_prereq_section_bg(colors))
        .border_1()
        .border_color(launch_prereq_section_border(colors))
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div().flex().items_center().gap(px(8.)).child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text_primary)
                    .child(i18n.t("LaunchPrereq.logs.title")),
            ),
        )
        .child(
            div()
                .min_h(px(0.))
                .max_h(px(156.))
                .overflow_y_scrollbar()
                .scrollbar_width(px(6.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .min_h(px(76.))
                .children(if logs.is_empty() {
                    vec![
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(i18n.t("McDeps.noLogs"))
                            .into_any_element(),
                    ]
                } else {
                    logs.into_iter()
                        .map(|line| {
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(log_color(colors, line.as_ref()))
                                .child(line)
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
        )
        .into_any_element()
}

fn render_shell(
    colors: &ThemeColors,
    title: SharedString,
    subtitle: SharedString,
    platform: LaunchPlatform,
    platform_label: SharedString,
    footer_hint: SharedString,
    content: impl IntoElement,
    recheck_button: Div,
    close_button: Div,
) -> Div {
    let shell_radius = px(22.);

    div()
        .w(px(696.))
        .max_w(relative(1.0))
        .max_h(relative(1.0))
        .rounded(shell_radius)
        .bg(launch_prereq_shell_bg(colors))
        .border_1()
        .border_color(launch_prereq_shell_border(colors))
        .shadow(launch_prereq_shell_shadow(colors))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .bg(launch_prereq_header_bg(colors))
                .rounded_t(shell_radius)
                .px(px(22.))
                .py(px(18.))
                .border_b_1()
                .border_color(launch_prereq_section_border(colors))
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(icon_shell(
                            colors,
                            lucide_icons::icon_shield_check(),
                            colors.accent,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(18.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(relative(1.45))
                                        .text_color(colors.text_secondary)
                                        .child(subtitle),
                                ),
                        ),
                )
                .child(platform_badge(colors, platform, platform_label)),
        )
        .child(
            div()
                .bg(launch_prereq_body_bg(colors))
                .flex_1()
                .min_h(px(0.))
                .p(px(18.))
                .flex()
                .flex_col()
                .child(content),
        )
        .child(
            div()
                .bg(launch_prereq_footer_bg(colors))
                .rounded_b(shell_radius)
                .px(px(18.))
                .py(px(14.))
                .border_t_1()
                .border_color(launch_prereq_section_border(colors))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(footer_hint),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(recheck_button)
                        .child(close_button),
                ),
        )
}

fn issue_card(
    colors: &ThemeColors,
    icon_path: &'static str,
    icon_color: Hsla,
    title: SharedString,
    description: SharedString,
    actions: Vec<AnyElement>,
) -> AnyElement {
    section_card(colors)
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(icon_shell(colors, icon_path, icon_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(relative(1.45))
                                        .text_color(colors.text_secondary)
                                        .child(description),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_end()
                        .gap(px(8.))
                        .children(actions),
                ),
        )
        .into_any_element()
}

fn section_card(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(16.))
        .bg(launch_prereq_section_bg(colors))
        .border_1()
        .border_color(launch_prereq_section_border(colors))
        .shadow(launch_prereq_section_shadow(colors))
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
}

fn developer_mode_actions(open_settings_button: Div, modify_registry_button: Div) -> AnyElement {
    div()
        .flex_none()
        .w(px(200.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(open_settings_button.flex_1())
        .child(modify_registry_button.flex_1())
        .into_any_element()
}

fn icon_shell(colors: &ThemeColors, icon_path: &'static str, icon_color: Hsla) -> Div {
    div()
        .w(px(36.))
        .h(px(36.))
        .rounded(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.16,
            ..colors.surface
        })
        .child(svg().path(icon_path).size(px(18.)).text_color(icon_color))
}

fn spinning_icon_shell(
    colors: &ThemeColors,
    icon_path: &'static str,
    icon_color: Hsla,
    rotation: f32,
) -> Div {
    div()
        .w(px(36.))
        .h(px(36.))
        .rounded(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.16,
            ..colors.surface
        })
        .child(
            svg()
                .path(icon_path)
                .size(px(18.))
                .text_color(icon_color)
                .with_transformation(Transformation::rotate(radians(rotation))),
        )
}

fn progress_bar(colors: &ThemeColors, ratio: f32) -> Div {
    div()
        .w_full()
        .h(px(8.))
        .rounded(px(999.))
        .bg(Hsla {
            a: 0.72,
            ..colors.progress_track
        })
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(ratio.clamp(0.0, 1.0)))
                .bg(colors.progress_fill)
                .rounded(px(999.)),
        )
}

fn metadata_chip(colors: &ThemeColors, label: SharedString) -> AnyElement {
    div()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(999.))
        .bg(launch_prereq_nested_bg(colors))
        .border_1()
        .border_color(launch_prereq_nested_border(colors))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors.text_secondary)
        .child(label)
        .into_any_element()
}

fn render_uwp_dependency_issue_row(
    colors: &ThemeColors,
    dependency: &crate::utils::mc_dependency::MissingUwpDependency,
    i18n: &I18n,
) -> AnyElement {
    let reason = format_uwp_dependency_issue_reason(dependency, i18n);
    let badge_label = match &dependency.issue_kind {
        crate::utils::mc_dependency::UwpDependencyIssueKind::Missing => {
            i18n.t("LaunchPrereq.issueUwpDependencies.reasonMissing")
        }
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch { .. } => {
            i18n.t("LaunchPrereq.issueUwpDependencies.reasonVersionShort")
        }
    };
    let badge_color = match &dependency.issue_kind {
        crate::utils::mc_dependency::UwpDependencyIssueKind::Missing => colors.stat_orange_text,
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch { .. } => {
            colors.badge_beta_text
        }
    };

    div()
        .rounded(px(14.))
        .bg(launch_prereq_issue_row_bg(colors, &dependency.issue_kind))
        .border_1()
        .border_color(launch_prereq_issue_row_border(
            colors,
            &dependency.issue_kind,
        ))
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(SharedString::from(dependency.name.clone())),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(relative(1.45))
                        .text_color(colors.text_secondary)
                        .child(reason),
                ),
        )
        .child(metadata_chip_with_color(colors, badge_label, badge_color))
        .into_any_element()
}

fn format_uwp_dependency_issue_reason(
    dependency: &crate::utils::mc_dependency::MissingUwpDependency,
    i18n: &I18n,
) -> SharedString {
    match &dependency.issue_kind {
        crate::utils::mc_dependency::UwpDependencyIssueKind::Missing => {
            i18n.t("LaunchPrereq.issueUwpDependencies.reasonMissing")
        }
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch {
            installed_version: Some(installed_version),
            required_version,
        } => i18n.t_args(
            "LaunchPrereq.issueUwpDependencies.reasonVersionMismatch",
            crate::i18n_args![
                ("current", installed_version),
                ("required", required_version)
            ],
        ),
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch {
            installed_version: None,
            required_version,
        } => i18n.t_args(
            "LaunchPrereq.issueUwpDependencies.reasonVersionUnknown",
            crate::i18n_args![("required", required_version)],
        ),
    }
}

fn metadata_chip_with_color(
    colors: &ThemeColors,
    label: SharedString,
    foreground: Hsla,
) -> AnyElement {
    div()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(999.))
        .bg(launch_prereq_nested_bg(colors))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..foreground
        })
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
        .child(label)
        .into_any_element()
}

fn status_banner(
    colors: &ThemeColors,
    icon_path: &'static str,
    background: Hsla,
    foreground: Hsla,
    message: SharedString,
) -> AnyElement {
    div()
        .rounded(px(16.))
        .bg(background)
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..foreground
        })
        .px(px(14.))
        .py(px(12.))
        .flex()
        .items_start()
        .gap(px(10.))
        .child(svg().path(icon_path).size(px(16.)).text_color(foreground))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.))
                .line_height(relative(1.45))
                .text_color(if foreground.a < 0.99 {
                    colors.text_primary
                } else {
                    foreground
                })
                .child(message),
        )
        .into_any_element()
}

fn platform_badge(
    colors: &ThemeColors,
    platform: LaunchPlatform,
    label: SharedString,
) -> AnyElement {
    let (background, foreground) = match platform {
        LaunchPlatform::Uwp => (colors.badge_stable_bg, colors.badge_stable_text),
        LaunchPlatform::Gdk => (colors.badge_beta_bg, colors.badge_beta_text),
    };

    div()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(999.))
        .bg(background)
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(foreground)
        .child(label)
        .into_any_element()
}

fn primary_button(colors: &ThemeColors, label: SharedString, enabled: bool) -> Div {
    div()
        .h(px(36.))
        .min_w(px(82.))
        .px(px(14.))
        .rounded(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.accent)
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.btn_primary_text)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.accent_hover))
        })
        .when(!enabled, |this| this.opacity(0.45))
        .child(label)
}

fn secondary_button(colors: &ThemeColors, label: SharedString, enabled: bool) -> Div {
    div()
        .h(px(36.))
        .min_w(px(82.))
        .px(px(14.))
        .rounded(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.68,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.text_primary)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.surface_hover))
        })
        .when(!enabled, |this| this.opacity(0.45))
        .child(label)
}

fn operation_label(operation: LaunchPrereqOperation, i18n: &I18n) -> SharedString {
    match operation {
        LaunchPrereqOperation::Checking => i18n.t("LaunchPrereq.operation.checking"),
        LaunchPrereqOperation::OpeningDeveloperSettings => {
            i18n.t("LaunchPrereq.operation.openingDeveloperSettings")
        }
        LaunchPrereqOperation::EnablingDeveloperMode => {
            i18n.t("LaunchPrereq.operation.enablingDeveloperMode")
        }
        LaunchPrereqOperation::InstallingUwpDependencies => {
            i18n.t("LaunchPrereq.operation.installingUwpDependencies")
        }
        LaunchPrereqOperation::InstallingGameInput => {
            i18n.t("LaunchPrereq.operation.installingGameInput")
        }
    }
}

fn log_color(colors: &ThemeColors, line: &str) -> Hsla {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error")
        || line.contains("失败")
        || line.contains("错误")
        || lower.contains("hresult")
    {
        colors.danger
    } else if lower.contains("warn") || line.contains("警告") {
        colors.stat_orange_text
    } else if line.contains("完成") || line.contains("成功") || lower.contains("done") {
        colors.stat_green_text
    } else {
        colors.text_secondary
    }
}

fn launch_prereq_is_dark(colors: &ThemeColors) -> bool {
    colors.bg.l < 0.45
}

fn launch_prereq_shell_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x111827).into()
        }
    } else {
        Hsla {
            a: 0.98,
            ..rgb(0xffffff).into()
        }
    }
}

fn launch_prereq_shell_border(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.82,
            ..rgb(0x334155).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xe2e8f0).into()
        }
    }
}

fn launch_prereq_shell_shadow(colors: &ThemeColors) -> Vec<BoxShadow> {
    let shadow_color = if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.34,
            ..rgb(0x020617).into()
        }
    } else {
        Hsla {
            a: 0.18,
            ..rgb(0x0f172a).into()
        }
    };

    vec![BoxShadow {
        color: shadow_color,
        blur_radius: px(40.),
        spread_radius: px(-12.),
        offset: point(px(0.), px(18.)),
    }]
}

fn launch_prereq_header_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x172033).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xf8fafc).into()
        }
    }
}

fn launch_prereq_body_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x111827).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xfcfdff).into()
        }
    }
}

fn launch_prereq_footer_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x0f172a).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xf8fafc).into()
        }
    }
}

fn launch_prereq_section_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x0f172a).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xf3f6fb).into()
        }
    }
}

fn launch_prereq_section_border(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.88,
            ..rgb(0x334155).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xe2e8f0).into()
        }
    }
}

fn launch_prereq_section_shadow(colors: &ThemeColors) -> Vec<BoxShadow> {
    let shadow_color = if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.14,
            ..rgb(0x020617).into()
        }
    } else {
        Hsla {
            a: 0.05,
            ..rgb(0x0f172a).into()
        }
    };

    vec![BoxShadow {
        color: shadow_color,
        blur_radius: px(24.),
        spread_radius: px(-16.),
        offset: point(px(0.), px(10.)),
    }]
}

fn launch_prereq_nested_bg(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.98,
            ..rgb(0x162033).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xffffff).into()
        }
    }
}

fn launch_prereq_nested_border(colors: &ThemeColors) -> Hsla {
    if launch_prereq_is_dark(colors) {
        Hsla {
            a: 0.86,
            ..rgb(0x475569).into()
        }
    } else {
        Hsla {
            a: 1.0,
            ..rgb(0xe5edf6).into()
        }
    }
}

fn launch_prereq_issue_row_bg(
    colors: &ThemeColors,
    issue_kind: &crate::utils::mc_dependency::UwpDependencyIssueKind,
) -> Hsla {
    match issue_kind {
        crate::utils::mc_dependency::UwpDependencyIssueKind::Missing => Hsla {
            a: if launch_prereq_is_dark(colors) {
                0.20
            } else {
                0.46
            },
            ..colors.stat_orange_bg
        },
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch { .. } => Hsla {
            a: if launch_prereq_is_dark(colors) {
                0.24
            } else {
                0.40
            },
            ..colors.badge_beta_bg
        },
    }
}

fn launch_prereq_issue_row_border(
    colors: &ThemeColors,
    issue_kind: &crate::utils::mc_dependency::UwpDependencyIssueKind,
) -> Hsla {
    let foreground = match issue_kind {
        crate::utils::mc_dependency::UwpDependencyIssueKind::Missing => colors.stat_orange_text,
        crate::utils::mc_dependency::UwpDependencyIssueKind::VersionMismatch { .. } => {
            colors.badge_beta_text
        }
    };

    Hsla {
        a: if launch_prereq_is_dark(colors) {
            0.30
        } else {
            0.18
        },
        ..foreground
    }
}
