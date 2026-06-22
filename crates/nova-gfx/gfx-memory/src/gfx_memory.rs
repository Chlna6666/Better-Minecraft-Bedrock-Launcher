//! GPU memory allocation wrappers for nova-gfx.
//!
//! `gfx-memory` owns allocation objects and exposes backend-neutral accounting.
//! It does not own buffers, images, or other graphics resources.
//!
//! Backends are compiled only when their matching Cargo feature is enabled:
//! `vulkan`, `dx12`, or `metal`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

#![cfg_attr(
    feature = "vulkan",
    expect(
        unsafe_code,
        reason = "backend allocation handles require narrow unsafe accessors with documented invariants"
    )
)]
#![cfg_attr(not(feature = "vulkan"), warn(unsafe_code))]

mod allocator;
mod common;
mod deferred;
mod upload;

#[cfg(feature = "dx12")]
mod dx12;
#[cfg(feature = "metal")]
mod metal;
#[cfg(feature = "vulkan")]
mod vulkan;

pub use allocator::{MemoryAllocation, MemoryAllocator};
pub use common::{MemoryAllocationDesc, MemoryError, MemoryStats, memory_location_to_allocator};
pub use deferred::{DeferredFree, DeferredFreeQueue};
pub use upload::{UploadAllocation, UploadRingAllocator, UploadRingAllocatorDesc, UploadStats};

#[cfg(feature = "dx12")]
pub use dx12::Dx12MemoryAllocatorDesc;
#[cfg(feature = "metal")]
pub use metal::MetalMemoryAllocatorDesc;
#[cfg(feature = "vulkan")]
pub use vulkan::VulkanMemoryAllocatorDesc;
