use std::{any::TypeId, sync::Arc};

use futures::FutureExt;

use crate::{
    ImagePipelineConfig, ImageRenderRequest, RenderImage, SizedImageLoader, SizedImageTask, Window,
    drop_image_asset_retained, hash,
};

use super::App;

impl App {
    /// Returns the active image pipeline configuration for callers that should not depend on the
    /// concrete App field layout.
    pub fn image_pipeline_config(&self) -> ImagePipelineConfig {
        self.image_pipeline_config
    }

    /// Retires a decoded image from every live window atlas.
    ///
    /// Image asset ownership is global, but atlas residency is per-window. The currently-updated
    /// window can be temporarily removed from `App.windows`, so callers that are already updating a
    /// window may pass it explicitly; external application code can pass `None` and let GPUI retire
    /// the image from all registered windows.
    pub fn drop_image(
        &mut self,
        image: Arc<RenderImage>,
        current_window: Option<&mut Window>,
    ) {
        let image_id = image.id;
        for window in self.windows.values_mut().flatten() {
            if let Err(error) = window.drop_image(image.clone()) {
                log::warn!(
                    "failed to drop image from window atlas: image_id={:?}: {error:#}",
                    image_id
                );
            }
        }

        if let Some(window) = current_window {
            if let Err(error) = window.drop_image(image) {
                log::warn!(
                    "failed to drop image from current window atlas: image_id={:?}: {error:#}",
                    image_id
                );
            }
        }
    }

    /// Releases one element-owned sized-image request and cleans up an orphaned decode that may
    /// finish after the last owner has already left the element tree.
    ///
    /// `loading_assets` is also the generation barrier here: if the same request is acquired again
    /// before the old decode settles, a new cache entry exists and the old completion must not
    /// retire the replacement image's shared ImageId/atlas entry.
    pub(crate) fn release_sized_image_element_request_lifecycle(
        &mut self,
        request: &ImageRenderRequest,
        fallback_image: Option<Arc<RenderImage>>,
        current_window: Option<&mut Window>,
    ) {
        let asset_id = (TypeId::of::<SizedImageLoader>(), hash(request));
        let pending_task = self
            .loading_assets
            .get(&asset_id)
            .and_then(|task| task.downcast_ref::<SizedImageTask>())
            .cloned()
            .filter(|task| task.clone().now_or_never().is_none());

        self.release_sized_image_element_request(request, fallback_image, current_window);

        let Some(pending_task) = pending_task else {
            return;
        };
        if self.loading_assets.contains_key(&asset_id) {
            // Another live element still owns this request, so the lower-level release intentionally
            // kept the task registered.
            return;
        }

        self.spawn(async move |cx| {
            let result = pending_task.await;
            let _ = cx.update(|cx| {
                // A later request with the same source hash supersedes this orphaned completion.
                if cx.loading_assets.contains_key(&asset_id) {
                    return;
                }
                if let Ok(image) = result {
                    cx.drop_image(image, None);
                }
                drop_image_asset_retained(asset_id.1);
            });
        })
        .detach();
    }
}
