use gfx_core::{Error, Result};

use crate::common::align_to;

/// Transient upload ring allocator descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UploadRingAllocatorDesc {
    /// Bytes in each upload page.
    pub page_size: u64,
    /// Required allocation alignment.
    pub alignment: u64,
    /// Target idle pages retained by [`UploadRingAllocator::trim_idle_pages`].
    pub max_retained_idle_pages: usize,
}

impl Default for UploadRingAllocatorDesc {
    fn default() -> Self {
        Self {
            page_size: 4 * 1024 * 1024,
            alignment: 256,
            max_retained_idle_pages: 2,
        }
    }
}

impl UploadRingAllocatorDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when page size or alignment is invalid.
    pub fn validate(self) -> Result<Self> {
        if self.page_size == 0 {
            return Err(Error::InvalidInput(
                "upload page size must be greater than zero".to_string(),
            ));
        }
        if !self.alignment.is_power_of_two() {
            return Err(Error::InvalidInput(
                "upload alignment must be a power of two".to_string(),
            ));
        }
        Ok(self)
    }
}

/// Allocation returned by [`UploadRingAllocator`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UploadAllocation {
    /// Page index selected by the allocator.
    pub page_index: usize,
    /// Byte offset within the page.
    pub offset: u64,
    /// Requested allocation size.
    pub size: u64,
    /// Byte offset where the next allocation starts after this allocation.
    pub end_offset: u64,
}

/// Upload ring accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UploadStats {
    /// Bytes reserved by all pages.
    pub reserved_bytes: u64,
    /// Bytes currently used in active pages.
    pub used_bytes: u64,
    /// Number of allocated pages.
    pub page_count: usize,
    /// Number of pages waiting for GPU fence completion.
    pub busy_page_count: usize,
}

#[derive(Clone, Debug)]
struct UploadPage {
    size: u64,
    offset: u64,
    retire_fence: Option<u64>,
}

/// Pure suballocator for transient CPU-to-GPU upload memory.
///
/// The allocator does not own backend buffers. Backends keep native buffers indexed by
/// `UploadAllocation::page_index` and call the fence methods as GPU work completes.
#[derive(Clone, Debug)]
pub struct UploadRingAllocator {
    desc: UploadRingAllocatorDesc,
    pages: Vec<UploadPage>,
}

impl UploadRingAllocator {
    /// Creates an upload ring allocator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the descriptor is invalid.
    pub fn new(desc: UploadRingAllocatorDesc) -> Result<Self> {
        Ok(Self {
            desc: desc.validate()?,
            pages: Vec::new(),
        })
    }

    /// Allocates a subrange from a free upload page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when `size` is zero.
    pub fn allocate(&mut self, size: u64) -> Result<UploadAllocation> {
        if size == 0 {
            return Err(Error::InvalidInput(
                "upload allocation size must be greater than zero".to_string(),
            ));
        }
        let allocation_size = align_to(size, self.desc.alignment)?;
        Ok(self.allocate_aligned(size, allocation_size))
    }

    // Both sizes have been validated before any batch mutation. Page offsets are always aligned:
    // allocation advances by aligned spans, and fence completion resets an offset to zero.
    fn allocate_aligned(&mut self, size: u64, allocation_size: u64) -> UploadAllocation {
        for (page_index, page) in self.pages.iter_mut().enumerate() {
            if page.retire_fence.is_some() {
                continue;
            }
            let offset = page.offset;
            let Some(end_offset) = offset.checked_add(allocation_size) else {
                continue;
            };
            if end_offset <= page.size {
                page.offset = end_offset;
                return UploadAllocation {
                    page_index,
                    offset,
                    size,
                    end_offset,
                };
            }
        }

        let page_size = self.desc.page_size.max(allocation_size);
        let page_index = self.pages.len();
        self.pages.push(UploadPage {
            size: page_size,
            offset: allocation_size,
            retire_fence: None,
        });
        UploadAllocation {
            page_index,
            offset: 0,
            size,
            end_offset: allocation_size,
        }
    }

    /// Allocates a batch of upload ranges.
    ///
    /// The allocator partitions the batch into page-sized contiguous ranges, avoiding a page scan
    /// for every item. Only an individual allocation larger than the configured page size creates
    /// an oversized page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] before changing allocator state when any size is zero or
    /// its aligned size overflows.
    pub fn allocate_batch(&mut self, sizes: &[u64]) -> Result<Vec<UploadAllocation>> {
        if sizes.is_empty() {
            return Ok(Vec::new());
        }

        let mut allocations = self.plan_batch(sizes)?;
        let mut group_start = 0;
        while group_start < sizes.len() {
            let mut group_end = group_start;
            let mut group_size = 0_u64;
            while group_end < sizes.len() {
                let aligned_size = allocations[group_end].end_offset;
                let Some(candidate_size) = group_size.checked_add(aligned_size) else {
                    break;
                };
                if group_end > group_start && candidate_size > self.desc.page_size {
                    break;
                }
                group_size = candidate_size;
                group_end += 1;
                if group_size > self.desc.page_size {
                    break;
                }
            }

            let group = self.allocate_aligned(group_size, group_size);
            let mut offset = group.offset;
            for allocation in &mut allocations[group_start..group_end] {
                // The prefix sums fit inside the reserved group, whose end was checked above.
                allocation.page_index = group.page_index;
                allocation.offset = offset;
                allocation.end_offset += offset;
                offset = allocation.end_offset;
            }
            group_start = group_end;
        }

        Ok(allocations)
    }

    fn plan_batch(&self, sizes: &[u64]) -> Result<Vec<UploadAllocation>> {
        // Preflight into the result storage. Until assigned a page, end_offset holds the aligned
        // span. Invalid input cannot mutate pages, and valid reservations are infallible.
        let mut allocations = Vec::with_capacity(sizes.len());
        for &size in sizes {
            if size == 0 {
                return Err(Error::InvalidInput(
                    "upload allocation size must be greater than zero".to_string(),
                ));
            }
            allocations.push(UploadAllocation {
                page_index: 0,
                offset: 0,
                size,
                end_offset: align_to(size, self.desc.alignment)?,
            });
        }
        Ok(allocations)
    }

    /// Retires pages containing allocations made since the previous retirement.
    ///
    /// Previously retired pages keep their own fence, since allocation never writes a busy page.
    /// Submit each allocation's bytes once before calling this method; do not submit an already
    /// retired range again. The backend must retain its native page until that fence completes.
    pub fn retire_used_pages(&mut self, fence_value: u64) {
        for page in &mut self.pages {
            if page.offset > 0 && page.retire_fence.is_none() {
                page.retire_fence = Some(fence_value);
            }
        }
    }

    /// Releases pages whose retire fence has completed back to the allocator.
    pub fn complete_fence(&mut self, completed_fence: u64) {
        for page in &mut self.pages {
            if page
                .retire_fence
                .is_some_and(|retire_fence| retire_fence <= completed_fence)
            {
                page.offset = 0;
                page.retire_fence = None;
            }
        }
    }

    /// Drops trailing idle pages beyond the configured retention target.
    ///
    /// Returns the number of page slots that remain. Backends must release native upload
    /// resources whose indices are outside this range.
    #[must_use]
    pub fn trim_idle_pages(&mut self) -> usize {
        self.trim_idle_pages_to(self.desc.max_retained_idle_pages)
    }

    /// Releases trailing fully-idle pages until at most the requested number remain.
    ///
    /// Busy pages are never removed. Backends use this after fence completion to reduce staging
    /// residency under memory pressure without imposing any ceiling on active upload sizes.
    #[must_use]
    pub fn trim_idle_pages_to(&mut self, max_retained_idle_pages: usize) -> usize {
        let mut idle_pages = self
            .pages
            .iter()
            .filter(|page| page.retire_fence.is_none() && page.offset == 0)
            .count();
        while idle_pages > max_retained_idle_pages {
            let Some(page) = self.pages.last() else {
                break;
            };
            if page.retire_fence.is_some() || page.offset > 0 {
                break;
            }
            self.pages.pop();
            idle_pages = idle_pages.saturating_sub(1);
        }
        self.pages.len()
    }

    /// Returns the configured normal staging page size. Individual uploads may allocate more.
    #[must_use]
    pub const fn configured_page_size(&self) -> u64 {
        self.desc.page_size
    }

    /// Returns upload ring accounting.
    #[must_use]
    pub fn stats(&self) -> UploadStats {
        self.pages
            .iter()
            .fold(UploadStats::default(), |mut stats, page| {
                stats.reserved_bytes = stats.reserved_bytes.saturating_add(page.size);
                stats.used_bytes = stats.used_bytes.saturating_add(page.offset);
                stats.page_count = stats.page_count.saturating_add(1);
                if page.retire_fence.is_some() {
                    stats.busy_page_count = stats.busy_page_count.saturating_add(1);
                }
                stats
            })
    }

    /// Returns the native page size required for `page_index`.
    #[must_use]
    pub fn page_size(&self, page_index: usize) -> Option<u64> {
        self.pages.get(page_index).map(|page| page.size)
    }
}

#[cfg(test)]
mod tests;
