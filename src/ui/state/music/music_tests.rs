use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

impl MusicState {
    fn from_controller_for_test(controller: MusicController) -> Self {
        Self {
            controller: Arc::new(Mutex::new(controller)),
            snapshot: MusicSnapshot::default(),
            rendered_cover_generation: 0,
            rendered_cover_cache_key: None,
            rendered_cover_image: None,
            expanded_from: 0.0,
            expanded_to: 0.0,
            expanded_started_at: None,
            expanded_duration: Duration::from_millis(180),
            expanded_target_open: false,
            drag_target: None,
            drag_progress_ratio: None,
            drag_volume_ratio: None,
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
