use crate::{App, SharedString, SharedUri, Task};
use futures::{
    Future, FutureExt, TryFutureExt,
    future::{AbortHandle, Aborted, Shared},
};

use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    Loading,
    Ready(T),
}

struct AssetLoadOwner {
    abort: AbortHandle,
}

impl Drop for AssetLoadOwner {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// An explicit ownership lease for an asynchronous asset load.
///
/// The application cache and every returned lease are owners of the underlying load. Dropping the
/// final owner cancels pending work. Completed values live in an explicit ready state rather than
/// in the executor task, and a lease can outlive cache eviction without exposing that task.
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

impl<T> AssetLease<T>
where
    T: Clone + Send + 'static,
{
    pub(crate) fn spawn(
        future: impl Future<Output = T> + Send + 'static,
        cx: &App,
    ) -> Self {
        let state = Arc::new(parking_lot::Mutex::new(AssetLeaseState::Loading));
        let weak_state = Arc::downgrade(&state);
        let (abort, registration) = AbortHandle::new_pair();
        let completion = cx
            .background_executor()
            .spawn(Abortable::new(
                async move {
                    let output = future.await;
                    if let Some(state) = weak_state.upgrade() {
                        *state.lock() = AssetLeaseState::Ready(output);
                    }
                },
                registration,
            ))
            .shared();

        Self {
            state,
            completion,
            owner: Arc::new(AssetLoadOwner { abort }),
        }
    }

    pub(crate) fn completion_signal(&self) -> Shared<Task<Result<(), Aborted>>> {
        self.completion.clone()
    }

    /// Returns the completed value without waiting, or None while the load is still pending.
    pub fn get(&self) -> Option<T> {
        match &*self.state.lock() {
            AssetLeaseState::Loading => None,
            AssetLeaseState::Ready(value) => Some(value.clone()),
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

/// Use a quick, non-cryptographically secure hash function to get an identifier from data
pub fn hash<T: Hash>(data: &T) -> u64 {
    let mut hasher = collections::FxHasher::default();
    data.hash(&mut hasher);
    hasher.finish()
}
