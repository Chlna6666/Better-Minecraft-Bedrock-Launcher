use std::{
    any::{Any, TypeId},
    rc::Rc,
    sync::Arc,
};

use anyhow::Result;
use crate::{
    AnyWindowHandle, Asset, AssetLease, AssetLocation, AssetRetentionPolicy,
    CompressedImagePreload, CompressedImageSource, EntityId, ImageCacheError, ImageMemoryTrimLevel,
    ImagePipelineConfig, ImageRenderRequest, ObjectFit, Pixels, RenderImage, Size,
    SizedImagePreload, Window, drop_image_asset_retained, hash,
};

use super::App;

type AssetId = (TypeId, u64);

struct OwnedAssetEntry<T>
where
    T: Clone + Send + 'static,
{
    identity: Rc<()>,
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
        let identity = Rc::new(());

        if A::RETENTION == AssetRetentionPolicy::TransientAfterReady {
            let completion = lease.completion_signal();
            let weak_identity = Rc::downgrade(&identity);
            cx.spawn(async move |cx| {
                if !completion.wait().await {
                    return;
                }
                let Some(identity) = weak_identity.upgrade() else {
                    return;
                };
                let _ = cx.update(move |cx| {
                    let is_same_entry = cx
                        .loading_assets
                        .get(&asset_id)
                        .and_then(|entry| entry.downcast_ref::<OwnedAssetEntry<T>>())
                        .is_some_and(|entry| Rc::ptr_eq(&entry.identity, &identity));
                    if is_same_entry {
                        cx.loading_assets.remove(&asset_id);
                    }
                });
            })
            .detach();
        }

        Self { identity, lease }
    }

    fn get(&self) -> Option<T> {
        self.lease.get()
    }

    fn lease(&self) -> AssetLease<T> {
        self.lease.clone()
    }

    fn pin_count(&self) -> usize {
        self.lease.pin_count()
    }

    fn shares_pin(&self, pin: &crate::AssetPin<T>) -> bool {
        pin.shares_load(&self.lease)
    }

    fn into_lease(self) -> AssetLease<T> {
        self.lease
    }

    fn use_by(&self, window: AnyWindowHandle, view: EntityId) -> Option<T> {
        self.lease.use_by(window, view)
    }
}

pub(super) fn cached_asset_output<T>(entry: &dyn Any) -> Option<T>
where
    T: Clone + Send + 'static,
{
    entry.downcast_ref::<OwnedAssetEntry<T>>()?.get()
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
            let is_pinned_target = asset_id.0 == target_type
                && entry
                    .downcast_ref::<OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>>()
                    .is_some_and(|entry| entry.pin_count() != 0);
            if !is_image || is_pinned_target {
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

    pub(crate) fn pin_sized_image_request(
        &mut self,
        request: &ImageRenderRequest,
    ) -> crate::AssetPin<Result<Arc<RenderImage>, ImageCacheError>> {
        self.fetch_asset::<crate::SizedImageLoader>(request).pin()
    }

    pub(crate) fn release_sized_image_element_pin(
        &mut self,
        request: &ImageRenderRequest,
        pin: crate::AssetPin<Result<Arc<RenderImage>, ImageCacheError>>,
        fallback_image: Option<Arc<RenderImage>>,
        current_window: Option<&mut Window>,
    ) {
        let asset_id = (TypeId::of::<crate::SizedImageLoader>(), hash(request));
        let should_retire = self
            .loading_assets
            .get(&asset_id)
            .and_then(|entry| {
                entry.downcast_ref::<
                    OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>,
                >()
            })
            .is_some_and(|entry| entry.shares_pin(&pin) && entry.pin_count() == 1);

        if !should_retire {
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
        self.loading_assets
            .get(&asset_id)
            .and_then(|entry| {
                entry.downcast_ref::<
                    OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>,
                >()
            })
            .map_or(0, OwnedAssetEntry::pin_count)
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
            && self
                .loading_assets
                .get(&asset_id)
                .and_then(|entry| entry.downcast_ref::<OwnedAssetEntry<A::Output>>())
                .is_some_and(|entry| entry.pin_count() != 0)
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
        let has_element_pins = self
            .loading_assets
            .get(&asset_id)
            .and_then(|entry| {
                entry.downcast_ref::<
                    OwnedAssetEntry<Result<Arc<RenderImage>, ImageCacheError>>,
                >()
            })
            .is_some_and(|entry| entry.pin_count() != 0);
        let preload = self.remove_image_render_request(target_source)?;

        if !has_element_pins {
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
