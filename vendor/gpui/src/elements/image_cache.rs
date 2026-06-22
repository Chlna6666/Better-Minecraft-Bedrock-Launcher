use crate::{
    AnyElement, AnyEntity, App, AppContext, AssetLogger, Bounds, Element, ElementId, Entity,
    EntityId, GlobalElementId, ImageAssetLoader, ImageCacheError, InspectorElementId, IntoElement,
    LayoutId, ParentElement, Pixels, RenderImage, Resource, Style, StyleRefinement, Styled, Task,
    Window, drop_image_cache_metrics, hash, record_image_cache_eviction,
    record_image_cache_metrics,
};

use collections::FxHashMap;
use futures::{FutureExt, future::Shared};
use linked_hash_map::LinkedHashMap;
use refineable::Refineable;
use smallvec::SmallVec;
use std::{
    cell::RefCell,
    fmt,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// An image cache element, all its child img elements will use the cache specified by this element.
/// Note that this could as simple as passing an `Entity<T: ImageCache>`
pub fn image_cache(image_cache_provider: impl ImageCacheProvider) -> ImageCacheElement {
    ImageCacheElement {
        image_cache_provider: Box::new(image_cache_provider),
        style: StyleRefinement::default(),
        children: SmallVec::default(),
    }
}

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
            load_fn: any_image_cache::load::<I>,
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

mod any_image_cache {
    use super::*;

    pub(crate) fn load<I: 'static + ImageCache>(
        image_cache: &AnyEntity,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let image_cache = image_cache.clone().downcast::<I>().unwrap();
        image_cache.update(cx, |image_cache, cx| image_cache.load(resource, window, cx))
    }
}

/// An image cache element.
pub struct ImageCacheElement {
    image_cache_provider: Box<dyn ImageCacheProvider>,
    style: StyleRefinement,
    children: SmallVec<[AnyElement; 2]>,
}

impl ParentElement for ImageCacheElement {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl Styled for ImageCacheElement {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for ImageCacheElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ImageCacheElement {
    type RequestLayoutState = SmallVec<[LayoutId; 4]>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let image_cache = self.image_cache_provider.provide(window, cx);
        window.with_image_cache(Some(image_cache), |window| {
            let child_layout_ids = self
                .children
                .iter_mut()
                .map(|child| child.request_layout(window, cx))
                .collect::<SmallVec<_>>();
            let mut style = Style::default();
            style.refine(&self.style);
            let layout_id = window.request_layout(style, child_layout_ids.iter().copied(), cx);
            (layout_id, child_layout_ids)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        for child in &mut self.children {
            if window.draw_budget_exhausted_for_optional_work() {
                window.degrade_current_draw();
                break;
            }
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let image_cache = self.image_cache_provider.provide(window, cx);
        window.with_image_cache(Some(image_cache), |window| {
            for child in &mut self.children {
                if window.draw_budget_exhausted_for_optional_work() {
                    window.degrade_current_draw();
                    break;
                }
                child.paint(window, cx);
            }
        })
    }
}

/// An image loading task associated with an image cache.
pub type ImageLoadingTask = Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>;

/// An image cache item
pub enum ImageCacheItem {
    /// The associated image is currently loading
    Loading(ImageLoadingTask),
    /// This item has loaded an image.
    Loaded(Result<Arc<RenderImage>, ImageCacheError>),
}

impl std::fmt::Debug for ImageCacheItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = match self {
            ImageCacheItem::Loading(_) => &"Loading...".to_string(),
            ImageCacheItem::Loaded(render_image) => &format!("{:?}", render_image),
        };
        f.debug_struct("ImageCacheItem")
            .field("status", status)
            .finish()
    }
}

impl ImageCacheItem {
    /// Attempt to get the image from the cache item.
    pub fn get(&mut self) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageCacheItem::Loading(task) => {
                let res = task.now_or_never()?;
                *self = ImageCacheItem::Loaded(res.clone());
                Some(res)
            }
            ImageCacheItem::Loaded(res) => Some(res.clone()),
        }
    }
}

/// An object that can handle the caching and unloading of images.
/// Implementations of this trait should ensure that images are removed from all windows when they are no longer needed.
pub trait ImageCache: 'static {
    /// Load an image given a resource
    /// returns the result of loading the image if it has finished loading, or None if it is still loading
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>>;
}

/// An object that can create an ImageCache during the render phase.
/// See the ImageCache trait for more information.
pub trait ImageCacheProvider: 'static {
    /// Called during the request_layout phase to create an ImageCache.
    fn provide(&mut self, _window: &mut Window, _cx: &mut App) -> AnyImageCache;
}

impl<T: ImageCache> ImageCacheProvider for Entity<T> {
    fn provide(&mut self, _window: &mut Window, _cx: &mut App) -> AnyImageCache {
        self.clone().into()
    }
}

/// An implementation of ImageCache, that uses an LRU caching strategy to unload images when the cache is full
pub struct RetainAllImageCache {
    items: FxHashMap<u64, ImageCacheItem>,
    notifier: ImageCacheNotifier,
}

impl fmt::Debug for RetainAllImageCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashMapImageCache")
            .field("num_images", &self.items.len())
            .finish()
    }
}

impl RetainAllImageCache {
    /// Create a new image cache.
    #[inline]
    pub fn new(cx: &mut App) -> Entity<Self> {
        let e = cx.new(|_cx| RetainAllImageCache {
            items: FxHashMap::default(),
            notifier: ImageCacheNotifier::default(),
        });
        cx.observe_release(&e, |image_cache, cx| {
            for (_, mut item) in std::mem::take(&mut image_cache.items) {
                if let Some(Ok(image)) = item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        e
    }

    /// Load an image from the given source.
    ///
    /// Returns `None` if the image is loading.
    pub fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let hash = hash(source);

        if let Some(item) = self.items.get_mut(&hash) {
            return item.get();
        }

        let (task, _) = cx.fetch_asset::<AssetLogger<ImageAssetLoader>>(source);
        self.items
            .insert(hash, ImageCacheItem::Loading(task.clone()));

        let result = self.items.get_mut(&hash).and_then(ImageCacheItem::get);
        if result.is_none() {
            self.notifier
                .schedule(window.current_view(), task, window, cx);
        }

        result
    }

    /// Clear the image cache.
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for (_, mut item) in std::mem::take(&mut self.items) {
            if let Some(Ok(image)) = item.get() {
                cx.drop_image(image, Some(window));
            }
        }
    }

    /// Remove the image from the cache by the given source.
    pub fn remove(&mut self, source: &Resource, window: &mut Window, cx: &mut App) {
        let hash = hash(source);
        if let Some(mut item) = self.items.remove(&hash)
            && let Some(Ok(image)) = item.get()
        {
            cx.drop_image(image, Some(window));
        }
    }

    /// Returns the number of images in the cache.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl ImageCache for RetainAllImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        RetainAllImageCache::load(self, resource, window, cx)
    }
}

/// Memory and count limits for [`BoundedImageCache`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedImageCacheConfig {
    /// Maximum number of loaded or loading cache entries to retain.
    pub max_items: usize,
    /// Maximum estimated decoded image bytes to retain.
    pub max_bytes: usize,
}

impl Default for BoundedImageCacheConfig {
    fn default() -> Self {
        Self {
            max_items: 256,
            max_bytes: 128 * 1024 * 1024,
        }
    }
}

struct BoundedImageCacheEntry {
    item: ImageCacheItem,
    estimated_bytes: usize,
}

/// An LRU image cache that releases decoded images and atlas tiles when limits are exceeded.
pub struct BoundedImageCache {
    cache_id: u64,
    config: BoundedImageCacheConfig,
    entries: LinkedHashMap<u64, BoundedImageCacheEntry>,
    estimated_bytes: usize,
    notifier: ImageCacheNotifier,
}

impl fmt::Debug for BoundedImageCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedImageCache")
            .field("max_items", &self.config.max_items)
            .field("max_bytes", &self.config.max_bytes)
            .field("cache_id", &self.cache_id)
            .field("items", &self.entries.len())
            .field("estimated_bytes", &self.estimated_bytes)
            .finish()
    }
}

impl BoundedImageCache {
    /// Create a new bounded image cache.
    pub fn new(config: BoundedImageCacheConfig, cx: &mut App) -> Entity<Self> {
        static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);
        let cache_id = NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed);
        let cache = cx.new(|_cx| Self {
            cache_id,
            config,
            entries: LinkedHashMap::new(),
            estimated_bytes: 0,
            notifier: ImageCacheNotifier::default(),
        });
        cx.observe_release(&cache, |cache, cx| {
            cache.drop_all(None, cx);
            drop_image_cache_metrics(cache.cache_id);
        })
        .detach();
        cache
    }

    /// Update cache limits and evict anything that no longer fits.
    pub fn set_config(
        &mut self,
        config: BoundedImageCacheConfig,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.config == config {
            return;
        }

        self.config = config;
        self.enforce_limits(None, window, cx);
        self.record_metrics();
    }

    pub(crate) fn set_config_without_window(
        &mut self,
        config: BoundedImageCacheConfig,
        cx: &mut App,
    ) {
        if self.config == config {
            return;
        }

        self.config = config;
        self.enforce_limits_without_window(None, cx);
        self.record_metrics();
    }

    /// Load an image from the given source.
    pub fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let image_hash = hash(source);

        if let Some(entry) = self.entries.get_refresh(&image_hash) {
            let result = {
                let result = entry.item.get();
                if entry.estimated_bytes == 0
                    && let Some(Ok(image)) = result.as_ref()
                {
                    entry.estimated_bytes = estimated_render_image_bytes(image);
                    self.estimated_bytes =
                        self.estimated_bytes.saturating_add(entry.estimated_bytes);
                }
                result
            };
            self.enforce_limits(Some(image_hash), window, cx);
            self.record_metrics();
            return result;
        }

        let (task, _) = cx.fetch_asset::<AssetLogger<ImageAssetLoader>>(source);
        self.entries.insert(
            image_hash,
            BoundedImageCacheEntry {
                item: ImageCacheItem::Loading(task.clone()),
                estimated_bytes: 0,
            },
        );
        let result = self.entries.get_refresh(&image_hash).and_then(|entry| {
            let result = entry.item.get();
            if entry.estimated_bytes == 0
                && let Some(Ok(image)) = result.as_ref()
            {
                entry.estimated_bytes = estimated_render_image_bytes(image);
                self.estimated_bytes = self.estimated_bytes.saturating_add(entry.estimated_bytes);
            }
            result
        });
        self.enforce_limits(Some(image_hash), window, cx);
        self.record_metrics();

        if result.is_none() && self.entries.contains_key(&image_hash) {
            self.notifier
                .schedule(window.current_view(), task, window, cx);
        }

        result
    }

    /// Clear the image cache.
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        self.drop_all(Some(window), cx);
    }

    /// Clear the image cache without a current window.
    pub fn clear_without_window(&mut self, cx: &mut App) {
        self.drop_all(None, cx);
    }

    /// Remove one image from the cache.
    pub fn remove(&mut self, source: &Resource, window: &mut Window, cx: &mut App) {
        let image_hash = hash(source);
        self.remove_hash(image_hash, Some(window), cx);
    }

    pub(crate) fn remove_hash(
        &mut self,
        image_hash: u64,
        mut window: Option<&mut Window>,
        cx: &mut App,
    ) {
        if let Some(entry) = self.entries.remove(&image_hash) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, window, cx);
        }
        self.record_metrics();
    }

    /// Enforce the current cache limits using only application-wide atlas cleanup.
    pub fn enforce_current_limits(&mut self, cx: &mut App) {
        self.enforce_limits_without_window(None, cx);
        self.record_metrics();
    }

    /// Returns the number of entries retained by the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the estimated decoded image bytes retained by the cache.
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn enforce_limits(&mut self, protected_hash: Option<u64>, window: &mut Window, cx: &mut App) {
        while self.entries.len() > self.config.max_items
            || self.estimated_bytes > self.config.max_bytes
        {
            let Some((&candidate_hash, _)) = self.entries.front() else {
                break;
            };
            if Some(candidate_hash) == protected_hash && self.entries.len() > 1 {
                _ = self.entries.get_refresh(&candidate_hash);
                continue;
            }
            let Some(entry) = self.entries.remove(&candidate_hash) else {
                break;
            };
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, Some(window), cx);
        }
    }

    fn enforce_limits_without_window(&mut self, protected_hash: Option<u64>, cx: &mut App) {
        while self.entries.len() > self.config.max_items
            || self.estimated_bytes > self.config.max_bytes
        {
            let Some((&candidate_hash, _)) = self.entries.front() else {
                break;
            };
            if Some(candidate_hash) == protected_hash && self.entries.len() > 1 {
                _ = self.entries.get_refresh(&candidate_hash);
                continue;
            }
            let Some(entry) = self.entries.remove(&candidate_hash) else {
                break;
            };
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            record_image_cache_eviction(1);
            drop_cache_entry(entry, None, cx);
        }
    }

    fn drop_all(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        let entries = std::mem::take(&mut self.entries);
        self.estimated_bytes = 0;
        record_image_cache_eviction(entries.len());
        for (_, entry) in entries.into_iter() {
            let current_window = window.as_deref_mut();
            drop_cache_entry(entry, current_window, cx);
        }
        self.record_metrics();
    }

    fn record_metrics(&self) {
        record_image_cache_metrics(self.cache_id, self.entries.len(), self.estimated_bytes);
    }
}

impl ImageCache for BoundedImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        BoundedImageCache::load(self, resource, window, cx)
    }
}

/// Constructs a bounded image cache that uses element state associated with the given ID.
pub fn bounded(
    id: impl Into<ElementId>,
    config: BoundedImageCacheConfig,
) -> BoundedImageCacheProvider {
    BoundedImageCacheProvider {
        id: id.into(),
        config,
    }
}

/// Provider for inline bounded image caches.
pub struct BoundedImageCacheProvider {
    id: ElementId,
    config: BoundedImageCacheConfig,
}

impl ImageCacheProvider for BoundedImageCacheProvider {
    fn provide(&mut self, window: &mut Window, cx: &mut App) -> AnyImageCache {
        window
            .with_global_id(self.id.clone(), |global_id, window| {
                window.with_element_state::<Entity<BoundedImageCache>, _>(
                    global_id,
                    |cache, _window| {
                        let cache =
                            cache.unwrap_or_else(|| BoundedImageCache::new(self.config, cx));
                        if cache.read(cx).config != self.config {
                            let cache = BoundedImageCache::new(self.config, cx);
                            (cache.clone(), cache)
                        } else {
                            (cache.clone(), cache)
                        }
                    },
                )
            })
            .into()
    }
}

fn drop_cache_entry(
    mut entry: BoundedImageCacheEntry,
    current_window: Option<&mut Window>,
    cx: &mut App,
) {
    if let Some(Ok(image)) = entry.item.get() {
        cx.drop_image(image, current_window);
    }
}

fn estimated_render_image_bytes(image: &RenderImage) -> usize {
    image.decoded_byte_len()
}

#[derive(Clone, Default)]
struct ImageCacheNotifier {
    state: Rc<RefCell<ImageCacheNotifierState>>,
}

#[derive(Default)]
struct ImageCacheNotifierState {
    entities: Vec<EntityId>,
    callback_pending: bool,
}

impl ImageCacheNotifier {
    fn schedule(
        &self,
        entity: EntityId,
        task: ImageLoadingTask,
        window: &mut Window,
        cx: &mut App,
    ) {
        window
            .spawn(cx, {
                let state = self.state.clone();
                async move |cx| {
                    if let Err(error) = task.await {
                        log::debug!("image cache load failed: {error}");
                    }

                    let should_schedule = {
                        let mut state = state.borrow_mut();
                        if !state.entities.contains(&entity) {
                            state.entities.push(entity);
                        }
                        if state.callback_pending {
                            false
                        } else {
                            state.callback_pending = true;
                            true
                        }
                    };

                    if should_schedule {
                        cx.on_next_frame(move |_, cx| {
                            let entities = {
                                let mut state = state.borrow_mut();
                                state.callback_pending = false;
                                std::mem::take(&mut state.entities)
                            };
                            for entity in entities {
                                cx.notify(entity);
                            }
                        });
                    }
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Context, Render, RequestFrameOptions, TestAppContext, performance_metrics_snapshot,
    };
    use image::{Frame, RgbaImage};
    use smallvec::smallvec;

    #[derive(Default)]
    struct ImageCacheNotifyTestView;

    impl Render for ImageCacheNotifyTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            crate::div()
        }
    }

    #[gpui::test]
    fn image_cache_completion_notifications_are_coalesced(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| ImageCacheNotifyTestView);
        let notifier = ImageCacheNotifier::default();
        let image = Arc::new(RenderImage::new(smallvec![Frame::new(RgbaImage::new(
            1, 1
        ))]));
        let task = Task::ready(Ok(image)).shared();
        let (test_window, baseline) = cx.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();
            notifier.schedule(view.entity_id(), task.clone(), window, _cx);
            notifier.schedule(view.entity_id(), task, window, _cx);
            (test_window, baseline)
        });

        cx.run_until_parked();
        assert_eq!(test_window.requested_frame_count(), baseline + 1);

        test_window.simulate_request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
            request_id: 0,
        });
        cx.run_until_parked();
        assert!(notifier.state.borrow().entities.is_empty());
        assert!(!notifier.state.borrow().callback_pending);
    }

    #[gpui::test]
    fn bounded_cache_eviction_drops_loaded_images(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let before = performance_metrics_snapshot();
            let image = Arc::new(RenderImage::new(smallvec![Frame::new(RgbaImage::new(
                1, 1
            ))]));
            let image_hash = 1;
            let protected_hash = 2;
            let mut cache = BoundedImageCache {
                cache_id: 999_999,
                config: BoundedImageCacheConfig {
                    max_items: 1,
                    max_bytes: usize::MAX,
                },
                entries: LinkedHashMap::default(),
                estimated_bytes: 4,
                notifier: ImageCacheNotifier::default(),
            };
            cache.entries.insert(
                image_hash,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Ok(image)),
                    estimated_bytes: 4,
                },
            );
            cache.entries.insert(
                protected_hash,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("protected".into()))),
                    estimated_bytes: 0,
                },
            );

            cache.enforce_limits(Some(protected_hash), window, cx);

            let after = performance_metrics_snapshot();
            assert!(!cache.entries.contains_key(&image_hash));
            assert!(cache.entries.contains_key(&protected_hash));
            assert!(after.image_cache_evictions > before.image_cache_evictions);
            assert!(after.image_drop_count > before.image_drop_count);
        });
    }

    #[gpui::test]
    fn bounded_cache_refreshes_most_recent_entries(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let mut cache = BoundedImageCache {
                cache_id: 999_998,
                config: BoundedImageCacheConfig {
                    max_items: 2,
                    max_bytes: usize::MAX,
                },
                entries: LinkedHashMap::default(),
                estimated_bytes: 0,
                notifier: ImageCacheNotifier::default(),
            };

            cache.entries.insert(
                1,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("one".into()))),
                    estimated_bytes: 1,
                },
            );
            cache.entries.insert(
                2,
                BoundedImageCacheEntry {
                    item: ImageCacheItem::Loaded(Err(ImageCacheError::Asset("two".into()))),
                    estimated_bytes: 1,
                },
            );
            _ = cache.entries.get_refresh(&1);
            cache.enforce_limits(None, window, cx);

            assert_eq!(cache.entries.front().map(|(hash, _)| *hash), Some(2));
            assert_eq!(cache.entries.back().map(|(hash, _)| *hash), Some(1));
        });
    }
}

/// Constructs a retain-all image cache that uses the element state associated with the given ID.
pub fn retain_all(id: impl Into<ElementId>) -> RetainAllImageCacheProvider {
    RetainAllImageCacheProvider { id: id.into() }
}

/// A provider struct for creating a retain-all image cache inline
pub struct RetainAllImageCacheProvider {
    id: ElementId,
}

impl ImageCacheProvider for RetainAllImageCacheProvider {
    fn provide(&mut self, window: &mut Window, cx: &mut App) -> AnyImageCache {
        window
            .with_global_id(self.id.clone(), |global_id, window| {
                window.with_element_state::<Entity<RetainAllImageCache>, _>(
                    global_id,
                    |cache, _window| {
                        let mut cache = cache.unwrap_or_else(|| RetainAllImageCache::new(cx));
                        (cache.clone(), cache)
                    },
                )
            })
            .into()
    }
}
