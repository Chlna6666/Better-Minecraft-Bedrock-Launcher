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
