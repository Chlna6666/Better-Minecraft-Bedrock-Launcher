use super::music::MusicState;
use crate::music::MusicController;
use gpui::{App, BorrowAppContext, Timer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static LIBRARY_LOAD_SCHEDULED: AtomicBool = AtomicBool::new(false);
static LIBRARY_SCAN_RUNNING: AtomicBool = AtomicBool::new(false);
static LIBRARY_SCAN_PENDING: AtomicBool = AtomicBool::new(false);
static INITIAL_LIBRARY_INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn spawn_library_load(cx: &mut App) {
    if LIBRARY_LOAD_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }

    match crate::music::library_changes() {
        Ok(changes) => {
            cx.spawn_stream(changes, |(), cx| {
                request_library_reload(cx);
            })
            .detach();
        }
        Err(error) => tracing::warn!(%error, "music: failed to start directory watcher"),
    }

    cx.spawn(async move |cx| {
        Timer::after(Duration::from_secs(3)).await;
        cx.update(request_library_reload)?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub(crate) fn request_library_reload(cx: &mut App) {
    if LIBRARY_SCAN_RUNNING.swap(true, Ordering::AcqRel) {
        LIBRARY_SCAN_PENDING.store(true, Ordering::Release);
        return;
    }

    cx.spawn(async move |cx| {
        Timer::after(Duration::from_millis(250)).await;
        let audio_decoders = cx.update(|cx| {
            cx.update_global(
                |registry: &mut crate::plugins::runtime::PluginRegistry, _cx| {
                    registry.audio_decoders()
                },
            )
        })?;

        let result = crate::tasks::runtime::run_io_blocking(move || {
            MusicController::scan_library_tracks(&audio_decoders)
        })
        .await;

        match result {
            Ok(Ok(tracks)) => {
                let initial = !INITIAL_LIBRARY_INSTALLED.swap(true, Ordering::AcqRel);
                if initial {
                    let music_config = crate::tasks::runtime::run_io_blocking(|| {
                        crate::config::config::read_config().map(|config| config.music)
                    })
                    .await;
                    let music_config = match music_config {
                        Ok(Ok(config)) => config,
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "music: failed to read startup config");
                            crate::config::config::MusicConfig::default()
                        }
                        Err(error) => {
                            tracing::warn!(%error, "music: startup config task failed");
                            crate::config::config::MusicConfig::default()
                        }
                    };
                    cx.update_global(|state: &mut MusicState, cx| {
                        state.install_tracks_with_config(tracks, &music_config, cx);
                    })?;
                } else {
                    cx.update_global(|state: &mut MusicState, cx| {
                        state.replace_library_tracks(tracks, cx);
                    })?;
                }
            }
            Ok(Err(error)) => tracing::warn!(%error, "music: failed to scan library"),
            Err(error) => tracing::warn!(%error, "music: library scan task failed"),
        }

        LIBRARY_SCAN_RUNNING.store(false, Ordering::Release);
        if LIBRARY_SCAN_PENDING.swap(false, Ordering::AcqRel) {
            cx.update(request_library_reload)?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}
