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
