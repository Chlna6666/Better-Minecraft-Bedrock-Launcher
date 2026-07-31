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

fn apply_cached_avatar_paths(snapshot: &mut AuthSnapshot) {
    if let Some(profile) = snapshot.profile.as_mut() {
        profile.avatar_url = crate::core::xbox_avatar_cache::cached_avatar_path(profile)
            .map(|path| path.to_string_lossy().into_owned());
    }
    for profile in &mut snapshot.accounts {
        profile.avatar_url = crate::core::xbox_avatar_cache::cached_avatar_path(profile)
            .map(|path| path.to_string_lossy().into_owned());
    }
}

pub(crate) fn start_event_bridge(cx: &mut App) {
    cx.spawn_stream(
        crate::core::xbox_avatar_cache::event_stream(),
        |_, cx| {
            cx.update_global(|state: &mut BedrockAuthState, _cx| {
                apply_cached_avatar_paths(&mut state.snapshot);
            });
            // Content-addressed file names give every changed avatar a new GPUI
            // resource key, so a repaint safely replaces the fallback or stale
            // cached image without invalidating unrelated image assets.
            cx.refresh_windows();
        },
    )
    .detach();

    cx.spawn_stream(
        crate::core::bedrock_auth::event_stream(),
        |mut snapshot, cx| {
            // Preserve the service URLs only long enough to schedule network
            // refreshes. The UI snapshot below receives local cache paths only.
            let mut profiles = snapshot.accounts.clone();
            if let Some(active_profile) = snapshot.profile.clone()
                && !profiles
                    .iter()
                    .any(|profile| profile.xuid == active_profile.xuid)
            {
                profiles.push(active_profile);
            }
            crate::core::xbox_avatar_cache::refresh_profiles(profiles);
            apply_cached_avatar_paths(&mut snapshot);

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
        },
    )
    .detach();
    crate::core::bedrock_auth::initialize();
}