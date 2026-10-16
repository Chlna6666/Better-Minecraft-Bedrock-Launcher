use super::*;
use crate::{PaintBlur, Quad};

#[test]
fn identity_capture_inside_gaussian_keeps_parent_output_damage() {
    let output = Bounds::new(
        crate::point(ScaledPixels(0.), ScaledPixels(0.)),
        crate::size(ScaledPixels(200.), ScaledPixels(120.)),
    );
    let build = |x| {
        let mut scene = Scene::default();
        for radius in [4., 0.] {
            scene.begin_blur(BlurCapture {
                animation_id: None,
                layer_id: None,
                bounds: output,
                content_mask: crate::ContentMask::new(output),
                radius: ScaledPixels(radius),
                opacity: 1.,
            });
        }
        scene.insert_primitive(Quad {
            bounds: Bounds::new(
                crate::point(ScaledPixels(x), ScaledPixels(20.)),
                crate::size(ScaledPixels(8.), ScaledPixels(6.)),
            ),
            content_mask: crate::ContentMask::new(output),
            ..Default::default()
        });
        scene.end_blur();
        scene.end_blur();
        scene.finish();
        scene
    };
    let previous = build(20.);
    let current = build(24.);
    let mut damage = Vec::new();
    assert!(for_each_changed_bounds(
        &current,
        0..current.len(),
        &previous,
        0..previous.len(),
        |bounds| damage.push(bounds),
    ));
    assert!(damage.contains(&output));
}
#[test]
fn gpu_resident_source_uses_submitted_animation_samples_not_ui_dirty_views() {
    let bounds = Bounds::new(
        crate::point(ScaledPixels(10.0), ScaledPixels(20.0)),
        crate::size(ScaledPixels(30.0), ScaledPixels(40.0)),
    );
    let mut scene = Scene::default();
    let animation_id = SceneAnimationId(7);
    scene.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        },
        animation_id,
    );
    let sample = |progress| SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Opacity,
        progress,
        from: [0.0; 4],
        to: [1.0, 0.0, 0.0, 0.0],
    };
    assert!(crate::paint::gpu_sample_damage(&scene, &[sample(0.5)], &[sample(0.5)]).is_empty());
    assert_eq!(
        crate::paint::gpu_sample_damage(&scene, &[sample(0.2)], &[sample(0.8)]).as_slice(),
        &[bounds]
    );
}

#[test]
fn translated_geometry_entering_a_fixed_clip_marks_visible_pixels() {
    let clip = Bounds::new(
        crate::point(ScaledPixels(0.), ScaledPixels(0.)),
        crate::size(ScaledPixels(100.), ScaledPixels(100.)),
    );
    let mut scene = Scene::default();
    let animation_id = SceneAnimationId(1 << 31);
    scene.insert_animated_primitive(
        Quad {
            bounds: Bounds::new(
                crate::point(ScaledPixels(-20.), ScaledPixels(20.)),
                crate::size(ScaledPixels(16.), ScaledPixels(16.)),
            ),
            content_mask: crate::ContentMask::new(clip),
            ..Default::default()
        },
        animation_id,
    );
    let value = |progress| SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Translation,
        progress,
        from: [0.; 4],
        to: [40., 0., 0., 0.],
    };
    let visible = Bounds::new(
        crate::point(ScaledPixels(20.), ScaledPixels(20.)),
        crate::size(ScaledPixels(16.), ScaledPixels(16.)),
    );
    let mut replayed = Scene::default();
    replayed.replay(0..scene.len(), &scene, &[]);
    for retained in [&scene, &replayed] {
        for property in [
            TransitionProperty::Translation,
            TransitionProperty::VisualState,
        ] {
            let sample = |progress| {
                let mut sample = value(progress);
                sample.property = property;
                if property == TransitionProperty::VisualState {
                    sample.from = [0., 0., 1., 1.];
                    sample.to = [40., 0., 1., 1.];
                }
                sample
            };
            let dirty = crate::paint::gpu_sample_damage(&retained, &[sample(0.)], &[sample(1.)]);
            assert!(!dirty.is_empty());
            assert!(
                dirty
                    .iter()
                    .any(|region| region.intersect(&visible) == visible)
            );
            assert!(
                dirty
                    .iter()
                    .all(|region| region.intersect(&clip) == *region)
            );
        }
    }
}

#[test]
fn zero_radius_nested_child_keeps_parent_damage_local() {
    let child_bounds = Bounds::new(
        crate::point(ScaledPixels(10.), ScaledPixels(20.)),
        crate::size(ScaledPixels(40.), ScaledPixels(20.)),
    );
    let parent_bounds = Bounds::new(
        crate::point(ScaledPixels(0.), ScaledPixels(0.)),
        crate::size(ScaledPixels(640.), ScaledPixels(480.)),
    );
    let child_id = SceneAnimationId(1 << 31);
    let parent_id = SceneAnimationId((1 << 31) + 1);
    let mut scene = Scene::default();
    scene.begin_blur(BlurCapture {
        animation_id: Some(parent_id),
        layer_id: None,
        bounds: parent_bounds,
        content_mask: crate::ContentMask::new(parent_bounds),
        radius: ScaledPixels(0.),
        opacity: 1.,
    });
    scene.insert_primitive(Quad {
        bounds: parent_bounds,
        content_mask: crate::ContentMask::new(parent_bounds),
        ..Default::default()
    });
    scene.insert_animated_primitive(
        Quad {
            bounds: child_bounds,
            content_mask: crate::ContentMask::new(child_bounds),
            ..Default::default()
        },
        child_id,
    );
    scene.end_blur();
    let child = |progress| SceneAnimationValue {
        animation_id: child_id,
        property: TransitionProperty::Opacity,
        progress,
        from: [0.; 4],
        to: [1., 0., 0., 0.],
    };
    let parent = SceneAnimationValue {
        animation_id: parent_id,
        property: TransitionProperty::Translation,
        progress: 1.,
        from: [10., 0., 0., 0.],
        to: [0.; 4],
    };
    let dirty =
        crate::paint::gpu_sample_damage(&scene, &[child(0.1), parent], &[child(0.9), parent]);
    assert!(!dirty.is_empty());
    assert!(
        dirty
            .iter()
            .all(|region| child_bounds.intersect(region) == *region),
        "a local child update must not invalidate the whole zero-radius parent"
    );
}

#[test]
fn retired_identity_parent_sample_does_not_damage_static_capture() {
    let bounds = Bounds::new(
        crate::point(ScaledPixels(0.), ScaledPixels(0.)),
        crate::size(ScaledPixels(640.), ScaledPixels(480.)),
    );
    let child = Bounds::new(
        crate::point(ScaledPixels(20.), ScaledPixels(20.)),
        crate::size(ScaledPixels(16.), ScaledPixels(16.)),
    );
    let mut scene = Scene::default();
    scene.begin_blur(BlurCapture {
        animation_id: Some(SceneAnimationId(2)),
        layer_id: None,
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        radius: ScaledPixels(0.),
        opacity: 1.,
    });
    scene.insert_primitive(Quad {
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        ..Default::default()
    });
    scene.insert_animated_primitive(
        Quad {
            bounds: child,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        },
        SceneAnimationId(1),
    );
    scene.end_blur();
    let value = |progress| SceneAnimationValue {
        animation_id: SceneAnimationId(1),
        property: TransitionProperty::Opacity,
        progress,
        from: [0.; 4],
        to: [1., 0., 0., 0.],
    };
    let identity = SceneAnimationValue {
        animation_id: SceneAnimationId(2),
        property: TransitionProperty::Translation,
        progress: 1.,
        from: [10., 0., 0., 0.],
        to: [0.; 4],
    };
    let damage = crate::paint::gpu_sample_damage(&scene, &[value(0.2), identity], &[value(0.8)]);
    assert!(!damage.is_empty());
    assert!(
        damage
            .iter()
            .all(|region| region.intersect(&child) == *region),
        "retiring an identity sample must not repaint a static ancestor: {damage:?}"
    );
}

#[test]
fn gpu_resident_nested_blur_child_invalidates_parent_color_output() {
    let bounds = Bounds::new(
        crate::point(ScaledPixels(10.0), ScaledPixels(20.0)),
        crate::size(ScaledPixels(40.0), ScaledPixels(20.0)),
    );
    let mut child = Scene::default();
    let animation_id = SceneAnimationId(1 << 31);
    child.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        },
        animation_id,
    );
    let mut parent = Scene::default();
    parent.insert_primitive(PaintBlur {
        order: 0,
        animation_id: None,
        layer_id: None,
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        radius: ScaledPixels(8.0),
        opacity: 1.0,
        content: std::sync::Arc::new(child),
    });
    let sample = |progress| SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Opacity,
        progress,
        from: [0.0; 4],
        to: [1.0, 0.0, 0.0, 0.0],
    };
    let dirty = crate::paint::gpu_sample_damage(&parent, &[sample(0.1)], &[sample(0.9)]);
    assert_eq!(dirty.as_slice(), &[bounds]);
}

#[test]
fn renderer_owned_opacity_tick_marks_pixels_without_ui_relayout() {
    let bounds = Bounds::new(
        crate::point(ScaledPixels(50.0), ScaledPixels(40.0)),
        crate::size(ScaledPixels(25.0), ScaledPixels(10.0)),
    );
    let mut scene = Scene::default();
    let animation_id = SceneAnimationId(1 << 31);
    scene.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        },
        animation_id,
    );
    let sample = |progress| SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Opacity,
        progress,
        from: [0.0; 4],
        to: [1.0, 0.0, 0.0, 0.0],
    };
    assert!(crate::paint::gpu_sample_damage(&scene, &[sample(0.1)], &[sample(0.1)]).is_empty());
    let dirty = crate::paint::gpu_sample_damage(&scene, &[sample(0.1)], &[sample(0.7)]);
    assert_eq!(dirty.as_slice(), &[bounds]);
}

#[test]
fn renderer_owned_translation_marks_union_of_old_and_new_pixel_bounds() {
    let bounds = Bounds::new(
        crate::point(ScaledPixels(20.0), ScaledPixels(30.0)),
        crate::size(ScaledPixels(40.0), ScaledPixels(20.0)),
    );
    let mut scene = Scene::default();
    let animation_id = SceneAnimationId(1 << 31);
    scene.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: crate::ContentMask::new(Bounds::new(
                bounds.origin,
                crate::size(ScaledPixels(120.), ScaledPixels(20.)),
            )),
            ..Default::default()
        },
        animation_id,
    );
    let sample = |progress| SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Translation,
        progress,
        from: [0.0; 4],
        to: [80.0, 0.0, 0.0, 0.0],
    };
    let dirty = crate::paint::gpu_sample_damage(&scene, &[sample(0.0)], &[sample(0.5)]);
    assert_eq!(
        dirty.as_slice(),
        &[Bounds::new(
            crate::point(ScaledPixels(20.0), ScaledPixels(30.0)),
            crate::size(ScaledPixels(80.0), ScaledPixels(20.0)),
        )]
    );
    // Custom easing may overshoot either endpoint. Damage follows the actual
    // visible samples, rather than needing an unbounded declaration envelope.
    for (previous, next, left) in [(0.5, 1.25, 60.0), (-0.25, 0.5, 20.0)] {
        let dirty = crate::paint::gpu_sample_damage(&scene, &[sample(previous)], &[sample(next)]);
        assert_eq!(
            dirty.as_slice(),
            &[Bounds::new(
                crate::point(ScaledPixels(left), ScaledPixels(30.0)),
                crate::size(ScaledPixels(80.0), ScaledPixels(20.0)),
            )]
        );
    }
}

#[test]
fn scale_damage_uses_unclipped_primitive_center() {
    let primitive_bounds = Bounds::new(
        crate::point(ScaledPixels(0.0), ScaledPixels(0.0)),
        crate::size(ScaledPixels(100.0), ScaledPixels(100.0)),
    );
    let clipped_bounds = Bounds::new(
        crate::point(ScaledPixels(50.0), ScaledPixels(0.0)),
        crate::size(ScaledPixels(50.0), ScaledPixels(100.0)),
    );
    let primitive = Primitive::Quad(Quad {
        bounds: primitive_bounds,
        content_mask: crate::ContentMask::new(clipped_bounds),
        ..Quad::default()
    });
    let value = SceneAnimationValue {
        animation_id: SceneAnimationId(0),
        property: TransitionProperty::Scale,
        progress: 1.0,
        from: [1.0, 0.0, 0.0, 0.0],
        to: [2.0, 0.0, 0.0, 0.0],
    };

    assert_eq!(
        animation_sampled_bounds(&primitive, Some(&value)),
        Bounds::new(
            crate::point(ScaledPixels(50.0), ScaledPixels(-50.0)),
            crate::size(ScaledPixels(100.0), ScaledPixels(200.0)),
        )
    );
}
