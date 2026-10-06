//! Sharing of Nova backend devices between windows in one process.

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// Identifies the backend device a window should render with.
///
/// Windows that agree on this key share one device; a different adapter or power preference
/// gets its own.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct DeviceKey {
    /// Backend the device belongs to.
    pub(super) backend: RendererBackend,
    /// Exact adapter name requested by the application, if any.
    pub(super) adapter_name: Option<String>,
    /// Adapter power preference used when the name does not pin one.
    pub(super) power_preference: PowerPreference,
    /// Pipeline-cache root; cache ownership is part of the shared-device identity.
    pub(super) pipeline_cache_dir: Option<PathBuf>,
}

/// A backend device shared by every window created on the same thread.
///
/// This is the ownership shape professional graphics libraries use: a cheap reference-counted
/// handle whose operations synchronize internally, instead of a device each window owns. A
/// second window that resolves to the same [`DeviceKey`] on this thread reuses the adapter, the
/// descriptor heaps, the upload ring, and the compiled pipelines the first window already paid
/// for.
///
/// The registry is thread local because [`NovaBackend`] is not `Send`: the DX12 device holds
/// `HANDLE`, `IUnknown`, and mapped-upload `NonNull` raw pointers. Windows initializes a
/// renderer on a [`crate::BackgroundExecutor`] worker and moves it to the UI thread through a
/// channel, so two windows can be initialized on different workers; those windows do not share
/// a device. Making the registry process wide requires a device that is `Send`, which means an
/// `unsafe` assertion of the kind `InitializedWindowsRenderer` already carries.
///
/// The lock is held for one backend operation at a time; a caller must not hold the guard
/// across another backend use.
pub(super) type SharedBackend = Arc<Mutex<NovaBackend>>;

thread_local! {
    /// Devices created on this thread, keyed by [`DeviceKey`].
    static DEVICES: RefCell<HashMap<DeviceKey, SharedBackend>> = RefCell::new(HashMap::new());
}

/// Locks a shared backend, recovering from a poisoned lock.
///
/// A panic while a backend operation held the lock does not make the device unusable, so the
/// guard is recovered rather than propagated as a second panic.
pub(super) fn lock_backend(backend: &SharedBackend) -> MutexGuard<'_, NovaBackend> {
    backend.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Runs `create` once per [`DeviceKey`] on this thread and returns the shared handle.
///
/// # Errors
///
/// Returns the creation error when no device exists for `key` on this thread yet and creating
/// one fails.
pub(super) fn shared_backend(
    key: DeviceKey,
    create: impl FnOnce() -> Result<NovaBackend>,
) -> Result<SharedBackend> {
    DEVICES.with(|devices| {
        let mut devices = devices.borrow_mut();
        if let Some(existing) = devices.get(&key) {
            log::info!(
                "reusing shared nova device: backend={:?} thread={:?}",
                key.backend,
                std::thread::current().id()
            );
            return Ok(Arc::clone(existing));
        }

        let backend = create()?;
        log::info!(
            "created shared nova device: backend={:?} thread={:?}",
            key.backend,
            std::thread::current().id()
        );
        let shared = Arc::new(Mutex::new(backend));
        devices.insert(key, Arc::clone(&shared));
        Ok(shared)
    })
}
