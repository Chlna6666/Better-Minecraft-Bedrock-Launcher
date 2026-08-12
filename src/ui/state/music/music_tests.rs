use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

impl MusicState {
    fn from_controller_for_test(controller: MusicController) -> Self {
        Self {
            controller: Arc::new(Mutex::new(controller)),
            controller_operation_gate: Arc::new(tokio::sync::Mutex::new(())),
            snapshot: MusicSnapshot::default(),
            rendered_cover_generation: 0,
            rendered_cover_cache_key: None,
            rendered_cover_image: None,
            pending_cover_application: None,
            expanded_from: 0.0,
            expanded_to: 0.0,
            expanded_started_at: None,
            expanded_duration: Duration::from_millis(180),
            expanded_target_open: false,
            #[cfg(target_os = "windows")]
            inline_collapse_generation: 0,
            drag_target: None,
            drag_progress_ratio: None,
            drag_volume_ratio: None,
            pending_progress_ratio: None,
            pending_volume_ratio: None,
            auto_next_pending: false,
        }
    }
}

#[test]
fn decoded_cover_result_is_used_without_png_bytes() {
    let track_path = PathBuf::from("song.mp3");
    let mut controller = MusicController::new();
    controller.install_tracks(vec![MusicTrack::for_test(track_path.clone(), Some(7))]);
    let request = controller
        .current_cover_request()
        .expect("test track should have a current cover request");
    let mut state = MusicState::from_controller_for_test(controller);
    let decoded_cover = DecodedCoverImage {
        width: 2,
        height: 2,
        bgra_pixels: vec![1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4],
        source_byte_len: 16,
        decode_elapsed: Duration::ZERO,
    };

    state.apply_decoded_cover_if_current(&request, Some(decoded_cover), Instant::now());

    assert!(state.snapshot.cover_render_image.is_some());
}

#[cfg(target_os = "windows")]
#[test]
fn inline_collapse_ignores_stale_request_generation() {
    let mut state = MusicState::from_controller_for_test(MusicController::new());
    let generation = state.inline_collapse_generation;

    assert!(state.should_collapse_inline(generation, Instant::now()));

    state.cancel_inline_collapse();

    assert!(!state.should_collapse_inline(generation, Instant::now()));
}

#[test]
fn pending_drag_value_survives_stale_controller_snapshot() {
    let mut state = MusicState::default();
    state.snapshot.total_seconds = 100.0;
    state.snapshot.current_seconds = 10.0;
    state.pending_progress_ratio = Some((state.snapshot.generation, 0.8));

    state.set_playback_snapshot(MusicPlaybackSnapshot {
        current_seconds: 10.0,
        total_seconds: 100.0,
        generation: state.snapshot.generation,
        ..MusicPlaybackSnapshot::default()
    });
    assert!((state.displayed_progress_ratio() - 0.8).abs() < f32::EPSILON);

    state.set_playback_snapshot(MusicPlaybackSnapshot {
        current_seconds: 80.0,
        total_seconds: 100.0,
        generation: state.snapshot.generation,
        ..MusicPlaybackSnapshot::default()
    });
    assert!((state.displayed_progress_ratio() - 0.8).abs() < 0.01);
    assert!(state.pending_progress_ratio.is_none());
}
