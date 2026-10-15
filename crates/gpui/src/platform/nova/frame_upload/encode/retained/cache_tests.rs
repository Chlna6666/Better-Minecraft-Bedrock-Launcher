use super::*;
use std::sync::Arc;

#[test]
fn retained_quad_cache_removes_unused_epochs_without_invalidating_in_flight_arc() {
    let mut cache = RetainedQuadChunkCache::default();
    let payload = Arc::new(vec![7u8; PACKED_QUAD_BYTES]);
    let handle = Arc::downgrade(&payload);
    let id = crate::RetainedChunkId::new(
        crate::GlobalElementId::from_path(&["retained-epoch".into()]),
        1,
    );
    cache.begin_frame();
    cache.retained_quad_chunks.insert(
        id,
        PackedRetainedQuadChunk {
            bytes: Arc::clone(&payload),
            byte_hash: 17,
            quad_count: 1,
            is_solid: false,
            visual_bounds: None,
            last_used_epoch: cache.frame_epoch,
        },
    );
    cache.begin_frame();
    cache.prune_retained_quads();
    assert!(cache.retained_quad_chunks.is_empty());
    assert!(handle.upgrade().is_some());
    drop(payload);
    assert!(handle.upgrade().is_none());
}
