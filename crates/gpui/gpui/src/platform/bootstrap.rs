use crate::RendererBackend;
#[cfg(target_os = "windows")]
use std::path::Path;

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

    #[cfg(all(target_os = "windows", feature = "nova-gfx-dx11"))]
    {
        if _resolved_backend == RendererBackend::NovaDx11 {
            match gfx_dx11::enumerate_adapter_info() {
                Ok(adapters) => {
                    return adapters
                        .into_iter()
                        .map(gpu_adapter_info_from_nova_dx11)
                        .collect();
                }
                Err(error) => {
                    log::warn!("failed to enumerate nova-dx11 adapters: {error}");
                }
            }
        }
    }
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

#[cfg(all(target_os = "windows", feature = "nova-gfx-dx11"))]
fn gpu_adapter_info_from_nova_dx11(info: gfx_core::AdapterInfo) -> crate::GpuAdapterInfo {
    crate::GpuAdapterInfo {
        name: info.name,
        backend: RendererBackend::NovaDx11,
        device_type: crate::GpuAdapterDeviceType::Other,
        vendor: info.vendor_id,
        device: info.device_id,
        driver: "nova-dx11".to_string(),
        driver_info: format!("{:?}", info.capabilities),
    }
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

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub use super::test::TestDispatcher;
