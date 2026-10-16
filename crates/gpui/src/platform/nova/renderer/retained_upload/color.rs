//! Select window color retention for substantial static raster work around local motion.
use super::DrawableSize;

/// Rectangles estimate raster work, not opaque coverage or GPU execution time.
/// Overlapping static primitives count separately because direct presentation
/// submits them separately. Actual repaint damage still comes from old/new
/// animation samples, including captured-layer dependencies.
pub(super) fn should_retain_local_animation(
    scene: &crate::Scene,
    size: DrawableSize,
    presentation_values: &[crate::SceneAnimationValue],
) -> bool {
    let viewport = crate::bounds(
        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
        crate::size(
            crate::ScaledPixels(size.width as f32),
            crate::ScaledPixels(size.height as f32),
        ),
    );
    let viewport_area = f64::from(size.width) * f64::from(size.height);
    if viewport_area == 0. {
        return false;
    }
    let (static_area, animated_area, has_animation) =
        raster_work(scene, viewport, presentation_values);
    // Estimate the static raster work avoided outside local motion. Retention
    // adds one viewport composite and a transparent clear over local damage;
    // a fixed two-viewport cutoff rejects beneficial pages just below it.
    // This remains a raster-work estimate, not a GPU-time prediction.
    let saved_static_area = static_area * (1. - animated_area / viewport_area);
    let added_present_area = viewport_area + animated_area;
    let eligible = has_animation
        && animated_area * 4. <= viewport_area
        && saved_static_area > added_present_area;
    log::trace!(
        target: "gpui::diagnostics",
        "GPUI color retention policy: scene_revision={} static_raster_pixels={:.0} animated_output_pixels={:.0} viewport_pixels={:.0} estimated_saved_static_pixels={:.0} added_present_pixels={:.0} has_animation={} eligible={}",
        scene.revision, static_area, animated_area, viewport_area, saved_static_area, added_present_area, has_animation, eligible,
    );
    eligible
}

fn raster_work(
    scene: &crate::Scene,
    viewport: crate::Bounds<crate::ScaledPixels>,
    presentation_values: &[crate::SceneAnimationValue],
) -> (f64, f64, bool) {
    let mut static_area = 0.;
    let mut animated_area = 0.;
    let mut has_animation = false;
    let mut capture_depth = 0usize;
    for operation in scene.paint_operations.iter() {
        let primitive = match operation {
            crate::scene::PaintOperation::StartBlur(_) => {
                capture_depth += 1;
                continue;
            }
            crate::scene::PaintOperation::EndBlur => {
                capture_depth = capture_depth.saturating_sub(1);
                continue;
            }
            crate::scene::PaintOperation::Primitive(primitive)
                if capture_depth == 0 && !matches!(primitive, crate::Primitive::Blur(_)) =>
            {
                primitive
            }
            _ => continue,
        };
        let moves = primitive
            .animation_id()
            .is_some_and(|id| !animation_is_settled(id, scene, presentation_values));
        let bounds = if moves {
            crate::paint::sampled_primitive_bounds(
                primitive,
                scene.animation_values.iter().chain(presentation_values),
            )
        } else {
            primitive.visual_bounds()
        }
        .intersect(&viewport);
        let area = f64::from(bounds.size.width.0.max(0.)) * f64::from(bounds.size.height.0.max(0.));
        if moves {
            animated_area += area;
            has_animation = true;
        } else {
            static_area += area;
        }
    }
    // Captured children rasterize into their own texture. Count their final
    // root output once, including a child's animation that changes that output.
    for blur in &scene.blurs {
        let bounds = blur
            .bounds
            .intersect(&blur.content_mask.bounds)
            .intersect(&viewport);
        let area = f64::from(bounds.size.width.0.max(0.)) * f64::from(bounds.size.height.0.max(0.));
        let owner_moves = blur
            .animation_id
            .is_some_and(|id| !animation_is_settled(id, scene, presentation_values));
        if owner_moves {
            animated_area += area;
            has_animation = true;
        } else {
            let (_, child_area, child_moves) =
                raster_work(&blur.content, bounds, presentation_values);
            let local = crate::paint::capture_preserves_source_coordinates(
                blur.radius,
                blur.animation_id,
                scene.animation_values.iter().chain(presentation_values),
            );
            let changing_area = if child_moves && !local {
                area
            } else {
                child_area.min(area)
            };
            animated_area += changing_area;
            static_area += area - changing_area;
            has_animation |= child_moves;
        }
    }
    (static_area, animated_area, has_animation)
}

// FillMode::Forwards retains both the final value and its ID. The binding
// alone does not mean that the layer is still moving. Missing samples are
// identity in the renderer; progress beyond 1 is not a spring completion.
fn animation_is_settled(
    id: crate::SceneAnimationId,
    scene: &crate::Scene,
    presentation_values: &[crate::SceneAnimationValue],
) -> bool {
    let mut values = scene
        .animation_values
        .iter()
        .chain(presentation_values)
        .filter(|value| value.animation_id == id);
    values.all(|value| {
        value.progress == 1.
            && (value.property != crate::TransitionProperty::VisualState
                || value.to == [0., 0., 1., 1.])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: DrawableSize = DrawableSize {
        width: 64,
        height: 64,
    };

    #[test]
    fn inactive_parent_binding_does_not_block_small_popup() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        let panel = quad(64., 48., false);
        scene.begin_blur(crate::BlurCapture {
            animation_id: Some(crate::SceneAnimationId(2)),
            layer_id: None,
            bounds: panel.bounds,
            content_mask: panel.content_mask,
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
        });
        scene.insert_primitive(panel);
        scene.end_blur();
        scene.insert_primitive(quad(8., 8., true));
        let running = crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: crate::TransitionProperty::Opacity,
            progress: 0.5,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        };
        assert!(
            should_retain_local_animation(&scene, SIZE, &[running]),
            "no GPU sample on an old parent binding means identity, not moving page content"
        );
    }

    fn quad(width: f32, height: f32, animated: bool) -> crate::Quad {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(width), crate::ScaledPixels(height)),
        );
        crate::Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            animation_id: animated.then_some(crate::SceneAnimationId(1)),
            ..Default::default()
        }
    }

    fn running(id: u32) -> crate::SceneAnimationValue {
        crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(id),
            property: crate::TransitionProperty::Opacity,
            progress: 0.5,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        }
    }

    fn begin_capture(scene: &mut crate::Scene, animated: bool) {
        let bounds = quad(8., 8., false).bounds;
        scene.begin_blur(crate::BlurCapture {
            animation_id: animated.then_some(crate::SceneAnimationId(1)),
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
        });
    }

    #[test]
    fn static_work_below_two_viewports_can_retain_local_motion() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(972., 600., false));
        scene.insert_primitive(quad(960., 585., false));
        scene.insert_primitive(quad(7., 7., false));
        scene.insert_primitive(quad(178., 36744. / 178., true));
        assert!(
            should_retain_local_animation(
                &scene,
                DrawableSize {
                    width: 972,
                    height: 600
                },
                &[running(1)],
            ),
            "local motion must not redraw static work merely because it is below two viewports"
        );
    }

    #[test]
    fn small_child_motion_does_not_make_a_large_static_capture_fully_dynamic() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        let panel = quad(64., 48., false);
        scene.begin_blur(crate::BlurCapture {
            animation_id: None,
            layer_id: None,
            bounds: panel.bounds,
            content_mask: panel.content_mask,
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
        });
        scene.insert_primitive(panel);
        scene.insert_primitive(quad(8., 8., true));
        scene.end_blur();
        assert!(
            should_retain_local_animation(&scene, SIZE, &[running(1)]),
            "a child's local motion must not discard its parent's static pixels"
        );
    }

    #[test]
    fn packed_visual_sample_progress_one_does_not_mean_motion_completed() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        scene.insert_primitive(quad(64., 48., false));
        scene.insert_primitive(quad(8., 8., true));
        let pose = [0., 0., 1., 0.5];
        let value = crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: crate::TransitionProperty::VisualState,
            progress: 1.,
            from: pose,
            to: pose,
        };
        assert!(should_retain_local_animation(&scene, SIZE, &[value]));
        let identity = [0., 0., 1., 1.];
        assert!(!should_retain_local_animation(
            &scene,
            SIZE,
            &[crate::SceneAnimationValue {
                from: identity,
                to: identity,
                ..value
            }]
        ));
    }

    #[test]
    fn captured_owner_and_child_motion_select_local_color_retention() {
        for animated_owner in [true, false] {
            let mut scene = crate::Scene::default();
            scene.insert_primitive(quad(64., 64., false));
            scene.insert_primitive(quad(64., 64., false));
            begin_capture(&mut scene, animated_owner);
            scene.insert_primitive(quad(8., 8., !animated_owner));
            scene.end_blur();
            assert!(should_retain_local_animation(&scene, SIZE, &[running(1)]));
        }
    }

    #[test]
    fn retained_capture_children_do_not_count_as_repeated_page_raster_work() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        scene.insert_primitive(quad(8., 8., true));
        begin_capture(&mut scene, false);
        for _ in 0..128 {
            scene.insert_primitive(quad(8., 8., false));
        }
        scene.end_blur();
        assert!(!should_retain_local_animation(&scene, SIZE, &[running(1)]));
    }

    #[test]
    fn settled_large_panel_does_not_block_a_running_small_popup() {
        for animated_owner in [true, false] {
            let mut scene = crate::Scene::default();
            scene.insert_primitive(quad(64., 64., false));
            scene.insert_primitive(quad(64., 64., false));
            let panel = quad(64., 48., !animated_owner);
            scene.begin_blur(crate::BlurCapture {
                animation_id: animated_owner.then_some(crate::SceneAnimationId(1)),
                layer_id: None,
                bounds: panel.bounds,
                content_mask: panel.content_mask,
                radius: crate::ScaledPixels(0.),
                opacity: 1.,
            });
            scene.insert_primitive(panel);
            scene.end_blur();
            let mut popup = quad(8., 8., true);
            popup.animation_id = Some(crate::SceneAnimationId(2));
            scene.insert_primitive(popup);
            let completed = crate::SceneAnimationValue {
                animation_id: crate::SceneAnimationId(1),
                property: crate::TransitionProperty::Translation,
                progress: 1.,
                from: [10., 0., 0., 0.],
                to: [0.; 4],
            };
            assert!(should_retain_local_animation(
                &scene,
                SIZE,
                &[completed, running(2)]
            ));
            for progress in [0.9, 1.1] {
                assert!(!should_retain_local_animation(
                    &scene,
                    SIZE,
                    &[
                        crate::SceneAnimationValue {
                            progress,
                            ..completed
                        },
                        running(2)
                    ]
                ));
            }
        }
    }

    #[test]
    fn local_animation_retains_static_content_on_both_sides_of_painter_order() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        scene.insert_primitive(quad(8., 8., true));
        scene.insert_primitive(quad(64., 64., false));
        assert!(should_retain_local_animation(&scene, SIZE, &[running(1)]));
    }

    #[test]
    fn cheap_pages_and_large_animated_surfaces_keep_direct_rendering() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        scene.insert_primitive(quad(8., 8., true));
        assert!(!should_retain_local_animation(&scene, SIZE, &[running(1)]));
        scene.insert_primitive(quad(64., 64., false));
        assert!(should_retain_local_animation(&scene, SIZE, &[running(1)]));
        scene.insert_primitive(quad(64., 64., true));
        assert!(!should_retain_local_animation(&scene, SIZE, &[running(1)]));
    }

    #[test]
    fn static_pages_and_clipped_static_work_do_not_allocate_an_animation_cache() {
        let mut scene = crate::Scene::default();
        scene.insert_primitive(quad(64., 64., false));
        scene.insert_primitive(quad(64., 64., false));
        assert!(!should_retain_local_animation(&scene, SIZE, &[running(1)]));
        scene.insert_primitive(quad(8., 8., true));
        scene.paint_operations.edit(0..1, |operation| {
            let crate::scene::PaintOperation::Primitive(crate::Primitive::Quad(first)) = operation else {
                panic!("first quad");
            };
            first.content_mask.bounds.size.width = crate::ScaledPixels(1.);
        });
        assert!(!should_retain_local_animation(&scene, SIZE, &[running(1)]));
    }
}
