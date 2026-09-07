use std::{any::TypeId, sync::Arc};

use futures::FutureExt;

use crate::{
    ImageRenderRequest, RenderImage, SizedImageLoader, SizedImageTask, Window,
    drop_image_asset_retained, hash,
};

use super::App;

impl App {
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
