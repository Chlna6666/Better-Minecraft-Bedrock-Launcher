use super::{
    BitmapPool, HUGE_BITMAP_BUCKET_GRANULARITY, LARGE_BITMAP_BUCKET_GRANULARITY,
    VERY_LARGE_BITMAP_BUCKET_GRANULARITY,
};
use std::sync::Arc;

#[test]
fn caller_owned_storage_keeps_the_vec_without_entering_the_decoder_pool() {
    let pixels = vec![1, 2, 3, 4];
    let pointer = pixels.as_ptr();
    let bytes = super::BitmapBytes::from_owned(pixels);
    assert!(matches!(&bytes.storage, super::BitmapStorage::Owned(_)));
    let weak = Arc::downgrade(&bytes);
    let cloned = bytes.clone();
    drop(bytes);
    assert_eq!(cloned.as_slice().as_ptr(), pointer);
    assert_eq!(cloned.as_slice(), &[1, 2, 3, 4]);
    drop(cloned);
    assert!(weak.upgrade().is_none());
}

#[test]
fn exact_decoder_capacities_below_the_rounded_bucket_are_reused() {
    let pool = BitmapPool::new(64 * 1024 * 1024);
    for requested in [257, 65_537, 1_048_577, 1920 * 1080 * 4, 33_554_433] {
        let buffer = Vec::with_capacity(requested);
        let pointer = buffer.as_ptr();
        pool.release(buffer);
        for _ in 0..32 {
            let buffer = pool.acquire_capacity(requested);
            assert_eq!(buffer.as_ptr(), pointer);
            assert_eq!(buffer.capacity(), requested);
            assert_eq!(pool.snapshot().free_buffers, 0);
            pool.release(buffer);
        }
        assert_eq!(pool.snapshot().retained_bytes, requested);
        pool.trim_to(0);
    }
}

#[test]
fn small_requests_leave_disproportionate_buffers_idle() {
    let pool = BitmapPool::new(64 * 1024 * 1024);
    let large = Vec::with_capacity(1024 * 1024);
    let large_pointer = large.as_ptr();
    let large_capacity = large.capacity();
    pool.release(large);

    let small = pool.acquire_capacity(256);
    assert!(
        small.capacity() <= 512,
        "small request inherited large buffer capacity"
    );
    assert_eq!(pool.snapshot().retained_bytes, large_capacity);
    assert_eq!(pool.snapshot().free_buffers, 1);

    let reused = pool.acquire_capacity(1024 * 1024);
    assert_eq!(reused.as_ptr(), large_pointer);
    assert_eq!(pool.snapshot().free_buffers, 0);
}

#[test]
fn reuse_accepts_double_bucket_but_preserves_larger_buffers() {
    let pool = BitmapPool::new(4096);
    let nearby = Vec::with_capacity(512);
    let nearby_pointer = nearby.as_ptr();
    pool.release(nearby);
    let reused = pool.acquire_capacity(256);
    assert_eq!(reused.as_ptr(), nearby_pointer);
    pool.release(reused);
    pool.trim_to(0);

    pool.release(Vec::with_capacity(513));
    let small = pool.acquire_capacity(256);
    assert!(small.capacity() <= 512);
    assert_eq!(pool.snapshot().free_buffers, 1);
}

#[test]
fn reuse_preserves_classes_and_zero_request_has_no_pool_effect() {
    let pool = BitmapPool::new(64 * 1024 * 1024);
    pool.release(Vec::with_capacity(2 * 1024 * 1024));
    let snapshot = pool.snapshot();
    assert_eq!(pool.acquire_capacity(0).capacity(), 0);
    assert_eq!(pool.snapshot(), snapshot);
    let small_class = pool.acquire_capacity(1024 * 1024);
    assert!(small_class.capacity() < 2 * 1024 * 1024);
    assert_eq!(pool.snapshot(), snapshot);
}

#[test]
fn reuses_capacity_buckets_without_per_buffer_size_limit() {
    let pool = BitmapPool::new(1024);
    let buffer = pool.acquire(200);
    assert!(buffer.capacity() >= 200);
    pool.release(buffer);
    assert_eq!(pool.snapshot().free_buffers, 1);

    let reused = pool.acquire(128);
    assert!(reused.capacity() >= 128);
    assert_eq!(pool.snapshot().free_buffers, 0);
    pool.release(reused);

    let large = pool.acquire_capacity(2048);
    assert!(large.capacity() >= 2048);
    pool.release(large);
    assert!(pool.snapshot().free_buffers >= 1);
    pool.trim_to(0);
    assert_eq!(pool.snapshot().retained_bytes, 0);
}

#[test]
fn moderately_oversized_image_buffer_is_kept_as_single_hot_reuse_slot() {
    let pool = BitmapPool::new(1024);
    pool.release(Vec::with_capacity(2048));

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.free_buffers, 1);
    assert!(snapshot.retained_bytes >= 2048);

    let reused = pool.acquire_capacity(1500);
    assert!(reused.capacity() >= 2048);
    assert_eq!(pool.snapshot().free_buffers, 0);
}

#[test]
fn extreme_one_off_image_buffer_is_not_kept_idle() {
    let pool = BitmapPool::new(1024);
    pool.release(Vec::with_capacity(4096));

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.free_buffers, 0);
    assert_eq!(snapshot.retained_bytes, 0);
}

#[test]
fn oversized_buffer_preserves_small_ui_reuse_reserve() {
    let pool = BitmapPool::new(1024 * 1024);
    pool.release(Vec::with_capacity(128 * 1024));
    pool.release(Vec::with_capacity(128 * 1024));
    pool.release(Vec::with_capacity(2 * 1024 * 1024));

    let snapshot = pool.snapshot();
    assert_eq!(snapshot.free_buffers, 3);
    assert!(snapshot.retained_bytes >= 2 * 1024 * 1024 + 256 * 1024);
}

#[test]
fn newest_buffer_replaces_stale_buffers_when_free_list_budget_is_full() {
    let pool = BitmapPool::new(1024);
    pool.release(Vec::with_capacity(512));
    pool.release(Vec::with_capacity(512));
    assert_eq!(pool.snapshot().retained_bytes, 1024);

    pool.release(Vec::with_capacity(768));
    let snapshot = pool.snapshot();
    assert_eq!(snapshot.free_buffers, 1);
    assert!(snapshot.retained_bytes >= 768);
}

#[test]
fn large_buffers_use_dense_buckets_to_limit_internal_fragmentation() {
    let pool = BitmapPool::new(8 * 1024 * 1024);
    let requested = 1024 * 1024 + 1;
    let buffer = pool.acquire_capacity(requested);

    assert!(buffer.capacity() >= requested);
    assert!(buffer.capacity() <= requested + LARGE_BITMAP_BUCKET_GRANULARITY);
    assert!(buffer.capacity() < requested.next_power_of_two());
}

#[test]
fn large_bucket_granularity_coarsens_as_buffers_grow() {
    let pool = BitmapPool::new(usize::MAX);
    assert_eq!(
        pool.bucket_capacity(2 * 1024 * 1024 + 1) % LARGE_BITMAP_BUCKET_GRANULARITY,
        0
    );
    assert_eq!(
        pool.bucket_capacity(8 * 1024 * 1024 + 1) % VERY_LARGE_BITMAP_BUCKET_GRANULARITY,
        0
    );
    assert_eq!(
        pool.bucket_capacity(40 * 1024 * 1024 + 1) % HUGE_BITMAP_BUCKET_GRANULARITY,
        0
    );
}

#[test]
fn supports_concurrent_acquire_and_release() {
    let pool = Arc::new(BitmapPool::new(64 * 1024));
    let workers = (0..8)
        .map(|_| {
            let pool = pool.clone();
            std::thread::spawn(move || {
                for _ in 0..64 {
                    let buffer = pool.acquire(1024);
                    assert_eq!(buffer.len(), 1024);
                    pool.release(buffer);
                }
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("bitmap pool worker should complete");
    }
    assert!(pool.snapshot().retained_bytes <= 64 * 1024);
}

#[test]
fn trim_releases_retained_capacity() {
    let pool = BitmapPool::new(4096);
    pool.release(Vec::with_capacity(1024));
    pool.release(Vec::with_capacity(2048));
    assert!(pool.snapshot().retained_bytes >= 3072);
    pool.trim_to(1024);
    assert!(pool.snapshot().retained_bytes <= 1024);
}

#[test]
#[ignore = "isolated global bitmap-pool accounting; run with --test-threads=1"]
fn bitmap_bytes_returns_owned_vec_to_pool_on_last_arc_drop() {
    let pool = super::global_bitmap_pool();
    pool.trim_to(0);
    let bytes = super::BitmapBytes::from_vec(Vec::with_capacity(256));
    let cloned = Arc::clone(&bytes);
    drop(bytes);
    assert_eq!(pool.snapshot().free_buffers, 0);
    drop(cloned);
    assert_eq!(pool.snapshot().free_buffers, 1);
    pool.trim_to(0);
}
