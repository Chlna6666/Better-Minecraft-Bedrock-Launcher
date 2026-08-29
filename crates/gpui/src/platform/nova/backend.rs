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
            Self::Dx12(device) => device.supports_partial_presentation(swapchain),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.supports_partial_presentation(swapchain),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.supports_partial_presentation(swapchain),
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

    /// Stretches composited swapchain content over a pending window resize.
    pub(super) fn set_swapchain_content_stretch(
        &mut self,
        swapchain: SwapchainId,
        scale: Option<[f32; 2]>,
    ) -> Result<()> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.set_swapchain_content_stretch(swapchain, scale)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => Ok(()),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(_) => Ok(()),
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

    #[cfg(target_os = "windows")]
    pub(super) fn has_pending_resize_work(&mut self) -> Result<bool> {
        match self {
            #[cfg(feature = "nova-gfx-dx12")]
            Self::Dx12(device) => Ok(device.has_pending_gpu_work()?),
            #[cfg(feature = "nova-gfx-vulkan")]
            Self::Vulkan(device) => Ok(device.has_pending_gpu_work()?),
            #[cfg(not(any(feature = "nova-gfx-dx12", feature = "nova-gfx-vulkan")))]
            Self::Unavailable => Ok(false),
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
