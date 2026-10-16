use super::*;
pub(crate) fn for_each_changed_bounds(
    scene: &Scene,
    current_range: Range<usize>,
    previous: &Scene,
    previous_range: Range<usize>,
    mut visit: impl FnMut(Bounds<ScaledPixels>),
) -> bool {
    visit_changed_bounds(scene, current_range, previous, previous_range, &mut visit)
}

// Erase the callback type at the recursive Capture boundary. Each nested clipped
// callback otherwise creates another monomorphization, without a finite type limit.
fn visit_changed_bounds(
    scene: &Scene,
    current_range: Range<usize>,
    previous: &Scene,
    previous_range: Range<usize>,
    mut visit: &mut dyn FnMut(Bounds<ScaledPixels>),
) -> bool {
    let Some(current) = scene.paint_operations.get_range(current_range.clone()) else {
        return false;
    };
    let Some(previous_operations) = previous.paint_operations.get_range(previous_range.clone())
    else {
        return false;
    };

    let prefix_len = current
        .iter()
        .zip(previous_operations.iter())
        .take_while(|(current, previous_operation)| {
            paint_operations_match_for_damage(scene, current, previous, previous_operation)
        })
        .count();
    let max_suffix_len = current
        .len()
        .min(previous_operations.len())
        .saturating_sub(prefix_len);
    let suffix_len = current
        .iter()
        .rev()
        .zip(previous_operations.iter().rev())
        .take(max_suffix_len)
        .take_while(|(current, previous_operation)| {
            paint_operations_match_for_damage(scene, current, previous, previous_operation)
        })
        .count();

    let previous_changed =
        previous_operations.range(prefix_len..previous_operations.len().saturating_sub(suffix_len));
    let current_changed = current.range(prefix_len..current.len().saturating_sub(suffix_len));
    // An unchanged zero-radius capture has identity source coordinates.
    // Its child Scene may differ in one small quad while the enclosing
    // PaintBlur still covers the entire page. Damaging that entire output
    // on every UI commit defeats retained Root/Element raster caching for
    // titlebar pills and dropdown animations on *all* GPU backends.
    //
    // This proof deliberately excludes changed capture geometry, animated
    // transforms, Gaussian support, nested filters and backdrop sources.
    // Only one aligned changed composite is refined; any structural
    // uncertainty falls back to the full old/new output bounds below.
    let refined_identity_capture = match (
        previous_changed.len(),
        previous_changed.get(0),
        current_changed.len(),
        current_changed.get(0),
    ) {
        (
            1,
            Some(PaintOperation::Primitive(Primitive::Blur(old))),
            1,
            Some(PaintOperation::Primitive(Primitive::Blur(new))),
        ) if old.order == new.order
            && old.layer_id == new.layer_id
            && old.animation_id.is_none()
            && new.animation_id.is_none()
            && old.bounds == new.bounds
            && old.content_mask == new.content_mask
            && old.radius.0 == 0.0
            && new.radius.0 == 0.0
            && old.opacity == new.opacity
            && old.content.blurs.is_empty()
            && new.content.blurs.is_empty()
            && !old.content.has_backdrop_blurs()
            && !new.content.has_backdrop_blurs()
            && !crate::paint::requires_full_redraw_fallback(&old.content)
            && !crate::paint::requires_full_redraw_fallback(&new.content) =>
        {
            let output = new.bounds.intersect(&new.content_mask.bounds);
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI identity capture child-damage refinement: full_output_area={} child_operations={} retained_layer={:?}",
                output.size.width.0 * output.size.height.0,
                new.content.len(),
                new.layer_id,
            );
            visit_changed_bounds(
                &new.content,
                0..new.content.len(),
                &old.content,
                0..old.content.len(),
                &mut |bounds| {
                    let local = bounds.intersect(&output);
                    if !local.is_empty() {
                        visit(local);
                    }
                },
            )
        }
        _ => false,
    };
    if !refined_identity_capture {
        for operation in previous_changed.iter() {
            if let Some(bounds) = operation.visual_bounds() {
                visit(bounds);
            }
        }
        for operation in current_changed.iter() {
            if let Some(bounds) = operation.visual_bounds() {
                visit(bounds);
            }
        }
    }

    let current_changed_range =
        current_range.start + prefix_len..current_range.end.saturating_sub(suffix_len);
    crate::paint::for_each_element_blur_damage(scene, current_changed_range, &mut visit);

    let previous_changed_range =
        previous_range.start + prefix_len..previous_range.end.saturating_sub(suffix_len);
    crate::paint::for_each_element_blur_damage(previous, previous_changed_range, &mut visit);
    true
}

pub(crate) fn for_each_element_blur_damage(
    scene: &Scene,
    changed_range: Range<usize>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    if changed_range.start >= changed_range.end {
        return;
    }
    crate::scene::geometry::for_each_capture_group(scene, |group_range, bounds| {
        // Identity captures do not spread source pixels. Their changed primitive
        // bounds (or changed capture descriptor) already describe the output damage.
        // Keep visiting enclosing Gaussian/transformed captures independently.
        if matches!(
            scene.paint_operations.get(group_range.start),
            Some(PaintOperation::StartBlur(capture))
                if capture.radius.0 == 0.0 && capture.animation_id.is_none()
        ) {
            return;
        }
        if group_range.start < changed_range.end && changed_range.start < group_range.end {
            visit(bounds);
        }
    });
}

pub(crate) fn requires_full_redraw_fallback(scene: &Scene) -> bool {
    !scene.surfaces.is_empty()
        || !scene.renderer_extensions.is_empty()
        || scene
            .blurs
            .iter()
            .any(|blur| crate::paint::requires_full_redraw_fallback(&blur.content))
}

pub(crate) fn collect_paint_operation_damage(
    current: crate::scene::OperationRange<'_>,
    previous: crate::scene::OperationRange<'_>,
    region: Bounds<ScaledPixels>,
    mut visit: impl FnMut(Bounds<ScaledPixels>),
) {
    let prefix_len = current
        .iter()
        .zip(previous.iter())
        .take_while(|(current, previous)| current.visually_eq(previous))
        .count();
    let max_suffix_len = current.len().min(previous.len()).saturating_sub(prefix_len);
    let suffix_len = current
        .iter()
        .rev()
        .zip(previous.iter().rev())
        .take(max_suffix_len)
        .take_while(|(current, previous)| current.visually_eq(previous))
        .count();

    let current_changed = current.range(prefix_len..current.len().saturating_sub(suffix_len));
    let previous_changed = previous.range(prefix_len..previous.len().saturating_sub(suffix_len));

    if current_changed.len() == previous_changed.len() {
        for (current_operation, previous_operation) in
            current_changed.iter().zip(previous_changed.iter())
        {
            if visit_opaque_solid_quad_move_damage(
                current_operation,
                previous_operation,
                region,
                &mut visit,
            ) {
                continue;
            }
            visit_operation_damage(current_operation, region, &mut visit);
            visit_operation_damage(previous_operation, region, &mut visit);
        }
        return;
    }

    current_changed
        .iter()
        .chain(previous_changed.iter())
        .for_each(|operation| visit_operation_damage(operation, region, &mut visit));
}

pub(crate) fn visit_operation_damage(
    operation: &PaintOperation,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    let Some(bounds) = operation.visual_bounds() else {
        return;
    };
    let damage = bounds.intersect(&region);
    if !damage.is_empty() {
        visit(damage);
    }
}

/// A moved opaque rectangular fill only changes pixels in the symmetric difference of its old and
/// new rectangles. The overlap is byte-for-byte identical and does not need to invalidate a
/// backdrop source. Keep the predicate deliberately strict; any uncertainty falls back to the
/// ordinary old+new bounds damage path.
pub(crate) fn visit_opaque_solid_quad_move_damage(
    current: &PaintOperation,
    previous: &PaintOperation,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) -> bool {
    let (
        PaintOperation::Primitive(Primitive::Quad(current)),
        PaintOperation::Primitive(Primitive::Quad(previous)),
    ) = (current, previous)
    else {
        return false;
    };

    if !is_solid_quad(current)
        || !is_solid_quad(previous)
        || current.order != previous.order
        || current.animation_id != previous.animation_id
        || current.background != previous.background
        || current.background.solid.a < 1.0
        || current.border_style != previous.border_style
        || current.border_color != previous.border_color
        || current.corner_radii != previous.corner_radii
        || current.border_widths != previous.border_widths
        || current.content_mask != previous.content_mask
    {
        return false;
    }

    let current_bounds = current.bounds.intersect(&current.content_mask.bounds);
    let previous_bounds = previous.bounds.intersect(&previous.content_mask.bounds);
    if current_bounds.is_empty() || previous_bounds.is_empty() {
        return false;
    }

    let overlap = current_bounds.intersect(&previous_bounds);
    if overlap.is_empty() {
        visit_clipped_damage(current_bounds, region, visit);
        visit_clipped_damage(previous_bounds, region, visit);
        return true;
    }

    visit_rect_difference(current_bounds, overlap, region, visit);
    visit_rect_difference(previous_bounds, overlap, region, visit);
    true
}

pub(crate) fn visit_rect_difference(
    bounds: Bounds<ScaledPixels>,
    overlap: Bounds<ScaledPixels>,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    let top = Bounds::new(
        crate::point(bounds.left(), bounds.top()),
        crate::size(bounds.size.width, overlap.top() - bounds.top()),
    );
    let bottom = Bounds::new(
        crate::point(bounds.left(), overlap.bottom()),
        crate::size(bounds.size.width, bounds.bottom() - overlap.bottom()),
    );
    let left = Bounds::new(
        crate::point(bounds.left(), overlap.top()),
        crate::size(overlap.left() - bounds.left(), overlap.size.height),
    );
    let right = Bounds::new(
        crate::point(overlap.right(), overlap.top()),
        crate::size(bounds.right() - overlap.right(), overlap.size.height),
    );

    for damage in [top, bottom, left, right] {
        visit_clipped_damage(damage, region, visit);
    }
}

pub(crate) fn visit_clipped_damage(
    bounds: Bounds<ScaledPixels>,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    if bounds.is_empty() {
        return;
    }
    let damage = bounds.intersect(&region);
    if !damage.is_empty() {
        visit(damage);
    }
}

pub(crate) fn paint_operations_match_for_damage(
    current_scene: &Scene,
    current: &PaintOperation,
    previous_scene: &Scene,
    previous: &PaintOperation,
) -> bool {
    if !current.visually_eq(previous) {
        return false;
    }

    match (current, previous) {
        (
            PaintOperation::Primitive(current_primitive),
            PaintOperation::Primitive(previous_primitive),
        ) => {
            let Some(current_animation_id) = current_primitive.animation_id() else {
                return true;
            };
            let Some(previous_animation_id) = previous_primitive.animation_id() else {
                return false;
            };
            current_scene.animation_value(current_animation_id)
                == previous_scene.animation_value(previous_animation_id)
        }
        (PaintOperation::StartBlur(current_blur), PaintOperation::StartBlur(previous_blur)) => {
            match (current_blur.animation_id, previous_blur.animation_id) {
                (Some(current_animation_id), Some(previous_animation_id)) => {
                    current_scene.animation_value(current_animation_id)
                        == previous_scene.animation_value(previous_animation_id)
                }
                (None, None) => true,
                _ => false,
            }
        }
        _ => true,
    }
}
