use super::TopbarRenderState;
use crate::core::bedrock_auth::{AuthPhase, AuthSnapshot, XboxProfile};
use crate::ui::components::scroll::ScrollableElement;
use crate::ui::navigation::{self, AppRoute, RouteTarget};
use crate::ui::state::bedrock_auth::BedrockAuthState;
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{DarkColors, LightColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::{Duration, Instant};

pub(crate) struct AppChromeState {
    pub(crate) titlebar_gesture: crate::ui::window::chrome::TitlebarGestureState,
}

impl Default for AppChromeState {
    fn default() -> Self {
        Self {
            titlebar_gesture: crate::ui::window::chrome::TitlebarGestureState::default(),
        }
    }
}

impl Global for AppChromeState {}

#[derive(Clone)]
struct NavItem {
    icon_path: &'static str,
    image_icon_path: Option<std::path::PathBuf>,
    label: SharedString,
    target: RouteTarget,
}

fn icon(path: &'static str, color: Hsla, size: Pixels) -> Svg {
    svg().path(path).size(size).text_color(color)
}

fn status_label(snapshot: &AuthSnapshot) -> SharedString {
    match snapshot.phase {
        AuthPhase::SignedIn => snapshot
            .profile
            .as_ref()
            .map(|profile| SharedString::from(profile.gamertag.clone()))
            .unwrap_or_else(|| SharedString::from("Xbox")),
        AuthPhase::Restoring => SharedString::from("恢复登录…"),
        AuthPhase::RequestingCode => SharedString::from("连接中…"),
        AuthPhase::WaitingForUser => SharedString::from("等待确认"),
        AuthPhase::AuthenticatingXbox => SharedString::from("Xbox 验证…"),
        AuthPhase::SwitchingAccount => SharedString::from("切换账号…"),
        AuthPhase::SigningOut => SharedString::from("正在退出…"),
        AuthPhase::Error => SharedString::from("登录失败"),
        AuthPhase::SignedOut => SharedString::from("登录 Xbox"),
    }
}

fn avatar(snapshot: &AuthSnapshot, foreground: Hsla, size: Pixels) -> AnyElement {
    snapshot
        .profile
        .as_ref()
        .and_then(|profile| profile.avatar_url.as_ref())
        .map_or_else(
            || {
                div()
                    .size(size)
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(foreground.opacity(0.08))
                    .child(icon(
                        lucide_icons::icon_circle_user_round(),
                        foreground,
                        px(18.),
                    ))
                    .into_any_element()
            },
            |url| {
                img(SharedString::from(url.clone()))
                    .size(size)
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .decode_to_bounds()
                    .into_any_element()
            },
        )
}

fn account_avatar(profile: &XboxProfile, foreground: Hsla, size: Pixels) -> AnyElement {
    profile.avatar_url.as_ref().map_or_else(
        || {
            div()
                .size(size)
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(foreground.opacity(0.08))
                .child(icon(
                    lucide_icons::icon_circle_user_round(),
                    foreground,
                    px(16.),
                ))
                .into_any_element()
        },
        |url| {
            img(SharedString::from(url.clone()))
                .size(size)
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .decode_to_bounds()
                .into_any_element()
        },
    )
}

fn account_list(
    snapshot: &AuthSnapshot,
    pending_delete_account_id: Option<&str>,
    text: Hsla,
    muted: Hsla,
    accent: Hsla,
    border: Hsla,
) -> AnyElement {
    let interactions_enabled = matches!(
        snapshot.phase,
        AuthPhase::SignedOut | AuthPhase::SignedIn | AuthPhase::Error
    );
    let active_account_id = snapshot.active_account_id.as_deref();
    let pending_delete = pending_delete_account_id.is_some();

    div()
        .mt(px(16.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(11.))
                .text_color(muted)
                .child("已保存账号")
                .child(format!("{} 个", snapshot.accounts.len())),
        )
        .child(
            div()
                .max_h(px(184.))
                .overflow_y_scrollbar()
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(snapshot.accounts.iter().cloned().map(|profile| {
                    let is_active = active_account_id == Some(profile.xuid.as_str());
                    let can_switch =
                        interactions_enabled && (!is_active || snapshot.phase == AuthPhase::Error);
                    let switch_account_id = profile.xuid.clone();
                    let delete_account_id = profile.xuid.clone();
                    let delete_confirmation_active =
                        pending_delete_account_id == Some(profile.xuid.as_str());
                    div()
                        .id(SharedString::from(format!(
                            "xbox-account-row-{}",
                            profile.xuid
                        )))
                        .h(px(54.))
                        .px(px(10.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(if is_active {
                            accent.opacity(0.46)
                        } else {
                            border
                        })
                        .bg(if is_active {
                            accent.opacity(0.075)
                        } else {
                            text.opacity(0.02)
                        })
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .when(can_switch, |row| {
                            row.cursor_pointer()
                                .hover(|style| style.bg(text.opacity(0.05)))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                    if let Err(error) = crate::core::bedrock_auth::switch_account(
                                        switch_account_id.clone(),
                                    ) {
                                        tracing::warn!(%error, "could not switch Xbox account");
                                    }
                                    cx.update_global(|state: &mut BedrockAuthState, _cx| {
                                        state.clear_account_deletion();
                                    });
                                })
                        })
                        .child(account_avatar(&profile, text, px(32.)))
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex_1()
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
                                        .text_color(text)
                                        .child(profile.display_name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(muted)
                                        .child(format!("@{}", profile.gamertag)),
                                ),
                        )
                        .when(is_active, |row| {
                            row.child(
                                div()
                                    .px(px(7.))
                                    .py(px(3.))
                                    .rounded_full()
                                    .bg(accent.opacity(0.12))
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(accent)
                                    .child("当前"),
                            )
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "delete-xbox-account-{}",
                                    profile.xuid
                                )))
                                .size(px(30.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(if delete_confirmation_active {
                                    rgb(0xdc2626).into()
                                } else {
                                    muted
                                })
                                .when(interactions_enabled, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgba(0xdc262614)))
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            cx.stop_propagation();
                                            let confirmed = cx
                                                .global::<BedrockAuthState>()
                                                .pending_delete_account_id
                                                .as_deref()
                                                == Some(delete_account_id.as_str());
                                            if confirmed {
                                                if let Err(error) =
                                                    crate::core::bedrock_auth::remove_account(
                                                        delete_account_id.clone(),
                                                    )
                                                {
                                                    tracing::warn!(
                                                        %error,
                                                        "could not remove Xbox account"
                                                    );
                                                }
                                                cx.update_global(
                                                    |state: &mut BedrockAuthState, _cx| {
                                                        state.clear_account_deletion();
                                                    },
                                                );
                                            } else {
                                                cx.update_global(
                                                    |state: &mut BedrockAuthState, _cx| {
                                                        state.request_account_deletion(
                                                            delete_account_id.clone(),
                                                        );
                                                    },
                                                );
                                            }
                                        })
                                })
                                .child(icon(
                                    lucide_icons::icon_trash_2(),
                                    if delete_confirmation_active {
                                        rgb(0xdc2626).into()
                                    } else {
                                        muted
                                    },
                                    px(14.),
                                )),
                        )
                })),
        )
        .when(pending_delete, |list| {
            list.child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(0xdc2626))
                    .child("再次点击红色删除按钮以移除账号和加密凭证"),
            )
        })
        .into_any_element()
}

fn auth_panel(
    snapshot: AuthSnapshot,
    pending_delete_account_id: Option<String>,
    text: Hsla,
    muted: Hsla,
    accent: Hsla,
    surface: Hsla,
    border: Hsla,
) -> AnyElement {
    let title = snapshot
        .profile
        .as_ref()
        .map_or("Microsoft 与 Xbox", |profile| {
            profile.display_name.as_str()
        });
    let subtitle = snapshot.profile.as_ref().map_or_else(
        || match snapshot.phase {
            AuthPhase::SignedOut => "登录后可在启动器中管理 Xbox 会话".to_string(),
            AuthPhase::Restoring => "正在从系统凭证库恢复加密会话".to_string(),
            AuthPhase::RequestingCode => "正在向 Microsoft 请求登录代码".to_string(),
            AuthPhase::WaitingForUser => "请在 Microsoft 页面确认此设备".to_string(),
            AuthPhase::AuthenticatingXbox => "正在建立 Xbox Live 与 Minecraft 服务令牌".to_string(),
            AuthPhase::SwitchingAccount => "正在刷新并验证所选 Xbox 账号".to_string(),
            AuthPhase::SigningOut => "正在从系统凭证库与兼容环境中清除会话".to_string(),
            AuthPhase::Error | AuthPhase::SignedIn => String::new(),
        },
        |profile| {
            profile.gamerscore.as_ref().map_or_else(
                || format!("@{}", profile.gamertag),
                |score| format!("@{} · {} G", profile.gamertag, score),
            )
        },
    );

    let body = match snapshot.phase {
        AuthPhase::SignedOut => div()
            .mt(px(18.))
            .h(px(40.))
            .rounded(px(10.))
            .bg(accent)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_color(rgb(0xffffff))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
                if let Err(error) = crate::core::bedrock_auth::start_login() {
                    tracing::warn!(%error, "could not start Xbox login");
                }
            })
            .child(icon(
                lucide_icons::icon_log_in(),
                rgb(0xffffff).into(),
                px(16.),
            ))
            .child(if snapshot.accounts.is_empty() {
                "使用 Microsoft 登录"
            } else {
                "添加 Microsoft 账号"
            })
            .into_any_element(),
        AuthPhase::WaitingForUser => {
            let code = snapshot.user_code.clone().unwrap_or_default();
            let verification_url = snapshot.verification_url.clone().unwrap_or_default();
            let copied_code = code.clone();
            let opened_url = verification_url.clone();
            let copied_url = verification_url.clone();
            div()
                .mt(px(16.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .h(px(48.))
                        .px(px(14.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(border)
                        .bg(text.opacity(0.035))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .font_family("monospace")
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(text)
                                .child(code),
                        )
                        .child(
                            div()
                                .id("copy-xbox-device-code")
                                .size(px(34.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(text.opacity(0.08)))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copied_code.clone(),
                                    ));
                                })
                                .child(icon(lucide_icons::icon_copy(), text, px(15.))),
                        ),
                )
                .child(
                    div()
                        .h(px(40.))
                        .rounded(px(10.))
                        .bg(accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .text_color(rgb(0xffffff))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            cx.open_url(&opened_url);
                        })
                        .child(icon(
                            lucide_icons::icon_external_link(),
                            rgb(0xffffff).into(),
                            px(15.),
                        ))
                        .child("打开 Microsoft 登录页面"),
                )
                .child(
                    div()
                        .h(px(38.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .text_color(text)
                        .hover(|style| style.bg(text.opacity(0.05)))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            cx.write_to_clipboard(ClipboardItem::new_string(copied_url.clone()));
                        })
                        .child(icon(lucide_icons::icon_link(), text, px(15.)))
                        .child("复制登录链接（可在其他浏览器打开）"),
                )
                .child(
                    div()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(muted)
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            crate::core::bedrock_auth::cancel_login();
                        })
                        .child("取消登录"),
                )
                .into_any_element()
        }
        AuthPhase::SignedIn => div()
            .mt(px(18.))
            .h(px(38.))
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_color(text)
            .hover(|style| style.bg(text.opacity(0.05)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
                if let Err(error) = crate::core::bedrock_auth::start_login() {
                    tracing::warn!(%error, "could not add another Xbox account");
                }
            })
            .child(icon(lucide_icons::icon_user_plus(), text, px(15.)))
            .child("添加其他 Microsoft 账号")
            .into_any_element(),
        AuthPhase::Error => {
            let message = snapshot
                .error
                .clone()
                .unwrap_or_else(|| "Microsoft/Xbox 登录失败".to_string());
            let retry_account_id = snapshot.active_account_id.clone();
            let has_saved_accounts = !snapshot.accounts.is_empty();
            div()
                .mt(px(14.))
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
                        .child(message),
                )
                .child(
                    div()
                        .h(px(38.))
                        .rounded(px(10.))
                        .bg(accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(rgb(0xffffff))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let result = retry_account_id.clone().map_or_else(
                                crate::core::bedrock_auth::start_login,
                                crate::core::bedrock_auth::switch_account,
                            );
                            if let Err(error) = result {
                                tracing::warn!(%error, "could not retry Xbox login");
                            }
                        })
                        .child("重新登录"),
                )
                .when(has_saved_accounts, |error_panel| {
                    error_panel.child(
                        div()
                            .h(px(36.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(7.))
                            .cursor_pointer()
                            .text_color(text)
                            .hover(|style| style.bg(text.opacity(0.05)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                                if let Err(error) = crate::core::bedrock_auth::start_login() {
                                    tracing::warn!(
                                        %error,
                                        "could not add another Xbox account after auth error"
                                    );
                                }
                            })
                            .child(icon(lucide_icons::icon_user_plus(), text, px(14.)))
                            .child("添加其他 Microsoft 账号"),
                    )
                })
                .into_any_element()
        }
        AuthPhase::Restoring
        | AuthPhase::RequestingCode
        | AuthPhase::AuthenticatingXbox
        | AuthPhase::SwitchingAccount
        | AuthPhase::SigningOut => div()
            .mt(px(18.))
            .h(px(42.))
            .rounded(px(10.))
            .bg(accent.opacity(0.08))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(9.))
            .text_color(accent)
            .child(
                icon(lucide_icons::icon_loader_circle(), accent, px(16.)).with_animation(
                    "xbox-auth-busy-spinner",
                    crate::ui::animation::repeating_linear_motion(Duration::from_millis(900)),
                    |icon, progress| {
                        icon.with_transformation(Transformation::rotate(radians(
                            progress * std::f32::consts::TAU,
                        )))
                    },
                ),
            )
            .child(status_label(&snapshot))
            .into_any_element(),
    };

    div()
        .absolute()
        .top(px(66.))
        .right(px(16.))
        .w(px(340.))
        .max_h(px(540.))
        .overflow_y_scrollbar()
        .p(px(18.))
        .rounded(px(16.))
        .bg(surface)
        .border_1()
        .border_color(border)
        .shadow_lg()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(avatar(&snapshot, text, px(44.)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text)
                                        .child(title.to_string()),
                                )
                                .child(div().text_size(px(11.5)).text_color(muted).child(subtitle)),
                        ),
                )
                .child(
                    div()
                        .id("close-xbox-panel")
                        .size(px(30.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(text.opacity(0.06)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                            cx.update_global(|state: &mut BedrockAuthState, _cx| {
                                state.close_dialog();
                            });
                        })
                        .child(icon(lucide_icons::icon_x(), muted, px(15.))),
                ),
        )
        .when(!snapshot.accounts.is_empty(), |panel| {
            panel.child(account_list(
                &snapshot,
                pending_delete_account_id.as_deref(),
                text,
                muted,
                accent,
                border,
            ))
        })
        .child(body)
        .into_any_element()
}

pub(super) fn render_app_chrome(
    state: TopbarRenderState,
    _route: RouteTarget,
    update_modal_open: bool,
) -> AnyElement {
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        state.theme_k,
        state.theme_accent,
    );
    let window_width_px = state.window_width / px(1.);
    let labels_layout_factor = state.labels_layout_factor.clamp(0.0, 1.0);
    let labels_opacity_factor = state.labels_opacity_factor.clamp(0.0, 1.0);
    let mut nav_items = vec![
        (lucide_icons::icon_house(), "启动", AppRoute::Home),
        (lucide_icons::icon_download(), "下载", AppRoute::Download),
        (lucide_icons::icon_list(), "版本", AppRoute::Manage),
        (lucide_icons::icon_wrench(), "工具", AppRoute::Tools),
        (lucide_icons::icon_activity(), "任务", AppRoute::Tasks),
        (lucide_icons::icon_settings(), "设置", AppRoute::Settings),
    ]
    .into_iter()
    .map(|(icon_path, label, target)| NavItem {
        icon_path,
        image_icon_path: None,
        label: SharedString::from(label),
        target: RouteTarget::Builtin(target),
    })
    .collect::<Vec<_>>();
    nav_items.extend(state.plugin_navigation_pages.iter().map(|page| NavItem {
        icon_path: lucide_icons::icon_plug(),
        image_icon_path: page.icon_path.clone(),
        label: page.navigation.as_ref().map_or_else(
            || page.title.clone(),
            |navigation| SharedString::from(navigation.label.clone()),
        ),
        target: RouteTarget::Plugin {
            plugin_id: page.plugin_id.clone(),
            page_id: page.page_id.clone(),
        },
    }));

    let link_padding_x = if window_width_px <= 1000.0 {
        px(10.)
    } else {
        px(13.)
    };
    let icon_width = px(18.);
    let label_width = px(33.) * labels_layout_factor;
    let label_gap = px(7.) * labels_layout_factor;
    let item_width = link_padding_x * 2. + icon_width + label_gap + label_width;
    let item_height = px(34.);
    let capsule_gap = px(3.);
    let capsule_padding = px(5.);
    let navigation_length = nav_items.len();
    let active_index = state
        .visual_active_index
        .min(navigation_length.saturating_sub(1));
    let step_width_px = (item_width + capsule_gap) / px(1.);
    let maximum_offset_px = step_width_px * navigation_length.saturating_sub(1) as f32;
    let overshoot_slack_px = step_width_px * 0.30;
    let maximum_right_px = maximum_offset_px + item_width / px(1.);
    let left_edge_px =
        (step_width_px * state.pill_left_steps).clamp(-overshoot_slack_px, maximum_right_px);
    let right_edge_px = (step_width_px * state.pill_right_steps + item_width / px(1.))
        .clamp(0.0, maximum_right_px + overshoot_slack_px);
    let pill_inner_inset_px = 1.5;
    let pill_offset = capsule_padding + px(left_edge_px.min(right_edge_px) + pill_inner_inset_px);
    let pill_width = px(((right_edge_px - left_edge_px).abs() - pill_inner_inset_px * 2.).max(0.));

    let nav = div()
        .relative()
        .flex()
        .items_center()
        .gap(capsule_gap)
        .p(capsule_padding)
        .rounded(px(24.))
        .bg(colors.text_primary.opacity(0.045))
        .child(
            div()
                .absolute()
                .left(pill_offset)
                .top(capsule_padding)
                .w(pill_width)
                .h(item_height)
                .rounded(px(17.))
                .bg(colors.accent),
        )
        .children(nav_items.into_iter().enumerate().map(|(index, item)| {
            let active = index == active_index;
            let foreground = if active {
                rgb(0xffffff).into()
            } else {
                colors.text_primary
            };
            let icon_element = item.image_icon_path.clone().map_or_else(
                || icon(item.icon_path, foreground, px(18.)).into_any_element(),
                |path| {
                    img(path)
                        .size(px(18.))
                        .rounded(px(4.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                },
            );
            div()
                .id(SharedString::from(format!("main-nav-{index}")))
                .relative()
                .w(item_width)
                .h(item_height)
                .rounded(px(17.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .occlude()
                .window_control_area(WindowControlArea::Client)
                .text_color(foreground)
                .hover(move |style| style.opacity(0.88))
                .active(|style| style.scale(0.94))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    navigation::navigate_target(cx, item.target.clone());
                })
                .child(
                    div()
                        .w(icon_width)
                        .h_full()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .child(icon_element),
                )
                .child(
                    div()
                        .w(label_width)
                        .ml(label_gap)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .opacity(labels_opacity_factor)
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(item.label),
                )
        }));

    let auth_snapshot = state.auth_snapshot.clone();
    let auth_inline = div()
        .id("xbox-auth-status")
        .h(px(38.))
        .max_w(px(178.))
        .px(px(7.))
        .pr(px(11.))
        .rounded(px(19.))
        .border_1()
        .border_color(colors.border.opacity(0.65))
        .bg(colors.text_primary.opacity(0.035))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .occlude()
        .window_control_area(WindowControlArea::Client)
        .hover(|style| style.bg(colors.text_primary.opacity(0.07)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
            cx.update_global(|state: &mut BedrockAuthState, _cx| state.toggle_dialog());
        })
        .child(avatar(&auth_snapshot, colors.text_primary, px(28.)))
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(status_label(&auth_snapshot)),
        );

    let icon_button = |id: &'static str, path: &'static str| {
        div()
            .id(id)
            .size(px(38.))
            .rounded(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .occlude()
            .window_control_area(WindowControlArea::Client)
            .text_color(colors.text_primary)
            .hover(|style| style.bg(colors.text_primary.opacity(0.07)))
            .child(icon(path, colors.text_primary, px(16.)))
    };
    let controls = div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(auth_inline)
        .child(
            icon_button(
                "theme-toggle-linux",
                if state.theme_target_dark {
                    lucide_icons::icon_sun()
                } else {
                    lucide_icons::icon_moon()
                },
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
                ThemeState::toggle_global(cx);
            }),
        )
        .child(
            icon_button("window-minimize-linux", lucide_icons::icon_minus()).on_mouse_down(
                MouseButton::Left,
                |_, window, cx| {
                    cx.stop_propagation();
                    window.minimize_window();
                },
            ),
        )
        .child(
            icon_button("window-close-linux", lucide_icons::icon_x()).on_mouse_down(
                MouseButton::Left,
                |_, window, cx| {
                    cx.stop_propagation();
                    window.remove_window();
                },
            ),
        );

    let titlebar_mouse_down = |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
        cx.update_global(|state: &mut AppChromeState, _cx| {
            state
                .titlebar_gesture
                .handle_mouse_down(event, window, Instant::now());
        });
    };
    let titlebar_mouse_move = |event: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
        if event.dragging() {
            cx.update_global(|state: &mut AppChromeState, _cx| {
                state.titlebar_gesture.handle_mouse_move(event, window);
            });
        }
    };

    let topbar = div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(60.))
        .px(px(18.))
        .flex()
        .items_center()
        .justify_between()
        .bg(colors.surface.opacity(if state.glass_effect_enabled {
            0.78
        } else {
            0.96
        }))
        .when(state.glass_effect_enabled, |element| {
            element.backdrop_blur(BackdropBlurStyle::new(px(18.)).auto_quality())
        })
        .border_b_1()
        .border_color(colors.border.opacity(0.55))
        .when(cfg!(target_os = "windows"), |element| {
            element.window_control_area(WindowControlArea::Drag)
        })
        .when(!cfg!(target_os = "windows"), |element| {
            element
                .on_mouse_down(MouseButton::Left, titlebar_mouse_down)
                .on_mouse_move(titlebar_mouse_move)
                .on_mouse_up(MouseButton::Left, |_, _, cx| {
                    cx.update_global(|state: &mut AppChromeState, _cx| {
                        state.titlebar_gesture.handle_mouse_up();
                    });
                })
        })
        .child(
            div()
                .w(px(162.))
                .flex()
                .items_center()
                .gap(px(9.))
                .child(
                    img("icons/logo.png")
                        .size(px(34.))
                        .rounded(px(0.))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.accent)
                                .child("BMCBL"),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(colors.text_secondary)
                                .child(format!("v{}", crate::utils::app_info::get_version())),
                        ),
                )
                .when(state.update_available && !update_modal_open, |element| {
                    element.child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(colors.accent)
                            .cursor_pointer()
                            .occlude()
                            .window_control_area(WindowControlArea::Client)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                                cx.update_global(|update: &mut UpdateState, _cx| {
                                    update.request_open_modal(Instant::now());
                                });
                            }),
                    )
                }),
        )
        .child(nav)
        .child(controls);

    let dialog_open = state.auth_dialog_open
        || state.auth_snapshot.phase == AuthPhase::WaitingForUser
        || state.auth_snapshot.phase == AuthPhase::Error;

    let mut root =
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .right(px(0.))
            .h(if cfg!(target_os = "windows") {
                state.window_height
            } else if dialog_open {
                px(620.)
            } else {
                px(60.)
            });

    root = root.child(topbar);

    root.when(dialog_open, |element| {
        element.child(auth_panel(
            state.auth_snapshot,
            state.auth_pending_delete_account_id,
            colors.text_primary,
            colors.text_secondary,
            colors.accent,
            colors.surface,
            colors.border,
        ))
    })
    .into_any_element()
}
