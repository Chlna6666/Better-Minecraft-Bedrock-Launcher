use std::{any::TypeId, sync::Arc};

use anyhow::Result;
use futures::{FutureExt, future::Shared};

use crate::{
    Asset, CompressedImageLoadingTask, CompressedImageSource, ImageCacheError, ImagePipelineConfig,
    ObjectFit, Pixels, RenderImage, Resource, Size, TargetSizeImageLoadingTask,
    TargetSizeImageSource, Task, Window, drop_image_asset_retained, hash,
};

use super::App;

#[cfg(test)]
#[path = "asset_loading_tests.rs"]
mod asset_loading_tests;

impl App {
    /// Remove an asset from GPUI's cache
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

    pub(crate) fn cached_asset_task<A: Asset>(
        &self,
        source: &A::Source,
    ) -> Option<Shared<Task<A::Output>>> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        self.loading_assets.get(&asset_id).map(|boxed_task| {
            boxed_task
                .downcast_ref::<Shared<Task<A::Output>>>()
                .unwrap()
                .clone()
        })
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time, and the results of this call will be cached
    pub fn fetch_asset<A: Asset>(&mut self, source: &A::Source) -> (Shared<Task<A::Output>>, bool) {
        let asset_id = (TypeId::of::<A>(), hash(source));
        let mut is_first = false;
        let task = self
            .loading_assets
            .remove(&asset_id)
            .map(|boxed_task| *boxed_task.downcast::<Shared<Task<A::Output>>>().unwrap())
            .unwrap_or_else(|| {
                is_first = true;
                let future = A::load(source.clone(), self);

                self.background_executor().spawn(future).shared()
            });

        self.loading_assets.insert(asset_id, Box::new(task.clone()));

        (task, is_first)
    }

    /// Starts loading resource images into GPUI's global image asset cache.
    pub fn preload_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = Resource>,
    ) -> Vec<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>> {
        sources
            .into_iter()
            .map(|source| self.fetch_asset::<crate::ImgResourceLoader>(&source).0)
            .collect()
    }

    /// Starts loading compressed image bytes into GPUI's global image asset cache.
    ///
    /// This is intended for images that will later be rendered with
    /// [`StyledImage::decode_to_bounds`](crate::StyledImage::decode_to_bounds). The final decode
    /// still happens after layout determines the target size, but file/network I/O and compressed
    /// byte retention can begin earlier and will be shared with target-size decodes.
    pub fn preload_compressed_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = Resource>,
    ) -> Vec<CompressedImageLoadingTask> {
        sources
            .into_iter()
            .map(|resource| {
                self.fetch_asset::<crate::CompressedImgResourceLoader>(&CompressedImageSource::new(
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
        source: &Resource,
    ) -> Option<CompressedImageLoadingTask> {
        self.take_asset::<crate::CompressedImgResourceLoader>(&CompressedImageSource::new(
            source.clone(),
        ))
    }

    /// Builds the opaque target-size image source GPUI uses for bounds-aware resource decoding.
    ///
    /// Applications that need to coordinate preloading across resize events can store the returned
    /// value and compare it before replacing a preload. Adjacent logical sizes may intentionally
    /// map to the same target because GPUI buckets decode dimensions internally.
    #[expect(
        clippy::unused_self,
        reason = "this is an App API so callers do not depend on GPUI's target bucketing internals"
    )]
    pub fn target_size_image_source(
        &self,
        source: Resource,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Option<TargetSizeImageSource> {
        crate::target_size_for_decode(logical_size, scale_factor)
            .map(|target| TargetSizeImageSource::new(source, target, scale_factor, object_fit))
    }

    /// Starts decoding a resource image for a previously computed GPUI target-size source.
    pub fn preload_target_size_image(
        &mut self,
        target_source: TargetSizeImageSource,
    ) -> TargetSizeImageLoadingTask {
        self.fetch_asset::<crate::TargetSizeImgResourceLoader>(&target_source)
            .0
    }

    /// Starts decoding resource images to a concrete target size in GPUI's global image asset cache.
    ///
    /// This is useful when an application already knows the expected paint size before the first
    /// frame. The resulting cache entry is shared with
    /// [`StyledImage::decode_to_bounds`](crate::StyledImage::decode_to_bounds), so the element can
    /// paint as soon as the matching target decode completes.
    pub fn preload_target_size_images(
        &mut self,
        sources: impl IntoIterator<Item = Resource>,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Vec<TargetSizeImageLoadingTask> {
        let mut tasks = Vec::new();
        for resource in sources {
            let Some(target_source) =
                self.target_size_image_source(resource, logical_size, scale_factor, object_fit)
            else {
                continue;
            };
            tasks.push(self.preload_target_size_image(target_source));
        }
        tasks
    }

    /// Removes a target-size image decode previously requested through
    /// [`preload_target_size_image`](Self::preload_target_size_image).
    pub fn remove_target_size_image_source(
        &mut self,
        target_source: &TargetSizeImageSource,
    ) -> Option<TargetSizeImageLoadingTask> {
        self.take_asset::<crate::TargetSizeImgResourceLoader>(target_source)
    }

    /// Removes a target-size image decode previously requested through
    /// [`preload_target_size_images`](Self::preload_target_size_images).
    pub fn remove_target_size_image(
        &mut self,
        source: &Resource,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
    ) -> Option<TargetSizeImageLoadingTask> {
        let target_source =
            self.target_size_image_source(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_target_size_image_source(&target_source)
    }

    /// Removes a target-size image decode and drops its completed render image from window atlases.
    pub fn remove_target_size_image_source_in(
        &mut self,
        target_source: &TargetSizeImageSource,
        current_window: Option<&mut Window>,
    ) -> Option<TargetSizeImageLoadingTask> {
        let task = self.remove_target_size_image_source(target_source)?;

        if let Some(Ok(image)) = task.clone().now_or_never() {
            self.drop_image(image, current_window);
            drop_image_asset_retained(hash(target_source));
        }

        Some(task)
    }

    /// Removes a target-size image decode and drops its completed render image from window atlases.
    ///
    /// This should be preferred over [`remove_target_size_image`](Self::remove_target_size_image)
    /// when a caller has the current window available, such as when replacing a bounds-aware
    /// background image.
    pub fn remove_target_size_image_in(
        &mut self,
        source: &Resource,
        logical_size: Size<Pixels>,
        scale_factor: f32,
        object_fit: ObjectFit,
        current_window: Option<&mut Window>,
    ) -> Option<TargetSizeImageLoadingTask> {
        let target_source =
            self.target_size_image_source(source.clone(), logical_size, scale_factor, object_fit)?;
        self.remove_target_size_image_source_in(&target_source, current_window)
    }

    /// Removes an image from the sprite atlas on all windows.
    ///
    /// If the current window is being updated, it will be removed from `App.windows`, you can use `current_window` to specify the current window.
    /// This is a no-op if the image is not in the sprite atlas.
    pub fn drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>) {
        // remove the texture from all other windows
        for window in self.windows.values_mut().flatten() {
            _ = window.drop_image(image.clone());
        }

        // remove the texture from the current window
        if let Some(window) = current_window {
            _ = window.drop_image(image);
        }
    }

    /// Returns the image pipeline configuration used by newly rendered image elements.
    pub fn image_pipeline_config(&self) -> ImagePipelineConfig {
        self.image_pipeline_config
    }
}
