use crate::music::cover;
use crate::music::types::DecodedCoverImage;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tracing::{debug, warn};

pub(super) const COVER_PRELOAD_LIMIT: usize = 16;

const COVER_CACHE_ENTRY_LIMIT: usize = 32;
const COVER_CACHE_BYTE_LIMIT: usize = 4 * 1024 * 1024;

static COVER_IMAGE_CACHE: OnceLock<Mutex<CoverImageCache>> = OnceLock::new();

#[derive(Clone)]
struct CachedCoverImage {
    image: DecodedCoverImage,
    decoded_byte_len: usize,
}

struct CoverImageCache {
    entries: HashMap<u64, CachedCoverImage>,
    usage_order: VecDeque<u64>,
    entry_limit: usize,
    decoded_byte_limit: usize,
    decoded_byte_len: usize,
}

impl CoverImageCache {
    fn new(entry_limit: usize, decoded_byte_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            usage_order: VecDeque::new(),
            entry_limit,
            decoded_byte_limit,
            decoded_byte_len: 0,
        }
    }

    fn get(&mut self, cover_cache_key: u64, started: Instant) -> Option<DecodedCoverImage> {
        let mut image = self
            .entries
            .get(&cover_cache_key)
            .map(|entry| entry.image.clone())?;
        image.decode_elapsed = started.elapsed();
        self.touch(cover_cache_key);
        Some(image)
    }

    fn insert(&mut self, cover_cache_key: u64, image: DecodedCoverImage) {
        if self.entry_limit == 0 || self.decoded_byte_limit == 0 {
            return;
        }

        let decoded_byte_len = image.bgra_pixels.len();
        if decoded_byte_len > self.decoded_byte_limit {
            self.remove(cover_cache_key);
            return;
        }

        self.remove(cover_cache_key);
        self.decoded_byte_len = self.decoded_byte_len.saturating_add(decoded_byte_len);
        self.entries.insert(
            cover_cache_key,
            CachedCoverImage {
                image,
                decoded_byte_len,
            },
        );
        self.usage_order.push_back(cover_cache_key);
        self.evict_over_limit();
    }

    fn touch(&mut self, cover_cache_key: u64) {
        self.remove_from_usage_order(cover_cache_key);
        self.usage_order.push_back(cover_cache_key);
    }

    fn remove(&mut self, cover_cache_key: u64) {
        if let Some(entry) = self.entries.remove(&cover_cache_key) {
            self.decoded_byte_len = self.decoded_byte_len.saturating_sub(entry.decoded_byte_len);
        }
        self.remove_from_usage_order(cover_cache_key);
    }

    fn remove_from_usage_order(&mut self, cover_cache_key: u64) {
        if let Some(index) = self
            .usage_order
            .iter()
            .position(|key| *key == cover_cache_key)
        {
            self.usage_order.remove(index);
        }
    }

    fn evict_over_limit(&mut self) {
        while self.entries.len() > self.entry_limit
            || self.decoded_byte_len > self.decoded_byte_limit
        {
            let Some(cover_cache_key) = self.usage_order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&cover_cache_key) {
                self.decoded_byte_len =
                    self.decoded_byte_len.saturating_sub(entry.decoded_byte_len);
            }
        }
    }
}

fn cover_image_cache() -> &'static Mutex<CoverImageCache> {
    COVER_IMAGE_CACHE.get_or_init(|| {
        Mutex::new(CoverImageCache::new(
            COVER_CACHE_ENTRY_LIMIT,
            COVER_CACHE_BYTE_LIMIT,
        ))
    })
}

fn cached_cover_image(cover_cache_key: u64, started: Instant) -> Option<DecodedCoverImage> {
    match cover_image_cache().lock() {
        Ok(mut cache) => cache.get(cover_cache_key, started),
        Err(error) => {
            warn!("music: cover cache lock poisoned while reading: {error}");
            None
        }
    }
}

fn cache_decoded_cover(cover_cache_key: u64, image: DecodedCoverImage) {
    match cover_image_cache().lock() {
        Ok(mut cache) => cache.insert(cover_cache_key, image),
        Err(error) => warn!("music: cover cache lock poisoned while writing: {error}"),
    }
}

pub(super) fn decode_cover_thumbnail_cached(
    cover_cache_key: u64,
    track_path: &Path,
) -> Option<DecodedCoverImage> {
    let started = Instant::now();
    if let Some(image) = cached_cover_image(cover_cache_key, started) {
        debug!(cover_cache_key, "music: cover cache hit");
        return Some(image);
    }

    let decoded = cover::decode_cover_thumbnail(track_path)?;
    cache_decoded_cover(cover_cache_key, decoded.clone());
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn decoded_cover(decoded_byte_len: usize) -> DecodedCoverImage {
        DecodedCoverImage {
            width: 1,
            height: 1,
            bgra_pixels: vec![0; decoded_byte_len],
            source_byte_len: decoded_byte_len,
            decode_elapsed: Duration::from_secs(1),
        }
    }

    #[test]
    fn cache_returns_entry_by_key() {
        let mut cache = CoverImageCache::new(2, usize::MAX);
        cache.insert(10, decoded_cover(4));

        let image = cache.get(10, Instant::now());

        assert_eq!(image.map(|image| image.source_byte_len), Some(4));
    }

    #[test]
    fn cache_evicts_least_recent_entry_when_entry_limit_is_exceeded() {
        let mut cache = CoverImageCache::new(2, usize::MAX);
        cache.insert(1, decoded_cover(4));
        cache.insert(2, decoded_cover(4));
        assert!(cache.get(1, Instant::now()).is_some());

        cache.insert(3, decoded_cover(4));

        assert!(cache.get(1, Instant::now()).is_some());
        assert!(cache.get(2, Instant::now()).is_none());
        assert!(cache.get(3, Instant::now()).is_some());
    }

    #[test]
    fn cache_evicts_until_decoded_byte_limit_is_met() {
        let mut cache = CoverImageCache::new(8, 8);
        cache.insert(1, decoded_cover(4));
        cache.insert(2, decoded_cover(4));
        assert!(cache.get(1, Instant::now()).is_some());

        cache.insert(3, decoded_cover(4));

        assert!(cache.get(1, Instant::now()).is_some());
        assert!(cache.get(2, Instant::now()).is_none());
        assert!(cache.get(3, Instant::now()).is_some());
    }

    #[test]
    fn cache_does_not_store_entry_larger_than_byte_limit() {
        let mut cache = CoverImageCache::new(8, 3);

        cache.insert(1, decoded_cover(4));

        assert!(cache.get(1, Instant::now()).is_none());
    }
}
