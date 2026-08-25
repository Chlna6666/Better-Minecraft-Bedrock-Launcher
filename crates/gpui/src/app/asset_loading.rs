use std::{any::TypeId, sync::Arc};

use anyhow::Result;
use futures::{FutureExt, future::Shared};

use crate::{
    Asset, AssetLocation, CompressedImageSource, CompressedImageTask, ImageCacheError,
    ImageMemoryTrimLevel, ImagePipelineConfig, ImageRenderRequest, ObjectFit, Pixels, RenderImage,
    Size, SizedImageTask, Task, Window, drop_image_asset_retained, hash,
};

use super::App;

#[cfg(test)]
#[path = "asset_loading_tests.rs"]
mod asset_loading_tests;

impl App {
    /// Trims idle image state without applying byte ceilings to active images.
    pub fn trim_image_memory(&mut self, level: ImageMemoryTrimLevel) {
        let bitmap_pool_limit = match level {
            ImageMemoryTrimLevel::Light => self.image_pipeline_config.bitmap_pool_bytes.saturating_mul(3) / 4,
            ImageMemoryTrimLevel::Moderate | ImageMemoryTrimLevel::Aggressive => 0,
        };
        crate::assets::trim_global_bitmap_pool_to(bitmap_pool_limit);
        crate::trim_compressed_cache();

        if matches!(level, ImageMemoryTrimLevel::Light) {
            return;
        }

        // Completed compressed-image tasks are reusable cache state rather than active image
        // ownership. Moderate/aggressive trims release those strong task outputs; the global
        // compressed cache contains only Weak references, so bytes disappear when no decode or
        // explicit preload still owns them.
        let compressed_type = TypeId::of::<crate::CompressedImageLoader>();
        let completed_compressed = self
            .loading_assets
            .iter()
            .filter_map(|(asset_id, task)| {
                if asset_id.0 != compressed_type {
                    return None;
                }
                task.downcast_ref::<
                    Shared<Task<Result<crate::CompressedImageBytes, ImageCacheError>>>,
                >()
                .is_some_and(|task| task.clone().now_or_never().is_some())
                .then_some(*asset_id)
            })
            .collect::<Vec<_>>();
        for asset_id in completed_compressed {
            self.loading_assets.remove(&asset_id);
        }
        crate::trim_compressed_cache();

        let resource_type = TypeId::of::<crate::ResourceImageLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<crate::ClipboardImageLoader>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<crate::EncodedImageLoader>>();
        let target_type = TypeId::of::<crate::SizedImageLoader>();
        let mut evicted = Vec::new();
        for (asset_id, task) in &self.loading_assets {
            let is_image = matches!(
                asset_id.0,
                id if id == resource_type
                    || id == inline_type
                    || id == inline_bytes_type
                    || id == target_type
            );
            if !is_image {
                continue;
            }
            let Some(task) =
                task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
            else {
                continue;
            };
            let Some(Ok(image)) = task.clone().now_or_never() else {
                continue;
            };
            if Arc::strong_count(&image) <= 2 {
                evicted.push((*asset_id, image));
            }
        }

        for (asset_id, image) in evicted {
            self.loading_assets.remove(&asset_id);
            self.drop_image(image, None);
        }
    }

    /// Remove an asset from GPUI's cache.
    pub fn remove_asset<A: Asset>(&mut self, source: &A::Source) {
        self.take_asset::<A>(source);
    }

    /// Remove an asset from GPUI's cache and return its task if it exists.
    pub fn take_asset<A: Asset>(&mut self, source: &A::Source) -> Option<Shared<Task<A::Output>>> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        self.loading_assets
            .remove(&asset_id)
            .map(|boxed_task| *boxed_task.downcast::<Shared<Task<A::Output>>>().unwrap())
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    ///
    /// Multiple calls only result in one `Asset::load` call at a time, and the result is shared
    /// through the normal GPUI asset task cache. Compressed image byte tasks are transient: GPUI
    /// keeps them only while I/O is in flight, then retires its internal strong task reference so
    /// completed bytes follow the lifetime of active decodes and explicit preload handles.
    pub fn fetch_asset<A: Asset>(&mut self, source: &A::Source) -> (Shared<Task<A::Output>>, bool) {
        let asset_id = (TypeId::of::<A>(), hash(source));
        // Fast path: clone an already registered task without removing and re-boxing it.
        let existing = self
            .loading_assets
            .get(&asset_id)
            .and_then(|task| task.downcast_ref::<Shared<Task<A::Output>>>())
            .cloned();
        let mut is_first = false;
        let mut transient_identity = None;
        let task = existing.unwrap_or_else(|| {
            is_first = true;
            let future = A::load(source.clone(), self);
            let task = self.background_executor().spawn(future).shared();
            if TypeId::of::<A>() == TypeId::of::<crate::CompressedImageLoader>() {
                transient_identity = task.downgrade();
            }
            self.loading_assets.insert(asset_id, Box::new(task.clone()));
            task
        });

        if let Some(transient_identity) = transient_identity {
            let completion = task.clone();
            self.spawn(async move |cx| {
                let _ = completion.await;
                let Some(completed_task) = transient_identity.upgrade() else {
                    return;
                };

                // The same source may have been explicitly removed and requested again before the
                // old load completed. Only retire the exact Shared future that scheduled this
                // cleanup; never let an old completion evict a replacement task with the same key.
                let _ = cx.update(|cx| {
                    let should_remove = cx
                        .loading_assets
                        .get(&asset_id)
                        .and_then(|task| task.downcast_ref::<Shared<Task<A::Output>>>())
                        .is_some_and(|task| task.ptr_eq(&completed_task));
                    if should_remove {
                        cx.loading_assets.remove(&asset_id);
                    }
                });
            })
            .detach();
        }

        (task, is_first)
    }

    /// Starts loading resource images into GPUI's global image asset cache.
    pub fn preload_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
    ) -> Vec<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>> {
        sources
            .into_iter()
            .map(|source| self.fetch_asset::<crate::ResourceImageLoader>(&source).0)
            .collect()
    }

    /// Starts loading compressed image bytes into GPUI's global image asset cache.
    ///
    /// This is intended for images that will later be rendered with
    /// [`StyledImage::render_to_bounds`](crate::StyledImage::render_to_bounds). The final decode
    /// still happens after layout determines the target size, but file/network I/O can begin
    /// earlier and the task output is shared with target-size decodes while it remains owned.
    /// Holding the returned task keeps completed compressed bytes strongly resident; GPUI's own
    /// task-cache reference is released automatically when the load settles.
    pub fn preload_compressed_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
    ) -> Vec<CompressedImageTask> {
        sources
            .into_iter()
            .map(|resource| {
                self.fetch_asset::<crate::CompressedImageLoader>(&CompressedImageSource::new(
                    resource,
                ))
                .0
            })
            .collect()
    }

    /// Removes compressed image bytes previously requested through
    /// [`preload_compressed_image_resources`](Self::preload_compressed_image_resources).
    pub fn remove_compressed_image_resource(
        &mut self,
        source: &AssetLocation,
    ) -> Option<CompressedImageTask> {
        self.take_asset::<crate::CompressedImageLoader>(&CompressedImageSource::new(source.clone()))
    }

    /// Builds the opaque target-size image source GPUI uses for bounds-aware resource decoding.
    ///
    /// Applications that need to coordinate preloading across resize events can store the returned
    /// value and compare it before replacing a preload. Adjacent logical sizes may intentionally
    /// map to the same target because GPUI buckets decode dimensions internally.
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
    pub fn preload_sized_image(&mut self, target_source: ImageRenderRequest) -> SizedImageTask {
        self.fetch_asset::<crate::SizedImageLoader>(&target_source)
            .0
    }

    /// Starts decoding resource images to a concrete target size in GPUI's global image asset cache.
    ///
    /// This is useful when an application already knows the expected paint size before the first
    /// frame. The resulting cache entry is shared with
    /// [`StyledImage::render_to_bounds`](crate::StyledImage::render_to_bounds), so the element can
    /// paint as soon as the matching target decode completes.
    pub fn preload_sized_images(
        &mut self,
        sources: impl IntoIterator<Item = AssetLocation>,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Vec<SizedImageTask> {
        let mut tasks = Vec::new();
        for resource in sources {
            let Some(target_source) =
                self.image_render_request(resource, logical_size, scale_factor, object_fit)
            else {
                continue;
            };
            tasks.push(self.preload_sized_image(target_source));
        }
        tasks
    }

    /// Removes a target-size image processing previously requested through
    /// [`preload_sized_image`](Self::preload_sized_image).
    pub fn remove_image_render_request(
        &mut self,
        target_source: &ImageRenderRequest,
    ) -> Option<SizedImageTask> {
        self.take_asset::<crate::SizedImageLoader>(target_source)
    }

    /// Removes a target-size image processing previously requested through
    /// [`preload_sized_images`](Self::preload_sized_images).
    pub fn remove_sized_image(
        &mut self,
        source: &AssetLocation,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Option<SizedImageTask> {
        let target_source =
            self.image_render_request(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_image_render_request(&target_source)
    }

    /// Removes a target-size image processing and drops its completed render image from window atlases.
    pub fn remove_image_render_request_in(
        &mut self,
        target_source: &ImageRenderRequest,
        current_window: Option<&mut Window>,
    ) -> Option<SizedImageTask> {
        let task = self.remove_image_render_request(target_source)?;

        if let Some(Ok(image)) = task.clone().now_or_never() {
            self.drop_image(image, current_window);
            drop_image_asset_retained(hash(target_source));
        }

        Some(task)
    }

    /// Removes a target-size image processing and drops its completed render image from window atlases.
    ///
    /// This should be preferred over [`remove_sized_image`](Self::remove_sized_image)
    /// when a caller has the current window available, such as when replacing a bounds-aware
    /// background image.
    pub fn remove_sized_image_from_windows(
        &mut self,
        source: &AssetLocation,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
        current_window: Option<&mut Window>,
    ) -> Option<SizedImageTask> {
        let target_source =
            self.image_render_request(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_image_render_request_in(&target_source, current_window)
    }

    /// Retires an image's window-side lookup state and GPU atlas allocations.
    ///
    /// Backends defer the physical GPU resource destruction until it is safe for submitted frames;
    /// a quick repaint can cancel a still-pending image retirement and reuse the existing upload.
    /// If the current window is being updated, it will be removed from `App.windows`; use
    /// `current_window` to include it explicitly.
    pub fn drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>) {
        // Remove the texture from all other windows.
        for window in self.windows.values_mut().flatten() {
            _ = window.drop_image(image.clone());
        }

        // Remove the texture from the current window.
        if let Some(window) = current_window {
            _ = window.drop_image(image);
        }
    }

    /// Returns the image pipeline configuration used by newly rendered image elements.
    pub fn image_pipeline_config(&self) -> ImagePipelineConfig {
        self.image_pipeline_config
    }
}
