use super::*;
use crate::ExtensionDevice;
use gfx_core::SurfaceDevice as _;

pub(super) enum NovaBackend {
    #[cfg(all(
        feature = "nova-gfx-opengl",
        any(target_os = "windows", target_os = "linux")
    ))]
    OpenGl(OpenGlDevice),
    #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
    Dx11(Dx11Device),
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
        all(
            feature = "nova-gfx-opengl",
            any(target_os = "windows", target_os = "linux")
        ),
        all(feature = "nova-gfx-dx11", target_os = "windows"),
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )))]
    Unavailable,
}

/// Backend facts a renderer keeps once it no longer owns the device.
///
/// A renderer reports these in logs and GPU specs long after creation, so they are captured
/// up front instead of being re-derived from a device it may no longer hold.
#[derive(Clone, Debug)]
pub(super) struct NovaBackendInfo {
    label: &'static str,
    adapter_name: String,
    kind: Option<gfx_core::BackendKind>,
}

impl NovaBackendInfo {
    /// Returns the backend label used in logs, such as `nova-dx12`.
    pub(super) fn label(&self) -> &'static str {
        self.label
    }

    /// Returns the adapter name the device reported when it was created.
    pub(super) fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Returns whether this backend renders through Vulkan.
    pub(super) fn is_vulkan(&self) -> bool {
        self.kind == Some(gfx_core::BackendKind::Vulkan)
    }

    /// Returns the `gfx-core` backend kind.
    ///
    /// # Errors
    ///
    /// Returns an error when this build has no usable nova-gfx backend.
    pub(super) fn kind(&self) -> Result<gfx_core::BackendKind> {
        self.kind
            .ok_or_else(|| anyhow::anyhow!("nova-gfx backend is unavailable"))
    }
}

impl NovaBackend {
    /// Captures the metadata a renderer needs without borrowing the device again.
    pub(super) fn info(&self) -> NovaBackendInfo {
        NovaBackendInfo {
            label: self.label(),
            adapter_name: self.adapter_name().to_string(),
            kind: self.extension_backend_kind().ok(),
        }
    }

    pub(super) fn extension_backend_kind(&self) -> Result<gfx_core::BackendKind> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(_) => Ok(gfx_core::BackendKind::OpenGl),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(_) => Ok(gfx_core::BackendKind::Dx11),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(_) => Ok(gfx_core::BackendKind::Dx12),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => Ok(gfx_core::BackendKind::Metal),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(_) => Ok(gfx_core::BackendKind::Vulkan),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => anyhow::bail!("nova-gfx backend is unavailable"),
        }
    }

    pub(super) fn with_extension_device<T>(
        &mut self,
        callback: impl FnOnce(&mut dyn ExtensionDevice) -> Result<T>,
    ) -> Result<T> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => callback(device),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => callback(device),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => callback(device),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => callback(device),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => callback(device),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => anyhow::bail!("nova-gfx backend is unavailable"),
        }
    }

    pub(super) fn adapter_name(&self) -> &str {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => gfx_core::Backend::adapter_name(device),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => gfx_core::Backend::adapter_name(device),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => gfx_core::Backend::adapter_name(device),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => "nova-gfx Metal",
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => gfx_core::Backend::adapter_name(device),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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

    /// Returns whether the current swapchain can accept a presentation without blocking GPUI.
    pub(super) fn can_present_without_wait(&mut self, swapchain: SwapchainId) -> Result<bool> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(_) => Ok(true),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(gfx_core::PresentationDevice::can_present_now(
                device, swapchain,
            )?),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(gfx_core::PresentationDevice::can_present_now(
                device, swapchain,
            )?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => Ok(true),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(_) => Ok(true),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(true),
        }
    }

    pub(super) fn request_frame_ready(
        &mut self,
        _swapchain: SwapchainId,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(device.request_frame_ready(_swapchain, callback)?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(device.request_frame_ready(_swapchain, callback)?),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.request_frame_ready(_swapchain, callback)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.request_frame_ready(_swapchain, callback)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.request_frame_ready(_swapchain, callback)?),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => {
                drop(callback);
                Ok(false)
            }
        }
    }

    pub(super) fn surface_capabilities(
        &self,
        swapchain: SwapchainId,
    ) -> gfx_core::SurfaceCapabilities {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => device.surface_capabilities(swapchain),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => device.surface_capabilities(swapchain),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => device.surface_capabilities(swapchain),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.surface_capabilities(swapchain),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.surface_capabilities(swapchain),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => gfx_core::SurfaceCapabilities::default(),
        }
    }

    pub(super) fn set_swapchain_source_extent(
        &mut self,
        swapchain: SwapchainId,
        extent: gfx_core::Extent2d,
    ) -> Result<()> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(device.set_swapchain_source_extent(swapchain, extent)?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(device.set_swapchain_source_extent(swapchain, extent)?),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.set_swapchain_source_extent(swapchain, extent)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.set_swapchain_source_extent(swapchain, extent)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.set_swapchain_source_extent(swapchain, extent)?),
            #[allow(unreachable_patterns)]
            _ => {
                let _ = (swapchain, extent);
                Ok(())
            }
        }
    }

    /// Presents the retained scene target over a native client area that grew ahead
    /// of the matching Scene. Backends without a retained-frame path return
    /// `Unavailable`, which the caller treats as "the compositor already covers it".
    pub(super) fn present_retained_frame(
        &mut self,
        swapchain: SwapchainId,
        target: gfx_core::Extent2d,
    ) -> Result<()> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(gfx_core::PresentationDevice::present_retained_frame(
                device, swapchain, target,
            )?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(gfx_core::PresentationDevice::present_retained_frame(
                device, swapchain, target,
            )?),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(gfx_core::PresentationDevice::present_retained_frame(
                device, swapchain, target,
            )?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(gfx_core::PresentationDevice::present_retained_frame(
                device, swapchain, target,
            )?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(gfx_core::PresentationDevice::present_retained_frame(
                device, swapchain, target,
            )?),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => {
                let _ = (swapchain, target);
                Err(gfx_core::Error::Unavailable(
                    "retained-frame presentation is not supported by this backend".into(),
                )
                .into())
            }
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(_) => "nova-opengl",
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(_) => "nova-dx11",
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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

    pub(super) fn async_capabilities(&self) -> AsyncCapabilities {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => device.async_capabilities(),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => device.async_capabilities(),
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => AsyncCapabilities::default(),
        }
    }

    pub(super) fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(device.poll_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(device.poll_submission(submission)?),
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(device.wait_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(device.wait_submission(submission)?),
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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

    /// Returns whether size-dependent renderer resources cannot be replaced without waiting.
    ///
    /// DXGI preparation follows this window's rendering/Present and stays fixed while other
    /// windows submit work. The renderer has already retired its own attachment readers.
    /// Vulkan maintenance1 follows this chain's WSI present fences as well; render fences
    /// alone cannot prove presentation-engine release of its images/semaphores.
    pub(super) fn has_pending_resize_work(&mut self, swapchain: SwapchainId) -> Result<bool> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            // The renderer has already retired its own post-SwapBuffers GL syncs.
            // Resize replaces only its FBO/color attachments; unrelated windows'
            // submissions cannot keep this window waiting for the whole device.
            Self::OpenGl(_) => Ok(false),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(!device.prepare_swapchain_resize(swapchain)?),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(!device.prepare_swapchain_resize(swapchain)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => Ok(false),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(!device.prepare_swapchain_resize(swapchain)?),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => {
                let _ = swapchain;
                Ok(false)
            }
        }
    }

    pub(super) fn trim_memory(&mut self, level: MemoryTrimLevel) -> Result<()> {
        match self {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => Ok(device.trim_memory(level)?),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => Ok(device.trim_memory(level)?),
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
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
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
