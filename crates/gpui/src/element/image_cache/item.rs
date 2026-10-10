use crate::{
    App, AssetLease, AssetLocation, ImageCacheError, RenderImage, ResourceImageLoader,
    WeakAssetLease, Window,
};
use std::{fmt, sync::Arc};

/// A resource-image cache entry backed by the App's shared decode.
///
/// Pending work is cancelled when the final cache/lease owner disappears. Completed images are
/// stored in the shared asset ready state, and pending users are invalidated by their retained
/// element paths within each view when the load finishes.
pub struct ImageCacheItem(Ownership);

enum Ownership {
    Owned(AssetLease<Result<Arc<RenderImage>, ImageCacheError>>),
    Shared(WeakAssetLease<Result<Arc<RenderImage>, ImageCacheError>>),
}

impl fmt::Debug for ImageCacheItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageCacheItem")
            .field("result", &self.get())
            .finish()
    }
}

impl ImageCacheItem {
    /// Acquires an owning lease for the App's shared resource-image decode.
    /// Other caches and ordinary image elements reuse the same pending or ready load.
    pub fn new(source: &AssetLocation, cx: &mut App) -> Self {
        Self(Ownership::Owned(
            cx.fetch_asset::<ResourceImageLoader>(source),
        ))
    }

    pub(crate) fn shared(source: &AssetLocation, cx: &mut App) -> Self {
        Self(Ownership::Shared(
            cx.fetch_asset::<ResourceImageLoader>(source).downgrade(),
        ))
    }

    pub(crate) fn is_shared(&self) -> bool {
        matches!(self.0, Ownership::Shared(_))
    }

    pub(crate) fn is_live(&self) -> bool {
        match &self.0 {
            Ownership::Owned(_) => true,
            Ownership::Shared(lease) => lease.upgrade().is_some(),
        }
    }

    pub(crate) fn ready(result: Result<Arc<RenderImage>, ImageCacheError>) -> Self {
        Self(Ownership::Owned(AssetLease::ready(result)))
    }

    /// Returns the completed image result without subscribing to readiness.
    pub fn get(&self) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match &self.0 {
            Ownership::Owned(lease) => lease.get(),
            Ownership::Shared(lease) => lease.upgrade()?.get(),
        }
    }

    /// Returns the completed image or subscribes the current view to exact retained invalidation.
    pub fn use_image(&self, window: &Window) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        if let Some(result) = self.get() {
            return Some(result);
        }
        let shared;
        let lease = match &self.0 {
            Ownership::Owned(lease) => lease,
            Ownership::Shared(weak) => {
                shared = weak.upgrade()?;
                &shared
            }
        };
        lease.use_by(
            window.any_window_handle(),
            window.current_view(),
            window.current_retained_element_id(),
        )
    }
}
