//! Bounded relocation planning without allocating new device-memory blocks.
use super::*;

impl Allocator {
    /// Selects a sparse general block whose live allocations fit the maintenance budget.
    /// A destination must already have at least that many free bytes in the same memory type;
    /// actual alignment/granularity fit is checked by `allocate_in_existing_blocks`.
    pub fn relocation_candidate(
        &self,
        max_bytes: u64,
        max_allocations: usize,
    ) -> Option<vk::DeviceMemory> {
        self.memory_types
            .iter()
            .flat_map(|memory_type| {
                memory_type
                    .memory_blocks
                    .iter()
                    .flatten()
                    .filter_map(move |source| {
                        let used = source.sub_allocator.allocated();
                        if !source.sub_allocator.supports_general_allocations()
                            || used == 0
                            || used > max_bytes
                            || used.saturating_mul(4) > source.size
                            || source.size < 4 * 1024 * 1024
                            || source.sub_allocator.allocation_count() > max_allocations
                        {
                            return None;
                        }
                        let free: u64 = memory_type
                            .memory_blocks
                            .iter()
                            .flatten()
                            .filter(|block| {
                                block.device_memory != source.device_memory
                                    && block.sub_allocator.supports_general_allocations()
                            })
                            .map(|block| block.size.saturating_sub(block.sub_allocator.allocated()))
                            .sum();
                        (free >= used).then_some((source.size, source.device_memory))
                    })
            })
            .max_by_key(|(size, _)| *size)
            .map(|(_, memory)| memory)
    }

    /// Reserves a relocation destination in existing general blocks of the source memory type.
    /// Never allocates VkDeviceMemory, uses the excluded source, or falls back to another type.
    ///
    /// # Errors
    /// Returns OutOfMemory when existing holes cannot satisfy size/alignment/granularity;
    /// InvalidAllocationCreateDesc for invalid requirements or dedicated allocation schemes.
    pub fn allocate_in_existing_blocks(
        &mut self,
        desc: &AllocationCreateDesc<'_>,
        excluded: vk::DeviceMemory,
    ) -> Result<Allocation> {
        if desc.requirements.size == 0
            || !desc.requirements.alignment.is_power_of_two()
            || desc.allocation_scheme != AllocationScheme::GpuAllocatorManaged
        {
            return Err(AllocationError::InvalidAllocationCreateDesc);
        }
        let memory_type = self
            .memory_types
            .iter_mut()
            .find(|memory_type| {
                memory_type
                    .memory_blocks
                    .iter()
                    .flatten()
                    .any(|block| block.device_memory == excluded)
            })
            .ok_or(AllocationError::InvalidAllocationCreateDesc)?;
        if desc.requirements.memory_type_bits & (1 << memory_type.memory_type_index) == 0 {
            return Err(AllocationError::NoCompatibleMemoryTypeFound);
        }
        let allocation_type = if desc.linear {
            AllocationType::Linear
        } else {
            AllocationType::NonLinear
        };
        #[cfg(feature = "std")]
        let backtrace = Arc::new(Backtrace::disabled());
        for (index, block) in memory_type.memory_blocks.iter_mut().enumerate() {
            let Some(block) = block else { continue };
            if block.device_memory == excluded
                || !block.sub_allocator.supports_general_allocations()
            {
                continue;
            }
            match block.sub_allocator.allocate(
                desc.requirements.size,
                desc.requirements.alignment,
                allocation_type,
                self.buffer_image_granularity,
                desc.name,
                #[cfg(feature = "std")]
                backtrace.clone(),
            ) {
                Ok((offset, chunk_id)) => {
                    let mapped_ptr = block.mapped_ptr.and_then(|SendSyncPtr(pointer)| {
                        // SAFETY: The suballocator returned a range within this mapped block.
                        core::ptr::NonNull::new(unsafe { pointer.as_ptr().add(offset as usize) })
                            .map(SendSyncPtr)
                    });
                    return Ok(Allocation {
                        chunk_id: Some(chunk_id),
                        offset,
                        size: desc.requirements.size,
                        memory_block_index: index,
                        memory_type_index: memory_type.memory_type_index,
                        device_memory: block.device_memory,
                        mapped_ptr,
                        dedicated_allocation: false,
                        memory_properties: memory_type.memory_properties,
                        name: Some(desc.name.into()),
                    });
                }
                Err(AllocationError::OutOfMemory) => {}
                Err(error) => return Err(error),
            }
        }
        Err(AllocationError::OutOfMemory)
    }
}
