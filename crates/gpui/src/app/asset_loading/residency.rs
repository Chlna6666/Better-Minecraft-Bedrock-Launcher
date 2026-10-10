use super::{App, AssetId, OwnedAssetEntry};
use crate::{ImageCacheError, RenderImage};
use collections::FxHashMap;
use linked_hash_map::LinkedHashMap;
use std::{
    any::TypeId,
    sync::{Arc, atomic::AtomicBool},
};

/// One recency list for decoded assets, independent of the number of cache/window lookups.
#[derive(Default)]
pub(in crate::app) struct ImageResidency {
    entries: LinkedHashMap<AssetId, ()>,
    pub(super) needs_scan: bool,
    sampled_budget: usize,
    reclaim_pending: Arc<AtomicBool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AssetLease, AssetLocation, ImageCacheItem, ResourceImageLoader, TestAppContext, hash,
    };
    use image::{Frame, RgbaImage};
    use smallvec::smallvec;
    use std::rc::Rc;

    fn insert_ready(cx: &mut App, name: &str) -> (AssetLocation, usize) {
        let source = AssetLocation::Embedded(name.to_string().into());
        let id = (TypeId::of::<ResourceImageLoader>(), hash(&source));
        let image = Arc::new(RenderImage::new(smallvec![Frame::new(RgbaImage::new(
            4, 4
        ))]));
        let bytes = image.cache_cost_byte_len();
        cx.asset_entries.insert(
            id,
            Box::new(ImageEntry {
                identity: Rc::new(()),
                lease: AssetLease::ready(Ok(image)),
                retire_when_unpinned: false,
            }),
        );
        cx.image_residency.touch(id);
        cx.image_residency.needs_scan = true;
        (source, bytes)
    }

    #[test]
    fn shared_lookups_do_not_multiply_decode_or_residency() {
        let test = TestAppContext::single();
        test.update(|cx| {
            let (source, bytes) = insert_ready(cx, "shared.png");
            let first = ImageCacheItem::shared(&source, cx);
            let second = ImageCacheItem::shared(&source, cx);
            let one = first.get().unwrap().unwrap();
            let two = second.get().unwrap().unwrap();
            assert!(Arc::ptr_eq(&one, &two));
            assert_eq!(
                cx.global_image_asset_cache_snapshot().cache_cost_bytes,
                bytes
            );
            drop(first);
            assert!(second.is_live());
        });
    }

    #[gpui::test]
    fn rendered_bounded_image_survives_global_idle_trim(cx: &mut TestAppContext) {
        use crate::{IntoElement, Render, Styled};
        struct ImageView {
            source: AssetLocation,
            cache: crate::Entity<crate::BoundedImageCache>,
            show: bool,
        }
        impl Render for ImageView {
            fn render(
                &mut self,
                _: &mut crate::Window,
                _: &mut crate::Context<Self>,
            ) -> impl IntoElement {
                if self.show {
                    crate::img(crate::ImageSource::Asset(self.source.clone()))
                        .image_cache(&self.cache)
                        .w(crate::px(4.0))
                        .h(crate::px(4.0))
                        .into_any_element()
                } else {
                    crate::div().into_any_element()
                }
            }
        }
        let source = cx.update(|cx| insert_ready(cx, "visible-bounded.png").0);
        let (view, visual) = cx.add_window_view(|_, cx| ImageView {
            source: source.clone(),
            cache: crate::BoundedImageCache::new(Default::default(), cx),
            show: true,
        });
        visual.update(|window, cx| {
            window.draw(cx).clear();
            cx.image_pipeline_config.idle_image_bytes = 0;
            cx.trim_image_memory(crate::ImageMemoryTrimLevel::Moderate);
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&source)
                    .is_some()
            );
            assert_eq!(cx.global_image_asset_cache_snapshot().over_budget_bytes, 64);
        });
        view.update(visual, |view, cx| {
            view.show = false;
            cx.notify();
        });
        visual.update(|window, cx| {
            window.draw(cx).clear();
        });
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&source)
                    .is_none()
            );
        });
    }

    #[gpui::test]
    fn bounded_caches_share_decode_and_expired_lookups_reload(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let (source, _) = insert_ready(cx, "two-cache-image.png");
            let first = crate::BoundedImageCache::new(Default::default(), cx);
            let second = crate::BoundedImageCache::new(Default::default(), cx);
            let one = first
                .update(cx, |cache, cx| cache.load(&source, window, cx))
                .unwrap()
                .unwrap();
            let two = second
                .update(cx, |cache, cx| cache.load(&source, window, cx))
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&one, &two));
            first.update(cx, |cache, cx| cache.clear(window, cx));
            assert!(
                second
                    .update(cx, |cache, cx| cache.load(&source, window, cx))
                    .is_some()
            );
            drop(one);
            drop(two);
            cx.image_pipeline_config.idle_image_bytes = 0;
            cx.reclaim_idle_image_cache();
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&source)
                    .is_none()
            );
            // Keep local metadata in second. Global retirement must expire it, not serve stale pixels.
            let (_, _) = insert_ready(cx, "two-cache-image.png");
            assert!(
                second
                    .update(cx, |cache, cx| cache.load(&source, window, cx))
                    .is_some()
            );
        });
    }

    #[test]
    fn global_budget_expires_idle_lookups_and_preserves_recent_image() {
        let mut test = TestAppContext::single();
        let (old, recent) = test.update(|cx| {
            let (old, bytes) = insert_ready(cx, "old.png");
            let old = ImageCacheItem::shared(&old, cx);
            let (recent, _) = insert_ready(cx, "recent.png");
            let recent_item = ImageCacheItem::shared(&recent, cx);
            cx.image_pipeline_config.idle_image_bytes = bytes;
            cx.enforce_image_cache_budget(Some((
                TypeId::of::<ResourceImageLoader>(),
                hash(&recent),
            )));
            assert!(!old.is_live());
            assert!(recent_item.is_live());
            assert_eq!(cx.global_image_asset_cache_snapshot().over_budget_bytes, 0);
            (old, recent_item)
        });
        test.run_until_parked();
        assert!(!old.is_live());
        assert!(recent.is_live());
    }

    #[test]
    fn active_arc_and_explicit_lease_survive_budget_and_trim() {
        let mut test = TestAppContext::single();
        let (source, lease, displayed) = test.update(|cx| {
            let (source, bytes) = insert_ready(cx, "visible.png");
            let lease = cx.fetch_asset::<ResourceImageLoader>(&source);
            let displayed = lease.get().unwrap().unwrap();
            cx.image_pipeline_config.idle_image_bytes = 0;
            cx.trim_image_memory(crate::ImageMemoryTrimLevel::Moderate);
            assert_eq!(
                cx.global_image_asset_cache_snapshot().over_budget_bytes,
                bytes
            );
            (source, lease, displayed)
        });
        drop(lease);
        test.update(|cx| {
            cx.reclaim_idle_image_cache();
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&source)
                    .is_some()
            );
        });
        drop(displayed);
        test.update(|cx| {
            cx.reclaim_idle_image_cache();
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&source)
                    .is_none()
            );
        });
        test.run_until_parked();
    }

    #[test]
    fn image_recency_is_global_across_local_lookups() {
        let test = TestAppContext::single();
        test.update(|cx| {
            let (first, bytes) = insert_ready(cx, "first.png");
            let (second, _) = insert_ready(cx, "second.png");
            let first_id = (TypeId::of::<ResourceImageLoader>(), hash(&first));
            cx.enforce_image_cache_budget(Some(first_id));
            cx.image_pipeline_config.idle_image_bytes = bytes;
            cx.reclaim_idle_image_cache();
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&first)
                    .is_some()
            );
            assert!(
                cx.cached_asset_lease::<ResourceImageLoader>(&second)
                    .is_none()
            );
        });
    }
}

impl ImageResidency {
    pub(super) fn touch(&mut self, id: AssetId) {
        self.entries.remove(&id);
        self.entries.insert(id, ());
    }
}

pub(super) fn is_image_asset(type_id: TypeId) -> bool {
    type_id == TypeId::of::<crate::ResourceImageLoader>()
        || type_id == TypeId::of::<crate::SizedImageLoader>()
        || type_id == TypeId::of::<crate::AssetLogger<crate::ClipboardImageLoader>>()
        || type_id == TypeId::of::<crate::AssetLogger<crate::EncodedImageLoader>>()
}

type ImageEntry = OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>;
// Allocation identity -> (cache-entry references, unique retained cost).
type ImageInventory = FxHashMap<usize, (usize, usize)>;

impl App {
    pub(crate) fn image_cache_release_signal(&self) -> (crate::AsyncApp, Arc<AtomicBool>) {
        (
            self.to_async(),
            self.image_residency.reclaim_pending.clone(),
        )
    }

    pub(crate) fn enforce_image_cache_budget(&mut self, protected: Option<AssetId>) {
        if let Some(id) = protected {
            self.image_residency.touch(id);
        }
        if !self.image_residency.needs_scan
            && self.image_residency.sampled_budget == self.image_pipeline_config.idle_image_bytes
        {
            return;
        }
        self.evict_image_residency(protected, false);
    }

    pub(crate) fn reclaim_idle_image_cache(&mut self) {
        self.image_residency.needs_scan = true;
        self.enforce_image_cache_budget(None);
    }

    pub(super) fn trim_image_residency(&mut self, all_idle: bool) {
        self.evict_image_residency(None, all_idle);
    }

    pub(in crate::app) fn image_cache_cost_bytes(&self) -> usize {
        self.image_inventory()
            .values()
            .map(|allocation| allocation.1)
            .sum()
    }

    fn image_inventory(&self) -> ImageInventory {
        let mut inventory = ImageInventory::default();
        for (id, entry) in &self.asset_entries {
            if !is_image_asset(id.0) {
                continue;
            }
            if let Some(Ok(image)) = entry.downcast_ref::<ImageEntry>().and_then(ImageEntry::get) {
                let allocation = inventory
                    .entry(Arc::as_ptr(&image) as usize)
                    .or_insert((0, image.cache_cost_byte_len()));
                allocation.0 += 1;
            }
        }
        inventory
    }

    fn evict_image_residency(&mut self, protected: Option<AssetId>, all_idle: bool) {
        let stale = self
            .image_residency
            .entries
            .keys()
            .filter(|id| !self.asset_entries.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        for id in stale {
            self.image_residency.entries.remove(&id);
        }
        let mut inventory = self.image_inventory();
        let mut bytes: usize = inventory.values().map(|allocation| allocation.1).sum();
        let keys = self
            .image_residency
            .entries
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let mut retired = Vec::new();
        for id in keys {
            if !all_idle && bytes <= self.image_pipeline_config.idle_image_bytes {
                break;
            }
            if Some(id) == protected {
                continue;
            }
            if let Some(image) = self.evict_idle_image(id, &mut inventory, &mut bytes) {
                retired.push((id, image));
            }
        }
        self.image_residency.sampled_budget = self.image_pipeline_config.idle_image_bytes;
        self.image_residency.needs_scan = false;
        self.retire_image_allocations(retired);
    }

    fn evict_idle_image(
        &mut self,
        id: AssetId,
        inventory: &mut ImageInventory,
        bytes: &mut usize,
    ) -> Option<Arc<RenderImage>> {
        let entry = self.asset_entries.get(&id)?.downcast_ref::<ImageEntry>()?;
        // Explicit preloads/cache owners and element pins own the load independently of the budget.
        if entry.lease.owner_count() != 1 || entry.pin_count() != 0 {
            return None;
        }
        let image = entry.get()?.ok()?;
        let allocation = inventory.get_mut(&(Arc::as_ptr(&image) as usize))?;
        // Ignore cache-owned aliases and this temporary clone; an element's ready Arc protects it.
        if Arc::strong_count(&image) > allocation.0 + 1 {
            return None;
        }
        self.asset_entries.remove(&id);
        self.image_residency.entries.remove(&id);
        self.idle_sized_images.remove(id);
        if id.0 == TypeId::of::<crate::SizedImageLoader>() {
            crate::drop_image_asset_retained(id.1);
        }
        allocation.0 -= 1;
        if allocation.0 != 0 {
            return None;
        }
        *bytes = bytes.saturating_sub(allocation.1);
        crate::record_image_cache_eviction(1);
        Some(image)
    }

    fn retire_image_allocations(&mut self, retired: Vec<(AssetId, Arc<RenderImage>)>) {
        if retired.is_empty() {
            return;
        }
        // Run after the current Window update returns it to App.windows. No GPU calls on UI workers.
        self.spawn(async move |cx| {
            if let Err(error) = cx.update(|cx| {
                for (id, image) in retired {
                    // A fresh request can reuse this stable ImageId before retirement is delivered.
                    if !cx.asset_entries.contains_key(&id) {
                        cx.drop_image(image, None);
                    }
                }
            }) {
                log::debug!("image residency retirement ended with application: {error}");
            }
        })
        .detach();
    }
}
