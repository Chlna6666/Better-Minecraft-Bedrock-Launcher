use super::*;
use std::time::Duration;

pub(super) fn body(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let body = match state.snapshot.phase {
        AuthPhase::SignedIn | AuthPhase::SignedOut => add_account(state, colors),
        AuthPhase::WaitingForUser => device_code(state, colors),
        AuthPhase::Error => error(state, colors),
        _ => busy(state, colors),
    };
    div().mt(px(16.)).child(body).into_any_element()
}

fn add_account(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    button(
        "add-xbox-account",
        Action::Login,
        state,
        colors,
        state.interactive(),
    )
    .w_full()
    .h(px(40.))
    .bg(colors.accent.opacity(0.08))
    .text_color(colors.accent)
    .font_weight(FontWeight::SEMIBOLD)
    .child(icon(icons::icon_user_plus(), colors.accent, 16.))
    .child(if state.snapshot.accounts.is_empty() {
        t!("Auth.login_with_microsoft")
    } else {
        t!("Auth.add_microsoft_account")
    })
    .into_any_element()
}

fn device_code(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let has_code = state
        .snapshot
        .user_code
        .as_ref()
        .is_some_and(|code| !code.is_empty());
    let has_link = state
        .snapshot
        .verification_url
        .as_ref()
        .is_some_and(|url| !url.is_empty());
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .when(state.snapshot.profile.is_some(), |body| {
            body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(colors.text_secondary)
                    .child(t!("Auth.waiting_confirmation_hint")),
            )
        })
        .child(
            div()
                .h(px(48.))
                .px(px(12.))
                .rounded(px(10.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.text_primary.opacity(0.035))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(state.snapshot.user_code.clone().unwrap_or_default()),
                )
                .child(
                    button(
                        "copy-xbox-device-code",
                        Action::CopyCode,
                        state,
                        colors,
                        state.open && has_code,
                    )
                    .size(px(34.))
                    .child(icon(
                        if state.copied == Some("code") {
                            icons::icon_check()
                        } else {
                            icons::icon_copy()
                        },
                        colors.accent,
                        16.,
                    )),
                ),
        )
        .child(
            button(
                "open-xbox-login",
                Action::OpenLink,
                state,
                colors,
                state.open && has_link,
            )
            .h(px(40.))
            .bg(colors.accent.opacity(0.12))
            .child(icon(icons::icon_external_link(), colors.accent, 16.))
            .child(t!("Auth.open_login_page")),
        )
        .child(
            button(
                "copy-xbox-login-link",
                Action::CopyLink,
                state,
                colors,
                state.open && has_link,
            )
            .h(px(38.))
            .border_1()
            .border_color(colors.border)
            .child(icon(
                if state.copied == Some("link") {
                    icons::icon_check()
                } else {
                    icons::icon_link()
                },
                colors.text_primary,
                15.,
            ))
            .child(t!("Auth.copy_login_link")),
        )
        .child(
            button(
                "cancel-xbox-login",
                Action::CancelLogin,
                state,
                colors,
                state.open,
            )
            .h(px(32.))
            .child(t!("Auth.cancel_login")),
        )
        .into_any_element()
}

fn error(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .p(px(12.))
                .rounded(px(9.))
                .bg(Hsla {
                    a: 0.08,
                    ..rgb(0xdc2626).into()
                })
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(rgb(0xdc2626))
                .child(
                    state
                        .snapshot
                        .error
                        .clone()
                        .unwrap_or_else(|| t!("Auth.login_error").to_string()),
                ),
        )
        .child(
            button(
                "retry-xbox-login",
                Action::Retry,
                state,
                colors,
                state.interactive(),
            )
            .h(px(38.))
            .bg(colors.accent.opacity(0.12))
            .child(t!("Auth.retry_login")),
        )
        .when(!state.snapshot.accounts.is_empty(), |panel| {
            panel.child(add_account(state, colors))
        })
        .into_any_element()
}

fn busy(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let spinner = icon(icons::icon_loader_circle(), colors.accent, 16.);
    let spinner = if state.reduced_motion {
        spinner.into_any_element()
    } else {
        spinner
            .with_animation(
                "xbox-auth-busy-spinner",
                crate::ui::animation::repeating_linear_motion(Duration::from_millis(900)),
                |icon, progress| {
                    icon.with_transformation(Transformation::rotate(radians(
                        progress * std::f32::consts::TAU,
                    )))
                },
            )
            .into_any_element()
    };
    div()
        .h(px(42.))
        .rounded(px(10.))
        .bg(colors.accent.opacity(0.08))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(9.))
        .text_size(px(12.))
        .text_color(colors.accent)
        .child(spinner)
        .child(status_label(&state.snapshot))
        .into_any_element()
}
