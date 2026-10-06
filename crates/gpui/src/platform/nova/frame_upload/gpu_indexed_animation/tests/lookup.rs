use super::*;

fn sampled_animation_value(
    animation_id: crate::SceneAnimationId,
    property: crate::TransitionProperty,
) -> crate::SceneAnimationValue {
    crate::SceneAnimationValue {
        animation_id,
        property,
        progress: 0.5,
        from: [0.0; 4],
        to: [1.0, 0.0, 0.0, 0.0],
    }
}

fn element_blur_upload(
    sampled_animation_values: Vec<crate::SceneAnimationValue>,
    animation_ids: &[crate::SceneAnimationId],
) -> FrameUpload {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(120.0), crate::ScaledPixels(80.0)),
    );
    let drawable_size = DrawableSize {
        width: 640,
        height: 480,
    };
    let mut upload = FrameUpload {
        globals: vec![0; GLOBAL_UPLOAD_BYTES],
        sampled_animation_values,
        ..Default::default()
    };

    for (index, animation_id) in animation_ids.iter().copied().enumerate() {
        let index = index as u32;
        let blur = crate::PaintBlur {
            order: index,
            animation_id: Some(animation_id),
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(24.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        write_paint_blur(&mut upload.backdrop_blurs, &blur, drawable_size);
        upload.batches.push(UploadedBatch::CompositeBlur { index });
        upload.animated_primitives.push(AnimatedUpload::new(
            crate::Primitive::Blur(blur),
            AnimatedPrimitiveKind::BackdropBlur,
            index,
        ));
    }

    upload
}

fn promotion_benchmark_upload(
    sample_count: usize,
    blur_count: usize,
    quad_count: usize,
) -> FrameUpload {
    let sampled_animation_values = (0..sample_count)
        .map(|index| {
            let property = if index % 2 == 0 {
                crate::TransitionProperty::Opacity
            } else {
                crate::TransitionProperty::Translation
            };
            sampled_animation_value(crate::SceneAnimationId(index as u32), property)
        })
        .collect::<Vec<_>>();

    if blur_count > 0 {
        let first_blur_id = sample_count.saturating_sub(blur_count);
        let animation_ids = (first_blur_id..sample_count)
            .map(|index| crate::SceneAnimationId(index as u32))
            .collect::<Vec<_>>();
        return element_blur_upload(sampled_animation_values, &animation_ids);
    }

    let mut upload = FrameUpload {
        globals: vec![0; GLOBAL_UPLOAD_BYTES],
        quads: vec![0; quad_count.saturating_mul(PACKED_QUAD_BYTES)],
        sampled_animation_values,
        ..Default::default()
    };
    for index in 0..quad_count {
        let index = index as u32;
        let animation_id = crate::SceneAnimationId(index);
        upload.animated_primitives.push(AnimatedUpload::new(
            crate::Primitive::Quad(crate::Quad {
                animation_id: Some(animation_id),
                ..Default::default()
            }),
            AnimatedPrimitiveKind::Quad,
            index,
        ));
    }
    upload
}

fn packed_promotion_checksum(upload: &FrameUpload) -> u64 {
    let mut checksum = upload.gpu_indexed_animation_slots.len() as u64;
    for quad in upload.quads.chunks_exact(PACKED_QUAD_BYTES) {
        checksum = checksum.rotate_left(5) ^ u64::from(read_u32(quad, 0));
    }
    for blur in upload
        .backdrop_blurs
        .chunks_exact(PACKED_BACKDROP_BLUR_BYTES)
    {
        checksum =
            checksum.rotate_left(7) ^ u64::from(read_u32(blur, PAINT_BLUR_COMPOSITE_KIND_OFFSET));
    }
    checksum ^ upload.animated_primitives.len() as u64
}

fn run_promotion_benchmark_case(
    name: &str,
    sample_count: usize,
    animated_primitive_count: usize,
    iterations: usize,
    mut make_upload: impl FnMut() -> FrameUpload,
) {
    // Keep fixture construction outside the measured interval; each iteration receives an
    // independent upload because promotion mutates its packed buffers and primitive list.
    let mut uploads = (0..iterations).map(|_| make_upload()).collect::<Vec<_>>();
    let started_at = std::time::Instant::now();
    for upload in &mut uploads {
        upload.promote_gpu_indexed_animations();
    }
    let elapsed = started_at.elapsed();
    let checksum = std::hint::black_box(
        uploads
            .iter()
            .map(packed_promotion_checksum)
            .fold(0_u64, u64::wrapping_add),
    );
    let elapsed_ns = elapsed.as_nanos();
    println!(
        "promotion_benchmark case={name} samples={sample_count} animated_primitives={animated_primitive_count} iterations={iterations} elapsed_ns={elapsed_ns} ns_per_iteration={} checksum={checksum:#018x}",
        elapsed_ns / iterations as u128,
    );
}

#[test]
fn element_blur_promotion_resolves_multiple_ids_and_properties() {
    let translation_id = crate::SceneAnimationId(31);
    let blur_radius_id = crate::SceneAnimationId(32);
    let mut upload = element_blur_upload(
        vec![
            sampled_animation_value(blur_radius_id, crate::TransitionProperty::FilterBlur),
            sampled_animation_value(translation_id, crate::TransitionProperty::Translation),
        ],
        &[translation_id, blur_radius_id],
    );

    upload.promote_gpu_indexed_animations();

    assert_eq!(
        upload.gpu_indexed_animation_slots.get(&translation_id),
        Some(&0)
    );
    assert_eq!(
        upload.gpu_indexed_animation_slots.get(&blur_radius_id),
        Some(&1)
    );
    assert!(upload.animated_primitives.is_empty());
    assert_eq!(
        [
            read_u32(&upload.backdrop_blurs, PAINT_BLUR_COMPOSITE_KIND_OFFSET),
            read_u32(
                &upload.backdrop_blurs,
                PACKED_BACKDROP_BLUR_BYTES + PAINT_BLUR_COMPOSITE_KIND_OFFSET,
            ),
        ],
        [
            ELEMENT_COMPOSITE_KIND | (1 << 2),
            ELEMENT_COMPOSITE_KIND | (2 << 2)
        ]
    );
    assert_eq!(
        upload
            .gpu_indexed_composite_element_blur_animation_ids
            .get(&0),
        Some(&translation_id)
    );
    assert!(
        !upload
            .gpu_indexed_composite_element_blur_animation_ids
            .contains_key(&1)
    );
    assert_eq!(
        upload.backdrop_blur_configs()[0].animation_slot_plus_one(),
        1
    );
    assert_eq!(
        upload.backdrop_blur_configs()[1].animation_slot_plus_one(),
        2
    );
}

#[test]
fn element_blur_missing_or_unsupported_animation_value_is_not_promoted() {
    let blur_id = crate::SceneAnimationId(41);
    let mut missing = element_blur_upload(
        vec![sampled_animation_value(
            crate::SceneAnimationId(42),
            crate::TransitionProperty::Translation,
        )],
        &[blur_id],
    );
    missing.promote_gpu_indexed_animations();
    assert!(missing.gpu_indexed_animation_slots.is_empty());
    assert_eq!(missing.animated_primitives.len(), 1);

    let mut unsupported = element_blur_upload(
        vec![sampled_animation_value(
            blur_id,
            crate::TransitionProperty::Width,
        )],
        &[blur_id],
    );
    unsupported.promote_gpu_indexed_animations();
    assert!(unsupported.gpu_indexed_animation_slots.is_empty());
    assert_eq!(unsupported.animated_primitives.len(), 1);
}

#[test]
fn element_blur_duplicate_id_keeps_first_unsupported_property() {
    let blur_id = crate::SceneAnimationId(51);
    let mut upload = element_blur_upload(
        vec![
            sampled_animation_value(blur_id, crate::TransitionProperty::Width),
            sampled_animation_value(blur_id, crate::TransitionProperty::Translation),
        ],
        &[blur_id],
    );

    upload.promote_gpu_indexed_animations();

    assert!(upload.gpu_indexed_animation_slots.is_empty());
    assert_eq!(upload.animated_primitives.len(), 1);
}

#[test]
fn element_blur_promotion_uses_updated_property_on_next_call() {
    let blur_id = crate::SceneAnimationId(61);
    let mut upload = element_blur_upload(
        vec![sampled_animation_value(
            blur_id,
            crate::TransitionProperty::Width,
        )],
        &[blur_id],
    );

    upload.promote_gpu_indexed_animations();
    assert!(upload.gpu_indexed_animation_slots.is_empty());
    assert_eq!(upload.animated_primitives.len(), 1);

    upload.sampled_animation_values[0].property = crate::TransitionProperty::Translation;
    upload.promote_gpu_indexed_animations();

    assert_eq!(upload.gpu_indexed_animation_slots.get(&blur_id), Some(&0));
    assert!(upload.animated_primitives.is_empty());
}

#[test]
#[ignore = "manual CPU promotion microbenchmark; run with --ignored --nocapture"]
fn promotion_lookup_benchmark() {
    const ITERATIONS: usize = 128;
    const SAMPLE_COUNT: usize = 512;
    const NO_BLUR_QUAD_COUNT: usize = 128;
    const FEW_BLUR_COUNT: usize = 4;
    const MANY_BLUR_COUNT: usize = 128;

    run_promotion_benchmark_case(
        "no_blur",
        SAMPLE_COUNT,
        NO_BLUR_QUAD_COUNT,
        ITERATIONS,
        || promotion_benchmark_upload(SAMPLE_COUNT, 0, NO_BLUR_QUAD_COUNT),
    );
    run_promotion_benchmark_case(
        "many_samples_few_blur",
        SAMPLE_COUNT,
        FEW_BLUR_COUNT,
        ITERATIONS,
        || promotion_benchmark_upload(SAMPLE_COUNT, FEW_BLUR_COUNT, 0),
    );
    run_promotion_benchmark_case(
        "many_animations_many_blur",
        SAMPLE_COUNT,
        MANY_BLUR_COUNT,
        ITERATIONS,
        || promotion_benchmark_upload(SAMPLE_COUNT, MANY_BLUR_COUNT, 0),
    );
}
