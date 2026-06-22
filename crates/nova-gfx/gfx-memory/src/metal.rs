use gfx_core::{MemoryLocation, Result};
use gpu_allocator::{
    AllocationSizes, AllocatorDebugSettings,
    metal::{
        AllocationCreateDesc as MetalAllocationCreateDesc, Allocator as MetalAllocator,
        AllocatorCreateDesc as MetalAllocatorCreateDesc,
    },
};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{MTLDevice, MTLTextureDescriptor};

use crate::{
    allocator::{MemoryAllocation, MemoryAllocator},
    common::{
        MemoryError, MemoryStats, memory_location_to_allocator, track_allocation,
        untrack_allocation,
    },
};

/// Metal allocation backed by `gpu-allocator`.
pub type MetalAllocation = gpu_allocator::metal::Allocation;

/// Metal memory allocator creation descriptor.
#[derive(Clone)]
pub struct MetalMemoryAllocatorDesc {
    /// Metal logical device.
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
}

#[doc(hidden)]
pub struct MetalMemoryAllocator {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    allocator: Box<MetalAllocator>,
    stats: MemoryStats,
}

impl MetalMemoryAllocator {
    fn new(desc: MetalMemoryAllocatorDesc) -> Result<Self> {
        let allocator = MetalAllocator::new(&MetalAllocatorCreateDesc {
            device: desc.device.clone(),
            debug_settings: AllocatorDebugSettings::default(),
            allocation_sizes: AllocationSizes::default(),
            create_residency_set: false,
        })
        .map_err(MemoryError::from)?;
        Ok(Self {
            device: desc.device,
            allocator: Box::new(allocator),
            stats: MemoryStats::default(),
        })
    }

    fn allocate_buffer(
        &mut self,
        name: &str,
        length: u64,
        location: MemoryLocation,
    ) -> Result<MemoryAllocation> {
        let desc = MetalAllocationCreateDesc::buffer(
            self.device.as_ref(),
            name,
            length,
            memory_location_to_allocator(location),
        );
        let allocation = self.allocator.allocate(&desc).map_err(MemoryError::from)?;
        track_allocation(&mut self.stats, allocation.size());
        Ok(MemoryAllocation::Metal(allocation))
    }

    fn allocate_texture(
        &mut self,
        name: &str,
        desc: &MTLTextureDescriptor,
    ) -> Result<MemoryAllocation> {
        let desc = MetalAllocationCreateDesc::texture(self.device.as_ref(), name, desc);
        let allocation = self.allocator.allocate(&desc).map_err(MemoryError::from)?;
        track_allocation(&mut self.stats, allocation.size());
        Ok(MemoryAllocation::Metal(allocation))
    }

    pub(crate) fn free(&mut self, allocation: MetalAllocation) -> Result<()> {
        let size = allocation.size();
        self.allocator
            .free(&allocation)
            .map_err(MemoryError::from)?;
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
    /// Creates a Metal memory allocator.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when allocator creation fails.
    pub fn new_metal(desc: MetalMemoryAllocatorDesc) -> Result<Self> {
        Ok(Self::Metal(MetalMemoryAllocator::new(desc)?))
    }

    /// Allocates Metal heap memory for a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when validation or allocation fails.
    pub fn allocate_metal_buffer(
        &mut self,
        name: &str,
        length: u64,
        location: MemoryLocation,
    ) -> Result<MemoryAllocation> {
        match self {
            Self::Metal(allocator) => allocator.allocate_buffer(name, length, location),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Metal allocator".to_string(),
            )),
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Metal allocator".to_string(),
            )),
        }
    }

    /// Allocates Metal heap memory for a texture.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when validation or allocation fails.
    pub fn allocate_metal_texture(
        &mut self,
        name: &str,
        desc: &MTLTextureDescriptor,
    ) -> Result<MemoryAllocation> {
        match self {
            Self::Metal(allocator) => allocator.allocate_texture(name, desc),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Metal allocator".to_string(),
            )),
            #[cfg(feature = "dx12")]
            Self::Dx12(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Metal allocator".to_string(),
            )),
        }
    }
}
