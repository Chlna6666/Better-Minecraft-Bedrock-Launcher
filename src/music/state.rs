use crate::music::service::{MusicController, MusicPersistedState};
use crate::music::types::{MusicDragTarget, MusicSnapshot};
use crate::ui::animation::{ease_out_cubic, eased_progress};
use gpui::{App, Global, RenderImage};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
}

impl Global for MusicState {}

impl Default for MusicState {
    fn default() -> Self {
        let controller = Arc::new(Mutex::new(MusicController::new()));
        let snapshot = controller
            .lock()
            .map(|mut controller| controller.refresh_snapshot_no_cover(Instant::now(), false))
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
        };
        state.set_snapshot(snapshot);
        state
    }
}

impl MusicState {
    pub fn install_tracks(&mut self, tracks: Vec<crate::music::service::MusicTrack>, now: Instant) {
        let snapshot = if let Ok(mut controller) = self.controller.lock() {
            controller.install_tracks(tracks);
            Some(controller.refresh_snapshot_no_cover(now, self.expanded_target_open))
        } else {
            None
        };

        if let Some(snapshot) = snapshot {
            self.set_snapshot(snapshot);
        }
    }

    pub fn install_tracks_with_config(
        &mut self,
        tracks: Vec<crate::music::service::MusicTrack>,
        config: &crate::config::config::MusicConfig,
        now: Instant,
    ) {
        let snapshot = if let Ok(mut controller) = self.controller.lock() {
            controller.install_tracks_with_config(tracks, config);
            Some(controller.refresh_snapshot_no_cover(now, self.expanded_target_open))
        } else {
            None
        };

        if let Some(snapshot) = snapshot {
            self.set_snapshot(snapshot);
        }
    }

    #[cfg(test)]
    fn from_controller_for_test(controller: MusicController) -> Self {
        let controller = Arc::new(Mutex::new(controller));
        Self {
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
        }
    }

    fn clear_rendered_cover(&mut self) {
        self.rendered_cover_generation = 0;
        self.rendered_cover_cache_key = None;
        self.rendered_cover_image = None;
        self.snapshot.cover_render_image = None;
    }

    fn sync_rendered_cover(&mut self) {
        let Some(cover_image) = self.snapshot.cover_render_image.clone() else {
            self.clear_rendered_cover();
            return;
        };

        if self.rendered_cover_generation == self.snapshot.cover_generation
            && self.rendered_cover_cache_key == self.snapshot.cover_cache_key
            && let Some(rendered_cover_image) = self.rendered_cover_image.clone()
        {
            self.snapshot.cover_render_image = Some(rendered_cover_image);
            return;
        }

        self.rendered_cover_generation = self.snapshot.cover_generation;
        self.rendered_cover_cache_key = self.snapshot.cover_cache_key;
        self.rendered_cover_image = Some(cover_image.clone());
        self.snapshot.cover_render_image = Some(cover_image);
    }

    fn set_snapshot(&mut self, snapshot: MusicSnapshot) {
        self.snapshot = snapshot;
        self.sync_rendered_cover();
    }

    fn refresh_controller_snapshot(
        &mut self,
        now: Instant,
        update_controller: impl FnOnce(&mut MusicController),
    ) {
        let snapshot = if let Ok(mut controller) = self.controller.lock() {
            update_controller(&mut controller);
            Some(controller.refresh_snapshot_no_cover(now, self.expanded_target_open))
        } else {
            None
        };

        if let Some(snapshot) = snapshot {
            self.set_snapshot(snapshot);
        }
    }

    fn refresh_controller_snapshot_and_persist(
        &mut self,
        now: Instant,
        update_controller: impl FnOnce(&mut MusicController),
        cx: &mut App,
    ) {
        let update = if let Ok(mut controller) = self.controller.lock() {
            let previous = controller.persisted_state();
            update_controller(&mut controller);
            let next = controller.persisted_state();
            let snapshot = controller.refresh_snapshot_no_cover(now, self.expanded_target_open);
            Some((previous, next, snapshot))
        } else {
            None
        };

        if let Some((previous, next, snapshot)) = update {
            self.set_snapshot(snapshot);
            if previous != next {
                Self::spawn_persist_music_state(next, cx);
            }
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
        })
        .detach();
    }

    fn sync_snapshot(&mut self, now: Instant, cx: &mut App) {
        let update = if let Ok(mut controller) = self.controller.lock() {
            let changed_track = controller.check_auto_next();
            let next = changed_track.then(|| controller.persisted_state());
            let snapshot = controller.refresh_snapshot_no_cover(now, self.expanded_target_open);
            Some((next, snapshot))
        } else {
            None
        };

        if let Some((next, snapshot)) = update {
            self.set_snapshot(snapshot);
            if let Some(next) = next {
                Self::spawn_persist_music_state(next, cx);
            }
        }
    }

    pub fn refresh(&mut self, now: Instant, cx: &mut App) {
        self.sync_snapshot(now, cx);
    }

    /// 快速刷新（不包含封面解码，用于切歌后首刷）
    pub fn refresh_no_cover(&mut self, now: Instant, cx: &mut App) {
        self.sync_snapshot(now, cx);
    }

    pub fn needs_passive_refresh(&self) -> bool {
        self.snapshot.is_playing || self.drag_target.is_some()
    }

    pub fn toggle_playback(&mut self, now: Instant) {
        self.refresh_controller_snapshot(now, |controller| {
            controller.toggle_playback();
            // 播放/暂停不改封面，走快路径
        });
    }

    pub fn play_next(&mut self, now: Instant, cx: &mut App) {
        self.refresh_controller_snapshot_and_persist(
            now,
            |controller| {
                controller.play_next();
                // 先更新音频，不等待封面解码
            },
            cx,
        );
    }

    pub fn play_previous(&mut self, now: Instant, cx: &mut App) {
        self.refresh_controller_snapshot_and_persist(
            now,
            |controller| {
                controller.play_previous();
                // 先更新音频，不等待封面解码
            },
            cx,
        );
    }

    pub fn set_volume(&mut self, volume: f32, now: Instant, cx: &mut App) {
        self.refresh_controller_snapshot_and_persist(
            now,
            |controller| {
                controller.set_volume(volume);
                // 音量不改封面，走快路径
            },
            cx,
        );
    }

    pub fn toggle_mute(&mut self, now: Instant, cx: &mut App) {
        self.refresh_controller_snapshot_and_persist(
            now,
            |controller| {
                controller.toggle_mute();
                // 静音不改封面，走快路径
            },
            cx,
        );
    }

    pub fn toggle_mode(&mut self, now: Instant, cx: &mut App) {
        self.refresh_controller_snapshot_and_persist(
            now,
            |controller| {
                controller.toggle_mode();
                // 模式不改封面，走快路径
            },
            cx,
        );
    }

    pub fn seek_ratio(&mut self, ratio: f32, now: Instant) {
        self.refresh_controller_snapshot(now, |controller| {
            controller.seek_ratio(ratio);
            // seek 不改封面，走快路径
        });
    }

    /// 获取当前封面解码请求
    pub fn current_cover_request(&self) -> Option<crate::music::service::CoverDecodeRequest> {
        let controller = self.controller.lock().ok()?;
        controller.current_cover_request()
    }

    /// 应用解码结果（带代际校验）
    pub fn apply_decoded_cover_if_current(
        &mut self,
        request: &crate::music::service::CoverDecodeRequest,
        cover_image: Option<Arc<RenderImage>>,
        now: Instant,
    ) {
        let snapshot = if let Ok(mut controller) = self.controller.lock() {
            if controller.apply_decoded_cover_if_current(request, cover_image) {
                Some(controller.refresh_snapshot_no_cover(now, self.expanded_target_open))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(snapshot) = snapshot {
            self.set_snapshot(snapshot);
        }
    }

    pub fn set_expanded(&mut self, expanded: bool, now: Instant) {
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
        // 关闭弹窗时清理拖拽状态，避免残留
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
        self.refresh_controller_snapshot(now, |controller| {
            controller.check_auto_next();
        });
    }

    pub fn toggle_expanded(&mut self, now: Instant) {
        self.set_expanded(!self.expanded_target_open, now);
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
                    self.seek_ratio(ratio, now);
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
        let eased = if self.expanded_to > self.expanded_from {
            ease_out_cubic(t)
        } else {
            ease_out_cubic(t)
        };
        (self.expanded_from + (self.expanded_to - self.expanded_from) * eased).clamp(0.0, 1.0)
    }

    pub fn ui_animating(&self, now: Instant) -> bool {
        self.popup_animating(now)
    }

    /// 弹窗动画状态（仅展开/收起动画，不包含拖拽）
    /// 拖拽期间不需要持续 RAF，只在 pointer move 时更新
    pub fn popup_animating(&self, now: Instant) -> bool {
        // 只检查展开/收起动画，不检查拖拽
        self.expanded_started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) < self.expanded_duration
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::image::RgbaImage;
    use std::path::PathBuf;

    #[test]
    fn decoded_cover_result_is_used_without_png_bytes() {
        let track_path = PathBuf::from("song.mp3");
        let mut controller = MusicController::new();
        controller.install_tracks(vec![crate::music::service::MusicTrack::for_test(
            track_path.clone(),
            Some(7),
        )]);
        let request = controller
            .current_cover_request()
            .expect("test track should have a current cover request");
        let mut state = MusicState::from_controller_for_test(controller);
        let render_image = Arc::new(RenderImage::new(vec![gpui::image::Frame::new(
            RgbaImage::from_pixel(2, 2, gpui::image::Rgba([1, 2, 3, 4])),
        )]));

        state.apply_decoded_cover_if_current(&request, Some(render_image.clone()), Instant::now());

        assert!(
            state
                .snapshot
                .cover_render_image
                .as_ref()
                .is_some_and(|image| Arc::ptr_eq(image, &render_image))
        );
    }
}
