use std::collections::VecDeque;

use super::{App, AssetId, OwnedAssetEntry};
use crate::{
    ImageCacheError, ImageRenderRequest, RenderImage, Window, drop_image_asset_retained, hash,
};
use std::{any::TypeId, sync::Arc};

// Only the idle working set is bounded. Active elements retain their full-sized images.
const MAX_IMAGE_BYTES: usize = 256 * 1024;
const MAX_IDLE_BYTES: usize = 4 * 1024 * 1024;
const MAX_IDLE_IMAGES: usize = 128;

/// Recency and decoded-byte accounting for unpinned small static image assets.
///
/// Images stay in App's asset entries; removing a key here does not retire its GPU resources.
#[derive(Default)]
pub(in crate::app) struct IdleImageCache {
    entries: VecDeque<(AssetId, usize)>,
    bytes: usize,
}

impl IdleImageCache {
    pub(super) fn accepts(bytes: usize) -> bool {
        bytes != 0 && bytes <= MAX_IMAGE_BYTES
    }

    pub(super) fn remove(&mut self, asset_id: AssetId) {
        if let Some(index) = self.entries.iter().position(|entry| entry.0 == asset_id)
            && let Some((_, bytes)) = self.entries.remove(index)
        {
            self.bytes -= bytes;
        }
    }

    pub(super) fn insert(&mut self, asset_id: AssetId, bytes: usize) -> Vec<AssetId> {
        debug_assert!(Self::accepts(bytes));
        self.remove(asset_id);
        self.entries.push_back((asset_id, bytes));
        self.bytes += bytes;
        let mut evicted = Vec::new();
        while self.bytes > MAX_IDLE_BYTES || self.entries.len() > MAX_IDLE_IMAGES {
            if let Some((asset_id, bytes)) = self.entries.pop_front() {
                self.bytes -= bytes;
                evicted.push(asset_id);
            }
        }
        evicted
    }

    pub(super) fn take_keys(&mut self) -> Vec<AssetId> {
        self.bytes = 0;
        self.entries.drain(..).map(|entry| entry.0).collect()
    }
}

impl App {
    pub(crate) fn pin_sized_image_request(
        &mut self,
        request: &ImageRenderRequest,
    ) -> crate::AssetPin<Result<Arc<RenderImage>, ImageCacheError>> {
        self.idle_sized_images
            .remove((TypeId::of::<crate::SizedImageLoader>(), hash(request)));
        self.fetch_asset::<crate::SizedImageLoader>(request).pin()
    }

    pub(crate) fn release_sized_image_element_pin(
        &mut self,
        request: &ImageRenderRequest,
        pin: crate::AssetPin<Result<Arc<RenderImage>, ImageCacheError>>,
        fallback_image: Option<Arc<RenderImage>>,
        mut current_window: Option<&mut Window>,
    ) {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(request));
        let should_retire = self
            .asset_entries
            .get(&asset_id)
            .and_then(|entry| {
                entry.downcast_ref::<OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>>()
            })
            .is_some_and(|entry| entry.shares_pin(&pin) && entry.pin_count() == 1);

        if !should_retire {
            return;
        }

        let ready_image = self.asset_entries.get(&asset_id).and_then(|entry| {
            let entry = entry
                .downcast_ref::<OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>>()?;
            if entry.retire_when_unpinned {
                None
            } else {
                entry.get()
            }
        });
        if let Some(Ok(image)) = ready_image
            && !image.is_animated()
            && IdleImageCache::accepts(image.cache_cost_byte_len())
        {
            for evicted in self
                .idle_sized_images
                .insert(asset_id, image.cache_cost_byte_len())
            {
                self.retire_sized_image(evicted, None, current_window.as_deref_mut());
            }
            return;
        }

        self.retire_sized_image(asset_id, fallback_image, current_window);
    }

    pub(super) fn retire_sized_image(
        &mut self,
        asset_id: AssetId,
        fallback_image: Option<Arc<RenderImage>>,
        current_window: Option<&mut Window>,
    ) {
        self.idle_sized_images.remove(asset_id);
        let cached_image = self
            .asset_entries
            .remove(&asset_id)
            .and_then(|entry| {
                entry
                    .downcast::<OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>>()
                    .ok()
            })
            .and_then(|entry| entry.get())
            .and_then(Result::ok);

        if let Some(image) = fallback_image.or(cached_image) {
            self.drop_image(image, current_window);
        }
        drop_image_asset_retained(asset_id.1);
    }

    #[cfg(test)]
    pub(crate) fn sized_image_element_ref_count_for_test(
        &self,
        request: &ImageRenderRequest,
    ) -> usize {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(request));
        self.asset_entries
            .get(&asset_id)
            .and_then(|entry| {
                entry.downcast_ref::<OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>>()
            })
            .map_or(0, OwnedAssetEntry::pin_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    fn key(index: usize) -> AssetId {
        (TypeId::of::<()>(), index as u64)
    }

    #[test]
    fn idle_images_evict_oldest_at_byte_budget() {
        let mut cache = IdleImageCache::default();
        for index in 0..MAX_IDLE_BYTES / MAX_IMAGE_BYTES {
            assert!(cache.insert(key(index), MAX_IMAGE_BYTES).is_empty());
        }
        cache.remove(key(0));
        assert!(cache.insert(key(0), MAX_IMAGE_BYTES).is_empty());
        assert_eq!(cache.insert(key(128), MAX_IMAGE_BYTES), vec![key(1)]);
        assert_eq!(cache.bytes, MAX_IDLE_BYTES);
    }

    #[test]
    fn idle_images_bound_entry_count_and_clear_accounting() {
        let mut cache = IdleImageCache::default();
        for index in 0..MAX_IDLE_IMAGES {
            assert!(cache.insert(key(index), 4).is_empty());
        }
        assert_eq!(cache.insert(key(MAX_IDLE_IMAGES), 4), vec![key(0)]);
        assert_eq!(cache.take_keys().len(), MAX_IDLE_IMAGES);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }
}
