use super::*;

fn key() -> Key {
    Key {
        content: Some(super::super::retained_upload::StaticStreamToken::default()),
        draw_plan: Some(1),
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
    let mut candidates = [original; 7];
    candidates[0].content = None;
    candidates[1].texture_view = TextureViewId::new(3);
    candidates[2].target_size = Extent2d::new(128, 64).expect("extent");
    candidates[3].viewport.width += 1;
    candidates[4].format = Format::Rgba8Unorm;
    candidates[5].pipeline = RenderPipelineId::new(4);
    candidates[6].draw_plan = Some(2);
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

#[test]
fn ordered_multi_batch_path_mask_reuses_identical_geometry_and_draw_plan() {
    let pipeline = RenderPipelineId::new(2);
    let set = ResourceSetId::new(5);
    let steps = [
        DrawStepDescriptor {
            pipeline,
            resource_sets: gfx_core::resource_set_list([set]),
            first_vertex: 0,
            vertex_count: 3,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        },
        DrawStepDescriptor {
            pipeline,
            resource_sets: gfx_core::resource_set_list([set]),
            first_vertex: 6,
            vertex_count: 3,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        },
    ];
    let packed_bytes = 9 * PACKED_PATH_RASTERIZATION_VERTEX_BYTES;
    let token = ordered_draw_plan_token(&steps, pipeline, set, 9, packed_bytes)
        .expect("multi-batch path mask plan");
    let mut key = key();
    key.draw_plan = Some(token);
    let mut residency = Residency::default();
    assert!(residency.begin(key));
    residency.commit(key);
    assert!(!residency.begin(key), "identical multi-step mask should be resident");

    let mut reordered = steps.clone();
    reordered.swap(0, 1);
    let reordered_token = ordered_draw_plan_token(&reordered, pipeline, set, 9, packed_bytes)
        .expect("reordered plan is valid but different");
    assert_ne!(token, reordered_token);
    key.draw_plan = Some(reordered_token);
    assert!(residency.begin(key), "changed painter order must refresh mask");
}

#[test]
fn path_draw_plan_rejects_unsupported_state_and_malformed_vertex_ranges() {
    let pipeline = RenderPipelineId::new(2);
    let set = ResourceSetId::new(5);
    let step = DrawStepDescriptor {
        pipeline,
        resource_sets: gfx_core::resource_set_list([set]),
        first_vertex: 3,
        vertex_count: 3,
        instance_count: 1,
        first_instance: 0,
        scissor: None,
    };
    let bytes = 9 * PACKED_PATH_RASTERIZATION_VERTEX_BYTES;
    assert!(ordered_draw_plan_token(&[step.clone()], pipeline, set, 9, bytes).is_some());
    assert!(ordered_draw_plan_token(&[step.clone()], pipeline, set, 9, bytes - 1).is_none());
    assert!(ordered_draw_plan_token(&[step.clone()], pipeline, ResourceSetId::new(6), 9, bytes).is_none());
    let mut invalid = step.clone();
    invalid.first_vertex = 8;
    assert!(ordered_draw_plan_token(&[invalid], pipeline, set, 9, bytes).is_none());
    let mut invalid = step.clone();
    invalid.scissor = Some(ScissorRect { x: 0, y: 0, width: 4, height: 4 });
    assert!(ordered_draw_plan_token(&[invalid], pipeline, set, 9, bytes).is_none());
    let mut invalid = step;
    invalid.instance_count = 2;
    assert!(ordered_draw_plan_token(&[invalid], pipeline, set, 9, bytes).is_none());
}
