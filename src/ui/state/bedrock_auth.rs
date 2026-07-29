use crate::core::bedrock_auth::AuthSnapshot;
use gpui::{App, BorrowAppContext as _, Global};

pub(crate) struct BedrockAuthState {
    pub(crate) snapshot: AuthSnapshot,
    pub(crate) dialog_open: bool,
    pub(crate) pending_delete_account_id: Option<String>,
}

impl Default for BedrockAuthState {
    fn default() -> Self {
        Self {
            snapshot: AuthSnapshot::signed_out(),
            dialog_open: false,
            pending_delete_account_id: None,
        }
    }
}

impl Global for BedrockAuthState {}

impl BedrockAuthState {
    pub(crate) fn toggle_dialog(&mut self) {
        self.dialog_open = !self.dialog_open;
    }

    pub(crate) fn close_dialog(&mut self) {
        self.dialog_open = false;
        self.pending_delete_account_id = None;
    }

    pub(crate) fn request_account_deletion(&mut self, account_id: String) {
        self.pending_delete_account_id = Some(account_id);
    }

    pub(crate) fn clear_account_deletion(&mut self) {
        self.pending_delete_account_id = None;
    }
}

pub(crate) fn start_event_bridge(cx: &mut App) {
    cx.spawn_stream(crate::core::bedrock_auth::event_stream(), |snapshot, cx| {
        cx.update_global(|state: &mut BedrockAuthState, _cx| {
            if state
                .pending_delete_account_id
                .as_deref()
                .is_some_and(|account_id| {
                    !snapshot
                        .accounts
                        .iter()
                        .any(|profile| profile.xuid == account_id)
                })
            {
                state.pending_delete_account_id = None;
            }
            state.snapshot = snapshot;
        });
    })
    .detach();
    crate::core::bedrock_auth::initialize();
}
