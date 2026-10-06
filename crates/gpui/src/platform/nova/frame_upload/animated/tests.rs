use super::primitive::*;
use super::values::*;
use super::*;
use crate::{Primitive, SceneAnimationValue, TransitionProperty};

#[test]
fn horizontal_edges_preserve_height_radius_and_crossing_order() {
    for edges in [[-20.0, -5.0], [-5.0, -20.0]] {
        let mut primitive = Primitive::Quad(Quad {
            bounds: crate::bounds(
                crate::point(crate::ScaledPixels(100.0), crate::ScaledPixels(10.0)),
                crate::size(crate::ScaledPixels(35.0), crate::ScaledPixels(34.0)),
            ),
            corner_radii: crate::Corners::all(crate::ScaledPixels(17.0)),
            ..Default::default()
        });
        apply_value(
            &mut primitive,
            &SceneAnimationValue {
                animation_id: crate::SceneAnimationId(1),
                property: TransitionProperty::HorizontalEdges,
                progress: 1.0,
                from: [edges[0], edges[1], 0.0, 0.0],
                to: [edges[0], edges[1], 0.0, 0.0],
            },
        );
        let Primitive::Quad(quad) = primitive else {
            panic!("quad decoration");
        };
        assert_eq!(quad.bounds.origin.x, crate::ScaledPixels(80.0));
        assert_eq!(quad.bounds.size.width, crate::ScaledPixels(50.0));
        assert_eq!(quad.bounds.origin.y, crate::ScaledPixels(10.0));
        assert_eq!(quad.bounds.size.height, crate::ScaledPixels(34.0));
        assert_eq!(
            quad.corner_radii,
            crate::Corners::all(crate::ScaledPixels(17.0))
        );
    }
}

#[test]
fn retained_translation_preserves_overshoot_without_accumulating_deltas() {
    let id = crate::SceneAnimationId(1);
    let quad = Quad {
        animation_id: Some(id),
        ..Default::default()
    };
    let upload = AnimatedUpload::new(Primitive::Quad(quad), AnimatedPrimitiveKind::Quad, 3);
    let mut value = SceneAnimationValue {
        animation_id: id,
        property: TransitionProperty::Translation,
        progress: 1.2,
        from: [0.0; 4],
        to: [10.0, 0.0, 0.0, 0.0],
    };
    let size = DrawableSize {
        width: 640,
        height: 480,
    };
    let mut bytes = Vec::new();
    upload.sample(&[value], size, &mut bytes);
    assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 12.0);
    assert_eq!(upload.offset(), (3 * PACKED_QUAD_BYTES) as u64);
    value.progress = 0.5;
    upload.sample(&[value], size, &mut bytes);
    assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 5.0);
    let expected = bytes.clone();
    let mut frame = FrameUpload {
        quads: vec![0; 4 * PACKED_QUAD_BYTES],
        animated_primitives: vec![upload],
        sampled_animation_values: vec![value],
        ..Default::default()
    };
    frame.sample_animated_primitives(size);
    assert_eq!(
        &frame.quads[..3 * PACKED_QUAD_BYTES],
        vec![0; 3 * PACKED_QUAD_BYTES]
    );
    assert_eq!(&frame.quads[3 * PACKED_QUAD_BYTES..], expected);
    assert_eq!(frame.animated_upload_bytes(), PACKED_QUAD_BYTES);
    assert!(frame.animated_primitive_staging.capacity() >= PACKED_QUAD_BYTES);
}

#[test]
fn opacity_clamps_the_property_not_the_motion_progress() {
    let mut primitive = Primitive::Quad(Quad {
        border_color: crate::rgba(0xffffffff).into(),
        ..Default::default()
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: TransitionProperty::Opacity,
            progress: 1.2,
            from: [0.0; 4],
            to: [1.0, 0.0, 0.0, 0.0],
        },
    );
    let Primitive::Quad(quad) = primitive else {
        panic!("quad");
    };
    assert_eq!(quad.border_color.a, 1.0);
}

#[test]
fn translation_opacity_fallback_applies_both_lanes() {
    let mut primitive = Primitive::Quad(Quad {
        bounds: crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(30.0), crate::ScaledPixels(40.0)),
        ),
        border_color: crate::rgba(0xffffffff).into(),
        ..Default::default()
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(2),
            property: TransitionProperty::Translation,
            progress: 0.5,
            from: [20.0, 4.0, 0.5, 1.0],
            to: [0.0, 0.0, 1.0, 1.0],
        },
    );
    let Primitive::Quad(quad) = primitive else {
        panic!("quad");
    };
    assert_eq!(quad.bounds.origin.x, crate::ScaledPixels(20.0));
    assert_eq!(quad.bounds.origin.y, crate::ScaledPixels(22.0));
    assert_eq!(quad.border_color.a, 0.75);
}

#[test]
fn blur_radius_overshoot_stays_inside_reserved_endpoint_footprint() {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let mut primitive = Primitive::Blur(crate::PaintBlur {
        order: 0,
        animation_id: Some(crate::SceneAnimationId(3)),
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        radius: crate::ScaledPixels(20.0),
        opacity: 1.0,
        content: std::sync::Arc::new(crate::Scene::default()),
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(3),
            property: TransitionProperty::FilterBlur,
            progress: 1.5,
            from: [4.0, 0.0, 0.0, 0.0],
            to: [20.0, 0.0, 0.0, 0.0],
        },
    );
    let Primitive::Blur(blur) = primitive else {
        panic!("blur");
    };
    assert_eq!(blur.radius, crate::ScaledPixels(20.0));
    assert_eq!(blur.bounds, bounds);
}

#[test]
fn retained_blur_animation_updates_radius_without_changing_capture_bounds() {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let mut primitive = Primitive::Blur(crate::PaintBlur {
        order: 0,
        animation_id: Some(crate::SceneAnimationId(3)),
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        radius: crate::ScaledPixels(24.0),
        opacity: 1.0,
        content: std::sync::Arc::new(crate::Scene::default()),
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(3),
            property: TransitionProperty::FilterBlur,
            progress: 0.5,
            from: [4.0, 0.0, 0.0, 0.0],
            to: [20.0, 0.0, 0.0, 0.0],
        },
    );
    let Primitive::Blur(blur) = primitive else {
        panic!("blur");
    };
    assert_eq!(blur.radius, crate::ScaledPixels(12.0));
    assert_eq!(blur.bounds, bounds);
}

#[test]
fn retained_clip_reveal_changes_only_the_content_mask() {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let mut primitive = Primitive::Quad(Quad {
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        ..Default::default()
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: TransitionProperty::ClipReveal,
            progress: 0.5,
            from: [10.0, 110.0, 20.0, 20.0],
            to: [10.0, 110.0, 20.0, 100.0],
        },
    );
    let Primitive::Quad(quad) = primitive else {
        panic!("quad");
    };
    assert_eq!(quad.bounds, bounds);
    assert_eq!(quad.content_mask.bounds.origin.x, crate::ScaledPixels(10.0));
    assert_eq!(
        quad.content_mask.bounds.size.width,
        crate::ScaledPixels(100.0)
    );
    assert_eq!(quad.content_mask.bounds.origin.y, crate::ScaledPixels(20.0));
    assert_eq!(
        quad.content_mask.bounds.size.height,
        crate::ScaledPixels(40.0)
    );
}

#[test]
fn retained_horizontal_clip_reveal_changes_only_mask_width() {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let mut primitive = Primitive::Quad(Quad {
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        ..Default::default()
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: TransitionProperty::ClipReveal,
            progress: 0.5,
            from: [10.0, 10.0, 20.0, 100.0],
            to: [10.0, 110.0, 20.0, 100.0],
        },
    );
    let Primitive::Quad(quad) = primitive else {
        panic!("quad");
    };
    assert_eq!(quad.bounds, bounds);
    assert_eq!(quad.content_mask.bounds.origin.x, crate::ScaledPixels(10.0));
    assert_eq!(
        quad.content_mask.bounds.size.width,
        crate::ScaledPixels(50.0)
    );
    assert_eq!(quad.content_mask.bounds.origin.y, crate::ScaledPixels(20.0));
    assert_eq!(
        quad.content_mask.bounds.size.height,
        crate::ScaledPixels(80.0)
    );
}

#[test]
fn retained_transform_scales_around_shared_origin_and_applies_opacity() {
    let mut primitive = Primitive::Quad(Quad {
        bounds: crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(30.0), crate::ScaledPixels(40.0)),
        ),
        border_color: crate::rgba(0xffffffff).into(),
        ..Default::default()
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: TransitionProperty::Transform,
            progress: 0.5,
            from: [0.5, 0.0, 0.0, 0.0],
            to: [1.0, 1.0, 0.0, 0.0],
        },
    );
    let Primitive::Quad(quad) = primitive else {
        panic!("quad");
    };
    assert_eq!(quad.bounds.origin.x, crate::ScaledPixels(7.5));
    assert_eq!(quad.bounds.origin.y, crate::ScaledPixels(15.0));
    assert_eq!(quad.bounds.size.width, crate::ScaledPixels(22.5));
    assert_eq!(quad.bounds.size.height, crate::ScaledPixels(30.0));
    assert_eq!(quad.border_color.a, 0.5);
}

#[test]
fn backdrop_transform_keeps_gaussian_radius_constant() {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let mut primitive = Primitive::BackdropBlur(crate::PaintBackdropBlur {
        order: 1,
        animation_id: Some(crate::SceneAnimationId(7)),
        bounds,
        content_mask: crate::ContentMask {
            bounds,
            ..Default::default()
        },
        corner_radii: Default::default(),
        radius: crate::ScaledPixels(12.0),
        downsample: 2,
        levels: 2,
        saturation: 1.0,
        opacity: 1.0,
        tint: None,
        recompute_overlap: false,
    });
    apply_value(
        &mut primitive,
        &SceneAnimationValue {
            animation_id: crate::SceneAnimationId(7),
            property: TransitionProperty::Transform,
            progress: 0.5,
            from: [0.5, 0.0, 60.0, 60.0],
            to: [1.0, 1.0, 60.0, 60.0],
        },
    );
    let Primitive::BackdropBlur(blur) = primitive else {
        panic!("backdrop blur");
    };
    assert_eq!(blur.radius, crate::ScaledPixels(12.0));
    assert_eq!(blur.opacity, 0.5);
    assert!(blur.bounds.size.width < bounds.size.width);
    assert!(bounds_contains(bounds, blur.bounds));
}

#[test]
fn element_blur_transform_changes_only_display_geometry_and_opacity() {
    let id = crate::SceneAnimationId(11);
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(80.0)),
    );
    let blur = crate::PaintBlur {
        order: 2,
        animation_id: Some(id),
        bounds,
        content_mask: crate::ContentMask {
            bounds,
            corner_bounds: bounds,
            ..Default::default()
        },
        radius: crate::ScaledPixels(14.0),
        opacity: 1.0,
        content: std::sync::Arc::new(crate::Scene::default()),
    };
    let upload = AnimatedUpload::new(
        Primitive::Blur(blur),
        AnimatedPrimitiveKind::BackdropBlur,
        4,
    );
    let mut bytes = Vec::new();
    upload.sample(
        &[SceneAnimationValue {
            animation_id: id,
            property: TransitionProperty::Transform,
            progress: 0.5,
            from: [0.5, 0.0, 60.0, 60.0],
            to: [1.0, 1.0, 60.0, 60.0],
        }],
        DrawableSize {
            width: 640,
            height: 480,
        },
        &mut bytes,
    );

    assert_eq!(
        read_packed_bounds_at(&bytes, BLUR_SOURCE_BOUNDS_OFFSET),
        [10.0, 20.0, 100.0, 80.0]
    );
    assert_ne!(
        read_packed_bounds_at(&bytes, BLUR_DISPLAY_BOUNDS_OFFSET),
        [10.0, 20.0, 100.0, 80.0]
    );
    assert_eq!(
        f32::from_ne_bytes(bytes[112..116].try_into().unwrap()),
        14.0
    );
    assert_eq!(f32::from_ne_bytes(bytes[128..132].try_into().unwrap()), 0.5);
    assert_eq!(upload.offset(), (4 * PACKED_BACKDROP_BLUR_BYTES) as u64);
}

#[test]
fn retained_rotation_is_one_composite_with_a_shared_pivot() {
    let id = crate::SceneAnimationId(17);
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
        crate::size(crate::ScaledPixels(30.0), crate::ScaledPixels(40.0)),
    );
    let blur = crate::PaintBlur {
        order: 3,
        animation_id: Some(id),
        bounds,
        content_mask: crate::ContentMask {
            bounds,
            corner_bounds: bounds,
            ..Default::default()
        },
        radius: crate::ScaledPixels(0.0),
        opacity: 1.0,
        content: std::sync::Arc::new(crate::Scene::default()),
    };
    let upload = AnimatedUpload::new(
        Primitive::Blur(blur),
        AnimatedPrimitiveKind::BackdropBlur,
        2,
    );
    let values = [SceneAnimationValue {
        animation_id: id,
        property: TransitionProperty::Rotation,
        progress: 1.0,
        from: [0.0, 25.0, 40.0, 0.0],
        to: [std::f32::consts::FRAC_PI_2, 25.0, 40.0, 0.0],
    }];
    let resolved = resolve_animation_values(&values);
    let mut bytes = Vec::new();
    let sample = upload.sample_resolved(
        &resolved,
        DrawableSize {
            width: 640,
            height: 480,
        },
        &mut bytes,
    );

    let rotation: [f32; 4] = std::array::from_fn(|index| {
        let offset = BLUR_ROTATION_METADATA_OFFSET + index * 4;
        f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    });
    assert_eq!(rotation, [std::f32::consts::FRAC_PI_2, 25.0, 40.0, 0.0]);
    assert_eq!(
        u32::from_ne_bytes(
            bytes[BLUR_COMPOSITE_KIND_OFFSET..BLUR_COMPOSITE_KIND_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        ROTATED_COMPOSITE_KIND
    );
    assert_eq!(sample.visual_bounds.origin.x, crate::ScaledPixels(5.0));
    assert_eq!(sample.visual_bounds.origin.y, crate::ScaledPixels(25.0));
    assert_eq!(sample.visual_bounds.size.width, crate::ScaledPixels(40.0));
    assert_eq!(sample.visual_bounds.size.height, crate::ScaledPixels(30.0));
}
