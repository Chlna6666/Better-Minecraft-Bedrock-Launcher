use super::*;
const ENGINE_ANIMATION_ID_START: u32 = 1 << 31;
pub(crate) fn animation_sample_is_identity(value: &SceneAnimationValue) -> bool {
    let progress = if value.progress.is_finite() {
        value.progress
    } else {
        0.
    };
    let sampled: [f32; 4] = std::array::from_fn(|index| {
        value.from[index] + (value.to[index] - value.from[index]) * progress
    });
    match value.property {
        TransitionProperty::Opacity => sampled[0].clamp(0., 1.) == 1.,
        TransitionProperty::Translation => {
            sampled[0] == 0.
                && sampled[1] == 0.
                && (sampled[3] <= 0.5 || sampled[2].clamp(0., 1.) == 1.)
        }
        TransitionProperty::Scale => sampled[0] == 1.,
        TransitionProperty::Transform => sampled[0] == 1. && sampled[1].clamp(0., 1.) == 1.,
        TransitionProperty::Rotation => sampled[0] == 0.,
        TransitionProperty::VisualState => {
            sampled[0] == 0.
                && sampled[1] == 0.
                && sampled[2] == 1.
                && sampled[3].clamp(0., 1.) == 1.
        }
        _ => false,
    }
}

/// A zero-radius capture with an identity coordinate mapping can propagate
/// child damage without repainting its entire output. Other mappings and
/// filters keep the conservative full-capture damage path.
pub(crate) fn capture_preserves_source_coordinates<'a>(
    radius: ScaledPixels,
    animation_id: Option<SceneAnimationId>,
    values: impl IntoIterator<Item = &'a SceneAnimationValue>,
) -> bool {
    if radius.0 != 0. {
        return false;
    }
    let Some(animation_id) = animation_id else {
        return true;
    };
    let mut seen = smallvec::SmallVec::<[TransitionProperty; 4]>::new();
    for value in values
        .into_iter()
        .filter(|value| value.animation_id == animation_id)
    {
        if seen.contains(&value.property) {
            continue;
        }
        seen.push(value.property);
        let sampled: [f32; 4] = std::array::from_fn(|index| {
            value.from[index] + (value.to[index] - value.from[index]) * value.progress
        });
        let identity = match value.property {
            TransitionProperty::Opacity | TransitionProperty::ClipReveal => true,
            TransitionProperty::Translation => sampled[0] == 0. && sampled[1] == 0.,
            TransitionProperty::Scale | TransitionProperty::Transform => sampled[0] == 1.,
            TransitionProperty::Rotation => sampled[0] == 0.,
            TransitionProperty::VisualState => {
                sampled[0] == 0. && sampled[1] == 0. && sampled[2] == 1.
            }
            _ => false,
        };
        if !identity {
            return false;
        }
    }
    // Missing values disable the indexed GPU slot and leave the base
    // geometry unchanged; CPU animation fallback has the same semantics.
    true
}

pub(crate) fn animation_value_for<'a>(
    scene: &'a Scene,
    animation_id: SceneAnimationId,
    presentation_values: &'a [SceneAnimationValue],
) -> Option<&'a SceneAnimationValue> {
    if animation_id.0 >= ENGINE_ANIMATION_ID_START {
        presentation_values
            .iter()
            .find(|value| value.animation_id == animation_id)
    } else {
        scene.animation_value(animation_id)
    }
}

pub(crate) fn animation_value_changed(
    scene: &Scene,
    previous_scene: &Scene,
    animation_id: Option<SceneAnimationId>,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
) -> bool {
    let Some(animation_id) = animation_id else {
        return false;
    };
    animation_value_for(scene, animation_id, presentation_values)
        != animation_value_for(previous_scene, animation_id, previous_presentation_values)
}

pub(crate) fn animation_swept_bounds(
    scene: &Scene,
    previous_scene: &Scene,
    primitive: &Primitive,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
) -> Bounds<ScaledPixels> {
    let Some(animation_id) = primitive.animation_id() else {
        return primitive.visual_bounds();
    };
    let current = animation_value_for(scene, animation_id, presentation_values);
    let previous = animation_value_for(previous_scene, animation_id, previous_presentation_values);
    animation_sampled_bounds(primitive, current)
        .union(&animation_sampled_bounds(primitive, previous))
}

pub(crate) fn blur_capture_animation_swept_bounds(
    scene: &Scene,
    previous_scene: &Scene,
    blur: &BlurCapture,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
) -> Bounds<ScaledPixels> {
    let Some(animation_id) = blur.animation_id else {
        return blur_capture_visual_bounds(blur);
    };
    let current = animation_value_for(scene, animation_id, presentation_values);
    let previous = animation_value_for(previous_scene, animation_id, previous_presentation_values);
    animation_sampled_blur_capture_bounds(blur, current)
        .union(&animation_sampled_blur_capture_bounds(blur, previous))
}

pub(crate) fn blur_capture_visual_bounds(blur: &BlurCapture) -> Bounds<ScaledPixels> {
    blur.bounds
        .dilate(blur_influence_radius(blur.radius))
        .intersect(&blur.content_mask.bounds)
}

pub(crate) fn animation_sampled_blur_capture_bounds(
    blur: &BlurCapture,
    value: Option<&SceneAnimationValue>,
) -> Bounds<ScaledPixels> {
    let bounds = blur_capture_visual_bounds(blur);
    let Some(value) = value else {
        return bounds;
    };
    let sampled = sampled_animation_components(value);
    match value.property {
        TransitionProperty::Translation => Bounds {
            origin: bounds.origin
                + crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1])),
            size: bounds.size,
        },
        TransitionProperty::Scale => scaled_animation_bounds(bounds, sampled[0], bounds.center()),
        TransitionProperty::Transform => scaled_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[2]), ScaledPixels(sampled[3])),
        ),
        TransitionProperty::Rotation => rotated_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[1]), ScaledPixels(sampled[2])),
        ),
        TransitionProperty::VisualState => {
            let mut bounds = scaled_animation_bounds(bounds, sampled[2], bounds.center());
            bounds.origin += crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1]));
            bounds
        }
        // Opacity changes pixels but not geometry, so the whole composite output is source damage
        // for a later backdrop barrier.
        TransitionProperty::Opacity => bounds,
        _ => bounds,
    }
}

pub(crate) fn animation_sampled_bounds(
    primitive: &Primitive,
    value: Option<&SceneAnimationValue>,
) -> Bounds<ScaledPixels> {
    let Some(value) = value else {
        return primitive.visual_bounds();
    };
    let sampled = sampled_animation_components(value);
    if matches!(
        value.property,
        TransitionProperty::Translation | TransitionProperty::VisualState
    ) && matches!(
        primitive,
        Primitive::Quad(_)
            | Primitive::Shadow(_)
            | Primitive::MonochromeSprite(_)
            | Primitive::PolychromeSprite(_)
            | Primitive::BackdropBlur(_)
    ) {
        let mut translated = primitive.unclipped_visual_bounds();
        let mut mask = primitive.content_mask().bounds;
        if value.property == TransitionProperty::VisualState {
            let center = primitive.bounds().center();
            translated = scaled_animation_bounds(translated, sampled[2], center);
            mask = scaled_animation_bounds(mask, sampled[2], center);
        }
        translated.origin += crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1]));
        return translated.intersect(&mask);
    }
    let bounds = primitive.visual_bounds();
    match value.property {
        TransitionProperty::Translation
            if matches!(
                primitive,
                Primitive::Quad(_)
                    | Primitive::Shadow(_)
                    | Primitive::MonochromeSprite(_)
                    | Primitive::PolychromeSprite(_)
                    | Primitive::BackdropBlur(_)
                    | Primitive::Blur(_)
            ) =>
        {
            Bounds {
                origin: bounds.origin
                    + crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1])),
                size: bounds.size,
            }
        }
        TransitionProperty::Scale => {
            scaled_animation_bounds(bounds, sampled[0], primitive.bounds().center())
        }
        TransitionProperty::Transform => scaled_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[2]), ScaledPixels(sampled[3])),
        ),
        TransitionProperty::Rotation => rotated_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[1]), ScaledPixels(sampled[2])),
        ),
        TransitionProperty::VisualState => {
            let mut bounds =
                scaled_animation_bounds(bounds, sampled[2], primitive.bounds().center());
            bounds.origin += crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1]));
            bounds
        }
        _ => bounds,
    }
}

/// Exact damage of an axis-aligned ClipReveal when both samples share a
/// fixed edge. The native GPU clip is [left, right, top, bottom]. Revealed
/// pixels and concealed pixels form only the band between the moving edges;
/// unchanged interior texels do not need another Capture Source raster pass.
///
/// Return None for retargeted/disjoint/invalid rectangles so callers keep
/// the full swept output. An empty strip means the sampled clip did not move.
pub(crate) fn clip_reveal_changed_strip(
    before: [f32; 4],
    after: [f32; 4],
    output_bounds: Bounds<ScaledPixels>,
) -> Option<Bounds<ScaledPixels>> {
    if !before.into_iter().chain(after).all(f32::is_finite) {
        return None;
    }
    let [bl, br, bt, bb] = before;
    let [al, ar, at, ab] = after;
    if bl > br || bt > bb || al > ar || at > ab {
        return None;
    }
    let rect = if bl == al && br == ar && bb == ab {
        // The top edge moves; the bottom and horizontal coverage stay fixed.
        (bl, bt.min(at), br - bl, (bt - at).abs())
    } else if bl == al && br == ar && bt == at {
        // The bottom edge moves.
        (bl, bb.min(ab), br - bl, (bb - ab).abs())
    } else if bt == at && bb == ab && br == ar {
        // The left edge moves.
        (bl.min(al), bt, (bl - al).abs(), bb - bt)
    } else if bt == at && bb == ab && bl == al {
        // The right edge moves.
        (br.min(ar), bt, (br - ar).abs(), bb - bt)
    } else {
        return None;
    };
    Some(
        Bounds {
            origin: crate::point(ScaledPixels(rect.0), ScaledPixels(rect.1)),
            size: crate::size(ScaledPixels(rect.2), ScaledPixels(rect.3)),
        }
        .intersect(&output_bounds),
    )
}

pub(crate) fn sampled_animation_components(value: &SceneAnimationValue) -> [f32; 4] {
    let progress = if value.progress.is_finite() {
        value.progress
    } else {
        0.0
    };
    std::array::from_fn(|index| {
        value.from[index] + (value.to[index] - value.from[index]) * progress
    })
}

pub(crate) fn scaled_animation_bounds(
    bounds: Bounds<ScaledPixels>,
    scale: f32,
    origin: crate::Point<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    let scale = if scale.is_finite() {
        scale.max(0.0)
    } else {
        1.0
    };
    Bounds {
        origin: origin + (bounds.origin - origin) * scale,
        size: bounds.size.map(|value| value * scale),
    }
}

pub(crate) fn rotated_animation_bounds(
    bounds: Bounds<ScaledPixels>,
    angle: f32,
    origin: crate::Point<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    if !angle.is_finite() || !origin.x.0.is_finite() || !origin.y.0.is_finite() {
        return bounds;
    }
    let (sin, cos) = angle.sin_cos();
    let rotate = |point: crate::Point<ScaledPixels>| {
        let x = point.x.0 - origin.x.0;
        let y = point.y.0 - origin.y.0;
        crate::point(
            ScaledPixels(origin.x.0 + x * cos - y * sin),
            ScaledPixels(origin.y.0 + x * sin + y * cos),
        )
    };
    let corners = [
        rotate(crate::point(bounds.left(), bounds.top())),
        rotate(crate::point(bounds.right(), bounds.top())),
        rotate(crate::point(bounds.left(), bounds.bottom())),
        rotate(crate::point(bounds.right(), bounds.bottom())),
    ];
    let min_x = corners
        .iter()
        .map(|point| point.x)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or(bounds.left());
    let max_x = corners
        .iter()
        .map(|point| point.x)
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or(bounds.right());
    let min_y = corners
        .iter()
        .map(|point| point.y)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or(bounds.top());
    let max_y = corners
        .iter()
        .map(|point| point.y)
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or(bounds.bottom());
    Bounds::new(
        crate::point(min_x, min_y),
        crate::size(max_x - min_x, max_y - min_y),
    )
    .dilate(ScaledPixels(1.0))
}
