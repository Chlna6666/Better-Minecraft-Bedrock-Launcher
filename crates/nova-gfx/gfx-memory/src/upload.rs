use gfx_core::{GfxError, Result};

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
    /// Returns [`GfxError::InvalidInput`] when page size or alignment is invalid.
    pub fn validate(self) -> Result<Self> {
        if self.page_size == 0 {
            return Err(GfxError::InvalidInput(
                "upload page size must be greater than zero".to_string(),
            ));
        }
        if !self.alignment.is_power_of_two() {
            return Err(GfxError::InvalidInput(
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
    /// Returns [`GfxError::InvalidInput`] when the descriptor is invalid.
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
    /// Returns [`GfxError::InvalidInput`] when `size` is zero.
    pub fn allocate(&mut self, size: u64) -> Result<UploadAllocation> {
        if size == 0 {
            return Err(GfxError::InvalidInput(
                "upload allocation size must be greater than zero".to_string(),
            ));
        }
        let allocation_size = align_to(size, self.desc.alignment)?;
        for (page_index, page) in self.pages.iter_mut().enumerate() {
            if page.retire_fence.is_some() {
                continue;
            }
            let offset = align_to(page.offset, self.desc.alignment)?;
            let Some(end_offset) = offset.checked_add(allocation_size) else {
                continue;
            };
            if end_offset <= page.size {
                page.offset = end_offset;
                return Ok(UploadAllocation {
                    page_index,
                    offset,
                    size,
                    end_offset,
                });
            }
        }

        let page_size = self.desc.page_size.max(allocation_size);
        let page_index = self.pages.len();
        self.pages.push(UploadPage {
            size: page_size,
            offset: allocation_size,
            retire_fence: None,
        });
        Ok(UploadAllocation {
            page_index,
            offset: 0,
            size,
            end_offset: allocation_size,
        })
    }

    /// Allocates a batch of upload ranges.
    ///
    /// The allocator partitions the batch into page-sized contiguous ranges, avoiding a page scan
    /// for every item. Only an individual allocation larger than the configured page size creates
    /// an oversized page.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] before changing allocator state when any size is zero or
    /// its aligned size overflows.
    pub fn allocate_batch(&mut self, sizes: &[u64]) -> Result<Vec<UploadAllocation>> {
        if sizes.is_empty() {
            return Ok(Vec::new());
        }

        let mut aligned_sizes = Vec::with_capacity(sizes.len());
        for &size in sizes {
            if size == 0 {
                return Err(GfxError::InvalidInput(
                    "upload allocation size must be greater than zero".to_string(),
                ));
            }
            let aligned_size = align_to(size, self.desc.alignment)?;
            aligned_sizes.push(aligned_size);
        }

        let mut planned = self.clone();
        let mut allocations = Vec::with_capacity(sizes.len());
        let mut group_start = 0;
        while group_start < sizes.len() {
            let mut group_end = group_start;
            let mut group_size = 0_u64;
            while group_end < sizes.len() {
                let aligned_size = aligned_sizes[group_end];
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

            let group = planned.allocate(group_size)?;
            let mut offset = group.offset;
            for item_index in group_start..group_end {
                let end_offset =
                    offset
                        .checked_add(aligned_sizes[item_index])
                        .ok_or_else(|| {
                            GfxError::InvalidInput("upload batch offset overflow".to_string())
                        })?;
                allocations.push(UploadAllocation {
                    page_index: group.page_index,
                    offset,
                    size: sizes[item_index],
                    end_offset,
                });
                offset = end_offset;
            }
            group_start = group_end;
        }

        self.pages = planned.pages;
        Ok(allocations)
    }

    /// Marks all pages with live data as unavailable until `fence_value` completes.
    pub fn retire_used_pages(&mut self, fence_value: u64) {
        for page in &mut self.pages {
            if page.offset > 0 {
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
        let mut idle_pages = self
            .pages
            .iter()
            .filter(|page| page.retire_fence.is_none() && page.offset == 0)
            .count();
        while idle_pages > self.desc.max_retained_idle_pages {
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
mod tests {
    use super::*;

    #[test]
    fn upload_ring_aligns_allocations() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 1024,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let first = ring.allocate(1).expect("first allocation should succeed");
        let second = ring.allocate(1).expect("second allocation should succeed");

        assert_eq!(first.offset, 0);
        assert_eq!(second.offset, 256);
    }

    #[test]
    fn upload_ring_supports_dx12_texture_placement_alignment() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 2048,
            alignment: 512,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let first = ring.allocate(1).expect("first allocation should succeed");
        let second = ring.allocate(1).expect("second allocation should succeed");
        let third = ring.allocate(1).expect("third allocation should succeed");

        assert_eq!(first.offset, 0);
        assert_eq!(second.offset, 512);
        assert_eq!(third.offset, 1024);
    }

    #[test]
    fn upload_ring_keeps_busy_page_until_fence_completion() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 512,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let first = ring.allocate(300).expect("first allocation should succeed");
        ring.retire_used_pages(7);
        ring.complete_fence(6);
        let second = ring
            .allocate(128)
            .expect("busy page should force a new page");
        ring.complete_fence(7);
        let third = ring
            .allocate(128)
            .expect("completed page should be reusable");

        assert_eq!(first.page_index, 0);
        assert_eq!(second.page_index, 1);
        assert_eq!(third.page_index, 0);
    }

    #[test]
    fn upload_ring_reserves_small_batch_contiguously() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 2048,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let allocations = ring
            .allocate_batch(&[1, 257, 1])
            .expect("batch allocation should succeed");

        assert_eq!(allocations.len(), 3);
        assert_eq!(allocations[0].offset, 0);
        assert_eq!(allocations[1].offset, 256);
        assert_eq!(allocations[2].offset, 768);
        assert_eq!(allocations[0].end_offset, 256);
        assert_eq!(allocations[1].end_offset, 768);
        assert_eq!(allocations[2].end_offset, 1024);
        assert_eq!(allocations[1].size, 257);
        assert!(
            allocations
                .iter()
                .all(|allocation| allocation.page_index == 0)
        );
        assert_eq!(ring.stats().used_bytes, 1024);
    }

    #[test]
    fn upload_ring_empty_batch_is_noop() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc::default())
            .expect("ring descriptor should be valid");
        ring.allocate(256)
            .expect("initial allocation should succeed");
        let before = ring.stats();

        let allocations = ring
            .allocate_batch(&[])
            .expect("empty batch should succeed");

        assert!(allocations.is_empty());
        assert_eq!(ring.stats(), before);
    }

    #[test]
    fn upload_ring_keeps_large_batch_page_sized() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 512,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let allocations = ring
            .allocate_batch(&[300, 300])
            .expect("batch allocation should succeed");

        assert_eq!(allocations[0].page_index, 0);
        assert_eq!(allocations[1].page_index, 1);
        assert_eq!(ring.stats().reserved_bytes, 1024);
    }

    #[test]
    fn upload_ring_rejects_invalid_batch_atomically() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc::default())
            .expect("ring descriptor should be valid");

        let error = ring
            .allocate_batch(&[128, 0])
            .expect_err("zero-sized batch item should fail");

        assert!(matches!(error, GfxError::InvalidInput(_)));
        assert_eq!(ring.stats(), UploadStats::default());
    }

    #[test]
    fn upload_ring_batch_continues_after_existing_allocation() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 2048,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");
        ring.allocate(1).expect("initial allocation should succeed");

        let allocations = ring
            .allocate_batch(&[1, 257])
            .expect("batch allocation should succeed");

        assert_eq!(allocations[0].offset, 256);
        assert_eq!(allocations[1].offset, 512);
        assert_eq!(ring.stats().used_bytes, 1024);
    }

    #[test]
    fn upload_ring_rejects_alignment_overflow_atomically() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc::default())
            .expect("ring descriptor should be valid");
        let before = ring.stats();

        let error = ring
            .allocate_batch(&[128, u64::MAX])
            .expect_err("alignment overflow should fail");

        assert!(matches!(error, GfxError::InvalidInput(_)));
        assert_eq!(ring.stats(), before);
    }

    #[test]
    fn upload_ring_batch_only_oversizes_individual_pages() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 512,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        let allocations = ring
            .allocate_batch(&[768, 1, 1])
            .expect("batch allocation should succeed");

        assert_eq!(ring.page_size(allocations[0].page_index), Some(768));
        assert_eq!(ring.page_size(allocations[1].page_index), Some(512));
        assert_eq!(allocations[1].page_index, allocations[2].page_index);
    }

    #[test]
    fn upload_ring_batch_waits_for_busy_page_fence() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 512,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");
        let first = ring.allocate(256).expect("allocation should succeed");
        ring.retire_used_pages(7);

        let busy_batch = ring
            .allocate_batch(&[1, 1])
            .expect("batch should use a new page");
        assert_ne!(busy_batch[0].page_index, first.page_index);

        ring.complete_fence(7);
        let reused = ring
            .allocate_batch(&[1])
            .expect("completed page should be reusable");
        assert_eq!(reused[0].page_index, first.page_index);
    }

    #[test]
    fn upload_ring_trim_keeps_configured_idle_floor() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 256,
            alignment: 256,
            max_retained_idle_pages: 1,
        })
        .expect("ring descriptor should be valid");

        ring.allocate(256).expect("allocation should succeed");
        ring.retire_used_pages(1);
        ring.allocate(256)
            .expect("allocation should use another page");
        ring.retire_used_pages(2);
        ring.complete_fence(2);
        let retained_page_count = ring.trim_idle_pages();

        assert_eq!(retained_page_count, 1);
        assert_eq!(ring.stats().page_count, 1);
    }

    #[test]
    fn upload_ring_trim_preserves_non_trailing_page_indices() {
        let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: 256,
            alignment: 256,
            max_retained_idle_pages: 0,
        })
        .expect("ring descriptor should be valid");

        let first = ring.allocate(256).expect("first allocation should succeed");
        ring.retire_used_pages(1);
        let second = ring
            .allocate(256)
            .expect("busy first page should force a second page");
        ring.complete_fence(1);
        let retained_page_count = ring.trim_idle_pages();

        assert_eq!(retained_page_count, 2);
        assert_eq!(first.page_index, 0);
        assert_eq!(second.page_index, 1);
        assert_eq!(ring.page_size(first.page_index), Some(256));
        assert_eq!(ring.page_size(second.page_index), Some(256));
    }
}
