use crate::{AnyWindowHandle, App, EntityId, SharedString, SharedUri, Task, WindowId};
use futures::{
    Future, FutureExt, TryFutureExt,
    future::{AbortHandle, Aborted, Shared},
};

use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use collections::{FxHashMap, FxHashSet};

/// An enum representing
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum AssetLocation {
    /// This resource is at a given URI
    Uri(SharedUri),
    /// This resource is at a given path in the file system
    Path(Arc<Path>),
    /// This resource is embedded in the application binary
    Embedded(SharedString),
}

impl From<SharedUri> for AssetLocation {
    fn from(value: SharedUri) -> Self {
        Self::Uri(value)
    }
}

impl From<PathBuf> for AssetLocation {
    fn from(value: PathBuf) -> Self {
        Self::Path(value.into())
    }
}

impl From<Arc<Path>> for AssetLocation {
    fn from(value: Arc<Path>) -> Self {
        Self::Path(value)
    }
}

/// Controls how the application cache retains a completed asset load.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AssetRetentionPolicy {
    /// Keep the completed load in the application cache until it is explicitly removed or trimmed.
    #[default]
    Persistent,
    /// Release the application cache ownership after the load completes.
    ///
    /// Explicit AssetLease values can keep the completed result alive independently.
    TransientAfterReady,
    /// Keep the cache entry while live element leases still own the resource.
    ElementOwned,
}

enum AssetLeaseState<T> {
    Loading {
        observers: FxHashMap<WindowId, AssetLeaseWindowObservers>,
    },
    Ready {
        value: T,
        observers: FxHashMap<WindowId, AssetLeaseWindowObservers>,
    },
}

struct AssetLeaseWindowObservers {
    window: AnyWindowHandle,
    views: FxHashSet<EntityId>,
}

struct AssetLoadOwner {
    abort: AbortHandle,
    pins: AtomicUsize,
}

impl Drop for AssetLoadOwner {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub(crate) struct AssetCompletion {
    completion: Shared<Task<Result<(), Aborted>>>,
}

impl AssetCompletion {
    pub(crate) async fn wait(self) -> bool {
        self.completion.await.is_ok()
    }
}

/// An explicit ownership lease for an asynchronous asset load.
///
/// The application cache and every returned lease are owners of the underlying load. Dropping the
/// final owner cancels pending work. Completed values live in an explicit ready state rather than
/// in the executor task. Views that use a pending lease are deduplicated per window and receive an
/// exact retained-subtree invalidation when the value becomes ready.
pub struct AssetLease<T>
where
    T: Clone + Send + 'static,
{
    state: Arc<parking_lot::Mutex<AssetLeaseState<T>>>,
    completion: Shared<Task<Result<(), Aborted>>>,
    owner: Arc<AssetLoadOwner>,
}

impl<T> Clone for AssetLease<T>
where
    T: Clone + Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            completion: self.completion.clone(),
            owner: self.owner.clone(),
        }
    }
}

/// A non-clone owning pin that keeps an element-owned cache entry registered.
///
/// A pin also owns the underlying load lease, so dropping the last pin can cancel pending work
/// after the cache releases its own lease.
pub struct AssetPin<T>
where
    T: Clone + Send + 'static,
{
    lease: AssetLease<T>,
}

impl<T> Drop for AssetPin<T>
where
    T: Clone + Send + 'static,
{
    fn drop(&mut self) {
        let previous = self.lease.owner.pins.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0, "asset pin count underflow");
    }
}

impl<T> AssetPin<T>
where
    T: Clone + Send + 'static,
{
    pub(crate) fn use_by(&self, window: AnyWindowHandle, view: EntityId) -> Option<T> {
        self.lease.use_by(window, view)
    }

    pub(crate) fn pin_count(&self) -> usize {
        self.lease.pin_count()
    }

    pub(crate) fn shares_load(&self, lease: &AssetLease<T>) -> bool {
        self.lease.shares_load(lease)
    }
}

impl<T> AssetLease<T>
where
    T: Clone + Send + 'static,
{
    pub(crate) fn spawn(
        future: impl Future<Output = T> + Send + 'static,
        cx: &App,
    ) -> Self {
        let state = Arc::new(parking_lot::Mutex::new(AssetLeaseState::Loading {
            observers: FxHashMap::default(),
        }));
        let weak_state = Arc::downgrade(&state);
        let (abort, registration) = AbortHandle::new_pair();
        let completion = cx
            .background_executor()
            .spawn(Abortable::new(
                async move {
                    let output = future.await;
                    if let Some(state) = weak_state.upgrade() {
                        let mut state = state.lock();
                        let observers = match &mut *state {
                            AssetLeaseState::Loading { observers } => std::mem::take(observers),
                            AssetLeaseState::Ready { .. } => return,
                        };
                        *state = AssetLeaseState::Ready {
                            value: output,
                            observers,
                        };
                    }
                },
                registration,
            ))
            .shared();

        let weak_state = Arc::downgrade(&state);
        let ready = completion.clone();
        cx.spawn(async move |cx| {
            if ready.await.is_err() {
                return;
            }
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let windows = {
                let mut state = state.lock();
                match &mut *state {
                    AssetLeaseState::Ready { observers, .. } => std::mem::take(observers),
                    AssetLeaseState::Loading { .. } => return,
                }
            };
            let _ = cx.update(move |cx| {
                for observer in windows.into_values() {
                    let views = observer.views;
                    let _ = observer.window.update(cx, move |_, window, _| {
                        window.schedule_asset_ready_views(views);
                    });
                }
            });
        })
        .detach();

        Self {
            state,
            completion,
            owner: Arc::new(AssetLoadOwner {
                abort,
                pins: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn ready(value: T) -> Self {
        let state = Arc::new(parking_lot::Mutex::new(AssetLeaseState::Ready {
            value,
            observers: FxHashMap::default(),
        }));
        let (abort, _registration) = AbortHandle::new_pair();
        Self {
            state,
            completion: Task::ready(Ok(())).shared(),
            owner: Arc::new(AssetLoadOwner {
                abort,
                pins: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn completion_signal(&self) -> AssetCompletion {
        AssetCompletion {
            completion: self.completion.clone(),
        }
    }

    pub(crate) fn pin(&self) -> AssetPin<T> {
        self.owner.pins.fetch_add(1, Ordering::Relaxed);
        AssetPin {
            lease: self.clone(),
        }
    }

    pub(crate) fn pin_count(&self) -> usize {
        self.owner.pins.load(Ordering::Relaxed)
    }

    pub(crate) fn shares_load(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
    }

    pub(crate) fn use_by(&self, window: AnyWindowHandle, view: EntityId) -> Option<T> {
        let mut state = self.state.lock();
        match &mut *state {
            AssetLeaseState::Loading { observers } => {
                observers
                    .entry(window.window_id())
                    .or_insert_with(|| AssetLeaseWindowObservers {
                        window,
                        views: FxHashSet::default(),
                    })
                    .views
                    .insert(view);
                None
            }
            AssetLeaseState::Ready { value, .. } => Some(value.clone()),
        }
    }

    /// Returns the completed value without waiting, or None while the load is still pending.
    pub fn get(&self) -> Option<T> {
        match &*self.state.lock() {
            AssetLeaseState::Loading { .. } => None,
            AssetLeaseState::Ready { value, .. } => Some(value.clone()),
        }
    }

    /// Waits for the asset value while retaining ownership of the load.
    pub async fn wait(&self) -> T {
        let _owner = self.owner.clone();
        self.completion
            .clone()
            .await
            .expect("asset load cannot be cancelled while an AssetLease is alive");
        self.get()
            .expect("asset completion signal must publish the ready value first")
    }
}

/// A trait for asynchronous asset loading.
pub trait Asset: 'static {
    /// The source of the asset.
    type Source: Clone + Hash + Send;

    /// The loaded asset
    type Output: Clone + Send;

    /// Defines how the application cache owns this asset after it becomes ready.
    const RETENTION: AssetRetentionPolicy = AssetRetentionPolicy::Persistent;

    /// Load the asset asynchronously
    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static;
}

/// An asset Loader which logs the [`Err`] variant of a [`Result`] during loading
pub enum AssetLogger<T> {
    #[doc(hidden)]
    _Phantom(PhantomData<T>, &'static dyn crate::seal::Sealed),
}

impl<T, R, E> Asset for AssetLogger<T>
where
    T: Asset<Output = Result<R, E>>,
    R: Clone + Send,
    E: Clone + Send + std::fmt::Display,
{
    type Source = T::Source;

    type Output = T::Output;

    const RETENTION: AssetRetentionPolicy = T::RETENTION;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let load = T::load(source, cx);
        load.inspect_err(|e| log::error!("Failed to load asset: {}", e))
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::TestAppContext;
    use futures::channel::oneshot;

    #[gpui::test]
    fn asset_lease_transitions_to_ready(cx: &mut TestAppContext) {
        let lease = cx.update(|cx| AssetLease::spawn(async { 42usize }, cx));
        assert_eq!(lease.get(), None);
        cx.run_until_parked();
        assert_eq!(lease.get(), Some(42));
    }

    #[gpui::test]
    fn dropping_last_asset_lease_cancels_pending_load(cx: &mut TestAppContext) {
        let (sender, receiver) = oneshot::channel::<usize>();
        let lease = cx.update(|cx| {
            AssetLease::spawn(
                async move { receiver.await.expect("sender is kept alive until cancellation") },
                cx,
            )
        });
        cx.run_until_parked();

        drop(lease);
        cx.run_until_parked();

        assert!(
            sender.send(7).is_err(),
            "dropping the final lease must cancel and drop the pending future"
        );
    }
}

/// Use a quick, non-cryptographically secure hash function to get an identifier from data
pub fn hash<T: Hash>(data: &T) -> u64 {
    let mut hasher = collections::FxHasher::default();
    data.hash(&mut hasher);
    hasher.finish()
}
