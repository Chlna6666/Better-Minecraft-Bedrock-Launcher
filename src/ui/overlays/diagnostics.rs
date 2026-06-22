use crate::ui::components::button::Button;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toast;
use crate::ui::state::diagnostics::DiagnosticsState;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::utils::diagnostics::{
    DiagnosticsDetail, DiagnosticsReport, diagnostics_share_payload, submit_report_to_sentry,
};
use gpui::*;

pub fn render_diagnostics_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    i18n: &I18n,
    state: &DiagnosticsState,
) -> Option<AnyElement> {
    let report = state.pending_report.clone()?;
    let share_payload = diagnostics_share_payload(&report);
    let auto_sentry_enabled = sentry_auto_enabled();
    let card_width = (window_width - px(48.)).max(px(360.)).min(px(760.));
    let card_height = (window_height - px(72.)).max(px(420.)).min(px(720.));
    let overlay_bg = hsla(0.0, 0.0, 0.0, 0.34);

    let title = i18n.t("Diagnostics.modal.title");
    let description = i18n.t("Diagnostics.modal.description");
    let report_id_label = i18n.t("Diagnostics.modal.report_id");
    let detail_label = i18n.t("Diagnostics.modal.detail");
    let log_tail_label = i18n.t("Diagnostics.modal.log_tail");
    let copy_label = i18n.t("Diagnostics.modal.copy");
    let github_label = i18n.t("Diagnostics.modal.github");
    let sentry_label = if auto_sentry_enabled {
        i18n.t("Diagnostics.modal.sentry_auto")
    } else if share_payload.sentry_dsn.is_some() {
        i18n.t("Diagnostics.modal.sentry")
    } else {
        i18n.t("Diagnostics.modal.sentry_unconfigured")
    };
    let dismiss_label = i18n.t("Diagnostics.modal.dismiss");

    let detail_text =
        serde_json::to_string_pretty(&report.detail).unwrap_or_else(|_| "{}".to_string());
    let code_panel = |heading: SharedString, body: String| {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_secondary)
                    .child(heading),
            )
            .child(
                div()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.18,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.72,
                        ..colors.surface
                    })
                    .p(px(12.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_family("Consolas")
                            .whitespace_normal()
                            .text_color(colors.text_primary)
                            .child(body),
                    ),
            )
    };

    let detail_summary = report_detail_summary(&report);
    let modal_body = div()
        .w(card_width)
        .max_w(card_width)
        .max_h(card_height)
        .rounded(px(20.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .p(px(22.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
        .child(
            div()
                .rounded(px(14.))
                .border_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.68,
                    ..colors.surface
                })
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child(report_id_label),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_family("Consolas")
                        .whitespace_normal()
                        .text_color(colors.text_primary)
                        .child(report.id.clone()),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(colors.text_primary)
                        .child(detail_summary),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(code_panel(detail_label, detail_text))
                .child(code_panel(log_tail_label, report.log_tail.clone())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .whitespace_normal()
                        .text_color(colors.text_muted)
                        .child(i18n.t("Diagnostics.modal.privacy_hint")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_end()
                        .gap(px(10.))
                        .child(
                            action_button(
                                "diagnostics-copy",
                                copy_label,
                                colors,
                                false,
                                true,
                            )
                            .on_click(move |_event, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    share_payload.body_markdown.clone(),
                                ));
                                toast::success(
                                    cx,
                                    cx.global::<I18n>().t("Diagnostics.toast.copied"),
                                );
                            }),
                        )
                        .child(
                            action_button(
                                "diagnostics-github",
                                github_label,
                                colors,
                                false,
                                true,
                            )
                            .on_click(move |_event, _window, cx| {
                                cx.open_url(&share_payload.github_issue_url);
                                toast::success(
                                    cx,
                                    cx.global::<I18n>().t("Diagnostics.toast.github_opened"),
                                );
                            }),
                        )
                        .child(
                            action_button(
                                "diagnostics-sentry",
                                sentry_label,
                                colors,
                                true,
                                !auto_sentry_enabled
                                    && share_payload.sentry_dsn.is_some()
                                    && !state.submitting_sentry,
                            )
                            .on_click({
                                let report = report.clone();
                                let dsn = share_payload.sentry_dsn.clone();
                                move |_event, _window, cx| {
                                    let Some(dsn) = dsn.clone() else {
                                        toast::error(
                                            cx,
                                            cx.global::<I18n>()
                                                .t("Diagnostics.toast.sentry_unconfigured"),
                                        );
                                        return;
                                    };

                                    cx.update_global(
                                        |diagnostics_state: &mut DiagnosticsState, _cx| {
                                            diagnostics_state.submitting_sentry = true;
                                        },
                                    );
                                    let report = report.clone();
                                    cx.spawn(async move |cx| {
                                        let result = cx
                                            .background_spawn(async move {
                                                submit_report_to_sentry(&report, &dsn)
                                            })
                                            .await;

                                        let _ = cx.update(move |cx| match result {
                                            Ok(()) => {
                                                if let Err(error) =
                                                    crate::utils::diagnostics::acknowledge_pending_report()
                                                {
                                                    toast::error(
                                                        cx,
                                                        SharedString::from(format!(
                                                            "{}: {error:#}",
                                                            cx.global::<I18n>()
                                                                .t("common.save_failed")
                                                        )),
                                                    );
                                                }
                                                clear_diagnostics_marker_files();
                                                cx.update_global(
                                                    |diagnostics_state: &mut DiagnosticsState, _cx| {
                                                        diagnostics_state.clear();
                                                    },
                                                );
                                                toast::success(
                                                    cx,
                                                    cx.global::<I18n>()
                                                        .t("Diagnostics.toast.sentry_success"),
                                                );
                                            }
                                            Err(error) => {
                                                cx.update_global(
                                                    |diagnostics_state: &mut DiagnosticsState, _cx| {
                                                        diagnostics_state.submitting_sentry = false;
                                                    },
                                                );
                                                toast::error(
                                                    cx,
                                                    SharedString::from(format!(
                                                        "{}: {error:#}",
                                                        cx.global::<I18n>()
                                                            .t("Diagnostics.toast.sentry_failed")
                                                    )),
                                                );
                                            }
                                        });

                                        Ok::<(), anyhow::Error>(())
                                    })
                                    .detach();
                                }
                            }),
                        )
                        .child(
                            action_button(
                                "diagnostics-dismiss",
                                dismiss_label,
                                colors,
                                false,
                                true,
                            )
                            .on_click(move |_event, _window, cx| {
                                if let Err(error) = crate::utils::diagnostics::acknowledge_pending_report()
                                {
                                    toast::error(
                                        cx,
                                        SharedString::from(format!(
                                            "{}: {error:#}",
                                            cx.global::<I18n>().t("common.save_failed")
                                        )),
                                    );
                                    return;
                                }
                                clear_diagnostics_marker_files();
                                cx.update_global(
                                    |diagnostics_state: &mut DiagnosticsState, _cx| {
                                        diagnostics_state.clear();
                                    },
                                );
                            }),
                        ),
                ),
        );

    Some(modal::modal_layer(modal_body, overlay_bg).into_any_element())
}

pub fn trigger_auto_sentry_submit_if_needed(cx: &mut App) {
    let report = {
        let state = cx.global::<DiagnosticsState>();
        if state.auto_report_attempted || state.submitting_sentry || !sentry_auto_enabled() {
            None
        } else {
            state.pending_report.clone()
        }
    };

    let Some(report) = report else {
        return;
    };
    let share_payload = diagnostics_share_payload(&report);
    let Some(dsn) = share_payload.sentry_dsn else {
        return;
    };

    cx.update_global(|diagnostics_state: &mut DiagnosticsState, _cx| {
        diagnostics_state.auto_report_attempted = true;
        diagnostics_state.submitting_sentry = true;
    });

    cx.spawn(async move |cx| {
        let result = cx
            .background_spawn(async move { submit_report_to_sentry(&report, &dsn) })
            .await;

        let _ = cx.update(move |cx| match result {
            Ok(()) => {
                if let Err(error) = crate::utils::diagnostics::acknowledge_pending_report() {
                    toast::error(
                        cx,
                        SharedString::from(format!(
                            "{}: {error:#}",
                            cx.global::<I18n>().t("common.save_failed")
                        )),
                    );
                }
                clear_diagnostics_marker_files();
                cx.update_global(|diagnostics_state: &mut DiagnosticsState, _cx| {
                    diagnostics_state.clear();
                });
                toast::success(
                    cx,
                    cx.global::<I18n>().t("Diagnostics.toast.sentry_success"),
                );
            }
            Err(error) => {
                cx.update_global(|diagnostics_state: &mut DiagnosticsState, _cx| {
                    diagnostics_state.submitting_sentry = false;
                });
                toast::error(
                    cx,
                    SharedString::from(format!(
                        "{}: {error:#}",
                        cx.global::<I18n>().t("Diagnostics.toast.sentry_failed")
                    )),
                );
            }
        });

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn sentry_auto_enabled() -> bool {
    match crate::config::config::read_config() {
        Ok(config) => crate::config::config::error_report_sentry_auto_enabled(&config.launcher),
        Err(_) => false,
    }
}

fn clear_diagnostics_marker_files() {
    if let Err(error) = crate::utils::diagnostics::clear_session_marker() {
        tracing::warn!(?error, "failed to clear diagnostics session marker");
    }
    if let Err(error) = crate::utils::diagnostics::clear_crash_signal() {
        tracing::warn!(?error, "failed to clear diagnostics crash signal");
    }
}

fn action_button(
    id: &'static str,
    label: SharedString,
    colors: &ThemeColors,
    primary: bool,
    enabled: bool,
) -> Button {
    let mut button = Button::new(id)
        .h(px(38.))
        .px(px(16.))
        .rounded(px(10.))
        .border_1()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .label(label);

    button = if primary {
        button
            .bg(colors.accent)
            .border_color(colors.accent)
            .text_color(colors.btn_primary_text)
    } else {
        button
            .bg(Hsla {
                a: 0.08,
                ..colors.text_secondary
            })
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .text_color(colors.text_primary)
    };

    if !enabled {
        button = button.opacity(0.58);
    }

    button
}

fn report_detail_summary(report: &DiagnosticsReport) -> SharedString {
    let body = match &report.detail {
        DiagnosticsDetail::Panic {
            location,
            payload,
            backtrace: _,
        } => format!(
            "panic at {}: {}",
            location.as_deref().unwrap_or("unknown"),
            payload
        ),
        DiagnosticsDetail::UnhandledException { code, address } => {
            format!("windows exception {code} at {address}")
        }
        DiagnosticsDetail::UnexpectedExit { reason } => format!("unexpected exit: {reason}"),
        DiagnosticsDetail::StartupFailure { stage, error } => {
            format!("startup failure at {stage}: {error}")
        }
        DiagnosticsDetail::ApplicationError { stage, error } => {
            format!("application error at {stage}: {error}")
        }
    };
    SharedString::from(body)
}
