use crate::{AnyEntity, App, Entity, ImageCacheError, RenderImage, Resource, Window};
use std::sync::Arc;

use super::ImageCache;

/// A dynamically typed image cache, which can be used to store any image cache
#[derive(Clone)]
pub struct AnyImageCache {
    image_cache: AnyEntity,
    load_fn: fn(
        image_cache: &AnyEntity,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>>,
}

impl<I: ImageCache> From<Entity<I>> for AnyImageCache {
    fn from(image_cache: Entity<I>) -> Self {
        Self {
            image_cache: image_cache.into_any(),
            load_fn: load::<I>,
        }
    }
}

impl AnyImageCache {
    /// Load an image given a resource
    /// returns the result of loading the image if it has finished loading, or None if it is still loading
    pub fn load(
        &self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        (self.load_fn)(&self.image_cache, resource, window, cx)
    }
}

fn load<I: 'static + ImageCache>(
    image_cache: &AnyEntity,
    resource: &Resource,
    window: &mut Window,
    cx: &mut App,
) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
    let image_cache = image_cache.clone().downcast::<I>().unwrap();
    image_cache.update(cx, |image_cache, cx| image_cache.load(resource, window, cx))
}
