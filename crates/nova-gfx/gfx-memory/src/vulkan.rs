use ash::vk;
use gfx_core::{MemoryLocation, Result};
use gpu_allocator::{
    AllocationSizes, AllocatorDebugSettings,
    vulkan::{
        AllocationCreateDesc as VulkanAllocationCreateDesc,
        AllocationScheme as VulkanAllocationScheme, Allocator as VulkanAllocator,
        AllocatorCreateDesc as VulkanAllocatorCreateDesc,
    },
};

use crate::{
    allocator::{MemoryAllocation, MemoryAllocator},
    common::{
        MemoryError, MemoryStats, memory_location_to_allocator, track_allocation,
        untrack_allocation,
    },
};

/// Vulkan allocation backed by `gpu-allocator`.
pub type VulkanAllocation = gpu_allocator::vulkan::Allocation;

/// Vulkan memory allocator creation descriptor.
#[derive(Clone)]
pub struct VulkanMemoryAllocatorDesc {
    /// Vulkan instance.
    pub instance: ash::Instance,
    /// Vulkan device.
    pub device: ash::Device,
    /// Vulkan physical device.
    pub physical_device: vk::PhysicalDevice,
}

#[doc(hidden)]
pub struct VulkanMemoryAllocator {
    allocator: Box<VulkanAllocator>,
    stats: MemoryStats,
}

impl VulkanMemoryAllocator {
    fn new(desc: VulkanMemoryAllocatorDesc) -> Result<Self> {
        let allocator = VulkanAllocator::new(&VulkanAllocatorCreateDesc {
            instance: desc.instance,
            device: desc.device,
            physical_device: desc.physical_device,
            debug_settings: AllocatorDebugSettings::default(),
            buffer_device_address: false,
            allocation_sizes: AllocationSizes::default(),
        })
        .map_err(MemoryError::from)?;
        Ok(Self {
            allocator: Box::new(allocator),
            stats: MemoryStats::default(),
        })
    }

    fn allocate_buffer(
        &mut self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        buffer: vk::Buffer,
    ) -> Result<MemoryAllocation> {
        let allocation = self
            .allocator
            .allocate(&VulkanAllocationCreateDesc {
                name,
                requirements,
                location: memory_location_to_allocator(location),
                linear: true,
                allocation_scheme: VulkanAllocationScheme::DedicatedBuffer(buffer),
            })
            .map_err(MemoryError::from)?;
        track_allocation(&mut self.stats, allocation.size());
        Ok(MemoryAllocation::Vulkan(allocation))
    }

    fn allocate_image(
        &mut self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        image: vk::Image,
    ) -> Result<MemoryAllocation> {
        let allocation = self
            .allocator
            .allocate(&VulkanAllocationCreateDesc {
                name,
                requirements,
                location: memory_location_to_allocator(location),
                linear: false,
                allocation_scheme: VulkanAllocationScheme::DedicatedImage(image),
            })
            .map_err(MemoryError::from)?;
        track_allocation(&mut self.stats, allocation.size());
        Ok(MemoryAllocation::Vulkan(allocation))
    }

    pub(crate) fn free(&mut self, allocation: VulkanAllocation) -> Result<()> {
        let size = allocation.size();
        self.allocator.free(allocation).map_err(MemoryError::from)?;
        untrack_allocation(&mut self.stats, size);
        Ok(())
    }

    pub(crate) const fn stats(&self) -> MemoryStats {
        self.stats
    }

    pub(crate) fn detailed_report(&self) -> MemoryStats {
        let report = self.allocator.generate_report();
        MemoryStats {
            allocated_bytes: report.total_allocated_bytes,
            reserved_bytes: report.total_capacity_bytes,
            allocation_count: report.allocations.len(),
            block_count: report.blocks.len(),
            ..MemoryStats::default()
        }
    }
}

impl MemoryAllocator {
    /// Creates a Vulkan memory allocator.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when allocator creation fails.
    pub fn new_vulkan(desc: VulkanMemoryAllocatorDesc) -> Result<Self> {
        Ok(Self::Vulkan(VulkanMemoryAllocator::new(desc)?))
    }

    /// Allocates Vulkan memory for a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when validation or allocation fails.
    pub fn allocate_vulkan_buffer(
        &mut self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        buffer: vk::Buffer,
    ) -> Result<MemoryAllocation> {
        match self {
            Self::Vulkan(allocator) => {
                allocator.allocate_buffer(name, requirements, location, buffer)
            }
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
            #[cfg(feature = "metal")]
            Self::Metal(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
        }
    }

    /// Allocates Vulkan memory for an image.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when validation or allocation fails.
    pub fn allocate_vulkan_image(
        &mut self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        image: vk::Image,
    ) -> Result<MemoryAllocation> {
        match self {
            Self::Vulkan(allocator) => {
                allocator.allocate_image(name, requirements, location, image)
            }
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
            #[cfg(feature = "metal")]
            Self::Metal(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
        }
    }
}
