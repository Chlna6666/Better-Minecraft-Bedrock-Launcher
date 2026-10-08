use super::*;

#[cfg(all(
    feature = "nova-gfx-opengl",
    any(target_os = "windows", target_os = "linux")
))]
pub(super) fn resize_opengl_swapchain(
    device: &mut OpenGlDevice,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<()> {
    match device.resize_swapchain(swapchain, config.size.width(), config.size.height()) {
        Ok(()) => Ok(()),
        Err(resize_error) => {
            log::warn!(
                "OpenGL swapchain resize failed; keeping previous swapchain: {resize_error:#}"
            );
            Err(resize_error.into())
        }
    }
}
#[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
pub(super) fn resize_dx11_swapchain(
    device: &mut Dx11Device,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<()> {
    match device.resize_swapchain(swapchain, config.size.width(), config.size.height()) {
        Ok(()) => Ok(()),
        Err(resize_error) => {
            log::warn!(
                "DX11 swapchain resize failed; keeping previous swapchain: {resize_error:#}"
            );
            Err(resize_error.into())
        }
    }
}
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) fn resize_dx12_swapchain(
    device: &mut Dx12Device,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<()> {
    match device.resize_swapchain(swapchain, config.size.width(), config.size.height()) {
        Ok(()) => Ok(()),
        Err(resize_error) => {
            log::warn!(
                "DX12 swapchain resize failed; keeping previous swapchain: {resize_error:#}"
            );
            Err(resize_error.into())
        }
    }
}

#[cfg(all(
    feature = "nova-gfx-opengl",
    any(target_os = "windows", target_os = "linux")
))]
pub(super) fn recreate_opengl_swapchain_for_config(
    device: &mut OpenGlDevice,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<SwapchainId> {
    match device.recreate_swapchain(swapchain, config) {
        Ok(next_swapchain) => Ok(next_swapchain),
        Err(recreate_error) => {
            log::warn!(
                "OpenGL swapchain recreate failed; keeping previous swapchain: {recreate_error:#}"
            );
            Err(recreate_error.into())
        }
    }
}
#[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
pub(super) fn recreate_dx11_swapchain_for_config(
    device: &mut Dx11Device,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<SwapchainId> {
    match device.recreate_swapchain(swapchain, config) {
        Ok(next_swapchain) => Ok(next_swapchain),
        Err(recreate_error) => {
            log::warn!(
                "DX11 swapchain recreate failed; keeping previous swapchain: {recreate_error:#}"
            );
            Err(recreate_error.into())
        }
    }
}
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) fn recreate_dx12_swapchain_for_config(
    device: &mut Dx12Device,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<SwapchainId> {
    match device.recreate_swapchain(swapchain, config) {
        Ok(next_swapchain) => Ok(next_swapchain),
        Err(recreate_error) => {
            log::warn!(
                "DX12 swapchain recreate failed; keeping previous swapchain: {recreate_error:#}"
            );
            Err(recreate_error.into())
        }
    }
}

#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) fn resize_vulkan_swapchain(
    device: &mut VulkanDevice,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<SwapchainId> {
    match device.resize_swapchain(swapchain, config.size.width(), config.size.height()) {
        Ok(()) => Ok(swapchain),
        Err(resize_error) => {
            log::warn!(
                "Vulkan swapchain resize failed; keeping previous swapchain: {resize_error:#}"
            );
            Err(resize_error.into())
        }
    }
}
