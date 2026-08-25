use crate::{App, AssetLocation, Entity, ImageCacheError, RenderImage, Window};
use std::sync::Arc;

use super::AnyImageCache;

/// Loads and releases rendered images.
pub trait ImageCache: 'static {
    /// Return a loaded image, or `None` while it is still loading.
    fn load(
        &mut self,
        resource: &AssetLocation,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>>;
}

/// Creates an image cache during layout.
pub trait ImageCacheProvider: 'static {
    /// Provide the cache used by an image element.
    fn provide(&mut self, window: &mut Window, cx: &mut App) -> AnyImageCache;
}

impl<I: ImageCache> ImageCacheProvider for Entity<I> {
    fn provide(&mut self, _window: &mut Window, _cx: &mut App) -> AnyImageCache {
        self.clone().into()
    }
}
