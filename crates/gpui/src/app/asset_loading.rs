use std::{
    any::{Any, TypeId},
    cell::RefCell,
    rc::Rc,
    sync::Arc,
};

use anyhow::Result;
use collections::{FxHashMap, FxHashSet};
use crate::{
    AnyWindowHandle, Asset, AssetLease, AssetLocation, AssetRetentionPolicy,
    CompressedImagePreload, CompressedImageSource, EntityId, ImageCacheError, ImageMemoryTrimLevel,
    ImagePipelineConfig, ImageRenderRequest, ObjectFit, Pixels, RenderImage, Size,
    SizedImagePreload, Window, WindowId, drop_image_asset_retained, hash,
};

use super::App;

type AssetId = (TypeId, u64);

struct AssetWindowObservers {
    window: AnyWindowHandle,
    views: FxHashSet<EntityId>,
}

enum OwnedAssetState {
    Loading {
        observers: FxHashMap<WindowId, AssetWindowObservers>,
    },
    Ready,
}

struct OwnedAssetEntry<T>
where
    T: Clone + Send + 'static,
{
    state: Rc<RefCell<OwnedAssetState>>,
    lease: AssetLease<T>,
}

impl<T> OwnedAssetEntry<T>
where
    T: Clone + Send + 'static,
{
    fn new<A>(source: &A::Source, asset_id: AssetId, cx: &mut App) -> Self
    where
        A: Asset<Output = T>,
    {
        let lease = AssetLease::spawn(A::load(source.clone(), cx), cx);
        let completion = lease.completion_signal();
        let state = Rc::new(RefCell::new(OwnedAssetState::Loading {
            observers: FxHashMap::default(),
        }));
        let weak_state = Rc::downgrade(&state);

        cx.spawn(async move |cx| {
            if completion.await.is_err() {
                return;
            }
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let observers = {
                let mut state = state.borrow_mut();
                match std::mem::replace(&mut *state, OwnedAssetState::Ready) {
                    OwnedAssetState::Loading { observers } => observers,
                    OwnedAssetState::Ready => return,
                }
            };

            let _ = cx.update(move |cx| {
                for observer in observers.into_values() {
                    let views = observer.views;
                    let _ = observer.window.update(cx, move |_, window, _| {
                        window.schedule_asset_ready_views(views);
                    });
                }

                if A::RETENTION == AssetRetentionPolicy::TransientAfterReady {
                    let is_same_entry = cx
                        .loading_assets
                        .get(&asset_id)
                        .and_then(|entry| entry.downcast_ref::<OwnedAssetEntry<T>>())
                        .is_some_and(|entry| Rc::ptr_eq(&entry.state, &state));
                    if is_same_entry {
                        cx.loading_assets.remove(&asset_id);
                    }
                }
            });
        })
        .detach();

        Self { state, lease }
    }

    fn get(&self) -> Option<T> {
        self.lease.get()
    }

    fn lease(&self) -> AssetLease<T> {
        self.lease.clone()
    }

    fn into_lease(self) -> AssetLease<T> {
        self.lease
    }

    fn use_by(&self, window: AnyWindowHandle, view: EntityId) -> Option<T> {
        if let Some(value) = self.lease.get() {
            return Some(value);
        }

        let mut state = self.state.borrow_mut();
        match &mut *state {
            OwnedAssetState::Loading { observers } => {
                observers
                    .entry(window.window_id())
                    .or_insert_with(|| AssetWindowObservers {
                        window,
                        views: FxHashSet::default(),
                    })
                    .views
                    .insert(view);
                None
            }
            OwnedAssetState::Ready => self.lease.get(),
        }
    }
}

pub(super) fn cached_asset_output<T>(entry: &dyn Any) -> Option<T>
where
    T: Clone + Send + 'static,
{
    entry.downcast_ref::<OwnedAssetEntry<T>>()?.get()
}

#[derive(Default)]
struct SizedImageElementOwners {
    current: FxHashMap<AssetId, usize>,
}

impl SizedImageElementOwners {
    fn retain(&mut self, asset_id: AssetId) {
        let references = self.current.entry(asset_id).or_default();
        *references = references
            .checked_add(1)
            .expect("sized image element reference count overflow");
    }

    fn release(&mut self, asset_id: AssetId) -> bool {
        let Some(references) = self.current.get_mut(&asset_id) else {
            debug_assert!(
                false,
                "sized image element reference released without owner"
            );
            return false;
        };

        debug_assert!(*references > 0);
        *references = references.saturating_sub(1);
        if *references == 0 {
            self.current.remove(&asset_id);
            true
        } else {
            false
        }
    }

    fn count(&self, asset_id: AssetId) -> usize {
        self.current.get(&asset_id).copied().unwrap_or(0)
    }
}

fn sized_image_owner_state(cx: &mut App) -> &mut SizedImageElementOwners {
    cx.globals_by_type
        .entry(TypeId::of::<SizedImageElementOwners>())
        .or_insert_with(|| Box::new(SizedImageElementOwners::default()))
        .downcast_mut::<SizedImageElementOwners>()
        .expect("sized image element owner state type mismatch")
}

fn sized_image_element_ref_count(cx: &App, asset_id: AssetId) -> usize {
    cx.globals_by_type
        .get(&TypeId::of::<SizedImageElementOwners>())
        .and_then(|state| state.downcast_ref::<SizedImageElementOwners>())
        .map_or(0, |state| state.count(asset_id))
}

#[cfg(test)]
#[path = "asset_loading_tests.rs"]
mod asset_loading_tests;

impl App {
    fn asset_entry<A: Asset>(&mut self, source: &A::Source) -> &OwnedAssetEntry<A::Output> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        if !self.loading_assets.contains_key(&asset_id) {
            let entry = OwnedAssetEntry::new::<A>(source, asset_id, self);
            self.loading_assets.insert(asset_id, Box::new(entry));
        }
        self.loading_assets
            .get(&asset_id)
            .and_then(|entry| entry.downcast_ref::<OwnedAssetEntry<A::Output>>())
            .expect("asset cache entries are keyed by asset type")
    }

    pub(crate) fn cached_asset_lease<A: Asset>(
        &self,
        source: &A::Source,
    ) -> Option<AssetLease<A::Output>> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        self.loading_assets
            .get(&asset_id)
            .and_then(|entry| entry.downcast_ref::<OwnedAssetEntry<A::Output>>())
            .map(OwnedAssetEntry::lease)
    }

    pub(crate) fn use_asset_in_window<A: Asset>(
        &mut self,
        source: &A::Source,
        window: AnyWindowHandle,
        view: EntityId,
    ) -> Option<A::Output> {
        self.asset_entry::<A>(source).use_by(window, view)
    }

    /// Trims idle image state without applying byte ceilings to active images.
    pub fn trim_image_memory(&mut self, level: ImageMemoryTrimLevel) {
        let bitmap_pool_limit = match level {
            ImageMemoryTrimLevel::Light => {
                self.image_pipeline_config
                    .bitmap_pool_bytes
                    .saturating_mul(3)
                    / 4
            }
            ImageMemoryTrimLevel::Moderate | ImageMemoryTrimLevel::Aggressive => 0,
        };
        crate::assets::trim_global_bitmap_pool_to(bitmap_pool_limit);
        crate::trim_compressed_cache();

        if matches!(level, ImageMemoryTrimLevel::Light) {
            return;
        }

        let resource_type = TypeId::of::<crate::ResourceImageLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<crate::ClipboardImageLoader>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<crate::EncodedImageLoader>>();
        let target_type = TypeId::of::<crate::SizedImageLoader>();
        let mut evicted = Vec::new();
        for (asset_id, entry) in &self.loading_assets {
            let is_image = matches!(
                asset_id.0,
                id if id == resource_type
                    || id == inline_type
                    || id == inline_bytes_type
                    || id == target_type
            );
            if !is_image || sized_image_element_ref_count(self, *asset_id) != 0 {
                continue;
            }
            let Some(Ok(image)) =
                cached_asset_output::<Result<Arc<RenderImage>, ImageCacheError>>(entry.as_ref())
            else {
                continue;
            };
            if Arc::strong_count(&image) <= 2 {
                evicted.push((*asset_id, image));
            }
        }

        for (asset_id, image) in evicted {
            self.loading_assets.remove(&asset_id);
            self.drop_image(image, None);
            if asset_id.0 == target_type {
                drop_image_asset_retained(asset_id.1);
            }
        }
        crate::trim_compressed_cache();
    }

    pub(crate) fn retain_sized_image_element_request(&mut self, request: &ImageRenderRequest) {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(request));
        sized_image_owner_state(self).retain(asset_id);
    }

    pub(crate) fn release_sized_image_element_request(
        &mut self,
        request: &ImageRenderRequest,
        fallback_image: Option<Arc<RenderImage>>,
        current_window: Option<&mut Window>,
    ) {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(request));
        let became_unowned = {
            let state = sized_image_owner_state(self);
            state.release(asset_id)
        };
        if !became_unowned {
            return;
        }

        let cached_image = self
            .loading_assets
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
        sized_image_element_ref_count(self, asset_id)
    }

    /// Remove an asset from GPUI's cache.
    pub fn remove_asset<A: Asset>(&mut self, source: &A::Source) {
        drop(self.take_asset::<A>(source));
    }

    /// Removes an asset cache entry and transfers its load ownership to the caller.
    ///
    /// Element-owned assets remain registered while at least one live element lease references
    /// the same key.
    pub fn take_asset<A: Asset>(&mut self, source: &A::Source) -> Option<AssetLease<A::Output>> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        if A::RETENTION == AssetRetentionPolicy::ElementOwned
            && sized_image_element_ref_count(self, asset_id) != 0
        {
            return self.cached_asset_lease::<A>(source);
        }

        self.loading_assets
            .remove(&asset_id)
            .and_then(|entry| entry.downcast::<OwnedAssetEntry<A::Output>>().ok())
            .map(|entry| (*entry).into_lease())
    }

    /// Returns an explicit ownership lease for an asset load.
    ///
    /// Multiple calls share one cache-owned load. Dropping the final cache or lease owner cancels
    /// pending work. Completed transient assets leave the App cache while explicit leases retain
    /// their result.
    pub fn fetch_asset<A: Asset>(&mut self, source: &A::Source) -> AssetLease<A::Output> {
        self.asset_entry::<A>(source).lease()
    }

    /// Starts loading resource images into GPUI's global image asset cache.
    pub fn preload_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
    ) -> Vec<AssetLease<Result<Arc<RenderImage>, ImageCacheError>>> {
        sources
            .into_iter()
            .map(|source| self.fetch_asset::<crate::ResourceImageLoader>(&source))
            .collect()
    }

    /// Starts loading compressed image bytes for bounds-aware decoding.
    ///
    /// Returned leases explicitly retain pending work and completed bytes. The App cache drops its
    /// own transient ownership when each load settles.
    pub fn preload_compressed_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
    ) -> Vec<CompressedImagePreload> {
        sources
            .into_iter()
            .map(|resource| {
                self.fetch_asset::<crate::CompressedImageLoader>(&CompressedImageSource::new(
                    resource,
                ))
            })
            .collect()
    }

    /// Removes the App cache ownership for compressed image bytes.
    pub fn remove_compressed_image_resource(
        &mut self,
        source: &AssetLocation,
    ) -> Option<CompressedImagePreload> {
        self.take_asset::<crate::CompressedImageLoader>(&CompressedImageSource::new(source.clone()))
    }

    /// Builds the opaque target-size image source GPUI uses for bounds-aware resource decoding.
    pub fn image_render_request(
        &self,
        source: AssetLocation,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Option<ImageRenderRequest> {
        crate::image_size_for_bounds(logical_size, scale_factor)
            .map(|target| ImageRenderRequest::new(source, target, scale_factor, object_fit))
    }

    /// Starts decoding a resource image for a previously computed GPUI target-size source.
    pub fn preload_sized_image(&mut self, target_source: ImageRenderRequest) -> SizedImagePreload {
        self.fetch_asset::<crate::SizedImageLoader>(&target_source)
    }

    /// Starts decoding resource images to a concrete target size in GPUI's global image asset cache.
    pub fn preload_sized_images(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Vec<SizedImagePreload> {
        let mut preloads = Vec::new();
        for resource in sources {
            let Some(target_source) =
                self.image_render_request(resource, logical_size, scale_factor, object_fit)
            else {
                continue;
            };
            preloads.push(self.preload_sized_image(target_source));
        }
        preloads
    }

    /// Removes the App cache ownership for a target-size image processing.
    pub fn remove_image_render_request(
        &mut self,
        target_source: &ImageRenderRequest,
    ) -> Option<SizedImagePreload> {
        self.take_asset::<crate::SizedImageLoader>(target_source)
    }

    /// Removes the App cache ownership for a target-size image processing.
    pub fn remove_sized_image(
        &mut self,
        source: &AssetLocation,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Option<SizedImagePreload> {
        let target_source =
            self.image_render_request(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_image_render_request(&target_source)
    }

    /// Removes a target-size image processing and drops its completed render image from window atlases.
    ///
    /// Active element owners take precedence over explicit cache removal: their GPU resource stays
    /// drawable until the final element releases the request.
    pub fn remove_image_render_request_in(
        &mut self,
        target_source: &ImageRenderRequest,
        current_window: Option<&mut Window>,
    ) -> Option<SizedImagePreload> {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(target_source));
        let has_element_owners = sized_image_element_ref_count(self, asset_id) != 0;
        let preload = self.remove_image_render_request(target_source)?;

        if !has_element_owners {
            if let Some(Ok(image)) = preload.get() {
                self.drop_image(image, current_window);
            }
            drop_image_asset_retained(asset_id.1);
        }

        Some(preload)
    }

    /// Removes a target-size image processing and drops its completed render image from window atlases.
    pub fn remove_sized_image_from_windows(
        &mut self,
        source: &AssetLocation,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
        current_window: Option<&mut Window>,
    ) -> Option<SizedImagePreload> {
        let target_source =
            self.image_render_request(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_image_render_request_in(&target_source, current_window)
    }

    /// Retires an image's window-side lookup state and GPU atlas allocations.
    pub fn drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>) {
        for window in self.windows.values_mut().flatten() {
            _ = window.drop_image(image.clone());
        }
        if let Some(window) = current_window {
            _ = window.drop_image(image);
        }
    }

    /// Returns the image pipeline configuration used by newly rendered image elements.
    pub fn image_pipeline_config(&self) -> ImagePipelineConfig {
        self.image_pipeline_config
    }
}
