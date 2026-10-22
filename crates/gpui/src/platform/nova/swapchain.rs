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
    match gfx_core::SurfaceDevice::resize_swapchain(device, swapchain, config.size) {
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
    match gfx_core::SurfaceDevice::resize_swapchain(device, swapchain, config.size) {
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
    match gfx_core::SurfaceDevice::resize_swapchain(device, swapchain, config.size) {
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
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) fn resize_vulkan_swapchain(
    device: &mut VulkanDevice,
    swapchain: SwapchainId,
    config: SurfaceConfig,
) -> Result<SwapchainId> {
    match gfx_core::SurfaceDevice::resize_swapchain(device, swapchain, config.size) {
        Ok(()) => Ok(swapchain),
        Err(resize_error) => {
            log::warn!(
                "Vulkan swapchain resize failed; keeping previous swapchain: {resize_error:#}"
            );
            Err(resize_error.into())
        }
    }
}
