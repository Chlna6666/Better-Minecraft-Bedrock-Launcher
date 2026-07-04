use super::*;
use std::path::PathBuf;

fn track(path: &str) -> MusicTrack {
    MusicTrack::for_test(PathBuf::from(path), None)
}

fn track_with_cover(path: &str) -> MusicTrack {
    MusicTrack::for_test(PathBuf::from(path), Some(42))
}

#[test]
fn install_tracks_with_config_selects_last_track_path() {
    let mut controller = MusicController::new();
    let config = MusicConfig {
        auto_play_on_startup: false,
        last_track_path: "two.mp3".to_string(),
        ..MusicConfig::default()
    };

    controller.install_tracks_with_config(vec![track("one.mp3"), track("two.mp3")], &config);

    assert_eq!(controller.persisted_state().last_track_path, "two.mp3");
}

#[test]
fn install_tracks_with_config_falls_back_to_first_track() {
    let mut controller = MusicController::new();
    let config = MusicConfig {
        auto_play_on_startup: false,
        last_track_path: "missing.mp3".to_string(),
        ..MusicConfig::default()
    };

    controller.install_tracks_with_config(vec![track("one.mp3"), track("two.mp3")], &config);

    assert_eq!(controller.persisted_state().last_track_path, "one.mp3");
}

#[test]
fn install_tracks_with_config_applies_persisted_common_state_without_playing() {
    let mut controller = MusicController::new();
    let config = MusicConfig {
        auto_play_on_startup: false,
        volume: 0.75,
        muted: true,
        playback_mode: MusicPlaybackMode::Shuffle,
        last_track_path: "one.mp3".to_string(),
    };

    controller.install_tracks_with_config(vec![track("one.mp3")], &config);
    let state = controller.persisted_state();

    assert_eq!(
        state,
        MusicPersistedState {
            volume: 0.75,
            muted: true,
            playback_mode: MusicPlaybackMode::Shuffle,
            last_track_path: "one.mp3".to_string(),
        }
    );
    assert!(!controller.refresh_snapshot_no_cover().is_playing);
}

#[test]
fn persisted_state_tracks_volume_mode_and_current_track() {
    let mut controller = MusicController::new();
    controller.install_tracks(vec![track("one.mp3"), track("two.mp3")]);
    controller.set_volume(2.0);
    controller.toggle_mute();
    controller.toggle_mode();
    controller.set_current_index_for_test(1);

    let state = controller.persisted_state();

    assert_eq!(state.volume, 1.0);
    assert!(state.muted);
    assert_eq!(state.playback_mode, MusicPlaybackMode::Shuffle);
    assert_eq!(state.last_track_path, "two.mp3");
}

#[test]
fn install_tracks_keeps_shuffle_order_when_library_is_unchanged() {
    let mut controller = MusicController::new();
    controller.install_tracks(vec![track("one.mp3"), track("two.mp3")]);
    controller.toggle_mode();
    let shuffle_order = controller.play_order.clone();

    controller.install_tracks(vec![track("one.mp3"), track("two.mp3")]);

    assert_eq!(controller.play_order, shuffle_order);
}

#[test]
fn cover_request_skips_tracks_without_embedded_cover() {
    let mut controller = MusicController::new();
    controller.install_tracks(vec![track("one.mp3")]);

    assert!(controller.current_cover_request().is_none());
}

#[test]
fn cover_request_is_suppressed_after_current_cover_attempt() {
    let mut controller = MusicController::new();
    controller.install_tracks(vec![track_with_cover("one.mp3")]);
    let request = controller.current_cover_request().expect("cover request");

    assert!(controller.apply_decoded_cover_if_current(&request, false));

    assert!(controller.current_cover_request().is_none());
}
