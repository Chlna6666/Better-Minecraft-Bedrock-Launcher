use crate::core::bedrock_auth::{AuthPhase, AuthSnapshot, XboxProfile};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::bedrock_auth::BedrockAuthState;
use crate::ui::theme::{ThemeColors, glass_backdrop_blur_style, tokens::motion};
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons;
use std::time::Instant;
use std::{cell::Cell, rc::Rc};

mod accounts;
mod actions;
mod login;

use actions::{Action, button};

pub(in crate::ui::main_window) struct Row {
    profile: XboxProfile,
    opacity: f32,
    selection: f32,
    present: bool,
}

pub(in crate::ui::main_window) struct RenderState {
    snapshot: AuthSnapshot,
    rows: Vec<Row>,
    open: bool,
    progress: f32,
    pub(in crate::ui::main_window) animating: bool,
    reduced_motion: bool,
    pending_delete: Option<String>,
    feedback: Option<String>,
    copied: Option<&'static str>,
    trigger_focus: FocusHandle,
    panel_focus: FocusHandle,
    trigger_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    available: bool,
}

impl RenderState {
    pub(in crate::ui::main_window) fn new(
        state: &BedrockAuthState,
        now: Instant,
        reduced_motion: bool,
        focus: (FocusHandle, FocusHandle),
        trigger_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    ) -> Self {
        let immediate = reduced_motion || state.keyboard_navigation;
        let sample = state.dialog_motion.sample(now);
        let rows: Vec<_> = state
            .rows
            .iter()
            .map(|row| Row {
                profile: row.profile.clone(),
                opacity: if immediate {
                    row.presence.target()
                } else {
                    row.presence.value(now).clamp(0.0, 1.0)
                },
                selection: if immediate {
                    row.selection.target()
                } else {
                    row.selection.value(now).clamp(0.0, 1.0)
                },
                present: row.presence.target() > 0.0,
            })
            .filter(|row| row.present || row.opacity > 0.001)
            .collect();
        Self {
            snapshot: state.snapshot.clone(),
            rows,
            open: state.dialog_open,
            progress: if immediate {
                f32::from(state.dialog_open)
            } else {
                sample.value
            },
            animating: !immediate
                && (!sample.done
                    || state.dialog_open
                        && state.rows.iter().any(|row| {
                            row.presence.is_animating(now) || row.selection.is_animating(now)
                        })),
            reduced_motion: immediate,
            pending_delete: state.pending_delete_account_id.clone(),
            feedback: state.feedback.clone(),
            copied: state.copied,
            trigger_focus: focus.0,
            panel_focus: focus.1,
            trigger_bounds,
            available: true,
        }
    }

    pub(in crate::ui::main_window) fn blocked(mut self, blocked: bool) -> Self {
        if blocked {
            self.available = false;
            self.open = false;
            self.progress = 0.0;
            self.animating = false;
        }
        self
    }

    pub(super) fn visible(&self) -> bool {
        self.open || self.progress > 0.001
    }

    fn interactive(&self) -> bool {
        self.open
            && matches!(
                self.snapshot.phase,
                AuthPhase::SignedOut | AuthPhase::SignedIn | AuthPhase::Error
            )
    }
}

fn icon(path: &'static str, color: Hsla, size: f32) -> Svg {
    svg().path(path).size(px(size)).text_color(color)
}

fn avatar(profile: Option<&XboxProfile>, color: Hsla, size: f32) -> AnyElement {
    let frame = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .bg(color.opacity(0.08))
        .flex()
        .items_center()
        .justify_center();
    if let Some(url) = profile.and_then(|profile| profile.avatar_url.as_ref()) {
        frame
            .child(
                img(SharedString::from(url.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .render_to_bounds(),
            )
            .into_any_element()
    } else {
        frame
            .child(icon(icons::icon_circle_user_round(), color, size * 0.6))
            .into_any_element()
    }
}

fn status_label(snapshot: &AuthSnapshot) -> SharedString {
    match snapshot.phase {
        AuthPhase::SignedIn => snapshot.profile.as_ref().map_or_else(
            || t!("Auth.xbox"),
            |profile| profile.gamertag.clone().into(),
        ),
        AuthPhase::Restoring => t!("Auth.restoring"),
        AuthPhase::RequestingCode => t!("Auth.requesting_code"),
        AuthPhase::WaitingForUser => t!("Auth.waiting_confirmation"),
        AuthPhase::AuthenticatingXbox => t!("Auth.authenticating"),
        AuthPhase::SwitchingAccount => t!("Auth.switching_account"),
        AuthPhase::SigningOut => t!("Auth.signing_out"),
        AuthPhase::Error => t!("Auth.login_failed"),
        AuthPhase::SignedOut => t!("Auth.xbox_login"),
    }
}

fn status_hint(phase: AuthPhase) -> SharedString {
    match phase {
        AuthPhase::SignedOut => t!("Auth.signed_out_hint"),
        AuthPhase::Restoring => t!("Auth.restoring_hint"),
        AuthPhase::RequestingCode => t!("Auth.requesting_code_hint"),
        AuthPhase::WaitingForUser => t!("Auth.waiting_confirmation_hint"),
        AuthPhase::AuthenticatingXbox => t!("Auth.authenticating_hint"),
        AuthPhase::SwitchingAccount => t!("Auth.switching_account_hint"),
        AuthPhase::SigningOut => t!("Auth.signing_out_hint"),
        AuthPhase::SignedIn | AuthPhase::Error => SharedString::default(),
    }
}

pub(super) fn trigger(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let trigger_bounds = state.trigger_bounds.clone();
    button(
        "xbox-auth-status",
        Action::Toggle {
            panel: state.panel_focus.clone(),
            trigger: state.trigger_focus.clone(),
        },
        state,
        colors,
        state.available,
    )
    .relative()
    .when(state.available, |trigger| {
        trigger.track_focus(&state.trigger_focus)
    })
    .h(px(38.))
    .max_w(px(178.))
    .px(px(7.))
    .pr(px(11.))
    .rounded(px(19.))
    .border_1()
    .border_color(colors.border.opacity(0.55))
    .bg(colors
        .text_primary
        .opacity(if state.open { 0.07 } else { 0.035 }))
    .window_control_area(WindowControlArea::Client)
    .child(avatar(
        state.snapshot.profile.as_ref(),
        colors.text_primary,
        28.,
    ))
    .child(
        div()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(status_label(&state.snapshot)),
    )
    .child(
        icon(icons::icon_chevron_down(), colors.text_secondary, 12.).with_transformation(
            Transformation::rotate(radians(std::f32::consts::PI * state.progress)),
        ),
    )
    .child(
        canvas(
            move |bounds, _, _| trigger_bounds.set(Some(bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    )
    .into_any_element()
}

fn header(state: &RenderState, colors: &ThemeColors) -> AnyElement {
    let title = state.snapshot.profile.as_ref().map_or_else(
        || t!("Auth.microsoft_xbox"),
        |profile| profile.display_name.clone().into(),
    );
    let subtitle = state.snapshot.profile.as_ref().map_or_else(
        || status_hint(state.snapshot.phase),
        |profile| {
            profile
                .gamerscore
                .as_ref()
                .map_or_else(
                    || format!("@{}", profile.gamertag),
                    |score| format!("@{} · {score} G", profile.gamertag),
                )
                .into()
        },
    );
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(avatar(
            state.snapshot.profile.as_ref(),
            colors.text_primary,
            40.,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            button(
                "close-xbox-panel",
                Action::Close(state.trigger_focus.clone()),
                state,
                colors,
                state.open,
            )
            .size(px(30.))
            .child(icon(icons::icon_x(), colors.text_secondary, 15.)),
        )
        .into_any_element()
}

pub(super) fn panel(
    state: &RenderState,
    colors: &ThemeColors,
    viewport: Size<Pixels>,
    glass_effect_enabled: bool,
) -> AnyElement {
    let close_focus = state.trigger_focus.clone();
    let trigger_bounds = state.trigger_bounds.clone();
    let width = px(340.).min((viewport.width - px(32.)).max(px(0.)));
    let origin = trigger_bounds.get().map_or(0.5, |bounds| {
        ((bounds.center().x - (viewport.width - px(16.) - width)) / width).clamp(0.0, 1.0)
    });
    let progress = state.progress;
    div()
        .absolute()
        .top(px(66.))
        .right(px(16.))
        .w(width)
        .max_h((viewport.height - px(82.)).max(px(0.)))
        .p(px(16.))
        .rounded(px(16.))
        .bg(colors
            .surface
            .opacity(if glass_effect_enabled { 0.92 } else { 1.0 }))
        .when(glass_effect_enabled, |panel| {
            panel.backdrop_blur(glass_backdrop_blur_style())
        })
        .id("xbox-auth-panel")
        .overflow_y_scrollbar()
        .text_color(colors.text_primary)
        .border_1()
        .border_color(colors.border.opacity(0.65))
        .shadow_lg()
        .occlude()
        .tab_group()
        .track_focus(&state.panel_focus)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(move |event, _, cx| {
            if !trigger_bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                cx.update_global(|state: &mut BedrockAuthState, _| state.close_dialog());
            }
        })
        .on_key_down(move |event, window, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                cx.update_global(|state: &mut BedrockAuthState, _| {
                    state.keyboard_navigation = true;
                    state.close_dialog();
                });
                window.focus(&close_focus);
            }
        })
        .child(header(state, colors))
        .when(!state.rows.is_empty(), |panel| {
            panel.child(accounts::list(state, colors))
        })
        .child(login::body(state, colors))
        .when_some(state.feedback.as_ref(), |panel, message| {
            panel.child(
                div()
                    .mt(px(12.))
                    .text_size(px(12.))
                    .text_color(rgb(0xdc2626))
                    .child(message.clone()),
            )
        })
        .with_sampled_animation(
            AnimationProperty::scale_opacity(
                motion::POPOVER_SCALE,
                1.0,
                0.0,
                1.0,
                TransformOrigin::new(origin, 0.0),
            ),
            progress,
        )
        .into_any_element()
}
