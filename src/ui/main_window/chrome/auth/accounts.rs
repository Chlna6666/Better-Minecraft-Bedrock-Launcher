use super::*;

pub(super) fn list(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let count = state.snapshot.accounts.len().to_string();
    div()
        .mt(px(16.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(t!("Auth.saved_accounts"))
                .child(t!("Auth.account_count", count = &count)),
        )
        .child(
            div()
                .max_h(px(184.))
                .overflow_y_scrollbar()
                .p(px(4.))
                .rounded(px(12.))
                .bg(colors.text_primary.opacity(0.035))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(state.rows.iter().map(|row| account(row, state, colors))),
        )
        .when(state.pending_delete.is_some(), |list| {
            list.child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(0xdc2626))
                    .child(t!("Auth.remove_account_hint")),
            )
        })
        .into_any_element()
}

fn account(row: &Row, state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let profile = &row.profile;
    let active = state.snapshot.active_account_id.as_deref() == Some(profile.xuid.as_str());
    let enabled = state.interactive() && row.present;
    let can_switch = enabled && (!active || state.snapshot.phase == AuthPhase::Error);
    let confirming = state.pending_delete.as_deref() == Some(profile.xuid.as_str());
    div()
        .id(SharedString::from(format!(
            "xbox-account-row-{}",
            profile.xuid
        )))
        .h(px(54.))
        .rounded(px(8.))
        .bg(colors.accent.opacity(0.10 * row.selection))
        .opacity(row.opacity)
        .flex()
        .items_center()
        .gap(px(4.))
        .pr(px(6.))
        .child(
            button(
                SharedString::from(format!("switch-xbox-account-{}", profile.xuid)),
                Action::Switch(profile.xuid.clone()),
                state,
                colors,
                can_switch,
            )
            // A current account is static, not visually disabled.
            .opacity(1.0)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .px(px(9.))
            .child(avatar(Some(profile), colors.text_primary, 32.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(profile.display_name.clone()),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(10.5))
                            .text_color(colors.text_secondary)
                            .child(format!("@{}", profile.gamertag)),
                    ),
            )
            // Keep the indicator slot stable while switching accounts.
            .child(icon(icons::icon_check(), colors.accent, 16.).opacity(row.selection)),
        )
        .when(
            !crate::core::bedrock_auth::is_system_local_account(&profile.xuid),
            |row| {
                row.child(
                    button(
                        SharedString::from(format!("delete-xbox-account-{}", profile.xuid)),
                        Action::Delete(profile.xuid.clone()),
                        state,
                        colors,
                        enabled,
                    )
                    .size(px(30.))
                    .bg(if confirming {
                        Hsla {
                            a: 0.1,
                            ..rgb(0xdc2626).into()
                        }
                    } else {
                        colors.surface.opacity(0.0)
                    })
                    .child(icon(
                        if confirming {
                            icons::icon_check()
                        } else {
                            icons::icon_trash_2()
                        },
                        if confirming {
                            rgb(0xdc2626).into()
                        } else {
                            colors.text_secondary
                        },
                        14.,
                    )),
                )
                .when(confirming, |row| {
                    row.child(
                        button(
                            "cancel-xbox-account-removal",
                            Action::CancelDeletion,
                            state,
                            colors,
                            enabled,
                        )
                        .size(px(26.))
                        .child(icon(
                            icons::icon_x(),
                            colors.text_secondary,
                            14.,
                        )),
                    )
                })
            },
        )
        .into_any_element()
}
