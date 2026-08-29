use super::*;
use std::time::Duration;

#[test]
fn attention_phase_opens_once_and_dismissal_survives_snapshot_refresh() {
    for phase in [AuthPhase::WaitingForUser, AuthPhase::Error] {
        let now = Instant::now();
        let mut state = BedrockAuthState::default();
        let mut snapshot = AuthSnapshot::signed_out();
        snapshot.phase = phase;
        state.apply_snapshot(snapshot.clone(), now);
        assert!(state.dialog_open);
        state.close_dialog();
        state.apply_snapshot(snapshot, now + Duration::from_millis(10));
        assert!(!state.dialog_open);
        assert_eq!(state.dialog_motion.target(), 0.0);
    }
}

#[test]
fn reversal_preserves_the_current_presentation() {
    let now = Instant::now();
    let mut state = BedrockAuthState::default();
    state.dialog_motion.retarget(1.0, now);
    let halfway = now + Duration::from_millis(80);
    let before = state.dialog_motion.sample(halfway);
    state.dialog_motion.retarget(0.0, halfway);
    let after = state.dialog_motion.sample(halfway);
    assert!((before.value - after.value).abs() < 0.0001);
    assert!((before.velocity - after.velocity).abs() < 0.0001);
    assert!(!after.done);
}

#[test]
fn closing_from_trigger_clears_delete_confirmation() {
    let mut state = BedrockAuthState::default();
    state.toggle_dialog();
    state.request_account_deletion("saved-account".to_owned());
    state.toggle_dialog();
    assert!(!state.dialog_open);
    assert!(state.pending_delete_account_id.is_none());
}

#[test]
fn keyboard_toggle_settles_motion_before_pointer_reopening() {
    let mut state = BedrockAuthState::default();
    state.keyboard_navigation = true;
    state.toggle_dialog();
    assert_eq!(state.dialog_motion.value(Instant::now()), 1.0);
    state.close_dialog();
    assert_eq!(state.dialog_motion.value(Instant::now()), 0.0);
    assert!(!state.dialog_motion.is_animating(Instant::now()));
}

#[test]
fn phase_change_clears_stale_feedback_and_copy_state() {
    let mut state = BedrockAuthState::default();
    state.feedback = Some("failure".to_owned());
    state.copied = Some("code");
    let mut snapshot = AuthSnapshot::signed_out();
    snapshot.phase = AuthPhase::RequestingCode;
    state.apply_snapshot(snapshot, Instant::now());
    assert!(state.feedback.is_none());
    assert!(state.copied.is_none());
}

fn profile(id: &str) -> XboxProfile {
    XboxProfile {
        xuid: id.to_owned(),
        gamertag: id.to_owned(),
        display_name: id.to_owned(),
        gamerscore: None,
        avatar_url: None,
    }
}

#[test]
fn removal_retains_row_for_exit_and_reinsertion_reverses_it() {
    let now = Instant::now();
    let mut state = BedrockAuthState::default();
    let mut snapshot = AuthSnapshot::signed_out();
    snapshot.accounts.push(profile("first"));
    state.apply_snapshot(snapshot.clone(), now);
    state.toggle_dialog();
    state.apply_snapshot(AuthSnapshot::signed_out(), now);
    assert_eq!(state.rows.len(), 1);
    assert_eq!(state.rows[0].presence.target(), 0.0);
    let interrupted = now + Duration::from_millis(80);
    let before = state.rows[0].presence.value(interrupted);
    state.apply_snapshot(snapshot, interrupted);
    assert_eq!(state.rows.len(), 1);
    assert!((state.rows[0].presence.value(interrupted) - before).abs() < 0.0001);
    assert_eq!(state.rows[0].presence.target(), 1.0);
}

#[test]
fn selection_follows_active_account_without_reordering_rows() {
    let now = Instant::now();
    let mut state = BedrockAuthState::default();
    let mut snapshot = AuthSnapshot::signed_out();
    snapshot.accounts = vec![profile("first"), profile("second")];
    snapshot.active_account_id = Some("first".to_owned());
    state.apply_snapshot(snapshot.clone(), now);
    snapshot.active_account_id = Some("second".to_owned());
    state.apply_snapshot(snapshot, now + Duration::from_millis(10));
    assert_eq!(state.rows[0].profile.xuid, "first");
    assert_eq!(state.rows[0].selection.target(), 0.0);
    assert_eq!(state.rows[1].selection.target(), 1.0);
}

#[test]
fn completed_exit_is_pruned_on_the_next_snapshot() {
    let now = Instant::now();
    let mut state = BedrockAuthState::default();
    let mut snapshot = AuthSnapshot::signed_out();
    snapshot.accounts.push(profile("first"));
    state.apply_snapshot(snapshot, now);
    state.apply_snapshot(AuthSnapshot::signed_out(), now);
    state.apply_snapshot(AuthSnapshot::signed_out(), now + Duration::from_secs(2));
    assert!(state.rows.is_empty());
}

#[test]
fn popover_preserves_overshoot_until_position_and_velocity_settle() {
    let now = Instant::now();
    let mut state = BedrockAuthState::default();
    state.dialog_motion.retarget(1.0, now);
    assert!(state.dialog_motion.value(now + Duration::from_millis(80)) >= 0.8);
    assert!(state.dialog_motion.value(now + Duration::from_millis(120)) > 1.0);
    assert!(
        !state
            .dialog_motion
            .sample(now + Duration::from_millis(180))
            .done
    );
    let settled = now + Duration::from_millis(400);
    assert!(state.dialog_motion.sample(settled).done);
    state.dialog_motion.retarget(0.0, settled);
    assert!(
        state
            .dialog_motion
            .sample(settled + Duration::from_millis(400))
            .done
    );
}

#[test]
fn account_feedback_does_not_discard_a_moving_tail() {
    let now = Instant::now();
    let mut value = account_motion(0.0, motion::FEEDBACK_RESPONSE);
    value.retarget(1.0, now);
    assert!(!value.sample(now + Duration::from_millis(140)).done);
    assert!(value.sample(now + Duration::from_millis(400)).done);
}
