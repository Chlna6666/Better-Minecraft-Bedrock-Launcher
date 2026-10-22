use super::*;
/// Resolves visible geometry with the same fixed-mask translation as the GPU.
pub(crate) fn sampled_primitive_bounds<'a>(
    primitive: &Primitive,
    values: impl IntoIterator<Item = &'a SceneAnimationValue>,
) -> Bounds<ScaledPixels> {
    let value = values
        .into_iter()
        .find(|value| Some(value.animation_id) == primitive.animation_id());
    animation_sampled_bounds(primitive, value)
}

/// Compares consumed GPU samples, including Scene-local animation values.
/// Retaining buffers does not imply that the pixels are unchanged.
pub(crate) fn gpu_sample_damage(
    scene: &Scene,
    previous: &[SceneAnimationValue],
    current: &[SceneAnimationValue],
) -> SmallVec<[Bounds<ScaledPixels>; 8]> {
    // Most compositor callbacks have unchanged shader parameters.
    // Avoid scanning all paint operations on those frames.
    if previous == current {
        return SmallVec::new();
    }
    let mut damage = SmallVec::new();
    let mut changed_operations = SmallVec::<[usize; 8]>::new();
    for (index, operation) in scene.paint_operations.iter().enumerate() {
        let animation_id = match operation {
            PaintOperation::Primitive(primitive) => primitive.animation_id(),
            PaintOperation::StartBlur(capture) => capture.animation_id,
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer | PaintOperation::EndBlur => {
                None
            }
        };
        if let Some(animation_id) = animation_id {
            let last = previous.iter().find(|v| v.animation_id == animation_id);
            let now = current.iter().find(|v| v.animation_id == animation_id);
            if previous
                .iter()
                .filter(|value| value.animation_id == animation_id)
                .ne(current
                    .iter()
                    .filter(|value| value.animation_id == animation_id))
                && !(previous
                    .iter()
                    .filter(|value| value.animation_id == animation_id)
                    .all(crate::paint::animation_sample_is_identity)
                    && current
                        .iter()
                        .filter(|value| value.animation_id == animation_id)
                        .all(crate::paint::animation_sample_is_identity))
            {
                // Rectangular ClipReveal changes only the moving-edge strip.
                // Rounded clips also change coverage near the fixed corners
                // when the exposed height falls below twice the radius.
                // Preserve correctness during fast reversals by using the
                // swept output for those masks; this does not repaint the
                // retained capture source or introduce another GPU pass.
                let rounded_reveal = match operation {
                    PaintOperation::Primitive(primitive) => {
                        primitive.content_mask().corner_radii != crate::Corners::default()
                    }
                    PaintOperation::StartBlur(capture) => {
                        capture.content_mask.corner_radii != crate::Corners::default()
                    }
                    _ => false,
                };
                let clip_delta = (!rounded_reveal).then_some(()).and_then(|_| last)
                    .zip(now)
                    .filter(|(before, after)| {
                        before.property == TransitionProperty::ClipReveal
                            && after.property == TransitionProperty::ClipReveal
                    })
                    .and_then(|(before, after)| {
                        let base = match operation {
                            PaintOperation::Primitive(primitive) => primitive.visual_bounds(),
                            PaintOperation::StartBlur(capture) => {
                                blur_capture_visual_bounds(capture)
                            }
                            _ => return None,
                        };
                        clip_reveal_changed_strip(
                            sampled_animation_components(before),
                            sampled_animation_components(after),
                            base,
                        )
                    });
                let bounds = clip_delta.unwrap_or_else(|| match operation {
                    PaintOperation::Primitive(primitive) => {
                        animation_sampled_bounds(primitive, last)
                            .union(&animation_sampled_bounds(primitive, now))
                    }
                    PaintOperation::StartBlur(capture) => {
                        animation_sampled_blur_capture_bounds(capture, last)
                            .union(&animation_sampled_blur_capture_bounds(capture, now))
                    }
                    _ => unreachable!("only primitive and capture operations carry animation ids"),
                });
                if !bounds.is_empty() {
                    damage.push(bounds);
                    changed_operations.push(index);
                }
            }
        }
        if let PaintOperation::Primitive(Primitive::Blur(blur)) = operation {
            let child_damage = crate::paint::gpu_sample_damage(&blur.content, previous, current);
            let local = crate::paint::capture_preserves_source_coordinates(
                blur.radius,
                blur.animation_id,
                previous.iter(),
            ) && crate::paint::capture_preserves_source_coordinates(
                blur.radius,
                blur.animation_id,
                current.iter(),
            );
            for region in child_damage {
                let output = blur.bounds.intersect(&blur.content_mask.bounds);
                let bounds = if local {
                    region.intersect(&output)
                } else {
                    output
                };
                if !bounds.is_empty() {
                    damage.push(bounds);
                    changed_operations.push(index);
                }
            }
        }
    }
    if !changed_operations.is_empty() {
        crate::scene::geometry::for_each_capture_group(scene, |range, bounds| {
            if let PaintOperation::StartBlur(capture) = &scene.paint_operations[range.start]
                && crate::paint::capture_preserves_source_coordinates(
                    capture.radius,
                    capture.animation_id,
                    previous.iter(),
                )
                && crate::paint::capture_preserves_source_coordinates(
                    capture.radius,
                    capture.animation_id,
                    current.iter(),
                )
            {
                return;
            }
            if changed_operations.iter().any(|index| range.contains(index)) && !bounds.is_empty() {
                damage.push(bounds);
            }
        });
    }
    damage
}
