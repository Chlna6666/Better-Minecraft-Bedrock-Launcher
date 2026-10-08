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
    /// Native GL display connection; other APIs select adapters without one.
    pub(super) native_display: Option<::winit::raw_window_handle::RawDisplayHandle>,
}

/// A backend device shared by every window created on the same thread.
///
/// A second window that resolves to the same [`DeviceKey`] on this thread reuses the adapter, the
/// descriptor heaps, the upload ring, and the compiled pipelines the first window already paid
/// for.
///
/// The registry is thread local because [`NovaBackend`] is not `Send`: the DX12 device holds
/// `HANDLE`, `IUnknown`, and mapped-upload `NonNull` raw pointers. Windows and Linux/FreeBSD
/// create and retain all Nova renderers on the shared GPU owner, so matching windows use this
/// same registry without transferring a backend device across threads. Its entries live until
/// that owner exits; window resources are released independently at window destruction.
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

/// Creates or reuses the device selected by the renderer, without requiring a native surface.
///
/// Startup preparation and window initialization must call this on the GPU owner with the same
/// options so the thread-local registry retains one device for both operations.
///
/// # Errors
///
/// Returns device initialization errors or an error for an unsupported backend.
pub(super) fn shared_device(
    backend: RendererBackend,
    options: &RendererOptions,
) -> Result<SharedBackend> {
    let key = DeviceKey {
        backend,
        adapter_name: options.adapter_name.clone(),
        power_preference: nova_power_preference(options),
        pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        native_display: None,
    };
    shared_backend(key, || {
        let application_name = match backend {
            RendererBackend::NovaOpenGl => "gpui nova opengl",
            RendererBackend::NovaDx11 => "gpui nova dx11",
            RendererBackend::NovaDx12 => "gpui nova dx12",
            RendererBackend::NovaMetal => "gpui nova metal",
            RendererBackend::NovaVulkan => "gpui nova vulkan",
            _ => anyhow::bail!("{backend} is not a concrete nova-gfx backend"),
        };
        let descriptor = DeviceDescriptor {
            application_name: application_name.to_string(),
            adapter_name: options.adapter_name.clone(),
            power_preference: nova_power_preference(options),
            pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        };
        create_backend(backend, &descriptor)
    })
}

fn create_backend(backend: RendererBackend, descriptor: &DeviceDescriptor) -> Result<NovaBackend> {
    match backend {
        #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
        RendererBackend::NovaDx11 => Ok(NovaBackend::Dx11(
            Dx11Device::new(descriptor).context("creating nova DX11 device")?,
        )),
        #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
        RendererBackend::NovaDx12 => Ok(NovaBackend::Dx12(
            Dx12Device::new(descriptor).context("creating nova DX12 device")?,
        )),
        #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
        RendererBackend::NovaMetal => Ok(NovaBackend::Metal(
            MetalDevice::new(descriptor).context("creating nova Metal device")?,
        )),
        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        RendererBackend::NovaVulkan => Ok(NovaBackend::Vulkan(
            VulkanDevice::new(descriptor).context("creating nova Vulkan device")?,
        )),
        _ => anyhow::bail!("{backend} is not an available nova-gfx backend"),
    }
}

/// Releases a failed initial candidate after its renderer locals have dropped.
/// Active windows keep their shared device; failed cold starts retain no cache IDs.
pub(super) fn discard_unused_device(
    backend: RendererBackend,
    options: &RendererOptions,
    native_display: Option<::winit::raw_window_handle::RawDisplayHandle>,
) {
    let key = DeviceKey {
        backend,
        adapter_name: options.adapter_name.clone(),
        power_preference: nova_power_preference(options),
        pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        native_display: if backend == RendererBackend::NovaOpenGl {
            native_display
        } else {
            None
        },
    };
    DEVICES.with(|devices| {
        let mut devices = devices.borrow_mut();
        if devices
            .get(&key)
            .is_some_and(|device| Arc::strong_count(device) == 1)
        {
            resources::forget_device(&key);
            devices.remove(&key);
        }
    });
}

#[cfg(all(
    feature = "nova-gfx-opengl",
    any(target_os = "windows", target_os = "linux")
))]
#[expect(
    unsafe_code,
    reason = "GL initialization borrows native handles retained by the platform owner"
)]
pub(super) fn shared_opengl_device(
    options: &RendererOptions,
    display: ::winit::raw_window_handle::RawDisplayHandle,
    window: ::winit::raw_window_handle::RawWindowHandle,
) -> Result<SharedBackend> {
    let key = DeviceKey {
        backend: RendererBackend::NovaOpenGl,
        adapter_name: options.adapter_name.clone(),
        power_preference: nova_power_preference(options),
        pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        native_display: Some(display),
    };
    shared_backend(key, || {
        let descriptor = DeviceDescriptor {
            application_name: "gpui nova opengl".into(),
            adapter_name: options.adapter_name.clone(),
            power_preference: nova_power_preference(options),
            pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        };
        // SAFETY: the GPU owner is torn down before the native platform connection;
        // initialization holds the window alive and WGL owns its bootstrap HWND.
        Ok(NovaBackend::OpenGl(
            unsafe { OpenGlDevice::new(&descriptor, display, window) }
                .context("creating native OpenGL 4.5 context")?,
        ))
    })
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
