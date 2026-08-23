use crate::config::agreement;
use crate::ui::components::markdown_renderer::{MarkdownDocument, render_markdown_document};
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::agreement::AgreementState;
use crate::ui::theme::{DarkColors, LightColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct UserAgreementModalOptions {
    pub show_close_button: bool,
    pub show_accept_button: bool,
    pub on_close: Option<Rc<dyn Fn(&mut App)>>,
}

impl UserAgreementModalOptions {
    pub fn required_acceptance() -> Self {
        Self {
            show_close_button: false,
            show_accept_button: true,
            on_close: None,
        }
    }

    pub fn read_only(on_close: Rc<dyn Fn(&mut App)>) -> Self {
        Self {
            show_close_button: true,
            show_accept_button: false,
            on_close: Some(on_close),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgreementScrollAction {
    Top,
    PageUp,
    PageDown,
    Bottom,
}

fn agreement_scroll_progress(handle: &ScrollHandle) -> f32 {
    let viewport_height = handle.bounds().size.height / px(1.0);
    if viewport_height <= 1.0 {
        return 0.0;
    }

    let max_scroll = handle.max_offset().height / px(1.0);
    if max_scroll <= 1.0 {
        return 1.0;
    }

    let current_scroll = -(handle.offset().y / px(1.0));
    (current_scroll / max_scroll).clamp(0.0, 1.0)
}

fn apply_agreement_scroll_action(
    handle: &ScrollHandle,
    action: AgreementScrollAction,
    window: &mut Window,
    check_acceptance: bool,
) {
    let current = handle.offset();
    let viewport_height = (handle.bounds().size.height / px(1.0)).max(1.0);
    let max_scroll = (handle.max_offset().height / px(1.0)).max(0.0);
    let current_scroll = -(current.y / px(1.0));
    let page_step = (viewport_height * 0.82).max(120.0);

    let target_scroll = match action {
        AgreementScrollAction::Top => 0.0,
        AgreementScrollAction::PageUp => (current_scroll - page_step).max(0.0),
        AgreementScrollAction::PageDown => (current_scroll + page_step).min(max_scroll),
        AgreementScrollAction::Bottom => max_scroll,
    };

    handle.set_offset(point(current.x, px(-target_scroll)));
    window.refresh();

    if check_acceptance {
        window.on_next_frame(|_window, cx| {
            cx.update_global(|agreement: &mut AgreementState, _cx| {
                agreement.unlock_accept_if_scrolled_to_end();
            });
        });
    }
}

fn scroll_action_button(
    id: &'static str,
    action: AgreementScrollAction,
    handle: ScrollHandle,
    colors: crate::ui::theme::colors::ThemeColors,
    check_acceptance: bool,
) -> impl IntoElement {
    let icon_path = match action {
        AgreementScrollAction::Top | AgreementScrollAction::PageUp => {
            lucide_icons::icon_chevron_up()
        }
        AgreementScrollAction::PageDown | AgreementScrollAction::Bottom => {
            lucide_icons::icon_chevron_down()
        }
    };
    let has_edge_line = matches!(action, AgreementScrollAction::Top | AgreementScrollAction::Bottom);
    let edge_line_before = action == AgreementScrollAction::Top;

    div()
        .id(id)
        .flex_none()
        .size(px(30.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.60,
            ..colors.border
        })
        .bg(colors.surface)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.0))
        .when(has_edge_line && edge_line_before, |this| {
            this.child(
                div()
                    .w(px(10.0))
                    .h(px(1.0))
                    .rounded(px(1.0))
                    .bg(colors.text_muted),
            )
        })
        .child(
            svg()
                .path(icon_path)
                .size(px(13.0))
                .text_color(colors.text_secondary),
        )
        .when(has_edge_line && !edge_line_before, |this| {
            this.child(
                div()
                    .w(px(10.0))
                    .h(px(1.0))
                    .rounded(px(1.0))
                    .bg(colors.text_muted),
            )
        })
        .on_mouse_down(MouseButton::Left, move |_event, window, _cx| {
            apply_agreement_scroll_action(&handle, action, window, check_acceptance);
        })
}

pub fn render_user_agreement_modal(
    markdown_document: Arc<MarkdownDocument>,
    window_width: Pixels,
    window_height: Pixels,
    theme_factor: f32,
    accent_override: Option<Hsla>,
    title: SharedString,
    accept_label: SharedString,
    agreement_scroll_handle: ScrollHandle,
    accept_unlocked: bool,
    options: UserAgreementModalOptions,
) -> impl IntoElement {
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme_factor,
        accent_override,
    );
    let card_w = (window_width - px(40.)).max(px(360.)).min(px(560.));
    let card_h = px(((window_height / px(1.)) * 0.82).clamp(420.0, 700.0));
    let content = render_markdown_document(markdown_document.as_ref(), &colors, theme_factor > 0.5);
    let overlay_bg = hsla(0., 0., 0.12, 0.30);
    let scroll_progress = agreement_scroll_progress(&agreement_scroll_handle);

    let mut header = div()
        .px(px(24.))
        .pt(px(22.))
        .pb(px(14.))
        .flex()
        .items_center()
        .gap(px(12.))
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .w(px(38.))
                .h(px(38.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    h: colors.accent.h,
                    s: colors.accent.s,
                    l: colors.accent.l,
                    a: 0.16,
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_shield_check())
                        .w(px(20.))
                        .h(px(20.))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(title),
        );

    if options.show_close_button {
        if let Some(on_close) = options.on_close.clone() {
            header = header.child(div().flex_1()).child(
                div()
                    .id("agreement-close")
                    .w(px(34.))
                    .h(px(34.))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface)
                    .cursor_pointer()
                    .hover(|this| this.bg(colors.surface_hover))
                    .child(
                        svg()
                            .path(lucide_icons::icon_x())
                            .w(px(16.))
                            .h(px(16.))
                            .text_color(colors.text_primary),
                    )
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        (on_close)(cx);
                    }),
            );
        }
    }

    let quick_scroll = div()
        .px(px(24.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(Hsla {
            a: 0.70,
            ..colors.border
        })
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .flex_1()
                .min_w(px(72.0))
                .h(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(colors.progress_track)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(scroll_progress))
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(colors.accent),
                ),
        )
        .child(
            div()
                .flex_none()
                .w(px(38.0))
                .text_right()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_muted)
                .child(format!("{:.0}%", scroll_progress * 100.0)),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(scroll_action_button(
                    "agreement-scroll-top",
                    AgreementScrollAction::Top,
                    agreement_scroll_handle.clone(),
                    colors,
                    options.show_accept_button,
                ))
                .child(scroll_action_button(
                    "agreement-scroll-page-up",
                    AgreementScrollAction::PageUp,
                    agreement_scroll_handle.clone(),
                    colors,
                    options.show_accept_button,
                ))
                .child(scroll_action_button(
                    "agreement-scroll-page-down",
                    AgreementScrollAction::PageDown,
                    agreement_scroll_handle.clone(),
                    colors,
                    options.show_accept_button,
                ))
                .child(scroll_action_button(
                    "agreement-scroll-bottom",
                    AgreementScrollAction::Bottom,
                    agreement_scroll_handle.clone(),
                    colors,
                    options.show_accept_button,
                )),
        );

    let scroll_area = div()
        .id("agreement-scroll")
        .size_full()
        .overflow_y_scrollbar()
        .scrollbar_width(px(10.0))
        .track_scroll(&agreement_scroll_handle)
        .when(options.show_accept_button, |this| {
            this.on_scroll_wheel(|_, window, _cx| {
                window.on_next_frame(|_window, cx| {
                    cx.update_global(|agreement: &mut AgreementState, _cx| {
                        agreement.unlock_accept_if_scrolled_to_end();
                    });
                });
            })
            .on_mouse_up(MouseButton::Left, |_, window, _cx| {
                window.on_next_frame(|_, cx| {
                    cx.update_global(|agreement: &mut AgreementState, _cx| {
                        agreement.unlock_accept_if_scrolled_to_end();
                    });
                });
            })
        })
        .child(
            div()
                .pr(px(10.0))
                .text_size(px(14.))
                .line_height(px(22.))
                .text_color(colors.text_secondary)
                .pb(px(12.))
                .child(content),
        );

    let body = div()
        .flex_1()
        .min_h(px(0.))
        .px(px(24.))
        .pt(px(12.))
        .pb(px(10.))
        .child(scroll_area);

    let accept_button = if accept_unlocked {
        div()
            .id("user-agreement-accept-button")
            .w_full()
            .h(px(48.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .bg(colors.accent)
            .border_1()
            .border_color(colors.accent)
            .cursor_pointer()
            .hover(|s| s.bg(colors.accent_hover))
            .active(|s| s.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.btn_primary_text)
            .child(accept_label)
            .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                cx.update_global(|agreement: &mut AgreementState, cx| {
                    agreement.accept();
                });

                if let Err(error) = crate::tasks::runtime::spawn_io(async {
                    let result =
                        crate::tasks::runtime::run_io_blocking(agreement::accept_current_agreement)
                            .await;

                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            eprintln!("persist agreement version failed: {error}");
                        }
                        Err(join_error) => {
                            eprintln!("persist agreement version join error: {join_error}");
                        }
                    }
                }) {
                    tracing::error!(%error, "failed to schedule agreement version persistence");
                }
            })
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(48.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.text_muted)
            .child(accept_label)
            .into_any_element()
    };

    let footer = div()
        .px(px(24.))
        .pt(px(14.))
        .pb(px(22.))
        .child(accept_button);

    let card = div()
        .w(card_w)
        .h(card_h)
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .occlude()
        .bg(colors.bg)
        .border_1()
        .border_color(colors.border)
        .shadow(vec![BoxShadow {
            color: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.16,
            },
            blur_radius: px(24.),
            spread_radius: px(0.),
            offset: point(px(0.), px(12.)),
        }])
        .flex()
        .flex_col()
        .child(header)
        .child(quick_scroll)
        .child(body)
        .when(options.show_accept_button, |this| this.child(footer));

    div()
        .absolute()
        .inset_0()
        .child(modal::modal_backdrop(overlay_bg))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(card),
        )
}
