use crate::{
    App, Asset, AssetLease, AssetLocation, AssetLogger, ImageAssetLoader, ImageCacheError,
    RenderImage, Window,
};
use std::{fmt, sync::Arc};

/// An owning image-cache entry.
///
/// Pending work is cancelled when the final cache/lease owner disappears. Completed images are
/// stored in the shared asset ready state, and pending users are invalidated by exact window/view
/// identity when the load finishes.
pub struct ImageCacheItem(AssetLease<Result<Arc<RenderImage>, ImageCacheError>>);

impl fmt::Debug for ImageCacheItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageCacheItem")
            .field("result", &self.get())
            .finish()
    }
}

impl ImageCacheItem {
    /// Starts loading an image into a new owning cache entry.
    pub fn new(source: &AssetLocation, cx: &mut App) -> Self {
        Self(AssetLease::spawn(
            AssetLogger::<ImageAssetLoader>::load(source.clone(), cx),
            cx,
        ))
    }

    pub(crate) fn ready(result: Result<Arc<RenderImage>, ImageCacheError>) -> Self {
        Self(AssetLease::ready(result))
    }

    /// Returns the completed image result without subscribing to readiness.
    pub fn get(&self) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.0.get()
    }

    /// Returns the completed image or subscribes the current view to exact retained invalidation.
    pub fn use_image(
        &self,
        window: &Window,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.0
            .use_by(window.any_window_handle(), window.current_view())
    }
}
