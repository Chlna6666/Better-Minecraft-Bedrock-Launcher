use super::{CoverDecodeRequest, MusicController};
use crate::music::cover::decode_cover_thumbnail as decode_cover_thumbnail_data;
use crate::music::cover_cache::{COVER_PRELOAD_LIMIT, decode_cover_thumbnail_cached};
use crate::music::library::MusicTrack;
use crate::music::types::DecodedCoverImage;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverPreloadItem {
    track_path: PathBuf,
    cover_cache_key: u64,
}

impl CoverPreloadItem {
    fn new(track_path: PathBuf, cover_cache_key: u64) -> Self {
        Self {
            track_path,
            cover_cache_key,
        }
    }
}

impl MusicController {
    pub fn decode_cover_thumbnail(request: &CoverDecodeRequest) -> Option<DecodedCoverImage> {
        match request.cover_cache_key {
            Some(cover_cache_key) => {
                decode_cover_thumbnail_cached(cover_cache_key, &request.track_path)
            }
            None => decode_cover_thumbnail_data(&request.track_path),
        }
    }

    pub fn cover_preload_items(
        tracks: &[MusicTrack],
        preferred_track_path: &str,
    ) -> Vec<CoverPreloadItem> {
        let preferred_track_path = preferred_track_path.trim();
        let preferred_track_path =
            (!preferred_track_path.is_empty()).then(|| Path::new(preferred_track_path));
        let mut items = Vec::with_capacity(COVER_PRELOAD_LIMIT.min(tracks.len()));

        if let Some(preferred_track_path) = preferred_track_path {
            Self::push_cover_preload_items(
                &mut items,
                tracks
                    .iter()
                    .filter(|track| track.path.as_ref() == preferred_track_path),
            );
        }

        Self::push_cover_preload_items(
            &mut items,
            tracks.iter().filter(|track| {
                preferred_track_path
                    .is_none_or(|preferred_track_path| track.path.as_ref() != preferred_track_path)
            }),
        );

        items
    }

    pub fn preload_cover_cache(items: Vec<CoverPreloadItem>) {
        if items.is_empty() {
            return;
        }

        let requested_count = items.len();
        let mut decoded_count = 0;
        for item in items {
            if decode_cover_thumbnail_cached(item.cover_cache_key, &item.track_path).is_some() {
                decoded_count += 1;
            }
        }

        debug!(
            requested_count,
            decoded_count, "music: cover cache preload completed"
        );
    }

    fn push_cover_preload_items<'a>(
        items: &mut Vec<CoverPreloadItem>,
        tracks: impl Iterator<Item = &'a MusicTrack>,
    ) {
        for track in tracks {
            if items.len() >= COVER_PRELOAD_LIMIT {
                return;
            }
            let Some(cover_cache_key) = track.cover_key else {
                continue;
            };
            if items
                .iter()
                .any(|item| item.cover_cache_key == cover_cache_key)
            {
                continue;
            }
            items.push(CoverPreloadItem::new(
                track.path.as_ref().clone(),
                cover_cache_key,
            ));
        }
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

    pub(super) fn clear_current_cover_tracking(&mut self) {
        self.current_cover_path = None;
        self.current_cover_cache_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, cover_cache_key: Option<u64>) -> MusicTrack {
        MusicTrack::for_test(PathBuf::from(path), cover_cache_key)
    }

    #[test]
    fn cover_preload_items_puts_preferred_track_first() {
        let tracks = vec![track("one.mp3", Some(1)), track("two.mp3", Some(2))];

        let items = MusicController::cover_preload_items(&tracks, "two.mp3");

        assert_eq!(
            items,
            vec![
                CoverPreloadItem::new(PathBuf::from("two.mp3"), 2),
                CoverPreloadItem::new(PathBuf::from("one.mp3"), 1),
            ]
        );
    }

    #[test]
    fn cover_preload_items_skips_tracks_without_cover() {
        let tracks = vec![track("one.mp3", None), track("two.mp3", Some(2))];

        let items = MusicController::cover_preload_items(&tracks, "");

        assert_eq!(
            items,
            vec![CoverPreloadItem::new(PathBuf::from("two.mp3"), 2)]
        );
    }

    #[test]
    fn cover_preload_items_deduplicates_cache_keys() {
        let tracks = vec![track("one.mp3", Some(1)), track("same.mp3", Some(1))];

        let items = MusicController::cover_preload_items(&tracks, "");

        assert_eq!(
            items,
            vec![CoverPreloadItem::new(PathBuf::from("one.mp3"), 1)]
        );
    }
}
