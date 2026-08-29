use super::*;
use crate::{Bounds, ContentMask, ScaledPixels, SceneAnimationValue, TransitionProperty, point, size};

fn test_bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

fn animated_source(scene: &mut Scene, bounds: Bounds<ScaledPixels>) -> SceneAnimationId {
    let animation_id = scene.allocate_animation_id();
    scene.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: ContentMask {
                bounds: test_bounds(0.0, 0.0, 2000.0, 1200.0),
                ..Default::default()
            },
            ..Default::default()
        },
        animation_id,
    );
    animation_id
}

fn backdrop(scene: &mut Scene, bounds: Bounds<ScaledPixels>) -> u32 {
    scene.insert_primitive(PaintBackdropBlur {
        order: 0,
        animation_id: None,
        bounds,
        content_mask: ContentMask {
            bounds: test_bounds(0.0, 0.0, 2000.0, 1200.0),
            ..Default::default()
        },
        corner_radii: Default::default(),
        radius: ScaledPixels(2.0),
        downsample: 1,
        levels: 2,
        saturation: 1.0,
        opacity: 1.0,
        tint: None,
        recompute_overlap: false,
    });
    scene.backdrop_blurs.last().expect("backdrop blur").order
}

#[test]
fn distant_translation_does_not_invalidate_unrelated_backdrop() {
    let mut scene = Scene::default();
    let animation_id = animated_source(&mut scene, test_bounds(20.0, 20.0, 40.0, 40.0));
    let blur_order = backdrop(&mut scene, test_bounds(800.0, 500.0, 200.0, 120.0));
    let current = SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Translation,
        progress: 0.0,
        from: [0.0; 4],
        to: [120.0, 0.0, 0.0, 0.0],
    };
    scene.push_animation_value(current);

    let next = SceneAnimationValue {
        progress: 0.5,
        ..current
    };
    assert!(!scene.backdrop_blur_source_animation_values_changed(&[next]));
    let refresh = scene.backdrop_blur_refresh_state();
    assert!(!refresh.force_all());
    assert!(!refresh.contains_order(blur_order));
}

#[test]
fn translation_entering_sampling_region_invalidates_only_that_backdrop() {
    let mut scene = Scene::default();
    let animation_id = animated_source(&mut scene, test_bounds(20.0, 20.0, 40.0, 40.0));
    let near_order = backdrop(&mut scene, test_bounds(180.0, 20.0, 120.0, 100.0));
    let far_order = backdrop(&mut scene, test_bounds(900.0, 500.0, 120.0, 100.0));
    let current = SceneAnimationValue {
        animation_id,
        property: TransitionProperty::Translation,
        progress: 0.0,
        from: [0.0; 4],
        to: [180.0, 0.0, 0.0, 0.0],
    };
    scene.push_animation_value(current);

    let next = SceneAnimationValue {
        progress: 1.0,
        ..current
    };
    assert!(scene.backdrop_blur_source_animation_values_changed(&[next]));
    let refresh = scene.backdrop_blur_refresh_state();
    assert!(refresh.contains_order(near_order));
    assert!(!refresh.contains_order(far_order));
}

#[test]
fn backdrop_inside_element_blur_is_not_a_root_backdrop_dependency() {
    let mut scene = Scene::default();
    scene.begin_blur(BlurCapture {
        bounds: test_bounds(0.0, 0.0, 300.0, 200.0),
        content_mask: ContentMask {
            bounds: test_bounds(0.0, 0.0, 1000.0, 800.0),
            ..Default::default()
        },
        radius: ScaledPixels(2.0),
        opacity: 1.0,
    });
    backdrop(&mut scene, test_bounds(20.0, 20.0, 120.0, 80.0));
    scene.end_blur();

    let mut previous = Scene::default();
    previous.begin_blur(BlurCapture {
        bounds: test_bounds(0.0, 0.0, 300.0, 200.0),
        content_mask: ContentMask {
            bounds: test_bounds(0.0, 0.0, 1000.0, 800.0),
            ..Default::default()
        },
        radius: ScaledPixels(2.0),
        opacity: 1.0,
    });
    backdrop(&mut previous, test_bounds(20.0, 20.0, 120.0, 80.0));
    previous.end_blur();

    assert!(!scene.backdrop_blur_refresh_required(&previous));
    assert!(scene.backdrop_blur_refresh_state().dirty_orders().is_empty());
}
