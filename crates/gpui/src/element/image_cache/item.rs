use crate::{ImageCacheError, RenderImage, Task};
use futures::{FutureExt, future::Shared};
use std::{fmt, sync::Arc};

/// An image loading task associated with an image cache.
pub type ImageLoadingTask = Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>;

/// An entry retained by an image cache.
pub enum ImageCacheItem {
    /// The image is still loading.
    Loading(ImageLoadingTask),
    /// The image load has completed.
    Loaded(Result<Arc<RenderImage>, ImageCacheError>),
}

impl fmt::Debug for ImageCacheItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = match self {
            Self::Loading(_) => "Loading...".to_string(),
            Self::Loaded(image) => format!("{image:?}"),
        };
        formatter
            .debug_struct("ImageCacheItem")
            .field("status", &status)
            .finish()
    }
}

impl ImageCacheItem {
    /// Return the completed image, if loading has finished.
    pub fn get(&mut self) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            Self::Loading(task) => {
                let result = task.now_or_never()?;
                *self = Self::Loaded(result.clone());
                Some(result)
            }
            Self::Loaded(result) => Some(result.clone()),
        }
    }
}
