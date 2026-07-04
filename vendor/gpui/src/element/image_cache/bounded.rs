use crate::{
    App, AppContext, Asset, AssetLogger, ElementId, Entity, ImageAssetLoader, ImageCacheError,
    RenderImage, Resource, Window, drop_image_cache_metrics, hash, record_image_cache_eviction,
    record_image_cache_metrics,
};
use futures::FutureExt;
use linked_hash_map::LinkedHashMap;
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use super::{AnyImageCache, ImageCache, ImageCacheItem, ImageCacheProvider};

/// Memory and count limits for [`BoundedImageCache`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedImageCacheConfig {
    /// Maximum number of loaded or loading cache entries to retain.
    pub max_items: usize,
    /// Maximum estimated decoded image bytes to retain.
    pub max_bytes: usize,
}

impl Default for BoundedImageCacheConfig {
    fn default() -> Self {
        Self {
            max_items: 256,
            max_bytes: 128 * 1024 * 1024,
        }
    }
}

struct BoundedImageCacheEntry {
    item: ImageCacheItem,
    estimated_bytes: usize,
}

/// An LRU image cache that releases decoded images and atlas tiles when limits are exceeded.
pub struct BoundedImageCache {
    cache_id: u64,
    config: BoundedImageCacheConfig,
    entries: LinkedHashMap<u64, BoundedImageCacheEntry>,
    estimated_bytes: usize,
}

impl fmt::Debug for BoundedImageCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedImageCache")
            .field("max_items", &self.config.max_items)
            .field("max_bytes", &self.config.max_bytes)
            .field("cache_id", &self.cache_id)
            .field("items", &self.entries.len())
            .field("estimated_bytes", &self.estimated_bytes)
            .finish()
    }
}

impl BoundedImageCache {
    /// Create a new bounded image cache.
    pub fn new(config: BoundedImageCacheConfig, cx: &mut App) -> Entity<Self> {
        static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);
        let cache_id = NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed);
        let cache = cx.new(|_cx| Self {
            cache_id,
            config,
            entries: LinkedHashMap::new(),
            estimated_bytes: 0,
        });
        cx.observe_release(&cache, |cache, cx| {
            cache.drop_all(None, cx);
            drop_image_cache_metrics(cache.cache_id);
        })
        .detach();
        cache
    }

    /// Update cache limits and evict anything that no longer fits.
    pub fn set_config(
        &mut self,
        config: BoundedImageCacheConfig,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.config == config {
            return;
        }

        self.config = config;
        self.enforce_limits(None, window, cx);
        self.record_metrics();
    }

    /// Load an image from the given source.
    pub fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let image_hash = hash(source);

        if let Some(entry) = self.entries.get_refresh(&image_hash) {
            let result = {
                let result = entry.item.get();
                if entry.estimated_bytes == 0
                    && let Some(Ok(image)) = result.as_ref()
                {
                    entry.estimated_bytes = estimated_render_image_bytes(image);
                    self.estimated_bytes =
                        self.estimated_bytes.saturating_add(entry.estimated_bytes);
                }
                result
            };
            self.enforce_limits(Some(image_hash), window, cx);
            self.record_metrics();
            return result;
        }

        let fut = AssetLogger::<ImageAssetLoader>::load(source.clone(), cx);
        let task = cx.background_executor().spawn(fut).shared();
        self.entries.insert(
            image_hash,
            BoundedImageCacheEntry {
                item: ImageCacheItem::Loading(task.clone()),
                estimated_bytes: 0,
            },
        );
        self.enforce_limits(Some(image_hash), window, cx);
        self.record_metrics();

        let entity = window.current_view();
        window
            .spawn(cx, {
                async move |cx| {
                    if let Err(error) = task.await {
                        log::debug!("bounded image cache load failed: {error}");
                    }
                    cx.update(move |window, cx| {
                        cx.notify(entity);
                        window.schedule_dirty_frame();
                    })
                    .ok();
                }
            })
            .detach();

        None
    }

    /// Clear the image cache.
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        self.drop_all(Some(window), cx);
    }

    /// Remove one image from the cache.
    pub fn remove(&mut self, source: &Resource, window: &mut Window, cx: &mut App) {
        let image_hash = hash(source);
        if let Some(entry) = self.entries.remove(&image_hash) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, Some(window), cx);
        }
        self.record_metrics();
    }

    /// Returns the number of entries retained by the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the estimated decoded image bytes retained by the cache.
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn enforce_limits(&mut self, protected_hash: Option<u64>, window: &mut Window, cx: &mut App) {
        while self.entries.len() > self.config.max_items
            || self.estimated_bytes > self.config.max_bytes
        {
            let Some((&candidate_hash, _)) = self.entries.front() else {
                break;
            };
            if Some(candidate_hash) == protected_hash && self.entries.len() > 1 {
                _ = self.entries.get_refresh(&candidate_hash);
                continue;
            }
            let Some(entry) = self.entries.remove(&candidate_hash) else {
                break;
            };
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, Some(window), cx);
        }
    }

    fn drop_all(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        let entries = std::mem::take(&mut self.entries);
        self.estimated_bytes = 0;
        record_image_cache_eviction(entries.len());
        for (_, entry) in entries.into_iter() {
            drop_cache_entry(entry, window.as_deref_mut(), cx);
        }
        self.record_metrics();
    }

    fn record_metrics(&self) {
        record_image_cache_metrics(self.cache_id, self.entries.len(), self.estimated_bytes);
    }
}

impl ImageCache for BoundedImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        BoundedImageCache::load(self, resource, window, cx)
    }
}

/// Constructs a bounded image cache that uses element state associated with the given ID.
pub fn bounded(
    id: impl Into<ElementId>,
    config: BoundedImageCacheConfig,
) -> BoundedImageCacheProvider {
    BoundedImageCacheProvider {
        id: id.into(),
        config,
    }
}

/// Provider for inline bounded image caches.
pub struct BoundedImageCacheProvider {
    id: ElementId,
    config: BoundedImageCacheConfig,
}

impl ImageCacheProvider for BoundedImageCacheProvider {
    fn provide(&mut self, window: &mut Window, cx: &mut App) -> AnyImageCache {
        window
            .with_global_id(self.id.clone(), |global_id, window| {
                window.with_element_state::<Entity<BoundedImageCache>, _>(
                    global_id,
                    |cache, _window| {
                        let cache =
                            cache.unwrap_or_else(|| BoundedImageCache::new(self.config, cx));
                        if cache.read(cx).config != self.config {
                            let cache = BoundedImageCache::new(self.config, cx);
                            (cache.clone(), cache)
                        } else {
                            (cache.clone(), cache)
                        }
                    },
                )
            })
            .into()
    }
}

fn drop_cache_entry(
    mut entry: BoundedImageCacheEntry,
    current_window: Option<&mut Window>,
    cx: &mut App,
) {
    if let Some(Ok(image)) = entry.item.get() {
        cx.drop_image(image, current_window);
    }
}

fn estimated_render_image_bytes(image: &RenderImage) -> usize {
    image.decoded_byte_len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TestAppContext, performance_metrics_snapshot};
    use image::{Frame, RgbaImage};
    use smallvec::smallvec;

    #[gpui::test]
    fn bounded_cache_eviction_drops_loaded_images(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let before = performance_metrics_snapshot();
            let image = Arc::new(RenderImage::new(smallvec![Frame::new(RgbaImage::new(
                1, 1,
            ))]));
            let image_hash = 1;
            let protected_hash = 2;
            let mut cache = BoundedImageCache {
                cache_id: 999_999,
                config: BoundedImageCacheConfig {
                    max_items: 1,
                    max_bytes: usize::MAX,
                },
                entries: LinkedHashMap::default(),
                estimated_bytes: 4,
            };
            cache.entries.insert(
                image_hash,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Ok(image)),
                    estimated_bytes: 4,
                },
            );
            cache.entries.insert(
                protected_hash,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("protected".into()))),
                    estimated_bytes: 0,
                },
            );

            cache.enforce_limits(Some(protected_hash), window, cx);

            let after = performance_metrics_snapshot();
            assert!(!cache.entries.contains_key(&image_hash));
            assert!(cache.entries.contains_key(&protected_hash));
            assert!(after.image_cache_evictions >= before.image_cache_evictions + 1);
            assert!(after.image_drop_count >= before.image_drop_count + 1);
        });
    }

    #[gpui::test]
    fn bounded_cache_refreshes_most_recent_entries(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let mut cache = BoundedImageCache {
                cache_id: 999_998,
                config: BoundedImageCacheConfig {
                    max_items: 2,
                    max_bytes: usize::MAX,
                },
                entries: LinkedHashMap::default(),
                estimated_bytes: 0,
            };

            cache.entries.insert(
                1,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("one".into()))),
                    estimated_bytes: 1,
                },
            );
            cache.entries.insert(
                2,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("two".into()))),
                    estimated_bytes: 1,
                },
            );
            _ = cache.entries.get_refresh(&1);
            cache.enforce_limits(None, window, cx);

            assert_eq!(cache.entries.front().map(|(hash, _)| *hash), Some(2));
            assert_eq!(cache.entries.back().map(|(hash, _)| *hash), Some(1));
        });
    }
}
