use super::*;

pub(super) enum NovaBackend {
    #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
    Dx12(Dx12Device),
    #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
    Metal(MetalDevice),
    #[cfg(all(
        feature = "nova-gfx-vulkan",
        any(target_os = "windows", target_os = "linux", target_os = "freebsd")
    ))]
    Vulkan(VulkanDevice),
    #[cfg(not(any(
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )))]
    Unavailable,
}

impl NovaBackend {
    pub(super) fn adapter_name(&self) -> &str {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => device.adapter_name(),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => "nova-gfx Metal",
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.adapter_name(),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => "nova-gfx unavailable",
        }
    }

    pub(super) fn supports_partial_presentation(&self, swapchain: SwapchainId) -> bool {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => {
                // DXGI Present1 dirty rectangles look attractive for small GPUI damage, but under
                // high-frequency titlebar/tab springs DWM moves preservation work onto the GPU Copy
                // engine. That makes the no-blur path substantially more expensive than a full
                // present. gpui-ce's Windows renderer uses ordinary Present as well. Keep GPUI's
                // retained scene/damage generation, but do not hand dirty rectangles to DXGI.
                let _ = (device, swapchain);
                false
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.supports_partial_presentation(swapchain),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => {
                #[cfg(target_os = "windows")]
                {
                    // VK_KHR_incremental_present reaches the same Windows compositor. On animation
                    // bursts it exhibits the same copy-engine amplification as DXGI dirty rects, so
                    // Windows Nova keeps one presentation policy across DX12 and Vulkan.
                    let _ = (device, swapchain);
                    false
                }
                #[cfg(not(target_os = "windows"))]
                {
                    device.supports_partial_presentation(swapchain)
                }
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => false,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(_) => "nova-dx12",
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => "nova-metal",
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(_) => "nova-vulkan",
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => "nova-unavailable",
        }
    }

    pub(super) fn async_capabilities(&self) -> BackendAsyncCapabilities {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => device.async_capabilities(),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.async_capabilities(),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.async_capabilities(),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => BackendAsyncCapabilities::default(),
        }
    }

    pub(super) fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.poll_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.poll_submission(submission)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.poll_submission(submission)?),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(SubmissionStatus::Complete),
        }
    }

    pub(super) fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.wait_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.wait_submission(submission)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.wait_submission(submission)?),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(()),
        }
    }

    pub(super) fn trim_memory(&mut self, level: GfxMemoryTrimLevel) -> Result<()> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.trim_memory(level)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.trim_memory(level)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.trim_memory(level)?),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(()),
        }
    }
}
