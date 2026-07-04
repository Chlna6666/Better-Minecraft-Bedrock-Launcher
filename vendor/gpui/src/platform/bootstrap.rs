use crate::{BackgroundExecutor, RendererBackend, RendererOptions};
use std::{path::Path, rc::Rc};

#[cfg(target_os = "macos")]
use super::MacPlatform;
use super::platform_traits::Platform;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use super::{HeadlessClient, WaylandClient, X11Client};
#[cfg(target_os = "windows")]
use super::{WindowsPlatform, show_error};

/// Returns the Windows application manifest GPUI uses for DPI awareness.
#[cfg(target_os = "windows")]
pub fn windows_manifest_path() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/windows/gpui.manifest.xml"
    ))
}

/// Returns GPU adapters visible to GPUI for the requested renderer backend.
#[cfg(target_os = "windows")]
pub fn enumerate_gpu_adapters(backend: RendererBackend) -> Vec<crate::GpuAdapterInfo> {
    let _resolved_backend = match backend {
        RendererBackend::Auto => RendererBackend::platform_default(),
        backend => backend,
    };

    #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
    {
        if _resolved_backend == RendererBackend::NovaDx12 {
            match gfx_dx12::enumerate_adapter_info() {
                Ok(adapters) => {
                    return adapters
                        .into_iter()
                        .map(gpu_adapter_info_from_nova_dx12)
                        .collect();
                }
                Err(error) => {
                    log::warn!("failed to enumerate nova-dx12 adapters: {error}");
                }
            }
        }
    }

    #[cfg(all(target_os = "windows", feature = "nova-gfx-vulkan"))]
    {
        if _resolved_backend == RendererBackend::NovaVulkan {
            match gfx_vulkan::enumerate_adapter_info() {
                Ok(adapters) => {
                    return adapters
                        .into_iter()
                        .map(gpu_adapter_info_from_nova_vulkan)
                        .collect();
                }
                Err(error) => {
                    log::warn!("failed to enumerate nova-vulkan adapters: {error}");
                }
            }
        }
    }

    let _ = backend;
    Vec::new()
}

/// Returns GPU adapters visible to GPUI for the requested renderer backend.
#[cfg(all(
    any(target_os = "linux", target_os = "freebsd"),
    any(feature = "x11", feature = "wayland")
))]
pub fn enumerate_gpu_adapters(backend: RendererBackend) -> Vec<crate::GpuAdapterInfo> {
    #[cfg(feature = "nova-gfx-vulkan")]
    {
        let _resolved_backend = match backend {
            RendererBackend::Auto => RendererBackend::platform_default(),
            backend => backend,
        };

        if _resolved_backend == RendererBackend::NovaVulkan {
            match gfx_vulkan::enumerate_adapter_info() {
                Ok(adapters) => {
                    return adapters
                        .into_iter()
                        .map(gpu_adapter_info_from_nova_vulkan)
                        .collect();
                }
                Err(error) => {
                    log::warn!("failed to enumerate nova-vulkan adapters: {error}");
                }
            }
        }
    }

    let _ = backend;
    Vec::new()
}

#[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
fn gpu_adapter_info_from_nova_dx12(info: gfx_core::AdapterInfo) -> crate::GpuAdapterInfo {
    crate::GpuAdapterInfo {
        name: info.name,
        backend: RendererBackend::NovaDx12,
        device_type: crate::GpuAdapterDeviceType::Other,
        vendor: info.vendor_id,
        device: info.device_id,
        driver: "nova-dx12".to_string(),
        driver_info: format!("{:?}", info.capabilities),
    }
}

#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(
        target_os = "windows",
        all(
            any(target_os = "linux", target_os = "freebsd"),
            any(feature = "x11", feature = "wayland")
        )
    )
))]
fn gpu_adapter_info_from_nova_vulkan(info: gfx_core::AdapterInfo) -> crate::GpuAdapterInfo {
    crate::GpuAdapterInfo {
        name: info.name,
        backend: RendererBackend::NovaVulkan,
        device_type: crate::GpuAdapterDeviceType::Other,
        vendor: info.vendor_id,
        device: info.device_id,
        driver: "nova-vulkan".to_string(),
        driver_info: format!("{:?}", info.capabilities),
    }
}

/// Returns GPU adapters visible to GPUI for the requested renderer backend.
#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        any(feature = "x11", feature = "wayland")
    ),
    target_os = "windows",
)))]
pub fn enumerate_gpu_adapters(_backend: RendererBackend) -> Vec<crate::GpuAdapterInfo> {
    Vec::new()
}

#[cfg(any(test, feature = "test-support"))]
pub use super::test::{TestDispatcher, TestScreenCaptureSource, TestScreenCaptureStream};

/// Returns a background executor for the current platform.
pub fn background_executor() -> BackgroundExecutor {
    current_platform(true, RendererOptions::default()).background_executor()
}

#[cfg(target_os = "macos")]
pub(crate) fn current_platform(
    headless: bool,
    _renderer_options: RendererOptions,
) -> Rc<dyn Platform> {
    Rc::new(MacPlatform::new(headless))
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn current_platform(
    headless: bool,
    renderer_options: RendererOptions,
) -> Rc<dyn Platform> {
    #[cfg(feature = "x11")]
    use anyhow::Context as _;

    if headless {
        return Rc::new(HeadlessClient::new());
    }

    match guess_compositor() {
        #[cfg(feature = "wayland")]
        "Wayland" => Rc::new(WaylandClient::new(renderer_options)),

        #[cfg(feature = "x11")]
        "X11" => Rc::new(
            X11Client::new(renderer_options)
                .context("Failed to initialize X11 client.")
                .unwrap(),
        ),

        "Headless" => Rc::new(HeadlessClient::new()),
        _ => unreachable!(),
    }
}

/// Return which compositor we're guessing we'll use.
/// Does not attempt to connect to the given compositor
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[inline]
pub fn guess_compositor() -> &'static str {
    if std::env::var_os("ZED_HEADLESS").is_some() {
        return "Headless";
    }

    #[cfg(feature = "wayland")]
    let wayland_display = std::env::var_os("WAYLAND_DISPLAY");
    #[cfg(not(feature = "wayland"))]
    let wayland_display: Option<std::ffi::OsString> = None;

    #[cfg(feature = "x11")]
    let x11_display = std::env::var_os("DISPLAY");
    #[cfg(not(feature = "x11"))]
    let x11_display: Option<std::ffi::OsString> = None;

    let use_wayland = wayland_display.is_some_and(|display| !display.is_empty());
    let use_x11 = x11_display.is_some_and(|display| !display.is_empty());

    if use_wayland {
        "Wayland"
    } else if use_x11 {
        "X11"
    } else {
        "Headless"
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn current_platform(
    headless: bool,
    renderer_options: RendererOptions,
) -> Rc<dyn Platform> {
    if headless {
        return Rc::new(WindowsPlatform::new_headless());
    }

    Rc::new(
        WindowsPlatform::new(renderer_options)
            .inspect_err(|err| show_error("Failed to launch", err.to_string()))
            .unwrap(),
    )
}
