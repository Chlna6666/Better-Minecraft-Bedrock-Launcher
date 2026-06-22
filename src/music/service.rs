use crate::config::config::{MusicConfig, clamp_music_volume};
use crate::music::types::{MusicPlaybackMode, MusicSnapshot};
use crate::utils::file_ops;
use anyhow::{Context, Result};
use gpui::image::Frame;
use gpui::{RenderImage, SharedString, render_fingerprint};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

const SUPPORTED_EXTENSIONS: &[&str] = &["m4a", "mp3", "wav", "flac", "ogg", "aac"];
/// 封面缩略图最大尺寸
const COVER_THUMB_MAX_SIZE: u32 = 128;

/// 封面解码请求：用于后台异步任务
#[derive(Clone, Debug)]
pub struct CoverDecodeRequest {
    pub generation: u64,
    pub track_path: PathBuf,
    pub cover_cache_key: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            !controller
                .refresh_snapshot_no_cover(Instant::now(), false)
                .is_playing
        );
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

        assert!(controller.apply_decoded_cover_if_current(&request, None));

        assert!(controller.current_cover_request().is_none());
    }
}

#[derive(Clone, Debug)]
pub struct MusicTrack {
    path: Arc<PathBuf>,
    title: SharedString,
    artist: SharedString,
    cover_key: Option<u64>, // 使用 hash 作为缓存 key，而不是直接持有 PathBuf
    duration: Duration,
}

impl MusicTrack {
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf, cover_key: Option<u64>) -> Self {
        Self {
            path: Arc::new(path),
            title: SharedString::from("Test Track"),
            artist: SharedString::from("Test Artist"),
            cover_key,
            duration: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicPersistedState {
    pub volume: f32,
    pub muted: bool,
    pub playback_mode: MusicPlaybackMode,
    pub last_track_path: String,
}

pub struct MusicController {
    tracks: Vec<MusicTrack>,
    current_index: usize,
    output_stream: Option<MixerDeviceSink>,
    sink: Option<Player>,
    paused: bool,
    volume: f32,
    muted: bool,
    mode: MusicPlaybackMode,
    last_position: Duration,
    last_error: Option<String>,
    /// 代际计数器：防止异步任务串台
    generation: u64,
    /// 封面代际计数器：每次封面变化时递增
    cover_generation: u64,
    /// 随机播放队列：预先打乱的索引列表
    play_order: Vec<usize>,
    /// 随机队列当前位置
    play_order_pos: usize,
    /// 当前封面数据缓存（避免重复读取文件）
    current_cover_path: Option<PathBuf>,
    /// 当前封面缓存键（用于 UI 层缓存 RenderImage）
    current_cover_cache_key: Option<u64>,
    /// 当前曲目的已解码缩略图。只保留当前曲目，避免播放列表常驻多张封面。
    current_cover_image: Option<Arc<RenderImage>>,
}

impl MusicController {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current_index: 0,
            output_stream: None,
            sink: None,
            paused: true,
            volume: 0.5,
            muted: false,
            mode: MusicPlaybackMode::Repeat,
            last_position: Duration::ZERO,
            last_error: None,
            generation: 0,
            cover_generation: 0,
            play_order: Vec::new(),
            play_order_pos: 0,
            current_cover_path: None,
            current_cover_cache_key: None,
            current_cover_image: None,
        }
    }

    pub fn persisted_state(&self) -> MusicPersistedState {
        MusicPersistedState {
            volume: self.volume,
            muted: self.muted,
            playback_mode: self.mode,
            last_track_path: self
                .tracks
                .get(self.current_index)
                .map(|track| track.path.as_ref().to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }

    #[cfg(test)]
    fn set_current_index_for_test(&mut self, index: usize) {
        self.current_index = index.min(self.tracks.len().saturating_sub(1));
    }

    pub fn scan_library_tracks() -> Result<Vec<MusicTrack>> {
        let music_dir = file_ops::bmcbl_subdir("music");
        fs::create_dir_all(&music_dir).with_context(|| {
            format!("failed to create music directory: {}", music_dir.display())
        })?;

        let mut tracks = Vec::new();
        for entry in fs::read_dir(&music_dir)? {
            let entry = entry?;
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase());
            if extension
                .as_deref()
                .is_none_or(|extension| !SUPPORTED_EXTENSIONS.contains(&extension))
            {
                continue;
            }

            tracks.push(Self::read_track(&path));
        }

        tracks.sort_by(|left, right| {
            left.title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase())
        });

        Ok(tracks)
    }

    fn library_matches(&self, tracks: &[MusicTrack]) -> bool {
        self.tracks.len() == tracks.len()
            && self.tracks.iter().zip(tracks).all(|(left, right)| {
                left.path == right.path
                    && left.title == right.title
                    && left.artist == right.artist
                    && left.cover_key == right.cover_key
                    && left.duration == right.duration
            })
    }

    pub fn install_tracks(&mut self, tracks: Vec<MusicTrack>) {
        let library_changed = !self.library_matches(&tracks);
        self.tracks = tracks;
        if self.current_index >= self.tracks.len() {
            self.current_index = 0;
        }
        self.last_position = Duration::ZERO;
        if library_changed {
            self.play_order.clear();
            self.play_order_pos = 0;
            self.clear_current_cover_tracking();
        }
        if self.tracks.is_empty() {
            self.release_sink();
            self.clear_current_cover_tracking();
        }
    }

    pub fn install_tracks_with_config(&mut self, tracks: Vec<MusicTrack>, config: &MusicConfig) {
        self.volume = clamp_music_volume(config.volume);
        self.muted = config.muted;
        self.mode = config.playback_mode;
        self.install_tracks(tracks);
        self.select_track_path(&config.last_track_path);

        if config.auto_play_on_startup
            && !self.tracks.is_empty()
            && let Err(err) = self.recreate_sink(false)
        {
            error!(error = %err, "music: failed to auto-start playback");
            self.last_error = Some(err.to_string());
        }
    }

    fn select_track_path(&mut self, track_path: &str) {
        let track_path = track_path.trim();
        if track_path.is_empty() {
            self.current_index = 0;
            return;
        }

        let target_path = Path::new(track_path);
        self.current_index = self
            .tracks
            .iter()
            .position(|track| track.path.as_ref() == target_path)
            .unwrap_or(0);
    }

    fn ensure_output_stream(&mut self) {
        if self.output_stream.is_none() {
            self.output_stream = DeviceSinkBuilder::open_default_sink().ok();
            if self.output_stream.is_none() {
                error!("music: no default audio output device available");
            }
        }
    }

    fn fallback_title(path: &Path) -> String {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Unknown")
            .to_string()
    }

    fn sanitize_metadata(value: Option<&str>) -> Option<String> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn read_track(path: &Path) -> MusicTrack {
        let file_stem = Self::fallback_title(path);

        let parsed = match lofty::read_from_path(path) {
            Ok(tagged_file) => Some(tagged_file),
            Err(err) => {
                warn!(path = %path.display(), error = %err, "music: failed to parse metadata");
                None
            }
        };
        let duration = parsed
            .as_ref()
            .map(|tagged_file| tagged_file.properties().duration())
            .unwrap_or(Duration::ZERO);

        let (title, artist, cover_key) = parsed
            .as_ref()
            .map(|tagged_file| {
                let tag = tagged_file
                    .primary_tag()
                    .or_else(|| tagged_file.first_tag());
                let title = Self::sanitize_metadata(tag.and_then(|tag| tag.title()).as_deref())
                    .unwrap_or_else(|| file_stem.clone());
                let artist = Self::sanitize_metadata(tag.and_then(|tag| tag.artist()).as_deref())
                    .unwrap_or_else(|| "Unknown Artist".to_string());
                let cover_key = tag
                    .and_then(|tag| tag.pictures().first())
                    .map(|_| render_fingerprint(path));
                (title, artist, cover_key)
            })
            .unwrap_or_else(|| (file_stem.clone(), "Unknown Artist".to_string(), None));

        debug!(
            path = %path.display(),
            title = %title,
            artist = %artist,
            has_cover = cover_key.is_some(),
            duration_seconds = duration.as_secs_f32(),
            "music: track indexed"
        );

        MusicTrack {
            path: Arc::new(path.to_path_buf()),
            title: SharedString::from(title),
            artist: SharedString::from(artist),
            cover_key,
            duration,
        }
    }

    /// 纯封面解码函数（无缓存，用于后台异步任务）
    /// 使用 Triangle 滤镜，性能优于 Lanczos3
    pub fn decode_cover_render_image(track_path: &Path) -> Option<Arc<RenderImage>> {
        use lofty::file::TaggedFileExt;
        use lofty::probe::Probe;

        let started = Instant::now();
        // 打开文件并读取标签
        let tagged_file = Probe::open(track_path).ok()?.read().ok()?;
        // 和 read_track() 保持一致：primary_tag() fallback 到 first_tag()
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())?;
        let picture = tag.pictures().first()?;

        // 解码图片
        let img = gpui::image::load_from_memory(picture.data()).ok()?;

        // 缩放到 128x128（Triangle 滤镜性能更好）
        let mut thumb = img
            .resize_to_fill(
                COVER_THUMB_MAX_SIZE,
                COVER_THUMB_MAX_SIZE,
                gpui::image::imageops::FilterType::Triangle,
            )
            .into_rgba8();

        for pixel in thumb.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let image = Arc::new(RenderImage::new(vec![Frame::new(thumb)]));
        let decode_elapsed = started.elapsed();
        gpui::record_image_decode_metrics_with_threshold(
            picture.data().len(),
            image.decoded_byte_len(),
            image.frame_count(),
            decode_elapsed,
            gpui::ImagePipelineConfig::default().slow_decode_threshold,
        );
        Some(image)
    }

    /// 获取当前封面解码请求
    pub fn current_cover_request(&self) -> Option<CoverDecodeRequest> {
        let track = self.tracks.get(self.current_index)?;
        let cover_cache_key = track.cover_key?;
        if self
            .current_cover_path
            .as_ref()
            .is_some_and(|path| path == track.path.as_ref())
            && self.current_cover_cache_key == Some(cover_cache_key)
        {
            return None;
        }

        Some(CoverDecodeRequest {
            generation: self.generation,
            track_path: track.path.as_ref().clone(),
            cover_cache_key: Some(cover_cache_key),
        })
    }

    /// 应用解码结果（带代际校验，旧结果自动丢弃）
    pub fn apply_decoded_cover_if_current(
        &mut self,
        request: &CoverDecodeRequest,
        cover_image: Option<Arc<RenderImage>>,
    ) -> bool {
        // 代际校验：如果当前 generation 已变化，丢弃结果
        if self.generation != request.generation {
            return false;
        }

        // 路径校验：如果当前曲目已变化，丢弃结果
        let Some(current_track) = self.tracks.get(self.current_index) else {
            return false;
        };
        if current_track.path.as_ref() != &request.track_path {
            return false;
        }

        self.current_cover_path = Some(request.track_path.clone());
        self.current_cover_cache_key = request.cover_cache_key;
        self.current_cover_image = cover_image;
        if self.current_cover_image.is_some() {
            self.cover_generation = self.cover_generation.wrapping_add(1);
        }

        true
    }

    /// 清除当前封面追踪（切歌时调用）
    fn clear_current_cover_tracking(&mut self) {
        self.current_cover_path = None;
        self.current_cover_cache_key = None;
        self.current_cover_image = None;
    }

    fn release_sink(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
            debug!("music: previous sink released");
        }
    }

    fn recreate_sink(&mut self, paused: bool) -> Result<()> {
        // 先释放旧资源
        self.release_sink();

        self.ensure_output_stream();
        let Some(output_stream) = &self.output_stream else {
            anyhow::bail!("no audio output device available")
        };
        let Some(track) = self.tracks.get(self.current_index) else {
            anyhow::bail!("no track selected")
        };

        let file = File::open(track.path.as_ref())
            .with_context(|| format!("failed to open track: {}", track.path.display()))?;
        let decoder =
            Decoder::try_from(BufReader::new(file)).context("failed to decode audio file")?;
        let sink = Player::connect_new(output_stream.mixer());
        sink.append(decoder);
        if self.last_position > Duration::ZERO {
            if let Err(err) = sink.try_seek(self.last_position) {
                warn!(error = %err, "music: failed to seek during sink recreation");
            }
        }
        sink.set_volume(if self.muted { 0.0 } else { self.volume });
        if paused {
            sink.pause();
        }
        self.paused = paused;
        self.sink = Some(sink);
        self.last_error = None;
        debug!(
            path = %track.path.display(),
            paused,
            seek_seconds = self.last_position.as_secs_f32(),
            generation = self.generation,
            "music: sink recreated"
        );
        Ok(())
    }

    /// 重建随机播放队列（Fisher-Yates shuffle）
    fn rebuild_shuffle_order(&mut self) {
        let len = self.tracks.len();
        if len == 0 {
            self.play_order.clear();
            self.play_order_pos = 0;
            return;
        }

        self.play_order.clear();
        self.play_order.reserve(len);
        self.play_order.extend(0..len);

        // Fisher-Yates shuffle
        let mut rng = StdRng::from_rng(&mut rand::rng());
        for i in (1..len).rev() {
            let j = rng.random_range(0..=i);
            self.play_order.swap(i, j);
        }

        // 将当前曲目移到队列开头
        if let Some(pos) = self
            .play_order
            .iter()
            .position(|&i| i == self.current_index)
        {
            self.play_order.swap(0, pos);
            self.play_order_pos = 0;
        } else {
            self.play_order_pos = 0;
        }

        debug!(
            play_order = ?self.play_order,
            pos = self.play_order_pos,
            "music: shuffle order rebuilt"
        );
    }

    fn choose_next_index(&mut self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        match self.mode {
            MusicPlaybackMode::Repeat => Some((self.current_index + 1) % self.tracks.len()),
            MusicPlaybackMode::Shuffle => {
                if self.play_order.is_empty() || self.play_order_pos >= self.play_order.len() {
                    self.rebuild_shuffle_order();
                }

                // 移动到下一首
                self.play_order_pos = (self.play_order_pos + 1) % self.play_order.len();
                Some(self.play_order[self.play_order_pos])
            }
        }
    }

    fn choose_prev_index(&mut self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        match self.mode {
            MusicPlaybackMode::Repeat => {
                Some((self.current_index + self.tracks.len() - 1) % self.tracks.len())
            }
            MusicPlaybackMode::Shuffle => {
                if self.play_order.is_empty() {
                    self.rebuild_shuffle_order();
                }

                // 移动到上一首
                if self.play_order_pos == 0 {
                    self.play_order_pos = self.play_order.len() - 1;
                } else {
                    self.play_order_pos -= 1;
                }
                Some(self.play_order[self.play_order_pos])
            }
        }
    }

    pub fn toggle_playback(&mut self) {
        if self.tracks.is_empty() {
            return;
        }

        if self.sink.is_none() {
            if let Err(err) = self.recreate_sink(false) {
                error!(error = %err, "music: failed to start playback");
                self.last_error = Some(err.to_string());
            }
            return;
        }

        if let Some(sink) = &self.sink {
            self.last_position = sink.get_pos();
        }
        self.release_sink();
        self.paused = true;
        debug!(
            position_seconds = self.last_position.as_secs_f32(),
            "music: playback paused and sink released"
        );
    }

    pub fn play_next(&mut self) {
        // 先停止并释放旧资源，确保旧音频流被正确释放
        // release_sink() 会自动清除封面追踪
        self.release_sink();

        let Some(next_index) = self.choose_next_index() else {
            return;
        };
        self.current_index = next_index;
        self.last_position = Duration::ZERO;
        self.paused = false;
        self.clear_current_cover_tracking();

        // 增加代际，防止旧异步任务干扰
        self.generation = self.generation.wrapping_add(1);

        if let Err(err) = self.recreate_sink(false) {
            error!(error = %err, index = self.current_index, "music: failed to play next track");
            self.last_error = Some(err.to_string());
        }
    }

    pub fn play_previous(&mut self) {
        // 先停止并释放旧资源，确保旧音频流被正确释放
        // release_sink() 会自动清除封面追踪
        self.release_sink();

        let Some(prev_index) = self.choose_prev_index() else {
            return;
        };
        self.current_index = prev_index;
        self.last_position = Duration::ZERO;
        self.paused = false;
        self.clear_current_cover_tracking();

        // 增加代际，防止旧异步任务干扰
        self.generation = self.generation.wrapping_add(1);

        if let Err(err) = self.recreate_sink(false) {
            error!(error = %err, index = self.current_index, "music: failed to play previous track");
            self.last_error = Some(err.to_string());
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(if self.muted { 0.0 } else { self.volume });
        }
        debug!(
            volume = self.volume,
            muted = self.muted,
            "music: volume changed"
        );
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Some(sink) = &self.sink {
            sink.set_volume(if self.muted { 0.0 } else { self.volume });
        }
        debug!(
            muted = self.muted,
            volume = self.volume,
            "music: mute toggled"
        );
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            MusicPlaybackMode::Repeat => {
                // 切到 Shuffle 时立即重建随机队列
                self.rebuild_shuffle_order();
                MusicPlaybackMode::Shuffle
            }
            MusicPlaybackMode::Shuffle => MusicPlaybackMode::Repeat,
        };
        debug!(mode = ?self.mode, "music: playback mode changed");
    }

    pub fn seek_ratio(&mut self, ratio: f32) {
        let Some(track) = self.tracks.get(self.current_index) else {
            return;
        };
        self.last_position = track.duration.mul_f32(ratio.clamp(0.0, 1.0));
        if let Some(sink) = &self.sink {
            if sink.try_seek(self.last_position).is_ok() {
                debug!(
                    ratio = ratio.clamp(0.0, 1.0),
                    seconds = self.last_position.as_secs_f32(),
                    "music: seek applied"
                );
                return;
            }
        } else {
            self.paused = true;
            return;
        }
        if let Err(err) = self.recreate_sink(self.paused) {
            error!(error = %err, ratio = ratio.clamp(0.0, 1.0), "music: seek fallback recreate failed");
            self.last_error = Some(err.to_string());
        }
    }

    /// 检查是否需要自动播放下一首（曲终自动下一首）
    /// 应该在 refresh_snapshot() 之前调用
    pub fn check_auto_next(&mut self) -> bool {
        if let Some(sink) = &self.sink {
            if !self.paused && sink.empty() && !self.tracks.is_empty() {
                self.play_next();
                return true;
            }
        }
        false
    }

    /// 刷新快照（不包含封面解码，用于快速响应切歌）
    /// 封面解码在后台线程进行
    pub fn refresh_snapshot_no_cover(&mut self, _now: Instant, expanded: bool) -> MusicSnapshot {
        let current_track = self.tracks.get(self.current_index);
        let current_seconds = self
            .sink
            .as_ref()
            .map(|sink| sink.get_pos().as_secs_f32())
            .unwrap_or_else(|| self.last_position.as_secs_f32());
        let total_seconds = current_track
            .map(|track| track.duration.as_secs_f32())
            .unwrap_or(0.0);

        let title = if let Some(track) = current_track {
            track.title.clone()
        } else if let Some(error) = &self.last_error {
            SharedString::from(error.clone())
        } else {
            SharedString::from("Not Playing")
        };

        let artist = current_track
            .map(|track| track.artist.clone())
            .unwrap_or_else(|| SharedString::from("BMCBL/music"));

        let cover_image = current_track.and_then(|track| {
            let current_cover_path = self.current_cover_path.as_ref()?;
            if track.path.as_ref() == current_cover_path {
                self.current_cover_image.clone()
            } else {
                None
            }
        });

        // 封面缓存键直接使用当前曲目的 cover_key
        let cover_cache_key = current_track.and_then(|track| track.cover_key);

        // 曲目的路径（用于后台解码校验）
        let track_path = current_track.map(|track| track.path.clone());

        MusicSnapshot {
            available: !self.tracks.is_empty(),
            title,
            artist,
            cover_render_image: cover_image,
            last_error: self.last_error.clone().map(SharedString::from),
            current_seconds,
            total_seconds,
            is_playing: !self.paused && self.sink.is_some(),
            muted: self.muted,
            volume: self.volume,
            mode: self.mode,
            expanded,
            generation: self.generation,
            cover_generation: self.cover_generation,
            cover_cache_key,
            track_path,
        }
    }
}
