use super::*;
use std::sync::Arc;

#[test]
fn retained_quad_cache_removes_unused_epochs_without_invalidating_in_flight_arc() {
    let mut cache = RetainedChunkCache::default();
    let payload = Arc::new(vec![7u8; PACKED_QUAD_BYTES]);
    let handle = Arc::downgrade(&payload);
    let id = crate::RetainedChunkId::new(
        crate::GlobalElementId::from_path(&["retained-epoch".into()]),
        1,
    );
    cache.begin_frame();
    cache.quad_payloads.insert(
        id,
        PackedQuadChunk {
            bytes: Arc::clone(&payload),
            byte_hash: 17,
            quad_count: 1,
            is_solid: false,
            visual_bounds: None,
            last_used_epoch: cache.frame_epoch,
        },
    );
    cache.begin_frame();
    cache.prune_retained_chunks();
    assert!(cache.quad_payloads.is_empty());
    assert!(handle.upgrade().is_some());
    drop(payload);
    assert!(handle.upgrade().is_none());
}

#[test]
fn frame_epoch_wrap_clears_both_retained_chunk_caches() {
    let mut cache = RetainedChunkCache {
        frame_epoch: u64::MAX,
        ..Default::default()
    };
    let id = crate::RetainedChunkId::new(
        crate::GlobalElementId::from_path(&["retained-wrap".into()]),
        1,
    );
    cache.quad_payloads.insert(
        id.clone(),
        PackedQuadChunk {
            bytes: Arc::new(vec![0; PACKED_QUAD_BYTES]),
            byte_hash: 1,
            quad_count: 1,
            is_solid: false,
            visual_bounds: None,
            last_used_epoch: u64::MAX,
        },
    );
    cache.retained_monochrome_sprite_chunks.insert(
        id,
        PackedRetainedMonochromeSpriteChunk {
            bytes: Arc::new(vec![0; PACKED_MONO_SPRITE_BYTES]),
            byte_hash: 2,
            atlas_tiles: Arc::new(Vec::new()),
            sprite_count: 1,
            texture_id: crate::AtlasTextureId {
                index: 0,
                kind: crate::AtlasTextureKind::Monochrome,
            },
            sampling: crate::MonochromeSpriteSampling::Glyph,
            visual_bounds: None,
            atlas_placement_generation: 0,
            last_used_epoch: u64::MAX,
        },
    );

    cache.begin_frame();

    assert_eq!(cache.frame_epoch, 1);
    assert!(cache.quad_payloads.is_empty());
    assert!(cache.retained_monochrome_sprite_chunks.is_empty());
}
