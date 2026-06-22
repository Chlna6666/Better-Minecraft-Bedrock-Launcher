use gpui::{RenderImage, SharedString};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MusicPlaybackMode {
    Shuffle,
    #[default]
    Repeat,
}

#[derive(Clone, Default)]
pub struct MusicSnapshot {
    pub available: bool,
    pub title: SharedString,
    pub artist: SharedString,
    /// 已经构建好的 UI 封面，避免 render 路径重复解码 PNG。
    pub cover_render_image: Option<Arc<RenderImage>>,
    pub last_error: Option<SharedString>,
    pub current_seconds: f32,
    pub total_seconds: f32,
    pub is_playing: bool,
    pub muted: bool,
    pub volume: f32,
    pub mode: MusicPlaybackMode,
    pub expanded: bool,
    /// 代际标记：用于检测切歌
    pub generation: u64,
    /// 封面代际标记：用于检测封面是否变化
    pub cover_generation: u64,
    /// 封面缓存键：用于 UI 层缓存 RenderImage
    pub cover_cache_key: Option<u64>,
    /// 当前曲目的路径：用于后台解码校验
    pub track_path: Option<Arc<PathBuf>>,
}

impl std::fmt::Debug for MusicSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MusicSnapshot")
            .field("available", &self.available)
            .field("title", &self.title)
            .field("artist", &self.artist)
            .field("has_cover_render_image", &self.cover_render_image.is_some())
            .field("last_error", &self.last_error)
            .field("current_seconds", &self.current_seconds)
            .field("total_seconds", &self.total_seconds)
            .field("is_playing", &self.is_playing)
            .field("muted", &self.muted)
            .field("volume", &self.volume)
            .field("mode", &self.mode)
            .field("expanded", &self.expanded)
            .field("generation", &self.generation)
            .field("cover_generation", &self.cover_generation)
            .field("cover_cache_key", &self.cover_cache_key)
            .field("track_path", &self.track_path)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MusicDragTarget {
    Progress,
    Volume,
}
