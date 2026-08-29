use crate::core::bedrock_auth::{AuthPhase, AuthSnapshot, XboxProfile};
use crate::ui::animation::{SpringValue, apple_spring};
use crate::ui::theme::tokens::motion;
use gpui::{App, BorrowAppContext as _, Global};
use std::time::Instant;

fn account_motion(value: f32, response: f32) -> SpringValue {
    SpringValue::new(value).with_spring(apple_spring(response, 1.0))
}

pub(crate) struct AccountRow {
    pub(crate) profile: XboxProfile,
    pub(crate) presence: SpringValue,
    pub(crate) selection: SpringValue,
}

pub(crate) struct BedrockAuthState {
    pub(crate) snapshot: AuthSnapshot,
    pub(crate) dialog_open: bool,
    pub(crate) pending_delete_account_id: Option<String>,
    pub(crate) dialog_motion: SpringValue,
    pub(crate) rows: Vec<AccountRow>,
    pub(crate) feedback: Option<String>,
    pub(crate) copied: Option<&'static str>,
    pub(crate) keyboard_navigation: bool,
}

impl Default for BedrockAuthState {
    fn default() -> Self {
        Self {
            snapshot: AuthSnapshot::signed_out(),
            dialog_open: false,
            pending_delete_account_id: None,
            dialog_motion: SpringValue::new(0.0)
                .with_spring(apple_spring(motion::POPOVER_RESPONSE, 0.72)),
            rows: Vec::new(),
            feedback: None,
            copied: None,
            keyboard_navigation: false,
        }
    }
}

impl Global for BedrockAuthState {}

impl BedrockAuthState {
    pub(crate) fn toggle_dialog(&mut self) {
        if self.dialog_open {
            self.close_dialog();
        } else {
            self.dialog_open = true;
            self.update_dialog_motion(Instant::now());
        }
    }

    pub(crate) fn close_dialog(&mut self) {
        self.dialog_open = false;
        self.pending_delete_account_id = None;
        self.update_dialog_motion(Instant::now());
    }

    fn update_dialog_motion(&mut self, now: Instant) {
        let target = f32::from(self.dialog_open);
        if self.keyboard_navigation {
            self.dialog_motion.snap_to(target);
        } else {
            self.dialog_motion.retarget(target, now);
        }
    }

    pub(crate) fn request_account_deletion(&mut self, account_id: String) {
        if crate::core::bedrock_auth::is_system_local_account(&account_id) {
            self.pending_delete_account_id = None;
            return;
        }
        self.pending_delete_account_id = Some(account_id);
    }

    pub(crate) fn clear_account_deletion(&mut self) {
        self.pending_delete_account_id = None;
    }

    fn apply_snapshot(&mut self, snapshot: AuthSnapshot, now: Instant) {
        // Open once when a flow needs attention, never from render: explicit dismissal wins.
        if self.snapshot.phase != snapshot.phase {
            self.copied = None;
            self.feedback = None;
            self.pending_delete_account_id = None;
            if matches!(snapshot.phase, AuthPhase::WaitingForUser | AuthPhase::Error) {
                self.dialog_open = true;
                self.update_dialog_motion(now);
            }
        }
        self.sync_rows(&snapshot, now);
        if self
            .pending_delete_account_id
            .as_ref()
            .is_some_and(|id| !snapshot.accounts.iter().any(|profile| &profile.xuid == id))
        {
            self.pending_delete_account_id = None;
        }
        self.snapshot = snapshot;
    }

    fn sync_rows(&mut self, snapshot: &AuthSnapshot, now: Instant) {
        self.rows
            .retain(|row| row.presence.target() > 0.0 || row.presence.is_animating(now));
        for row in &mut self.rows {
            let profile = snapshot
                .accounts
                .iter()
                .find(|profile| profile.xuid == row.profile.xuid);
            row.presence.retarget(f32::from(profile.is_some()), now);
            row.selection.retarget(
                f32::from(snapshot.active_account_id.as_deref() == Some(row.profile.xuid.as_str())),
                now,
            );
            if let Some(profile) = profile {
                row.profile = profile.clone();
            }
        }
        for profile in &snapshot.accounts {
            if !self.rows.iter().any(|row| row.profile.xuid == profile.xuid) {
                let mut presence =
                    account_motion(f32::from(!self.dialog_open), motion::FEEDBACK_RESPONSE);
                presence.retarget(1.0, now);
                self.rows.push(AccountRow {
                    profile: profile.clone(),
                    presence,
                    selection: account_motion(
                        f32::from(
                            snapshot.active_account_id.as_deref() == Some(profile.xuid.as_str()),
                        ),
                        motion::FEEDBACK_RESPONSE,
                    ),
                });
            }
        }
    }
}

fn apply_profile_avatar_path(profile: &mut crate::core::bedrock_auth::XboxProfile) {
    if crate::core::bedrock_auth::is_system_local_account(&profile.xuid) {
        return;
    }
    profile.avatar_url = crate::core::xbox_avatar_cache::cached_avatar_path(profile)
        .map(|path| path.to_string_lossy().into_owned());
}

fn apply_cached_avatar_paths(snapshot: &mut AuthSnapshot) {
    if let Some(profile) = snapshot.profile.as_mut() {
        apply_profile_avatar_path(profile);
    }
    for profile in &mut snapshot.accounts {
        apply_profile_avatar_path(profile);
    }
}

pub(crate) fn start_event_bridge(cx: &mut App) {
    cx.spawn_stream(crate::core::xbox_avatar_cache::event_stream(), |_, cx| {
        cx.update_global(|state: &mut BedrockAuthState, _cx| {
            apply_cached_avatar_paths(&mut state.snapshot);
            for row in &mut state.rows {
                apply_profile_avatar_path(&mut row.profile);
            }
        });
        cx.refresh_windows();
    })
    .detach();

    cx.spawn_stream(
        crate::core::bedrock_auth::event_stream(),
        |mut snapshot, cx| {
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
                state.apply_snapshot(snapshot, Instant::now());
            });
        },
    )
    .detach();

    // Startup schedules both independent account preloads before GPUI starts.
    // The UI layer only subscribes to the retained watch-channel results.
}

#[cfg(test)]
mod tests;
