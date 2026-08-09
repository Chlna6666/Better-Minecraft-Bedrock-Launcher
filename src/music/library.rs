use crate::music::cover::{cover_fingerprint, has_embedded_cover};
use crate::utils::file_ops;
use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const SUPPORTED_EXTENSIONS: &[&str] = &["m4a", "mp3", "wav", "flac", "ogg", "aac"];
const PLUGIN_AUDIO_CACHE_REVISION: u8 = 2;

#[derive(Clone, Debug)]
pub struct MusicTrack {
    pub(super) path: Arc<PathBuf>,
    pub(super) playback_path: Arc<PathBuf>,
    pub(super) cover_path: Option<Arc<PathBuf>>,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) cover_key: Option<u64>,
    pub(super) duration: Duration,
}

impl MusicTrack {
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf, cover_key: Option<u64>) -> Self {
        Self {
            path: Arc::new(path.clone()),
            playback_path: Arc::new(path),
            cover_path: cover_key.map(|_| Arc::new(PathBuf::from("test-cover.jpg"))),
            title: "Test Track".to_string(),
            artist: "Test Artist".to_string(),
            cover_key,
            duration: Duration::from_secs(1),
        }
    }
}

pub fn scan_library_tracks(
    audio_decoders: &[crate::plugins::runtime::PluginAudioDecoder],
) -> Result<Vec<MusicTrack>> {
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
        let Some(extension) = extension.as_deref() else {
            continue;
        };
        if SUPPORTED_EXTENSIONS.contains(&extension) {
            tracks.push(read_track(&path, &path, None, None));
        } else if let Some(decoder) = audio_decoders
            .iter()
            .find(|decoder| decoder.supports_extension(extension))
        {
            match decode_plugin_track(&path, decoder) {
                Ok(track) => tracks.push(track),
                Err(error) => warn!(path = %path.display(), %error, "music: plugin decode failed"),
            }
        }
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

fn read_track(
    source_path: &Path,
    playback_path: &Path,
    metadata: Option<&bmcbl_plugin_api::AudioTrackMetadata>,
    external_cover_path: Option<&Path>,
) -> MusicTrack {
    let file_stem = fallback_title(source_path);

    let parsed = match lofty::read_from_path(playback_path) {
        Ok(tagged_file) => Some(tagged_file),
        Err(err) => {
            warn!(
                path = %playback_path.display(),
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

    let (title, artist, has_embedded_cover) = parsed
        .as_ref()
        .map(|tagged_file| {
            let tag = tagged_file
                .primary_tag()
                .or_else(|| tagged_file.first_tag());
            let title = sanitize_metadata(tag.and_then(|tag| tag.title()).as_deref())
                .unwrap_or_else(|| file_stem.clone());
            let artist = sanitize_metadata(tag.and_then(|tag| tag.artist()).as_deref())
                .unwrap_or_else(|| "Unknown Artist".to_string());
            (title, artist, has_embedded_cover(tagged_file))
        })
        .unwrap_or_else(|| (file_stem.clone(), "Unknown Artist".to_string(), false));
    let title = metadata
        .map(|metadata| metadata.title.trim())
        .filter(|title| !title.is_empty())
        .map_or(title, str::to_string);
    let artist = metadata
        .map(|metadata| metadata.artists.join(" / "))
        .filter(|artist| !artist.is_empty())
        .unwrap_or(artist);
    let cover_path = external_cover_path
        .map(Path::to_path_buf)
        .or_else(|| has_embedded_cover.then(|| playback_path.to_path_buf()));
    let cover_key = cover_path.as_deref().map(cover_fingerprint);

    debug!(
        path = %source_path.display(),
        title = %title,
        artist = %artist,
        has_cover = cover_key.is_some(),
        duration_seconds = duration.as_secs_f32(),
        "music: track indexed"
    );

    MusicTrack {
        path: Arc::new(source_path.to_path_buf()),
        playback_path: Arc::new(playback_path.to_path_buf()),
        cover_path: cover_path.map(Arc::new),
        title,
        artist,
        cover_key,
        duration,
    }
}

fn decode_plugin_track(
    source_path: &Path,
    decoder: &crate::plugins::runtime::PluginAudioDecoder,
) -> Result<MusicTrack> {
    let cache_dir = crate::utils::file_ops::cache_subdir("music-decoded");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create music decode cache {}", cache_dir.display()))?;
    let cache_key = decoded_cache_key(source_path)?;
    for extension in SUPPORTED_EXTENSIONS {
        let cached_path = cache_dir.join(format!("{cache_key}.{extension}"));
        if cached_path.exists() {
            let cover_path = cached_cover_path(&cache_dir, &cache_key);
            let no_cover_marker = cache_dir.join(format!("{cache_key}.cover.none"));
            if cover_path.is_some() || no_cover_marker.exists() {
                return Ok(read_track(
                    source_path,
                    &cached_path,
                    None,
                    cover_path.as_deref(),
                ));
            }
        }
    }

    let temporary_path = cache_dir.join(format!("{cache_key}.tmp"));
    let response = match decoder.decode_to_path(source_path, &temporary_path) {
        Ok(response) => response,
        Err(error) => {
            remove_temporary_decode(&temporary_path);
            return Err(error);
        }
    };
    let extension = response.format_extension.to_ascii_lowercase();
    if !SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
        remove_temporary_decode(&temporary_path);
        anyhow::bail!("plugin returned unsupported audio extension {extension}");
    }
    let decoded_path = cache_dir.join(format!("{cache_key}.{extension}"));
    if decoded_path.exists() {
        remove_temporary_decode(&temporary_path);
    } else {
        fs::rename(&temporary_path, &decoded_path).with_context(|| {
            format!(
                "publish decoded audio {} -> {}",
                temporary_path.display(),
                decoded_path.display()
            )
        })?;
    }
    let cover_path = publish_plugin_cover(&cache_dir, &cache_key, response.cover.as_ref())?;
    Ok(read_track(
        source_path,
        &decoded_path,
        Some(&response.metadata),
        cover_path.as_deref(),
    ))
}

fn cached_cover_path(cache_dir: &Path, cache_key: &str) -> Option<PathBuf> {
    ["jpg", "png"]
        .into_iter()
        .map(|extension| cache_dir.join(format!("{cache_key}.cover.{extension}")))
        .find(|path| path.exists())
}

fn publish_plugin_cover(
    cache_dir: &Path,
    cache_key: &str,
    cover: Option<&crate::plugins::runtime::PluginAudioCover>,
) -> Result<Option<PathBuf>> {
    let Some(cover) = cover else {
        fs::write(cache_dir.join(format!("{cache_key}.cover.none")), [])?;
        return Ok(None);
    };
    if cover.bytes.is_empty() || cover.bytes.len() > bmcbl_plugin_api::MAX_AUDIO_COVER_BYTES {
        fs::write(cache_dir.join(format!("{cache_key}.cover.none")), [])?;
        return Ok(None);
    }
    let extension = match cover.mime_type.as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        _ => {
            fs::write(cache_dir.join(format!("{cache_key}.cover.none")), [])?;
            return Ok(None);
        }
    };
    let cover_path = cache_dir.join(format!("{cache_key}.cover.{extension}"));
    let temporary_path = cache_dir.join(format!("{cache_key}.cover.tmp"));
    fs::write(&temporary_path, &cover.bytes)
        .with_context(|| format!("write decoded cover {}", temporary_path.display()))?;
    fs::rename(&temporary_path, &cover_path).with_context(|| {
        format!(
            "publish decoded cover {} -> {}",
            temporary_path.display(),
            cover_path.display()
        )
    })?;
    Ok(Some(cover_path))
}

fn remove_temporary_decode(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(path = %path.display(), %error, "music: failed to remove temporary decode")
        }
    }
}

fn decoded_cache_key(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("read audio source metadata {}", path.display()))?;
    let mut hasher = DefaultHasher::new();
    PLUGIN_AUDIO_CACHE_REVISION.hash(&mut hasher);
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}
