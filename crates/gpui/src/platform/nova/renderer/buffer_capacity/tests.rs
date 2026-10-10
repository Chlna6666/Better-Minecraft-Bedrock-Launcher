use super::*;

fn frame_buffers() -> FrameResourceBuffers {
    FrameResourceBuffers {
        global_buffer: BufferId::from_parts(1, 1),
        text_raster_buffer: BufferId::from_parts(2, 1),
        quad_buffer: BufferId::from_parts(3, 1),
        quad_capacity: INITIAL_QUADS,
        shadow_buffer: BufferId::from_parts(4, 1),
        shadow_capacity: INITIAL_SHADOWS,
        path_rasterization_vertex_buffer: BufferId::from_parts(5, 1),
        path_rasterization_vertex_capacity: INITIAL_PATH_VERTICES,
        path_sprite_buffer: BufferId::from_parts(6, 1),
        path_sprite_capacity: INITIAL_PATH_SPRITES,
        mono_sprite_buffer: BufferId::from_parts(7, 1),
        mono_sprite_capacity: INITIAL_MONO_SPRITES,
        poly_sprite_buffer: BufferId::from_parts(8, 1),
        poly_sprite_capacity: INITIAL_POLY_SPRITES,
        underline_buffer: BufferId::from_parts(9, 1),
        backdrop_blur_pass_buffer: BufferId::from_parts(10, 1),
        backdrop_blur_buffer: BufferId::from_parts(11, 1),
        animation_value_buffer: BufferId::from_parts(12, 1),
        animation_value_capacity: INITIAL_ANIMATION_VALUES,
    }
}

#[test]
fn every_dependent_binding_uses_the_replacement_buffer_and_actual_capacity() {
    let old = frame_buffers();
    let ids = std::array::from_fn(|index| BufferId::from_parts(20 + index as u32, 1));
    let sizes = [128, 128, 512, 128, 256];
    let grown = resized_buffers(old, ids, sizes);
    let view = TextureViewId::from_parts(1, 1);
    let sampler = SamplerId::from_parts(1, 1);
    let sets = [
        quad_resource_bindings(&grown),
        shadow_resource_bindings(&grown),
        underline_resource_bindings(&grown),
        path_resource_bindings(&grown, view, sampler),
        mono_atlas_resource_bindings(grown, view, sampler),
        poly_atlas_resource_bindings(grown, view, sampler),
        backdrop_blur_pass_resource_bindings(&grown, view, sampler),
        backdrop_blur_resource_bindings(&grown, view, sampler),
    ];
    let expected = [
        128 * PACKED_SHADOW_BYTES,
        128 * PACKED_PATH_SPRITE_BYTES,
        512 * PACKED_MONO_SPRITE_BYTES,
        128 * PACKED_POLY_SPRITE_BYTES,
        256 * PACKED_ANIMATION_VALUE_BYTES,
    ];
    let mut references = [0; 5];
    for set in sets {
        for binding in set {
            if let BindingResource::Buffer(buffer) = binding.resource {
                assert!(
                    !buffer_ids(old).contains(&buffer.buffer),
                    "old growable buffer must not remain bound"
                );
                if let Some(index) = ids.iter().position(|id| *id == buffer.buffer) {
                    assert_eq!(buffer.size, expected[index] as u64);
                    references[index] += 1;
                }
            }
        }
    }
    assert_eq!(references, [1, 1, 1, 1, 7]);
    assert_eq!(
        capacities(old),
        [
            INITIAL_SHADOWS,
            INITIAL_PATH_SPRITES,
            INITIAL_MONO_SPRITES,
            INITIAL_POLY_SPRITES,
            INITIAL_ANIMATION_VALUES
        ]
    );
}

#[test]
fn growth_rejects_an_in_flight_slot_and_accepts_the_other_slot() {
    assert!(validate_growth_slot(0, [0].into_iter()).is_err());
    assert!(validate_growth_slot(1, [0].into_iter()).is_ok());
    assert!(validate_growth_slot(1, [0, 1].into_iter()).is_err());
    assert!(validate_growth_slot(0, [].into_iter()).is_ok());
}

#[test]
fn capacity_grows_only_when_needed_and_preserves_hard_limit() {
    assert_eq!(next_capacity(64, 0, MAX_ANIMATION_VALUES).unwrap(), 64);
    assert_eq!(next_capacity(64, 64, MAX_ANIMATION_VALUES).unwrap(), 64);
    assert_eq!(next_capacity(64, 65, MAX_ANIMATION_VALUES).unwrap(), 128);
    assert_eq!(next_capacity(64, 513, MAX_ANIMATION_VALUES).unwrap(), 1024);
    assert_eq!(
        next_capacity(16_384, MAX_ANIMATION_VALUES, MAX_ANIMATION_VALUES).unwrap(),
        MAX_ANIMATION_VALUES
    );
    assert!(next_capacity(64, MAX_ANIMATION_VALUES + 1, MAX_ANIMATION_VALUES).is_err());
}

#[test]
fn initial_capacity_stays_small_for_every_growable_stream() {
    for (initial, limit) in [
        (INITIAL_SHADOWS, MAX_SHADOWS),
        (INITIAL_PATH_SPRITES, MAX_PATH_SPRITES),
        (INITIAL_MONO_SPRITES, MAX_MONO_SPRITES),
        (INITIAL_POLY_SPRITES, MAX_POLY_SPRITES),
        (INITIAL_ANIMATION_VALUES, MAX_ANIMATION_VALUES),
    ] {
        assert!(initial > 0 && initial < limit);
        // Repeated growth reaches the complete previous supported workload.
        let mut capacity = initial;
        while capacity < limit {
            let grown = next_capacity(capacity, capacity + 1, limit).unwrap();
            assert!(grown > capacity && grown <= limit);
            capacity = grown;
        }
    }
}
