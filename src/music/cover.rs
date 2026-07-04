use crate::music::types::DecodedCoverImage;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Instant;

use lofty::file::TaggedFileExt;
use lofty::probe::Probe;

const COVER_THUMB_MAX_SIZE: u32 = 128;

pub fn cover_fingerprint(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);

    if let Ok(metadata) = std::fs::metadata(path) {
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            duration.as_nanos().hash(&mut hasher);
        }
    }

    hasher.finish()
}

pub fn decode_cover_thumbnail(track_path: &Path) -> Option<DecodedCoverImage> {
    let started = Instant::now();
    let tagged_file = Probe::open(track_path).ok()?.read().ok()?;
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())?;
    let picture = tag.pictures().first()?;
    let source_byte_len = picture.data().len();

    let image = image::load_from_memory(picture.data()).ok()?;
    let mut thumbnail = image
        .resize_to_fill(
            COVER_THUMB_MAX_SIZE,
            COVER_THUMB_MAX_SIZE,
            image::imageops::FilterType::Triangle,
        )
        .into_rgba8();

    for pixel in thumbnail.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let (width, height) = thumbnail.dimensions();
    Some(DecodedCoverImage {
        width,
        height,
        bgra_pixels: thumbnail.into_raw(),
        source_byte_len,
        decode_elapsed: started.elapsed(),
    })
}
