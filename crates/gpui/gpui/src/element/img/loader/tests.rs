use super::*;

#[test]
fn render_bounds_dimension_is_bucketed() {
    assert_eq!(bucket_image_dimension(1), 16);
    assert_eq!(bucket_image_dimension(38), 48);
    assert_eq!(bucket_image_dimension(800), 800);
}

#[test]
fn compressed_cache_does_not_keep_payload_alive() {
    let mut cache = CompressedCache::new();
    let bytes = CompressedImageBytes::Shared(Arc::from(vec![1_u8; 8]));
    cache.insert(1, &bytes);

    let cached = cache.get(1).expect("live payload should be reusable");
    assert_eq!(cached.len(), 8);
    drop(cached);
    drop(bytes);

    assert!(cache.get(1).is_none());
    assert!(cache.entries.is_empty());
}

#[test]
fn compressed_cache_prunes_dead_metadata_during_repeated_inserts() {
    let mut cache = CompressedCache::new();

    for key in 0..(COMPRESSED_CACHE_PRUNE_INTERVAL * 4 + 1) {
        let bytes = CompressedImageBytes::from(vec![1_u8; 8]);
        cache.insert(key as u64, &bytes);
    }

    assert!(cache.entries.len() <= COMPRESSED_CACHE_PRUNE_INTERVAL);
    assert!(cache.entries.capacity() <= COMPRESSED_CACHE_PRUNE_INTERVAL * 2);
}

#[test]
fn compressed_cache_preserves_vector_allocation_and_counts_spare_capacity() {
    let mut cache = CompressedCache::new();
    let mut buffer = Vec::with_capacity(256);
    buffer.extend_from_slice(&[1, 2, 3, 4]);
    let pointer = buffer.as_ptr();
    let bytes = CompressedImageBytes::from(buffer);
    cache.insert(1, &bytes);
    let cached = cache.get(1).expect("live vector");
    assert_eq!(cached.as_bytes().as_ptr(), pointer);
    assert_eq!(cache.snapshot(), (1, 256));
    drop(bytes);
    assert_eq!(cached.as_bytes(), &[1, 2, 3, 4]);
    drop(cached);
    assert!(cache.get(1).is_none());
    assert_eq!(cache.snapshot(), (0, 0));
}
