use std::ptr::NonNull;

use gfx_core::{GfxError, Result};

use crate::common::MemoryStats;

/// GPU memory allocation.
pub enum MemoryAllocation {
    /// Vulkan allocation backed by `gpu-allocator`.
    #[cfg(feature = "vulkan")]
    Vulkan(crate::vulkan::VulkanAllocation),
    /// Direct3D 12 allocation backed by `gpu-allocator`.
    #[cfg(feature = "dx12")]
    Dx12(crate::dx12::Dx12Allocation),
    /// Metal allocation backed by `gpu-allocator`.
    #[cfg(feature = "metal")]
    Metal(crate::metal::MetalAllocation),
}

impl MemoryAllocation {
    /// Returns allocation size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(allocation) => allocation.size(),
            #[cfg(feature = "dx12")]
            Self::Dx12(allocation) => allocation.size(),
            #[cfg(feature = "metal")]
            Self::Metal(allocation) => allocation.size(),
            #[cfg(not(any(feature = "vulkan", feature = "dx12", feature = "metal")))]
            _ => unreachable!("MemoryAllocation has no variants without backend features"),
        }
    }

    /// Returns a mapped pointer for CPU-visible memory when the backend exposes one.
    #[must_use]
    pub fn mapped_ptr(&self) -> Option<NonNull<core::ffi::c_void>> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(allocation) => allocation.mapped_ptr(),
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => None,
            #[cfg(feature = "metal")]
            Self::Metal(_) => None,
            #[cfg(not(any(feature = "vulkan", feature = "dx12", feature = "metal")))]
            _ => None,
        }
    }

    /// Returns a mutable mapped byte slice for CPU-visible memory.
    #[must_use]
    pub fn mapped_slice_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(allocation) => allocation.mapped_slice_mut(),
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => None,
            #[cfg(feature = "metal")]
            Self::Metal(_) => None,
            #[cfg(not(any(feature = "vulkan", feature = "dx12", feature = "metal")))]
            _ => None,
        }
    }

    /// Returns Vulkan device memory and offset for binding.
    ///
    /// # Safety
    ///
    /// The returned memory handle and offset may only be bound to the resource whose
    /// memory requirements produced this allocation, and the allocation must outlive
    /// the bound Vulkan resource.
    #[cfg(feature = "vulkan")]
    #[must_use]
    pub unsafe fn vulkan_memory(&self) -> Option<(ash::vk::DeviceMemory, u64)> {
        match self {
            Self::Vulkan(allocation) => {
                // SAFETY: The caller upholds the binding and lifetime requirements.
                let memory = unsafe { allocation.memory() };
                Some((memory, allocation.offset()))
            }
            #[cfg(any(feature = "dx12", feature = "metal"))]
            _ => None,
        }
    }
}

/// GPU memory allocator.
pub enum MemoryAllocator {
    /// Vulkan allocator backed by `gpu-allocator`.
    #[cfg(feature = "vulkan")]
    Vulkan(crate::vulkan::VulkanMemoryAllocator),
    /// Direct3D 12 allocator backed by `gpu-allocator`.
    #[cfg(feature = "dx12")]
    Dx12(crate::dx12::Dx12MemoryAllocator),
    /// Metal allocator backed by `gpu-allocator`.
    #[cfg(feature = "metal")]
    Metal(crate::metal::MetalMemoryAllocator),
}

impl MemoryAllocator {
    /// Frees a memory allocation.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the allocator rejects the free operation.
    pub fn free(&mut self, allocation: MemoryAllocation) -> Result<()> {
        match (self, allocation) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(allocator), MemoryAllocation::Vulkan(allocation)) => {
                allocator.free(allocation)
            }
            #[cfg(feature = "dx12")]
            (Self::Dx12(allocator), MemoryAllocation::Dx12(allocation)) => {
                allocator.free(allocation)
            }
            #[cfg(feature = "metal")]
            (Self::Metal(allocator), MemoryAllocation::Metal(allocation)) => {
                allocator.free(allocation)
            }
            #[allow(unreachable_patterns)]
            _ => Err(GfxError::InvalidInput(
                "allocation does not belong to allocator backend".to_string(),
            )),
        }
    }

    /// Returns current allocator statistics.
    #[must_use]
    pub fn stats(&self) -> MemoryStats {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(allocator) => allocator.stats(),
            #[cfg(feature = "dx12")]
            Self::Dx12(allocator) => allocator.stats(),
            #[cfg(feature = "metal")]
            Self::Metal(allocator) => allocator.stats(),
            #[cfg(not(any(feature = "vulkan", feature = "dx12", feature = "metal")))]
            _ => MemoryStats::default(),
        }
    }

    /// Returns a detailed backend allocator report.
    #[must_use]
    pub fn detailed_report(&self) -> MemoryStats {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(allocator) => allocator.detailed_report(),
            #[cfg(feature = "dx12")]
            Self::Dx12(allocator) => allocator.detailed_report(),
            #[cfg(feature = "metal")]
            Self::Metal(allocator) => allocator.detailed_report(),
            #[cfg(not(any(feature = "vulkan", feature = "dx12", feature = "metal")))]
            _ => MemoryStats::default(),
        }
    }
}
