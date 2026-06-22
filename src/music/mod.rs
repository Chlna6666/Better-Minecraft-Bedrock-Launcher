use gpui::{App, Timer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub mod service;
pub mod state;
pub mod types;

static LIBRARY_LOAD_SCHEDULED: AtomicBool = AtomicBool::new(false);

pub fn spawn_library_load(cx: &mut App) {
    if LIBRARY_LOAD_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }

    cx.spawn(async move |cx| {
        Timer::after(Duration::from_secs(3)).await;

        let result = tokio::task::spawn_blocking(|| {
            let tracks = crate::music::service::MusicController::scan_library_tracks()?;
            let music_config = crate::config::config::read_config()
                .map(|config| config.music)
                .unwrap_or_else(|error| {
                    tracing::warn!("music: failed to read startup config: {error}");
                    crate::config::config::MusicConfig::default()
                });
            Ok::<_, anyhow::Error>((tracks, music_config))
        })
        .await;

        match result {
            Ok(Ok((tracks, music_config))) => {
                let now = Instant::now();
                let _ = cx.update_global(|state: &mut state::MusicState, cx| {
                    state.install_tracks_with_config(tracks, &music_config, now);
                });
            }
            Ok(Err(error)) => {
                tracing::warn!("music: failed to scan library: {error:?}");
            }
            Err(error) => {
                tracing::warn!("music: library scan task join failed: {error:?}");
            }
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub use state::MusicState;
pub use types::{MusicDragTarget, MusicPlaybackMode, MusicSnapshot};
