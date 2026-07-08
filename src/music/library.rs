use crate::music::cover::{cover_fingerprint, has_embedded_cover};
use crate::utils::file_ops;
use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const SUPPORTED_EXTENSIONS: &[&str] = &["m4a", "mp3", "wav", "flac", "ogg", "aac"];

#[derive(Clone, Debug)]
pub struct MusicTrack {
    pub(super) path: Arc<PathBuf>,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) cover_key: Option<u64>,
    pub(super) duration: Duration,
}

impl MusicTrack {
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf, cover_key: Option<u64>) -> Self {
        Self {
            path: Arc::new(path),
            title: "Test Track".to_string(),
            artist: "Test Artist".to_string(),
            cover_key,
            duration: Duration::from_secs(1),
        }
    }
}

pub fn scan_library_tracks() -> Result<Vec<MusicTrack>> {
    let music_dir = file_ops::bmcbl_subdir("music");
    fs::create_dir_all(&music_dir)
        .with_context(|| format!("failed to create music directory: {}", music_dir.display()))?;

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

        tracks.push(read_track(&path));
    }

    tracks.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
    });

    Ok(tracks)
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
    let file_stem = fallback_title(path);

    let parsed = match lofty::read_from_path(path) {
        Ok(tagged_file) => Some(tagged_file),
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "music: failed to parse metadata"
            );
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
            let title = sanitize_metadata(tag.and_then(|tag| tag.title()).as_deref())
                .unwrap_or_else(|| file_stem.clone());
            let artist = sanitize_metadata(tag.and_then(|tag| tag.artist()).as_deref())
                .unwrap_or_else(|| "Unknown Artist".to_string());
            let cover_key = has_embedded_cover(tagged_file).then(|| cover_fingerprint(path));
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
        title,
        artist,
        cover_key,
        duration,
    }
}
