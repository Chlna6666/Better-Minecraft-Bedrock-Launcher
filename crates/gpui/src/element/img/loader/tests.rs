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
    let bytes: Arc<[u8]> = Arc::from(vec![1_u8; 8]);
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
        let bytes: Arc<[u8]> = Arc::from(vec![1_u8; 8]);
        cache.insert(key as u64, &bytes);
    }

    assert!(cache.entries.len() <= COMPRESSED_CACHE_PRUNE_INTERVAL);
    assert!(cache.entries.capacity() <= COMPRESSED_CACHE_PRUNE_INTERVAL * 2);
}
