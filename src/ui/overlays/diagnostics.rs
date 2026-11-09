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
    _i18n: &I18n,
    state: &DiagnosticsState,
    auto_sentry_enabled: bool,
) -> Option<AnyElement> {
    // Crash report state must remain independent of recoverable operation errors.
    let crash_report = state.pending_report.is_some();
    let report = state
        .pending_report
        .clone()
        .or_else(|| state.active_operation_report().cloned())?;
    let share_payload = diagnostics_share_payload(&report);
    let card_width = (window_width - px(48.)).max(px(360.)).min(px(760.));
    let card_height = (window_height - px(72.)).max(px(420.)).min(px(720.));
    let overlay_bg = hsla(0.0, 0.0, 0.0, 0.34);

    let title = if crash_report {
        t!("Diagnostics.modal.title")
    } else {
        t!("Diagnostics.modal.operation_title")
    };
    let description = if crash_report {
        t!("Diagnostics.modal.description")
    } else {
        t!("Diagnostics.modal.operation_description")
    };
    let report_id_label = t!("Diagnostics.modal.report_id");
    let detail_label = t!("Diagnostics.modal.detail");
    let log_tail_label = t!("Diagnostics.modal.log_tail");
    let copy_label = t!("Diagnostics.modal.copy");
    let github_label = t!("Diagnostics.modal.github");
    let sentry_label = if auto_sentry_enabled {
        t!("Diagnostics.modal.sentry_auto")
    } else if share_payload.sentry_dsn.is_some() {
        t!("Diagnostics.modal.sentry")
    } else {
        t!("Diagnostics.modal.sentry_unconfigured")
    };
    let dismiss_label = t!("Diagnostics.modal.dismiss");

    let detail_text = crate::utils::log_privacy::redact_log_text(
        &serde_json::to_string_pretty(&report.detail).unwrap_or_else(|_| "{}".to_string())
    ).into_owned();
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
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
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

    let detail_summary = SharedString::from(
        crate::utils::log_privacy::redact_log_text(report_detail_summary(&report).as_ref())
            .into_owned()
    );
    let modal_body = div()
        .w(card_width)
        .max_w(card_width)
        .max_h(card_height)
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                .child(code_panel(
                    log_tail_label,
                    if report.log_tail.is_empty() {
                        t!("Diagnostics.modal.log_tail_export").to_string()
                    } else {
                        crate::utils::log_privacy::redact_log_text(&report.log_tail).into_owned()
                    },
                )),
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
                        .child(t!("Diagnostics.modal.privacy_hint")),
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
                                    t!("Diagnostics.toast.copied"),
                                );
                            }),
                        )
                        .child(
                            action_button(
                                "diagnostics-export",
                                t!("Diagnostics.modal.export_bundle"),
                                colors,
                                false,
                                !state.exporting_bundle,
                            )
                            .on_click({
                                let report = report.clone();
                                move |_event, _window, cx| {
                                    let can_start = cx.update_global(
                                        |state: &mut DiagnosticsState, _cx| {
                                            if state.exporting_bundle {
                                                false
                                            } else {
                                                state.exporting_bundle = true;
                                                true
                                            }
                                        },
                                    );
                                    if !can_start {
                                        return;
                                    }
                                    let report = report.clone();
                                    cx.spawn(async move |cx| {
                                        let result = cx.background_spawn(async move {
                                            crate::utils::diagnostics::export_diagnostics_bundle(
                                                &report
                                            )
                                        }).await;
                                        let _ = cx.update(move |cx| {
                                            cx.update_global(
                                                |state: &mut DiagnosticsState, _cx| {
                                                    state.exporting_bundle = false;
                                                },
                                            );
                                            match result {
                                                Ok(path) => {
                                                    toast::success(
                                                        cx,
                                                        t!("Diagnostics.toast.bundle_saved"),
                                                    );
                                                    if let Some(directory) = path.parent() {
                                                        let directory =
                                                            directory.to_string_lossy().to_string();
                                                        cx.spawn(async move |_cx| {
                                                            let _ = crate::utils::open_path::open_path(
                                                                directory
                                                            ).await;
                                                            Ok::<(), anyhow::Error>(())
                                                        }).detach();
                                                    }
                                                }
                                                Err(error) => {
                                                    let safe = crate::utils::log_privacy::redact_log_text(
                                                        &error.to_string()
                                                    ).into_owned();
                                                    tracing::error!(
                                                        target: "bmcbl::diagnostics",
                                                        reason = %safe,
                                                        "导出诊断包失败"
                                                    );
                                                    toast::error(
                                                        cx,
                                                        SharedString::from(format!(
                                                            "诊断包导出失败：{safe}"
                                                        )),
                                                    );
                                                }
                                            }
                                        });
                                        Ok::<(), anyhow::Error>(())
                                    }).detach();
                                }
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
                                    t!("Diagnostics.toast.github_opened"),
                                );
                            }),
                        )
                        .child(
                            if crash_report {
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
                                            t!("Diagnostics.toast.sentry_unconfigured"),
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
                                                            t!("common.save_failed")
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
                                                    t!("Diagnostics.toast.sentry_success"),
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
                                                        t!("Diagnostics.toast.sentry_failed")
                                                    )),
                                                );
                                            }
                                        });

                                        Ok::<(), anyhow::Error>(())
                                    })
                                    .detach();
                                }
                            }).into_any_element()
                            } else {
                                div().into_any_element()
                            },
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
                                if crash_report {
                                    if let Err(error) =
                                        crate::utils::diagnostics::acknowledge_pending_report()
                                    {
                                        toast::error(
                                            cx,
                                            SharedString::from(format!(
                                                "{}: {error:#}",
                                                t!("common.save_failed")
                                            )),
                                        );
                                        return;
                                    }
                                    clear_diagnostics_marker_files();
                                    cx.update_global(
                                        |state: &mut DiagnosticsState, _cx| {
                                            state.clear();
                                        },
                                    );
                                } else {
                                    cx.update_global(
                                        |state: &mut DiagnosticsState, _cx| {
                                            state.close_operation_report();
                                        },
                                    );
                                }
                            }),
                        ),
                ),
        );

    Some(modal::modal_layer(modal_body, overlay_bg).into_any_element())
}

pub fn trigger_auto_sentry_submit_if_needed(cx: &mut App) {
    let auto_sentry_enabled = cx
        .global::<crate::ui::views::settings::state::SettingsPageState>()
        .error_report_sentry_auto;
    let report = {
        let state = cx.global::<DiagnosticsState>();
        if state.auto_report_attempted || state.submitting_sentry || !auto_sentry_enabled {
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
                        SharedString::from(format!("{}: {error:#}", t!("common.save_failed"))),
                    );
                }
                clear_diagnostics_marker_files();
                cx.update_global(|diagnostics_state: &mut DiagnosticsState, _cx| {
                    diagnostics_state.clear();
                });
                toast::success(cx, t!("Diagnostics.toast.sentry_success"));
            }
            Err(error) => {
                cx.update_global(|diagnostics_state: &mut DiagnosticsState, _cx| {
                    diagnostics_state.submitting_sentry = false;
                });
                toast::error(
                    cx,
                    SharedString::from(format!(
                        "{}: {error:#}",
                        t!("Diagnostics.toast.sentry_failed")
                    )),
                );
            }
        });

        Ok::<(), anyhow::Error>(())
    })
    .detach();
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
        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
