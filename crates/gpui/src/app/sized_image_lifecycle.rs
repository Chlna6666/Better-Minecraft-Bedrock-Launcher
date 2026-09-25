use std::{any::TypeId, sync::Arc};

use crate::{
    ImageRenderRequest, RenderImage, SizedImageLoader, Window,
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
        let pending_preload = self
            .cached_asset_lease::<SizedImageLoader>(request)
            .filter(|preload| preload.get().is_none());

        self.release_sized_image_element_request(request, fallback_image, current_window);

        let Some(pending_preload) = pending_preload else {
            return;
        };
        if self.loading_assets.contains_key(&asset_id) {
            // Another live element still owns this request, so the lower-level release intentionally
            // kept the task registered.
            return;
        }

        self.spawn(async move |cx| {
            let result = pending_preload.wait().await;
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
