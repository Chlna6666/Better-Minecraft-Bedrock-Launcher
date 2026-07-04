use crate::config::config::{MusicConfig, clamp_music_volume};
use crate::music::cover::decode_cover_thumbnail as decode_cover_thumbnail_data;
use crate::music::library::{self, MusicTrack};
use crate::music::types::{DecodedCoverImage, MusicPlaybackMode, MusicPlaybackSnapshot};
use anyhow::{Context, Result};
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, error, warn};

#[derive(Clone, Debug)]
pub struct CoverDecodeRequest {
    pub generation: u64,
    pub track_path: PathBuf,
    pub cover_cache_key: Option<u64>,
}

#[cfg(test)]
mod service_tests;

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
    generation: u64,
    cover_generation: u64,
    play_order: Vec<usize>,
    play_order_pos: usize,
    current_cover_path: Option<PathBuf>,
    current_cover_cache_key: Option<u64>,
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
        library::scan_library_tracks()
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

    pub fn decode_cover_thumbnail(track_path: &Path) -> Option<DecodedCoverImage> {
        decode_cover_thumbnail_data(track_path)
    }

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

    pub fn apply_decoded_cover_if_current(
        &mut self,
        request: &CoverDecodeRequest,
        decoded: bool,
    ) -> bool {
        if self.generation != request.generation {
            return false;
        }

        let Some(current_track) = self.tracks.get(self.current_index) else {
            return false;
        };
        if current_track.path.as_ref() != &request.track_path {
            return false;
        }

        self.current_cover_path = Some(request.track_path.clone());
        self.current_cover_cache_key = request.cover_cache_key;
        if decoded {
            self.cover_generation = self.cover_generation.wrapping_add(1);
        }

        true
    }

    fn clear_current_cover_tracking(&mut self) {
        self.current_cover_path = None;
        self.current_cover_cache_key = None;
    }

    fn release_sink(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
            debug!("music: previous sink released");
        }
    }

    fn recreate_sink(&mut self, paused: bool) -> Result<()> {
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

        let mut rng = StdRng::from_rng(&mut rand::rng());
        for i in (1..len).rev() {
            let j = rng.random_range(0..=i);
            self.play_order.swap(i, j);
        }

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
        self.release_sink();

        let Some(next_index) = self.choose_next_index() else {
            return;
        };
        self.current_index = next_index;
        self.last_position = Duration::ZERO;
        self.paused = false;
        self.clear_current_cover_tracking();

        self.generation = self.generation.wrapping_add(1);

        if let Err(err) = self.recreate_sink(false) {
            error!(error = %err, index = self.current_index, "music: failed to play next track");
            self.last_error = Some(err.to_string());
        }
    }

    pub fn play_previous(&mut self) {
        self.release_sink();

        let Some(prev_index) = self.choose_prev_index() else {
            return;
        };
        self.current_index = prev_index;
        self.last_position = Duration::ZERO;
        self.paused = false;
        self.clear_current_cover_tracking();

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

    pub fn needs_auto_next(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|sink| !self.paused && sink.empty() && !self.tracks.is_empty())
    }

    pub fn refresh_snapshot_no_cover(&mut self) -> MusicPlaybackSnapshot {
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
            error.clone()
        } else {
            "Not Playing".to_string()
        };

        let artist = current_track
            .map(|track| track.artist.clone())
            .unwrap_or_else(|| "BMCBL/music".to_string());

        let cover_cache_key = current_track.and_then(|track| track.cover_key);

        let track_path = current_track.map(|track| track.path.clone());

        MusicPlaybackSnapshot {
            available: !self.tracks.is_empty(),
            title,
            artist,
            last_error: self.last_error.clone(),
            current_seconds,
            total_seconds,
            is_playing: !self.paused && self.sink.is_some(),
            muted: self.muted,
            volume: self.volume,
            mode: self.mode,
            generation: self.generation,
            cover_generation: self.cover_generation,
            cover_cache_key,
            track_path,
        }
    }
}
