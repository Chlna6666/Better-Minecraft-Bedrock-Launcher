use crate::{
    App, AppContext, AssetLocation, ElementId, Entity, ImageCacheError, RenderImage, Window,
    drop_image_cache_metrics, hash, record_image_cache_eviction, record_image_cache_metrics,
};
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
    /// Maximum working-set estimate for local lookups, including active stream sources.
    /// Shared decoded allocations are limited once by [`crate::ImagePipelineConfig::idle_image_bytes`].
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

/// A bounded local lookup into the App's shared decoded-image working set.
/// Local eviction removes lookup metadata; App-wide idle eviction retires GPU tiles safely.
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
    pub fn update_limits(
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
        source: &AssetLocation,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let image_hash = hash(source);

        if let Some(entry) = self.entries.get_refresh(&image_hash) {
            if !entry.item.is_live() {
                entry.item = ImageCacheItem::shared(source, cx);
                self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
                entry.estimated_bytes = 0;
            }
            let result = entry.item.use_image(window);
            if let Some(Ok(image)) = result.as_ref() {
                let current_bytes = estimated_render_image_bytes(image);
                self.estimated_bytes = self
                    .estimated_bytes
                    .saturating_sub(entry.estimated_bytes)
                    .saturating_add(current_bytes);
                entry.estimated_bytes = current_bytes;
            }
            self.enforce_limits(Some(image_hash), window, cx);
            cx.enforce_image_cache_budget(Some((
                std::any::TypeId::of::<crate::ResourceImageLoader>(),
                image_hash,
            )));
            self.record_metrics();
            return result;
        }

        let item = ImageCacheItem::shared(source, cx);
        let result = item.use_image(window);
        let estimated_bytes = result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or(0, |image| estimated_render_image_bytes(image));
        self.entries.insert(
            image_hash,
            BoundedImageCacheEntry {
                item,
                estimated_bytes,
            },
        );
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.enforce_limits(Some(image_hash), window, cx);
        cx.enforce_image_cache_budget(Some((
            std::any::TypeId::of::<crate::ResourceImageLoader>(),
            image_hash,
        )));
        self.record_metrics();

        result
    }

    /// Clears local lookups and checks the App-wide idle budget.
    /// Shared images may remain cached for other users or reuse within that budget.
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        self.drop_all(Some(window), cx);
    }

    /// Removes one local lookup and checks the App-wide idle budget.
    /// This does not force retirement of an image used by another cache or visible element.
    pub fn remove(&mut self, source: &AssetLocation, window: &mut Window, cx: &mut App) {
        let image_hash = hash(source);
        if let Some(entry) = self.entries.remove(&image_hash) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, Some(window), cx);
            cx.reclaim_idle_image_cache();
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

    /// Returns this lookup working set's estimated cost, including active stream sources.
    /// This is not additional physical memory when another cache refers to the same image.
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn enforce_limits(&mut self, protected_hash: Option<u64>, window: &mut Window, cx: &mut App) {
        let mut evicted = false;
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
            evicted = true;
        }
        if evicted {
            cx.reclaim_idle_image_cache();
        }
    }

    fn drop_all(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        let entries = std::mem::take(&mut self.entries);
        self.estimated_bytes = 0;
        record_image_cache_eviction(entries.len());
        for (_, entry) in entries.into_iter() {
            drop_cache_entry(entry, window.as_deref_mut(), cx);
        }
        cx.reclaim_idle_image_cache();
        self.record_metrics();
    }

    fn record_metrics(&self) {
        // Shared lookups do not own another decoded allocation; App diagnostics count it once.
        let owned_bytes = self
            .entries
            .values()
            .filter(|entry| !entry.item.is_shared())
            .map(|entry| entry.estimated_bytes)
            .sum();
        record_image_cache_metrics(self.cache_id, self.entries.len(), owned_bytes);
    }
}

impl ImageCache for BoundedImageCache {
    fn load(
        &mut self,
        resource: &AssetLocation,
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
    entry: BoundedImageCacheEntry,
    current_window: Option<&mut Window>,
    cx: &mut App,
) {
    if entry.item.is_shared() {
        return;
    }
    if let Some(Ok(image)) = entry.item.get() {
        cx.drop_image(image, current_window);
    }
}

fn estimated_render_image_bytes(image: &RenderImage) -> usize {
    image.cache_cost_byte_len()
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
                    item: ImageCacheItem::ready(Ok(image)),
                    estimated_bytes: 4,
                },
            );
            cache.entries.insert(
                protected_hash,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::ready(Err(ImageCacheError::Asset("protected".into()))),
                    estimated_bytes: 0,
                },
            );

            cache.enforce_limits(Some(protected_hash), window, cx);

            let after = performance_metrics_snapshot();
            assert!(!cache.entries.contains_key(&image_hash));
            assert!(cache.entries.contains_key(&protected_hash));
            assert!(after.image_cache_evictions > before.image_cache_evictions);
            assert!(after.image_drop_count > before.image_drop_count);
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
                    item: ImageCacheItem::ready(Err(ImageCacheError::Asset("one".into()))),
                    estimated_bytes: 1,
                },
            );
            cache.entries.insert(
                2,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::ready(Err(ImageCacheError::Asset("two".into()))),
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
