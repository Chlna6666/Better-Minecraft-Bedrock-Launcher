use super::*;

#[test]
fn discarded_unsubmitted_ranges_do_not_release_in_flight_pages() {
    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: 256,
        alignment: 256,
        max_retained_idle_pages: 2,
    })
    .expect("descriptor");
    let busy = ring.allocate(256).expect("in-flight page");
    ring.retire_used_pages(7);
    let unsubmitted = ring.allocate(256).expect("unsubmitted page");
    ring.discard_unsubmitted();
    assert_eq!(ring.stats().busy_page_count, 1);
    let reused = ring.allocate(256).expect("discarded page reused");
    assert_eq!(reused.page_index, unsubmitted.page_index);
    assert_ne!(reused.page_index, busy.page_index);
    ring.retire_used_pages(9);
    ring.complete_fence(7);
    assert_eq!(ring.stats().busy_page_count, 1);
}

#[test]
fn overlapping_submissions_keep_page_growth_bounded_by_in_flight_work() {
    let mut ring =
        UploadRingAllocator::new(UploadRingAllocatorDesc::default()).expect("descriptor");
    for fence in 1u64..=64 {
        ring.allocate(ring.configured_page_size())
            .expect("upload page");
        ring.retire_used_pages(fence);
        ring.complete_fence(fence.saturating_sub(2));
        assert!(ring.stats().page_count <= 3);
    }
    assert_eq!(ring.stats().busy_page_count, 2);
    ring.complete_fence(64);
    assert_eq!(ring.stats().busy_page_count, 0);
    assert_eq!(ring.trim_idle_pages(), 2);
}

#[test]
fn later_submission_does_not_delay_earlier_page_reuse() {
    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: 256,
        alignment: 256,
        max_retained_idle_pages: 2,
    })
    .expect("valid descriptor");
    let first = ring.allocate(256).expect("first page");
    ring.retire_used_pages(7);
    let second = ring.allocate(256).expect("second page");
    ring.retire_used_pages(9);
    ring.complete_fence(7);
    assert_eq!(ring.stats().busy_page_count, 1);
    let reused = ring.allocate(256).expect("reuse completed first page");
    assert_eq!(reused.page_index, first.page_index);
    assert_ne!(reused.page_index, second.page_index);
    assert_eq!(ring.stats().page_count, 2);
}

#[test]
fn batch_handles_unaligned_page_capacity_and_maximum_aligned_spans() {
    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: 300,
        alignment: 256,
        max_retained_idle_pages: 1,
    })
    .expect("valid unaligned page capacity");
    let allocations = ring.allocate_batch(&[1, 1]).expect("valid batch");
    assert_ne!(allocations[0].page_index, allocations[1].page_index);
    assert_eq!(allocations[0].end_offset, 256);
    assert_eq!(allocations[1].end_offset, 256);

    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: u64::MAX,
        alignment: 1,
        max_retained_idle_pages: 1,
    })
    .expect("valid byte-aligned descriptor");
    let allocations = ring
        .allocate_batch(&[u64::MAX, 1])
        .expect("split overflowing group");
    assert_ne!(allocations[0].page_index, allocations[1].page_index);
    assert_eq!(allocations[0].end_offset, u64::MAX);
    assert_eq!(allocations[1].end_offset, 1);
}

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
fn pressure_trim_releases_all_completed_idle_pages() {
    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: 512,
        alignment: 256,
        max_retained_idle_pages: 2,
    })
    .expect("ring descriptor should be valid");

    ring.allocate(300).expect("allocation should succeed");
    ring.retire_used_pages(1);
    ring.complete_fence(1);

    assert_eq!(ring.trim_idle_pages_to(0), 0);
    assert_eq!(ring.stats().reserved_bytes, 0);
}

#[test]
fn pressure_trim_never_releases_busy_pages() {
    let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
        page_size: 512,
        alignment: 256,
        max_retained_idle_pages: 2,
    })
    .expect("ring descriptor should be valid");

    ring.allocate(300).expect("allocation should succeed");
    ring.retire_used_pages(1);

    assert_eq!(ring.trim_idle_pages_to(0), 1);
    assert_eq!(ring.stats().busy_page_count, 1);
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

    assert!(matches!(error, Error::InvalidInput(_)));
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

    assert!(matches!(error, Error::InvalidInput(_)));
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
