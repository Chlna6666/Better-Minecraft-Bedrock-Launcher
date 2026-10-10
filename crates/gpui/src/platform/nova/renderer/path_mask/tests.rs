use super::*;

fn key() -> Key {
    Key {
        content: Some(super::super::retained_upload::StaticStreamToken::default()),
        texture_view: TextureViewId::new(1),
        target_size: Extent2d::new(64, 64).expect("extent"),
        viewport: DrawableSize {
            width: 64,
            height: 64,
        },
        format: Format::Bgra8Unorm,
        pipeline: RenderPipelineId::new(2),
    }
}

#[test]
fn static_pixels_are_reused_without_per_slot_identity() {
    let mut residency = Residency::default();
    assert!(residency.begin(key()));
    residency.commit(key());
    for _ in 0..4 {
        assert!(!residency.begin(key()));
    }
}

#[test]
fn missing_identity_target_viewport_format_and_pipeline_changes_require_rasterization() {
    let original = key();
    let mut candidates = [original; 6];
    candidates[0].content = None;
    candidates[1].texture_view = TextureViewId::new(3);
    candidates[2].target_size = Extent2d::new(128, 64).expect("extent");
    candidates[3].viewport.width += 1;
    candidates[4].format = Format::Rgba8Unorm;
    candidates[5].pipeline = RenderPipelineId::new(4);
    for changed in candidates {
        let mut residency = Residency::default();
        residency.commit(original);
        assert!(residency.begin(changed));
        // The old token must also be invalid after a clear that fails partway through.
        assert!(residency.begin(original));
    }
}

#[test]
fn failed_or_unsubmitted_pass_cannot_become_resident() {
    let mut residency = Residency::default();
    assert!(residency.begin(key()));
    assert!(residency.begin(key()));
    residency.commit(key());
    residency.invalidate();
    assert!(residency.begin(key()));
}

#[test]
fn missing_content_identity_always_rasterizes() {
    let mut residency = Residency::default();
    let mut key = key();
    key.content = None;
    for _ in 0..4 {
        assert!(residency.begin(key));
        residency.commit(key);
    }
}
