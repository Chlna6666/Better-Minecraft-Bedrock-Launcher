use gfx_core::{GfxError, MemoryLocation, Result};
use thiserror::Error;

/// Memory-specific error.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Allocation descriptor was invalid.
    #[error("invalid memory input: {0}")]
    InvalidInput(String),
    /// Backend allocator failed.
    #[error("gpu allocator error: {0}")]
    Allocator(#[from] gpu_allocator::AllocationError),
}

impl From<MemoryError> for GfxError {
    fn from(error: MemoryError) -> Self {
        match error {
            MemoryError::InvalidInput(message) => Self::InvalidInput(message),
            MemoryError::Allocator(error) => Self::Backend(error.to_string()),
        }
    }
}

/// Backend-neutral allocation descriptor used by tests and future backends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryAllocationDesc {
    /// Debug allocation name.
    pub name: String,
    /// Allocation size in bytes.
    pub size: u64,
    /// Required byte alignment.
    pub alignment: u64,
    /// Desired memory placement.
    pub location: MemoryLocation,
}

impl MemoryAllocationDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when size is zero or alignment is invalid.
    pub fn validate(&self) -> Result<()> {
        if self.size == 0 {
            return Err(MemoryError::InvalidInput(
                "allocation size must be greater than zero".to_string(),
            )
            .into());
        }
        if !self.alignment.is_power_of_two() {
            return Err(MemoryError::InvalidInput(
                "allocation alignment must be a power of two".to_string(),
            )
            .into());
        }
        Ok(())
    }
}

/// Allocator memory statistics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryStats {
    /// Bytes currently allocated to live resources.
    pub allocated_bytes: u64,
    /// Bytes reserved by allocator memory blocks.
    pub reserved_bytes: u64,
    /// Number of live allocation records.
    pub allocation_count: usize,
    /// Number of allocator memory blocks.
    pub block_count: usize,
    /// Number of allocations waiting for GPU fence completion before free.
    pub pending_free_count: usize,
    /// Bytes in allocations waiting for GPU fence completion before free.
    pub pending_free_bytes: u64,
}

impl MemoryStats {
    /// Creates stats from raw live allocator values.
    #[must_use]
    pub const fn new(
        allocated_bytes: u64,
        reserved_bytes: u64,
        allocation_count: usize,
        block_count: usize,
    ) -> Self {
        Self {
            allocated_bytes,
            reserved_bytes,
            allocation_count,
            block_count,
            pending_free_count: 0,
            pending_free_bytes: 0,
        }
    }

    /// Returns a copy with pending-free accounting added.
    #[must_use]
    pub const fn with_pending_free(
        mut self,
        pending_free_count: usize,
        pending_free_bytes: u64,
    ) -> Self {
        self.pending_free_count = pending_free_count;
        self.pending_free_bytes = pending_free_bytes;
        self
    }
}

/// Converts nova-gfx memory placement into `gpu-allocator` placement.
#[must_use]
pub const fn memory_location_to_allocator(
    location: MemoryLocation,
) -> gpu_allocator::MemoryLocation {
    match location {
        MemoryLocation::CpuToGpu => gpu_allocator::MemoryLocation::CpuToGpu,
        MemoryLocation::GpuOnly => gpu_allocator::MemoryLocation::GpuOnly,
    }
}

pub(crate) fn track_allocation(stats: &mut MemoryStats, size: u64) {
    stats.allocated_bytes = stats.allocated_bytes.saturating_add(size);
    stats.reserved_bytes = stats.reserved_bytes.saturating_add(size);
    stats.allocation_count = stats.allocation_count.saturating_add(1);
}

pub(crate) fn untrack_allocation(stats: &mut MemoryStats, size: u64) {
    stats.allocated_bytes = stats.allocated_bytes.saturating_sub(size);
    stats.reserved_bytes = stats.reserved_bytes.saturating_sub(size);
    stats.allocation_count = stats.allocation_count.saturating_sub(1);
}

pub(crate) fn align_to(value: u64, alignment: u64) -> Result<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(GfxError::InvalidInput(
            "alignment must be a non-zero power of two".to_string(),
        ));
    }
    Ok(value.div_ceil(alignment) * alignment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_desc_rejects_zero_size() {
        let desc = MemoryAllocationDesc {
            name: "test".to_string(),
            size: 0,
            alignment: 8,
            location: MemoryLocation::CpuToGpu,
        };

        assert!(desc.validate().is_err());
    }

    #[test]
    fn allocation_desc_rejects_non_power_of_two_alignment() {
        let desc = MemoryAllocationDesc {
            name: "test".to_string(),
            size: 128,
            alignment: 3,
            location: MemoryLocation::CpuToGpu,
        };

        assert!(desc.validate().is_err());
    }

    #[test]
    fn memory_stats_preserves_values() {
        let stats = MemoryStats::new(1, 2, 3, 4).with_pending_free(5, 6);

        assert_eq!(stats.allocated_bytes, 1);
        assert_eq!(stats.reserved_bytes, 2);
        assert_eq!(stats.allocation_count, 3);
        assert_eq!(stats.block_count, 4);
        assert_eq!(stats.pending_free_count, 5);
        assert_eq!(stats.pending_free_bytes, 6);
    }

    #[test]
    fn memory_location_mapping_preserves_upload_memory() {
        assert_eq!(
            memory_location_to_allocator(MemoryLocation::CpuToGpu),
            gpu_allocator::MemoryLocation::CpuToGpu
        );
    }
}
