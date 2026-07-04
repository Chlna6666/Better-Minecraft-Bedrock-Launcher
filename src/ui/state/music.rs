use crate::music::{
    CoverDecodeRequest, DecodedCoverImage, MusicController, MusicPersistedState,
    MusicPlaybackSnapshot, MusicTrack,
};
use crate::ui::animation::{ease_out_cubic, eased_progress};
use gpui::{App, Global, RenderImage};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

pub use super::music_loader::spawn_library_load;
use super::music_types::render_image_from_decoded_cover;
pub use super::music_types::{MusicDragTarget, MusicSnapshot};

pub struct MusicState {
    controller: Arc<Mutex<MusicController>>,
    pub snapshot: MusicSnapshot,
    rendered_cover_generation: u64,
    rendered_cover_cache_key: Option<u64>,
    rendered_cover_image: Option<Arc<RenderImage>>,
    expanded_from: f32,
    expanded_to: f32,
    expanded_started_at: Option<Instant>,
    expanded_duration: Duration,
    expanded_target_open: bool,
    drag_target: Option<MusicDragTarget>,
    drag_progress_ratio: Option<f32>,
    drag_volume_ratio: Option<f32>,
    auto_next_pending: bool,
}

impl Global for MusicState {}

impl Default for MusicState {
    fn default() -> Self {
        let controller = Arc::new(Mutex::new(MusicController::new()));
        let playback_snapshot = controller
            .lock()
            .map(|mut controller| controller.refresh_snapshot_no_cover())
            .unwrap_or_default();
        let mut state = Self {
            controller,
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
        };
        state.set_playback_snapshot(playback_snapshot);
        state
    }
}

struct ControllerUpdateOutcome {
    snapshot: MusicPlaybackSnapshot,
    persisted_state: Option<MusicPersistedState>,
}

#[derive(Clone, Copy)]
enum PersistMusicState {
    No,
    WhenChanged,
}

impl MusicState {
    pub fn install_tracks_with_config(
        &mut self,
        tracks: Vec<MusicTrack>,
        config: &crate::config::config::MusicConfig,
        cx: &mut App,
    ) {
        let config = config.clone();
        self.spawn_controller_update(
            "install tracks with config",
            PersistMusicState::No,
            cx,
            move |controller| {
                controller.install_tracks_with_config(tracks, &config);
            },
        );
    }

    fn clear_rendered_cover(&mut self) {
        self.rendered_cover_generation = 0;
        self.rendered_cover_cache_key = None;
        self.rendered_cover_image = None;
        self.snapshot.cover_render_image = None;
    }

    fn sync_rendered_cover(&mut self) {
        if self.rendered_cover_generation == self.snapshot.cover_generation
            && self.rendered_cover_cache_key == self.snapshot.cover_cache_key
            && let Some(rendered_cover_image) = self.rendered_cover_image.clone()
        {
            self.snapshot.cover_render_image = Some(rendered_cover_image);
            return;
        }

        self.snapshot.cover_render_image = None;
    }

    fn set_playback_snapshot(&mut self, snapshot: MusicPlaybackSnapshot) {
        let snapshot = MusicSnapshot::from_playback(snapshot, self.expanded_target_open);
        if self.rendered_cover_cache_key != snapshot.cover_cache_key {
            self.clear_rendered_cover();
        }
        self.snapshot = snapshot;
        self.sync_rendered_cover();
    }

    fn controller_update_outcome(
        controller: &mut MusicController,
        previous: Option<MusicPersistedState>,
    ) -> ControllerUpdateOutcome {
        let persisted_state = previous.and_then(|previous| {
            let next = controller.persisted_state();
            (previous != next).then_some(next)
        });
        ControllerUpdateOutcome {
            snapshot: controller.refresh_snapshot_no_cover(),
            persisted_state,
        }
    }

    fn spawn_controller_update(
        &self,
        operation_name: &'static str,
        persist: PersistMusicState,
        cx: &mut App,
        update_controller: impl FnOnce(&mut MusicController) + Send + 'static,
    ) {
        let controller = self.controller.clone();
        cx.spawn(async move |cx| {
            let result = tokio::task::spawn_blocking(move || {
                let mut controller = controller
                    .lock()
                    .map_err(|_| anyhow::anyhow!("music controller lock poisoned"))?;
                let previous = match persist {
                    PersistMusicState::No => None,
                    PersistMusicState::WhenChanged => Some(controller.persisted_state()),
                };
                update_controller(&mut controller);
                Ok::<_, anyhow::Error>(Self::controller_update_outcome(&mut controller, previous))
            })
            .await;

            match result {
                Ok(Ok(outcome)) => {
                    match cx.update_global(|state: &mut MusicState, cx| {
                        state.apply_controller_outcome(outcome, cx);
                    }) {
                        Ok(()) => {}
                        Err(error) => {
                            tracing::warn!(
                                operation = operation_name,
                                "music: failed to apply controller update: {error:?}"
                            );
                        }
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        operation = operation_name,
                        "music: controller update failed: {error:?}"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        operation = operation_name,
                        "music: controller update task failed: {error}"
                    );
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn apply_controller_outcome(&mut self, outcome: ControllerUpdateOutcome, cx: &mut App) {
        self.auto_next_pending = false;
        self.set_playback_snapshot(outcome.snapshot);
        if let Some(persisted_state) = outcome.persisted_state {
            Self::spawn_persist_music_state(persisted_state, cx);
        }
    }

    fn spawn_persist_music_state(state: MusicPersistedState, cx: &mut App) {
        cx.spawn(async move |_cx| {
            let result = tokio::task::spawn_blocking(move || {
                crate::config::config::update_config(|config| {
                    config.music.volume = crate::config::config::clamp_music_volume(state.volume);
                    config.music.muted = state.muted;
                    config.music.playback_mode = state.playback_mode;
                    config.music.last_track_path = state.last_track_path;
                })?;
                Ok::<(), std::io::Error>(())
            })
            .await;

            match result {
                Err(error) => tracing::warn!("music: persist state join error: {error}"),
                Ok(Err(error)) => tracing::warn!("music: persist state failed: {error}"),
                Ok(Ok(())) => {}
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn try_refresh_playback_snapshot(&mut self) -> Option<bool> {
        let refresh = match self.controller.try_lock() {
            Ok(mut controller) => {
                let needs_auto_next = controller.needs_auto_next();
                let snapshot = controller.refresh_snapshot_no_cover();
                Some((needs_auto_next, snapshot))
            }
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                tracing::warn!("music: controller lock poisoned during refresh");
                None
            }
        };

        let (needs_auto_next, snapshot) = refresh?;
        self.set_playback_snapshot(snapshot);
        Some(needs_auto_next)
    }

    fn sync_snapshot(&mut self, cx: &mut App) {
        if self.try_refresh_playback_snapshot() == Some(true) && !self.auto_next_pending {
            self.auto_next_pending = true;
            self.spawn_controller_update(
                "auto play next",
                PersistMusicState::WhenChanged,
                cx,
                |controller| {
                    controller.play_next();
                },
            );
        }
    }

    pub fn refresh_no_cover(&mut self, _now: Instant, cx: &mut App) {
        self.sync_snapshot(cx);
    }

    pub fn toggle_playback(&mut self, _now: Instant, cx: &mut App) {
        self.auto_next_pending = false;
        self.spawn_controller_update("toggle playback", PersistMusicState::No, cx, |controller| {
            controller.toggle_playback();
        });
    }

    pub fn play_next(&mut self, _now: Instant, cx: &mut App) {
        self.auto_next_pending = false;
        self.spawn_controller_update(
            "play next",
            PersistMusicState::WhenChanged,
            cx,
            |controller| {
                controller.play_next();
            },
        );
    }

    pub fn play_previous(&mut self, _now: Instant, cx: &mut App) {
        self.auto_next_pending = false;
        self.spawn_controller_update(
            "play previous",
            PersistMusicState::WhenChanged,
            cx,
            |controller| {
                controller.play_previous();
            },
        );
    }

    pub fn set_volume(&mut self, volume: f32, _now: Instant, cx: &mut App) {
        self.spawn_controller_update(
            "set volume",
            PersistMusicState::WhenChanged,
            cx,
            move |controller| {
                controller.set_volume(volume);
            },
        );
    }

    pub fn toggle_mute(&mut self, _now: Instant, cx: &mut App) {
        self.spawn_controller_update(
            "toggle mute",
            PersistMusicState::WhenChanged,
            cx,
            |controller| {
                controller.toggle_mute();
            },
        );
    }

    pub fn toggle_mode(&mut self, _now: Instant, cx: &mut App) {
        self.spawn_controller_update(
            "toggle mode",
            PersistMusicState::WhenChanged,
            cx,
            |controller| {
                controller.toggle_mode();
            },
        );
    }

    pub fn seek_ratio(&mut self, ratio: f32, _now: Instant, cx: &mut App) {
        self.spawn_controller_update("seek", PersistMusicState::No, cx, move |controller| {
            controller.seek_ratio(ratio);
        });
    }

    pub fn current_cover_request(&self) -> Option<CoverDecodeRequest> {
        let controller = self.controller.try_lock().ok()?;
        controller.current_cover_request()
    }

    pub fn apply_decoded_cover_if_current(
        &mut self,
        request: &CoverDecodeRequest,
        decoded_cover: Option<DecodedCoverImage>,
        _now: Instant,
    ) {
        let cover_image = decoded_cover.and_then(render_image_from_decoded_cover);
        let decoded = cover_image.is_some();
        let snapshot = match self.controller.try_lock() {
            Ok(mut controller) => {
                if controller.apply_decoded_cover_if_current(request, decoded) {
                    Some(controller.refresh_snapshot_no_cover())
                } else {
                    None
                }
            }
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                tracing::warn!("music: controller lock poisoned while applying cover");
                None
            }
        };

        let Some(snapshot) = snapshot else {
            return;
        };

        if let Some(cover_image) = cover_image {
            self.rendered_cover_generation = snapshot.cover_generation;
            self.rendered_cover_cache_key = snapshot.cover_cache_key;
            self.rendered_cover_image = Some(cover_image);
        }
        self.set_playback_snapshot(snapshot);
    }

    pub fn set_expanded(&mut self, expanded: bool, now: Instant, cx: &mut App) {
        let factor = self.expanded_factor(now);
        let fully_at_target = if expanded {
            factor >= 0.999
        } else {
            factor <= 0.001
        };
        if self.expanded_target_open == expanded
            && self.snapshot.expanded == expanded
            && fully_at_target
            && !self.popup_animating(now)
        {
            return;
        }
        if !expanded {
            self.clear_drag();
        }
        self.expanded_target_open = expanded;
        self.expanded_from = factor;
        self.expanded_to = if expanded { 1.0 } else { 0.0 };
        self.expanded_started_at = Some(now);
        self.expanded_duration = if expanded {
            Duration::from_millis(140)
        } else {
            Duration::from_millis(110)
        };
        self.snapshot.expanded = expanded;
        self.refresh_no_cover(now, cx);
    }

    pub fn expanded_target_open(&self) -> bool {
        self.expanded_target_open
    }

    pub fn begin_drag(&mut self, target: MusicDragTarget) {
        self.drag_target = Some(target);
        match target {
            MusicDragTarget::Progress => {
                self.drag_progress_ratio = Some(self.current_progress_ratio());
            }
            MusicDragTarget::Volume => {
                self.drag_volume_ratio = Some(self.snapshot.volume.clamp(0.0, 1.0));
            }
        }
    }

    pub fn drag_target(&self) -> Option<MusicDragTarget> {
        self.drag_target
    }

    pub fn clear_drag(&mut self) {
        self.drag_target = None;
        self.drag_progress_ratio = None;
        self.drag_volume_ratio = None;
    }

    fn ratio_changed(current: Option<f32>, next: f32) -> bool {
        current.is_none_or(|current| (current - next).abs() >= 0.004)
    }

    pub fn update_drag_ratio(&mut self, ratio: f32) -> bool {
        match self.drag_target {
            Some(MusicDragTarget::Progress) => {
                let ratio = ratio.clamp(0.0, 1.0);
                if Self::ratio_changed(self.drag_progress_ratio, ratio) {
                    self.drag_progress_ratio = Some(ratio);
                    return true;
                }
            }
            Some(MusicDragTarget::Volume) => {
                let ratio = ratio.clamp(0.0, 1.0);
                if Self::ratio_changed(self.drag_volume_ratio, ratio) {
                    self.drag_volume_ratio = Some(ratio);
                    return true;
                }
            }
            None => {}
        }
        false
    }

    pub fn commit_drag(&mut self, now: Instant, cx: &mut App) {
        match self.drag_target {
            Some(MusicDragTarget::Progress) => {
                if let Some(ratio) = self.drag_progress_ratio {
                    self.seek_ratio(ratio, now, cx);
                }
            }
            Some(MusicDragTarget::Volume) => {
                if let Some(ratio) = self.drag_volume_ratio {
                    self.set_volume(ratio, now, cx);
                }
            }
            None => {}
        }
    }

    fn current_progress_ratio(&self) -> f32 {
        if self.snapshot.total_seconds <= 0.0 {
            0.0
        } else {
            (self.snapshot.current_seconds / self.snapshot.total_seconds).clamp(0.0, 1.0)
        }
    }

    pub fn displayed_progress_ratio(&self) -> f32 {
        self.drag_progress_ratio
            .unwrap_or_else(|| self.current_progress_ratio())
    }

    pub fn displayed_volume_ratio(&self) -> f32 {
        self.drag_volume_ratio
            .unwrap_or_else(|| self.snapshot.volume.clamp(0.0, 1.0))
    }

    pub fn expanded_factor(&self, now: Instant) -> f32 {
        let Some(started_at) = self.expanded_started_at else {
            return if self.expanded_target_open { 1.0 } else { 0.0 };
        };
        let t = eased_progress(now, started_at, self.expanded_duration);
        let eased = ease_out_cubic(t);
        (self.expanded_from + (self.expanded_to - self.expanded_from) * eased).clamp(0.0, 1.0)
    }

    pub fn popup_animating(&self, now: Instant) -> bool {
        self.expanded_started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) < self.expanded_duration
        })
    }
}

#[cfg(test)]
mod music_tests;
