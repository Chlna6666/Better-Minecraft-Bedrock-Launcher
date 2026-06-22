use crate::config::config;
use crate::ui::components::markdown_renderer::{MarkdownDocument, render_markdown_document};
use crate::ui::components::modal;
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
                .rounded(px(12.))
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
                    .rounded(px(10.))
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

    let scroll_area = div()
        .id("agreement-scroll")
        .size_full()
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
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
        .pt(px(16.))
        .pb(px(10.))
        .child(scroll_area);

    let accept_button = if accept_unlocked {
        div()
            .w_full()
            .h(px(48.))
            .rounded(px(12.))
            .bg(colors.accent)
            .border_1()
            .border_color(colors.accent)
            .cursor_pointer()
            .hover(|s| s.bg(colors.accent_hover))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD)
            .text_color(hsla(0., 0., 1., 1.0))
            .child(accept_label)
            .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                cx.update_global(|agreement: &mut AgreementState, cx| {
                    agreement.accept();
                });

                tokio::spawn(async {
                    let result = tokio::task::spawn_blocking(|| {
                        config::update_config(|cfg| {
                            cfg.agreement_accepted = true;
                        })
                    })
                    .await;

                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            eprintln!("persist agreement_accepted failed: {error}");
                        }
                        Err(join_error) => {
                            eprintln!("persist agreement_accepted join error: {join_error}");
                        }
                    }
                });
            })
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(48.))
            .rounded(px(12.))
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
        .rounded(px(22.))
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
                a: 0.35,
            },
            blur_radius: px(40.),
            spread_radius: px(0.),
            offset: point(px(0.), px(16.)),
        }])
        .flex()
        .flex_col()
        .child(header)
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
