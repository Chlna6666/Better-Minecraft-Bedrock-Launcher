use super::*;

#[derive(Clone)]
pub(super) enum Action {
    Toggle {
        panel: FocusHandle,
        trigger: FocusHandle,
    },
    Close(FocusHandle),
    Login,
    Switch(String),
    Delete(String),
    CancelDeletion,
    CancelLogin,
    Retry,
    CopyCode,
    CopyLink,
    OpenLink,
}

pub(super) fn button(
    id: impl Into<ElementId>,
    action: Action,
    state: &RenderState,
    colors: &ThemeColors,
    enabled: bool,
) -> Stateful<Div> {
    let hover = colors.text_primary.opacity(0.06);
    let accent = colors.accent;
    let reduced_motion = state.reduced_motion;
    let panel_focus = state.panel_focus.clone();
    let button = div()
        .id(id)
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(colors.text_primary);
    if !enabled {
        return button.opacity(0.45);
    }
    button
        .cursor_pointer()
        .tab_index(0)
        .hover(move |style| style.bg(hover))
        .focus(move |style| style.bg(accent.opacity(0.16)))
        .active(move |style| {
            let style = style.opacity(0.8);
            if reduced_motion {
                style
            } else {
                style.scale(motion::PRESS_SCALE)
            }
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |event, window, cx| {
            if event.standard_click() {
                cx.stop_propagation();
                let focus_panel = event.is_keyboard() && action.changes_content(cx);
                action.run(event.is_keyboard(), window, cx);
                if focus_panel {
                    window.focus(&panel_focus);
                }
            }
        })
}

impl Action {
    fn changes_content(&self, cx: &App) -> bool {
        match self {
            Self::Login | Self::Switch(_) | Self::Retry | Self::CancelLogin => true,
            Self::Delete(id) => {
                cx.global::<BedrockAuthState>()
                    .pending_delete_account_id
                    .as_ref()
                    == Some(id)
            }
            _ => false,
        }
    }

    fn run(&self, keyboard: bool, window: &mut Window, cx: &mut App) {
        cx.update_global(|state: &mut BedrockAuthState, _| state.keyboard_navigation = keyboard);
        match self {
            Self::Toggle { panel, trigger } => {
                cx.update_global(|state: &mut BedrockAuthState, _| state.toggle_dialog());
                if cx.global::<BedrockAuthState>().dialog_open {
                    window.focus(panel);
                } else {
                    window.focus(trigger);
                }
            }
            Self::Close(focus) => {
                cx.update_global(|state: &mut BedrockAuthState, _| state.close_dialog());
                window.focus(focus);
            }
            Self::CancelDeletion => {
                cx.update_global(|state: &mut BedrockAuthState, _| state.clear_account_deletion())
            }
            Self::CancelLogin => crate::core::bedrock_auth::cancel_login(),
            Self::CopyCode | Self::CopyLink | Self::OpenLink => self.link_action(cx),
            _ => self.account_action(cx),
        }
    }

    fn link_action(&self, cx: &mut App) {
        let snapshot = &cx.global::<BedrockAuthState>().snapshot;
        let (value, copied) = match self {
            Self::CopyCode => (snapshot.user_code.clone(), "code"),
            _ => (snapshot.verification_url.clone(), "link"),
        };
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            return;
        };
        if matches!(self, Self::OpenLink) {
            cx.open_url(&value);
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(value));
            cx.update_global(|state: &mut BedrockAuthState, _| state.copied = Some(copied));
        }
    }

    fn account_action(&self, cx: &mut App) {
        let state = cx.global::<BedrockAuthState>();
        if !matches!(
            state.snapshot.phase,
            AuthPhase::SignedIn | AuthPhase::SignedOut | AuthPhase::Error
        ) {
            return;
        }
        if let Self::Delete(id) = self {
            if crate::core::bedrock_auth::is_system_local_account(id) {
                return;
            }
            if state.pending_delete_account_id.as_ref() != Some(id) {
                cx.update_global(|state: &mut BedrockAuthState, _| {
                    state.request_account_deletion(id.clone())
                });
                return;
            }
        }
        let result = match self {
            Self::Login => crate::core::bedrock_auth::start_login(),
            Self::Switch(id) => crate::core::bedrock_auth::switch_account(id.clone()),
            Self::Delete(id) => crate::core::bedrock_auth::remove_account(id.clone()),
            Self::Retry => state.snapshot.active_account_id.clone().map_or_else(
                crate::core::bedrock_auth::start_login,
                crate::core::bedrock_auth::switch_account,
            ),
            _ => return,
        };
        cx.update_global(|state: &mut BedrockAuthState, _| {
            state.clear_account_deletion();
            state.feedback = result.err().map(|error| {
                tracing::warn!(%error, "Xbox account action failed");
                error.to_string()
            });
        });
    }
}
