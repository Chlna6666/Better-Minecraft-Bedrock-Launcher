use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MusicPlaybackMode {
    Shuffle,
    #[default]
    Repeat,
}

#[derive(Clone, Debug, Default)]
pub struct MusicPlaybackSnapshot {
    pub available: bool,
    pub title: String,
    pub artist: String,
    pub last_error: Option<String>,
    pub current_seconds: f32,
    pub total_seconds: f32,
    pub is_playing: bool,
    pub muted: bool,
    pub volume: f32,
    pub mode: MusicPlaybackMode,
    pub generation: u64,
    pub cover_generation: u64,
    pub cover_cache_key: Option<u64>,
    pub track_path: Option<Arc<PathBuf>>,
}

#[derive(Clone, Debug)]
pub struct DecodedCoverImage {
    pub width: u32,
    pub height: u32,
    pub bgra_pixels: Vec<u8>,
    pub source_byte_len: usize,
    pub decode_elapsed: Duration,
}
