//! Vulkan backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Vulkan. Native
//! raw-window-handle integration is kept in this backend crate, not in
//! `gfx-core`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

mod device;
mod error;
mod registry;

pub use device::{
    BaselineMetrics, VulkanDevice, VulkanSurfaceTarget, VulkanTriangle, VulkanTriangleConfig,
    enumerate_adapter_info,
};
pub use error::VulkanError;

impl VulkanDevice {
    /// Reports whether a swapchain resize must be deferred until currently tracked GPU work
    /// retires. The renderer passes the concrete swapchain so this API can be narrowed to
    /// swapchain-owned fences without changing callers when the backend adopts present fences.
    pub fn has_pending_swapchain_work(
        &self,
        _swapchain: gfx_core::SwapchainId,
    ) -> gfx_core::Result<bool> {
        self.has_pending_gpu_work()
    }
}
