use crate::music::{DecodedCoverImage, MusicPlaybackMode, MusicPlaybackSnapshot};
use gpui::{RenderImage, SharedString};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct MusicSnapshot {
    pub available: bool,
    pub title: SharedString,
    pub artist: SharedString,
    pub cover_render_image: Option<Arc<RenderImage>>,
    pub last_error: Option<SharedString>,
    pub current_seconds: f32,
    pub total_seconds: f32,
    pub is_playing: bool,
    pub muted: bool,
    pub volume: f32,
    pub mode: MusicPlaybackMode,
    pub expanded: bool,
    pub generation: u64,
    pub cover_generation: u64,
    pub cover_cache_key: Option<u64>,
    pub track_path: Option<Arc<std::path::PathBuf>>,
}

impl MusicSnapshot {
    pub(super) fn from_playback(snapshot: MusicPlaybackSnapshot, expanded: bool) -> Self {
        Self {
            available: snapshot.available,
            title: SharedString::from(snapshot.title),
            artist: SharedString::from(snapshot.artist),
            cover_render_image: None,
            last_error: snapshot.last_error.map(SharedString::from),
            current_seconds: snapshot.current_seconds,
            total_seconds: snapshot.total_seconds,
            is_playing: snapshot.is_playing,
            muted: snapshot.muted,
            volume: snapshot.volume,
            mode: snapshot.mode,
            expanded,
            generation: snapshot.generation,
            cover_generation: snapshot.cover_generation,
            cover_cache_key: snapshot.cover_cache_key,
            track_path: snapshot.track_path,
        }
    }
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

pub(super) fn render_image_from_decoded_cover(
    decoded_cover: DecodedCoverImage,
) -> Option<Arc<RenderImage>> {
    let decoded_byte_len = decoded_cover.bgra_pixels.len();
    let image_buffer = gpui::image::RgbaImage::from_raw(
        decoded_cover.width,
        decoded_cover.height,
        decoded_cover.bgra_pixels,
    )?;
    let render_image = Arc::new(RenderImage::new(vec![gpui::image::Frame::new(
        image_buffer,
    )]));
    gpui::record_image_decode_metrics_with_threshold(
        decoded_cover.source_byte_len,
        decoded_byte_len,
        render_image.frame_count(),
        decoded_cover.decode_elapsed,
        gpui::ImagePipelineConfig::default().slow_decode_threshold,
    );
    Some(render_image)
}
