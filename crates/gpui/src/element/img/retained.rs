use std::{
    any::TypeId,
    cell::RefCell,
    rc::{Rc, Weak},
    sync::Arc,
    time::Instant,
};

use linked_hash_map::LinkedHashMap;

use crate::{AnimatedFrame, App, AsyncApp, RenderImage, Task, hash};

use super::loader::ImageRenderRequest;

const WARM_SIZED_IMAGE_CACHE_ITEMS: usize = 128;
const WARM_SIZED_IMAGE_CACHE_BYTES: usize = 128 * 1024 * 1024;

type SizedImageWarmCacheHandle = Rc<RefCell<SizedImageWarmCache>>;
type SizedImageWarmCacheWeak = Weak<RefCell<SizedImageWarmCache>>;

/// Playback and loading values retained by ordinary image elements between frames.
pub(crate) struct ImageElementState {
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(crate) current_frame: Option<AnimatedFrame>,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) started_loading: Option<(Instant, Task<()>)>,
}

struct WarmSizedImageEntry {
    lease: SizedImageRequestLease,
    estimated_bytes: usize,
}

#[derive(Default)]
struct SizedImageWarmCache {
    entries: LinkedHashMap<u64, WarmSizedImageEntry>,
    estimated_bytes: usize,
}

impl SizedImageWarmCache {
    fn take(&mut self, request: &ImageRenderRequest) -> Option<SizedImageRequestLease> {
        let key = hash(request);
        let entry = self.entries.remove(&key)?;
        if entry.lease.request() != request {
            // Hash collisions should be vanishingly rare, but never transfer ownership to a
            // different concrete render request. Restore the entry without changing its owner
            // count and let the caller acquire a normal lease.
            self.entries.insert(key, entry);
            return None;
        }
        self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
        Some(entry.lease)
    }

    fn insert(
        &mut self,
        lease: SizedImageRequestLease,
        estimated_bytes: usize,
    ) -> Vec<SizedImageRequestLease> {
        let key = hash(lease.request());
        let mut releases = Vec::new();

        // Multiple elements may display the same request. Keep only one warm owner for a request;
        // redundant leases are released after this cache mutation, leaving the new warm owner in
        // place so the decoded task and GPU residency never hit an owner-count gap.
        if let Some(previous) = self.entries.remove(&key) {
            self.estimated_bytes = self
                .estimated_bytes
                .saturating_sub(previous.estimated_bytes);
            releases.push(previous.lease);
        }

        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.entries.insert(
            key,
            WarmSizedImageEntry {
                lease,
                estimated_bytes,
            },
        );

        while self.entries.len() > WARM_SIZED_IMAGE_CACHE_ITEMS
            || self.estimated_bytes > WARM_SIZED_IMAGE_CACHE_BYTES
        {
            let Some((_key, entry)) = self.entries.pop_front() else {
                break;
            };
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            releases.push(entry.lease);
        }

        releases
    }
}

fn sized_image_warm_cache(cx: &mut App) -> SizedImageWarmCacheHandle {
    cx.globals_by_type
        .entry(TypeId::of::<SizedImageWarmCacheHandle>())
        .or_insert_with(|| {
            Box::new(Rc::new(RefCell::new(SizedImageWarmCache::default())))
        })
        .downcast_ref::<SizedImageWarmCacheHandle>()
        .expect("sized image warm cache type mismatch")
        .clone()
}

/// One owner of a concrete bounds-aware image request.
///
/// A lease is transferable between an on-tree element and the warm working-set cache. Promotion
/// from warm back to active does not touch the application-wide owner count, which keeps the
/// completed `SizedImageLoader` task and its atlas allocation resident across virtualization,
/// tab/window activation changes, and other short-lived element-tree gaps.
pub(super) struct SizedImageRequestLease {
    request: ImageRenderRequest,
    app: AsyncApp,
    // The warm cache owns warm leases, so leases keep only a weak back-reference. This avoids an
    // App-lifetime Rc cycle while preserving zero-owner-gap promotion between warm and active state.
    warm_cache: SizedImageWarmCacheWeak,
}

impl SizedImageRequestLease {
    pub(super) fn acquire(request: &ImageRenderRequest, cx: &mut App) -> Self {
        let warm_cache = sized_image_warm_cache(cx);
        let warm_hit = { warm_cache.borrow_mut().take(request) };
        if let Some(lease) = warm_hit {
            return lease;
        }

        cx.retain_sized_image_element_request(request);
        Self {
            request: request.clone(),
            app: cx.to_async(),
            warm_cache: Rc::downgrade(&warm_cache),
        }
    }

    pub(super) fn request(&self) -> &ImageRenderRequest {
        &self.request
    }

    pub(crate) fn into_request(self) -> ImageRenderRequest {
        self.request
    }

    fn defer_release(self, image: Option<Arc<RenderImage>>) {
        let Self {
            request,
            app,
            warm_cache: _,
        } = self;
        app.spawn(async move |cx| {
            let _ = cx.update(|cx| {
                cx.release_sized_image_element_request_lifecycle(&request, image, None);
            });
        })
        .detach();
    }

    fn defer_warm(self, image: Option<Arc<RenderImage>>) {
        let Some(warm_cache) = self.warm_cache.upgrade() else {
            self.defer_release(image);
            return;
        };
        let estimated_bytes = image
            .as_ref()
            .map(|image| image.cache_cost_byte_len())
            .unwrap_or_default();
        let releases = warm_cache.borrow_mut().insert(self, estimated_bytes);

        // The loader task remains the canonical decoded-image owner while the warm lease is
        // resident, so the element's Arc is no longer required. Evicted/redundant leases go
        // through the existing lifecycle-aware release path, including orphaned in-flight loads.
        drop(image);
        for lease in releases {
            lease.defer_release(None);
        }
    }
}

/// Bounds-aware image state is kept separate from ordinary image playback state so switching
/// rendering modes naturally retires the old state at the frame boundary.
pub(crate) struct SizedImageElementState {
    pub(crate) playback: ImageElementState,
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(super) sized_image_request: Option<SizedImageRequestLease>,
    pub(super) pending_sized_image_drop: Option<SizedImageRequestLease>,
}

impl SizedImageElementState {
    pub(crate) fn new(current_frame: Option<AnimatedFrame>) -> Self {
        Self {
            playback: ImageElementState {
                current_image: None,
                current_frame,
                next_frame_at: None,
                started_loading: None,
            },
            current_image: None,
            sized_image_request: None,
            pending_sized_image_drop: None,
        }
    }
}

impl Drop for SizedImageElementState {
    fn drop(&mut self) {
        if let Some(current) = self.sized_image_request.take() {
            current.defer_warm(self.current_image.take());
        } else {
            self.current_image = None;
        }

        // An image may leave the virtualized tree while its newest target decode is still pending.
        // Keep that request warm as well so returning to the same page resumes the in-flight work
        // instead of cancelling it and issuing the same network/decode/upload chain again.
        if let Some(pending) = self.pending_sized_image_drop.take() {
            pending.defer_warm(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetLocation, ImageRenderSize, ObjectFit, SharedString, TestAppContext};

    fn request(label: &'static str) -> ImageRenderRequest {
        ImageRenderRequest::new(
            AssetLocation::Embedded(SharedString::from(label)),
            ImageRenderSize::new(64, 64).unwrap(),
            1.0,
            ObjectFit::Cover,
        )
    }

    #[test]
    fn warm_lease_promotes_without_owner_count_gap() {
        let test = TestAppContext::single();
        let request = request("warm-promotion");

        test.update(|cx| {
            let lease = SizedImageRequestLease::acquire(&request, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 1);

            lease.defer_warm(None);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 1);

            let promoted = SizedImageRequestLease::acquire(&request, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 1);

            let request = promoted.into_request();
            cx.release_sized_image_element_request(&request, None, None);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 0);
        });
    }

    #[test]
    fn duplicate_warm_request_keeps_single_cached_owner() {
        let mut test = TestAppContext::single();
        let request = request("warm-dedup");

        test.update(|cx| {
            let first = SizedImageRequestLease::acquire(&request, cx);
            let second = SizedImageRequestLease::acquire(&request, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 2);

            first.defer_warm(None);
            second.defer_warm(None);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 2);
        });

        // Releasing the redundant warm lease is foreground-deferred. After it runs there must be
        // exactly one warm owner left, and promotion must transfer that owner rather than add one.
        test.run_until_parked();
        test.update(|cx| {
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 1);
            let promoted = SizedImageRequestLease::acquire(&request, cx);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 1);
            let request = promoted.into_request();
            cx.release_sized_image_element_request(&request, None, None);
            assert_eq!(cx.sized_image_element_ref_count_for_test(&request), 0);
        });
    }
}
