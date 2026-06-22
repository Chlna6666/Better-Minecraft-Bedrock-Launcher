use gfx_core::Result;
use gpu_allocator::{
    AllocationSizes, AllocatorDebugSettings,
    d3d12::{
        AllocationCreateDesc as Dx12AllocationCreateDesc, Allocator as Dx12Allocator,
        AllocatorCreateDesc as Dx12AllocatorCreateDesc, ID3D12DeviceVersion,
    },
};
use windows::Win32::Graphics::Direct3D12::ID3D12Device;

use crate::{
    allocator::{MemoryAllocation, MemoryAllocator},
    common::{MemoryError, MemoryStats, track_allocation, untrack_allocation},
};

/// Direct3D 12 allocation backed by `gpu-allocator`.
pub type Dx12Allocation = gpu_allocator::d3d12::Allocation;

/// Direct3D 12 memory allocator creation descriptor.
#[derive(Clone)]
pub struct Dx12MemoryAllocatorDesc {
    /// D3D12 logical device.
    pub device: ID3D12Device,
}

#[doc(hidden)]
pub struct Dx12MemoryAllocator {
    allocator: Box<Dx12Allocator>,
    stats: MemoryStats,
}

impl Dx12MemoryAllocator {
    fn new(desc: Dx12MemoryAllocatorDesc) -> Result<Self> {
        let allocator = Dx12Allocator::new(&Dx12AllocatorCreateDesc {
            device: ID3D12DeviceVersion::Device(desc.device),
            debug_settings: AllocatorDebugSettings::default(),
            allocation_sizes: AllocationSizes::default(),
        })
        .map_err(MemoryError::from)?;
        Ok(Self {
            allocator: Box::new(allocator),
            stats: MemoryStats::default(),
        })
    }

    fn allocate_resource(
        &mut self,
        desc: &Dx12AllocationCreateDesc<'_>,
    ) -> Result<MemoryAllocation> {
        let allocation = self.allocator.allocate(desc).map_err(MemoryError::from)?;
        track_allocation(&mut self.stats, allocation.size());
        Ok(MemoryAllocation::Dx12(allocation))
    }

    pub(crate) fn free(&mut self, allocation: Dx12Allocation) -> Result<()> {
        let size = allocation.size();
        self.allocator.free(allocation).map_err(MemoryError::from)?;
        untrack_allocation(&mut self.stats, size);
        Ok(())
    }

    pub(crate) const fn stats(&self) -> MemoryStats {
        self.stats
    }

    pub(crate) fn detailed_report(&self) -> MemoryStats {
        let report = self.allocator.generate_detailed_report();
        let reports = [report.gpu_only, report.cpu_to_gpu, report.gpu_to_cpu];
        reports
            .into_iter()
            .fold(MemoryStats::default(), |mut stats, report| {
                stats.allocated_bytes = stats
                    .allocated_bytes
                    .saturating_add(report.allocated_bytes)
                    .saturating_add(report.committed_allocated_bytes);
                stats.reserved_bytes = stats.reserved_bytes.saturating_add(report.reserved_bytes);
                stats.allocation_count = stats
                    .allocation_count
                    .saturating_add(report.committed_allocation_count);
                stats.block_count = stats.block_count.saturating_add(report.block_count);
                stats
            })
    }
}

impl MemoryAllocator {
    /// Creates a Direct3D 12 memory allocator.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when allocator creation fails.
    pub fn new_dx12(desc: Dx12MemoryAllocatorDesc) -> Result<Self> {
        Ok(Self::Dx12(Dx12MemoryAllocator::new(desc)?))
    }

    /// Allocates Direct3D 12 memory using a resource allocation descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`gfx_core::GfxError`] when validation or allocation fails.
    pub fn allocate_dx12_resource(
        &mut self,
        desc: &Dx12AllocationCreateDesc<'_>,
    ) -> Result<MemoryAllocation> {
        match self {
            Self::Dx12(allocator) => allocator.allocate_resource(desc),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Direct3D 12 allocator".to_string(),
            )),
            #[cfg(feature = "metal")]
            Self::Metal(_) => Err(gfx_core::GfxError::InvalidInput(
                "allocator is not a Direct3D 12 allocator".to_string(),
            )),
        }
    }
}
