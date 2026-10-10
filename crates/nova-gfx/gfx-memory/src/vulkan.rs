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

/// Requirements for an existing-block relocation reservation. Recreate the native resource
/// with identical requirements before binding this allocation; mapped addresses may change.
pub struct VulkanRelocationDescriptor<'a> {
    /// Allocation diagnostic name.
    pub name: &'a str,
    /// Requirements queried from the replacement VkBuffer or VkImage.
    pub requirements: vk::MemoryRequirements,
    /// Original resource memory location; relocation keeps the source memory type.
    pub location: MemoryLocation,
    /// Buffer/linear-image granularity class; false for optimal-tiled images.
    pub linear: bool,
}

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
            // Allocate only on demand. Small workloads start with small blocks;
            // each memory type grows geometrically when its existing blocks fill.
            allocation_sizes: AllocationSizes::new(4 * 1024 * 1024, 1024 * 1024)
                .with_max_device_memblock_size(256 * 1024 * 1024)
                .with_max_host_memblock_size(64 * 1024 * 1024)
                .with_max_readback_memblock_size(64 * 1024 * 1024),
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
        _buffer: vk::Buffer,
    ) -> Result<MemoryAllocation> {
        let allocation = self
            .allocator
            .allocate(&VulkanAllocationCreateDesc {
                name,
                requirements,
                location: memory_location_to_allocator(location),
                linear: true,
                allocation_scheme: VulkanAllocationScheme::GpuAllocatorManaged,
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
        _image: vk::Image,
    ) -> Result<MemoryAllocation> {
        let allocation = self
            .allocator
            .allocate(&VulkanAllocationCreateDesc {
                name,
                requirements,
                location: memory_location_to_allocator(location),
                linear: false,
                allocation_scheme: VulkanAllocationScheme::GpuAllocatorManaged,
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

    pub(crate) fn trim(&mut self) {
        self.allocator.trim();
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
    /// Selects one sparse block with at most 8 MiB / 128 allocations and an existing destination.
    /// Returns None when relocation cannot release a block within that work budget.
    pub fn vulkan_relocation_candidate(&self) -> Option<vk::DeviceMemory> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Vulkan(allocator) => allocator
                .allocator
                .relocation_candidate(8 * 1024 * 1024, 128),
            _ => None,
        }
    }

    /// Reserves space outside the source block without allocating new native backing memory.
    /// The reservation is temporary until the caller copies contents and replaces the resource.
    ///
    /// Returns None when existing holes cannot satisfy the requirements. Callers must
    /// cancel their pass and free its temporary reservations; ordinary allocation fallback would
    /// defeat the peak-memory guarantee.
    /// # Errors
    /// Returns InvalidInput for another backend, or a backend error for invalid requirements.
    pub fn allocate_vulkan_relocation(
        &mut self,
        source: vk::DeviceMemory,
        desc: VulkanRelocationDescriptor<'_>,
    ) -> Result<Option<MemoryAllocation>> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Vulkan(allocator) => {
                let reservation = allocator.allocator.allocate_in_existing_blocks(
                    &VulkanAllocationCreateDesc {
                        name: desc.name,
                        requirements: desc.requirements,
                        location: memory_location_to_allocator(desc.location),
                        linear: desc.linear,
                        allocation_scheme: VulkanAllocationScheme::GpuAllocatorManaged,
                    },
                    source,
                );
                let allocation = match reservation {
                    Ok(allocation) => allocation,
                    Err(gpu_allocator::AllocationError::OutOfMemory) => return Ok(None),
                    Err(error) => return Err(MemoryError::from(error).into()),
                };
                track_allocation(&mut allocator.stats, allocation.size());
                Ok(Some(MemoryAllocation::Vulkan(allocation)))
            }
            _ => Err(gfx_core::Error::InvalidInput(
                "relocation requires a Vulkan allocator".into(),
            )),
        }
    }
    /// Creates a Vulkan memory allocator.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] when allocator creation fails.
    pub fn new_vulkan(desc: VulkanMemoryAllocatorDesc) -> Result<Self> {
        Ok(Self::Vulkan(VulkanMemoryAllocator::new(desc)?))
    }

    /// Allocates Vulkan memory for a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] when validation or allocation fails.
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
            Self::Dx12(_) => Err(gfx_core::Error::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
            #[cfg(feature = "metal")]
            Self::Metal(_) => Err(gfx_core::Error::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
        }
    }

    /// Allocates Vulkan memory for an image.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::Error`] when validation or allocation fails.
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
            Self::Dx12(_) => Err(gfx_core::Error::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
            #[cfg(feature = "metal")]
            Self::Metal(_) => Err(gfx_core::Error::InvalidInput(
                "allocator is not a Vulkan allocator".to_string(),
            )),
        }
    }
}
