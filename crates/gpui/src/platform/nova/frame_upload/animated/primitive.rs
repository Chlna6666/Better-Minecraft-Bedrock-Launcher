use super::values::{ResolvedAnimationValue, ResolvedAnimationValues, resolve_animation_values};
use super::*;
use crate::{Primitive, SceneAnimationId, SceneAnimationValue, TransitionProperty};

pub(super) const BLUR_SOURCE_BOUNDS_OFFSET: usize = 16;
pub(super) const BLUR_ROTATION_METADATA_OFFSET: usize = 80;
#[cfg(test)]
pub(super) const BLUR_DISPLAY_BOUNDS_OFFSET: usize = 96;
pub(super) const BLUR_COMPOSITE_KIND_OFFSET: usize = 132;
pub(super) const ROTATED_COMPOSITE_KIND: u32 = 2;
/// Metadata for one packed animated primitive record.
///
/// The bytes themselves live in FrameUpload's shared staging buffer and final packed primitive
/// buffers. Keeping only the fixed record length here avoids one heap allocation per animated
/// primitive while preserving the existing renderer range API.
#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct AnimatedByteMetadata {
    len: usize,
}

impl AnimatedByteMetadata {
    #[inline]
    const fn new(kind: AnimatedPrimitiveKind) -> Self {
        Self { len: kind.stride() }
    }

    #[inline]
    pub(in crate::platform::nova) const fn len(self) -> usize {
        self.len
    }
}

/// A retained primitive and its small, independently uploadable animated range.
/// Animation ownership is stored directly so GPU promotion never needs a parallel packed binding
/// stream or a second parse to recover `(animation_id, kind, index)`.
pub(in crate::platform::nova) struct AnimatedUpload {
    pub(in crate::platform::nova) animation_id: SceneAnimationId,
    pub(in crate::platform::nova) kind: AnimatedPrimitiveKind,
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) bytes: AnimatedByteMetadata,
    primitive: Primitive,
}

#[derive(Clone, Copy)]
pub(super) struct BackdropBlurAnimationSample {
    pub(super) index: u32,
    pub(super) animation_id: Option<SceneAnimationId>,
    pub(super) order: u32,
    pub(super) base_bounds: crate::Bounds<crate::ScaledPixels>,
    pub(super) base_mask_bounds: crate::Bounds<crate::ScaledPixels>,
    pub(super) sampled_bounds: crate::Bounds<crate::ScaledPixels>,
    pub(super) sampled_mask_bounds: crate::Bounds<crate::ScaledPixels>,
    pub(super) base_radius: crate::ScaledPixels,
    pub(super) sampled_radius: crate::ScaledPixels,
}

#[derive(Clone, Copy)]
pub(super) struct AnimatedPrimitiveSample {
    pub(super) visual_bounds: crate::Bounds<crate::ScaledPixels>,
    pub(super) backdrop_blur: Option<BackdropBlurAnimationSample>,
    pub(super) filter_parameters_changed: bool,
}

impl BackdropBlurAnimationSample {
    pub(super) fn can_use_base_filter(self) -> bool {
        self.base_radius == self.sampled_radius
            && bounds_contains(self.base_bounds, self.sampled_bounds)
            && bounds_contains(self.base_mask_bounds, self.sampled_mask_bounds)
    }

    pub(super) fn base_source_region(self) -> crate::Bounds<crate::ScaledPixels> {
        let sigma = self.base_radius.0.abs();
        let support = if sigma.is_finite() && sigma > 0.0 {
            crate::ScaledPixels(sigma * 3.0 + 0.5)
        } else {
            crate::ScaledPixels(0.0)
        };
        self.base_bounds
            .intersect(&self.base_mask_bounds)
            .dilate(support)
    }
}

impl AnimatedUpload {
    pub(in crate::platform::nova::frame_upload) fn new(
        primitive: Primitive,
        kind: AnimatedPrimitiveKind,
        index: u32,
    ) -> Self {
        let animation_id = primitive
            .animation_id()
            .expect("animated upload requires animation ownership");
        Self {
            animation_id,
            primitive,
            kind,
            index,
            bytes: AnimatedByteMetadata::new(kind),
        }
    }

    pub(in crate::platform::nova) fn offset(&self) -> u64 {
        u64::from(self.index) * self.kind.stride() as u64
    }

    #[cfg(test)]
    pub(super) fn sample(
        &self,
        values: &[SceneAnimationValue],
        size: DrawableSize,
        bytes: &mut Vec<u8>,
    ) -> Option<BackdropBlurAnimationSample> {
        let resolved = resolve_animation_values(values);
        self.sample_resolved(&resolved, size, bytes).backdrop_blur
    }

    pub(super) fn sample_resolved(
        &self,
        values: &ResolvedAnimationValues,
        size: DrawableSize,
        bytes: &mut Vec<u8>,
    ) -> AnimatedPrimitiveSample {
        let mut primitive = self.primitive.clone();
        let resolved_value = values.get(&self.animation_id).copied();
        let filter_parameters_changed = resolved_value.is_some_and(|value| {
            value.property == TransitionProperty::FilterBlur
                && matches!(primitive, Primitive::BackdropBlur(_) | Primitive::Blur(_))
        });
        let composite_rotation = resolved_value.filter(|value| {
            value.property == TransitionProperty::Rotation
                && matches!(primitive, Primitive::Blur(_))
        });
        if let Some(value) = resolved_value
            && composite_rotation.is_none()
        {
            apply_resolved_value(&mut primitive, value);
        }
        let visual_bounds = composite_rotation
            .map(|value| {
                rotated_bounds(
                    primitive.visual_bounds(),
                    value.sampled[0],
                    crate::point(
                        crate::ScaledPixels(value.sampled[1]),
                        crate::ScaledPixels(value.sampled[2]),
                    ),
                )
            })
            .unwrap_or_else(|| primitive.visual_bounds());
        let backdrop_blur = match (&self.primitive, &primitive) {
            (Primitive::BackdropBlur(base), Primitive::BackdropBlur(sampled)) => {
                Some(BackdropBlurAnimationSample {
                    index: self.index,
                    animation_id: base.animation_id,
                    order: base.order,
                    base_bounds: base.bounds,
                    base_mask_bounds: base.content_mask.bounds,
                    sampled_bounds: sampled.bounds,
                    sampled_mask_bounds: sampled.content_mask.bounds,
                    base_radius: base.radius,
                    sampled_radius: sampled.radius,
                })
            }
            _ => None,
        };
        bytes.clear();
        match primitive {
            Primitive::Quad(quad) => write_quad(bytes, &quad),
            Primitive::Shadow(shadow) => write_shadow(bytes, &shadow),
            Primitive::MonochromeSprite(sprite) => write_monochrome_sprite(bytes, &sprite),
            Primitive::PolychromeSprite(sprite) => write_polychrome_sprite(bytes, &sprite),
            Primitive::BackdropBlur(blur) => write_backdrop_blur(bytes, &blur, size),
            Primitive::Blur(blur) => {
                write_paint_blur(bytes, &blur, size);
                if let Primitive::Blur(base) = &self.primitive {
                    write_packed_bounds_at(bytes, BLUR_SOURCE_BOUNDS_OFFSET, base.bounds);
                }
                if let Some(rotation) = composite_rotation {
                    write_rotation_composite_metadata(bytes, rotation);
                }
            }
            _ => {}
        }
        debug_assert_eq!(bytes.len(), self.bytes.len());
        AnimatedPrimitiveSample {
            visual_bounds,
            backdrop_blur,
            filter_parameters_changed,
        }
    }

    pub(super) fn animation_id(&self) -> Option<SceneAnimationId> {
        Some(self.animation_id)
    }

    pub(super) fn order(&self) -> u32 {
        self.primitive.order()
    }

    pub(in crate::platform::nova) fn base_backdrop_blur(
        &self,
    ) -> Option<&crate::PaintBackdropBlur> {
        match &self.primitive {
            Primitive::BackdropBlur(blur) => Some(blur),
            _ => None,
        }
    }

    pub(in crate::platform::nova) fn base_paint_blur(&self) -> Option<&crate::PaintBlur> {
        match &self.primitive {
            Primitive::Blur(blur) => Some(blur),
            _ => None,
        }
    }
}

fn write_packed_bounds_at(
    bytes: &mut [u8],
    offset: usize,
    bounds: crate::Bounds<crate::ScaledPixels>,
) {
    for (field_offset, value) in [
        (0usize, bounds.origin.x.0),
        (4, bounds.origin.y.0),
        (8, bounds.size.width.0),
        (12, bounds.size.height.0),
    ] {
        bytes[offset + field_offset..offset + field_offset + 4]
            .copy_from_slice(&value.to_ne_bytes());
    }
}

fn write_rotation_composite_metadata(bytes: &mut [u8], value: ResolvedAnimationValue) {
    for (index, component) in [value.sampled[0], value.sampled[1], value.sampled[2], 0.0]
        .into_iter()
        .enumerate()
    {
        let offset = BLUR_ROTATION_METADATA_OFFSET + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&component.to_ne_bytes());
    }
    bytes[BLUR_COMPOSITE_KIND_OFFSET..BLUR_COMPOSITE_KIND_OFFSET + 4]
        .copy_from_slice(&ROTATED_COMPOSITE_KIND.to_ne_bytes());
}

#[cfg(test)]
pub(super) fn read_packed_bounds_at(bytes: &[u8], offset: usize) -> [f32; 4] {
    std::array::from_fn(|index| {
        let start = offset + index * 4;
        f32::from_ne_bytes(bytes[start..start + 4].try_into().unwrap())
    })
}

pub(super) fn bounds_contains(
    outer: crate::Bounds<crate::ScaledPixels>,
    inner: crate::Bounds<crate::ScaledPixels>,
) -> bool {
    inner.left() >= outer.left()
        && inner.top() >= outer.top()
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn rotated_bounds(
    bounds: crate::Bounds<crate::ScaledPixels>,
    angle: f32,
    origin: crate::Point<crate::ScaledPixels>,
) -> crate::Bounds<crate::ScaledPixels> {
    if !angle.is_finite() || !origin.x.0.is_finite() || !origin.y.0.is_finite() {
        return bounds;
    }
    let (sin, cos) = angle.sin_cos();
    let rotate = |point: crate::Point<crate::ScaledPixels>| {
        let x = point.x.0 - origin.x.0;
        let y = point.y.0 - origin.y.0;
        crate::point(
            crate::ScaledPixels(origin.x.0 + x * cos - y * sin),
            crate::ScaledPixels(origin.y.0 + x * sin + y * cos),
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
    crate::Bounds::new(
        crate::point(min_x, min_y),
        crate::size(max_x - min_x, max_y - min_y),
    )
}

#[cfg(test)]
pub(super) fn apply_value(primitive: &mut Primitive, value: &SceneAnimationValue) {
    apply_resolved_value(primitive, ResolvedAnimationValue::new(value));
}

fn apply_resolved_value(primitive: &mut Primitive, value: ResolvedAnimationValue) {
    let sampled = value.sampled;
    match value.property {
        TransitionProperty::Opacity => apply_opacity(primitive, sampled[0].clamp(0.0, 1.0)),
        TransitionProperty::Translation => {
            let translation = crate::point(
                crate::ScaledPixels(sampled[0]),
                crate::ScaledPixels(sampled[1]),
            );
            match primitive {
                Primitive::Quad(quad) => quad.bounds.origin += translation,
                Primitive::Shadow(shadow) => shadow.bounds.origin += translation,
                Primitive::MonochromeSprite(sprite) => sprite.bounds.origin += translation,
                Primitive::PolychromeSprite(sprite) => sprite.bounds.origin += translation,
                Primitive::BackdropBlur(blur) => blur.bounds.origin += translation,
                Primitive::Blur(blur) => {
                    blur.bounds.origin += translation;
                    blur.content_mask.bounds.origin += translation;
                    blur.content_mask.corner_bounds.origin += translation;
                }
                _ => {}
            }
            if sampled[3] > 0.5 {
                apply_opacity(primitive, sampled[2].clamp(0.0, 1.0));
            }
        }
        TransitionProperty::Rotation => {}
        TransitionProperty::FilterBlur => {
            let radius = if sampled[0].is_finite() {
                crate::ScaledPixels(sampled[0].max(0.0))
            } else {
                crate::ScaledPixels(0.0)
            };
            match primitive {
                Primitive::BackdropBlur(blur) => blur.radius = radius,
                Primitive::Blur(blur) => blur.radius = radius,
                _ => {}
            }
        }
        TransitionProperty::ClipReveal => {
            apply_clip_reveal(primitive, sampled[0], sampled[1], sampled[2], sampled[3])
        }
        TransitionProperty::Scale => apply_scale(primitive, sampled[0], None),
        TransitionProperty::Transform => {
            apply_opacity(primitive, sampled[1].clamp(0.0, 1.0));
            apply_scale(
                primitive,
                sampled[0],
                Some(crate::point(
                    crate::ScaledPixels(sampled[2]),
                    crate::ScaledPixels(sampled[3]),
                )),
            );
        }
        TransitionProperty::VisualState => {
            apply_scale(primitive, sampled[2], None);
            apply_resolved_value(
                primitive,
                ResolvedAnimationValue {
                    property: TransitionProperty::Translation,
                    sampled: [sampled[0], sampled[1], 0.0, 0.0],
                },
            );
            apply_opacity(primitive, sampled[3].clamp(0.0, 1.0));
        }
        TransitionProperty::HorizontalEdges => {
            let bounds = match primitive {
                Primitive::Quad(value) => &mut value.bounds,
                Primitive::Shadow(value) => &mut value.bounds,
                _ => return,
            };
            bounds.origin.x += crate::ScaledPixels(sampled[0].min(sampled[1]));
            bounds.size.width += crate::ScaledPixels((sampled[0] - sampled[1]).abs());
        }
        _ => {}
    }
}

fn apply_clip_reveal(primitive: &mut Primitive, left: f32, right: f32, top: f32, bottom: f32) {
    if !left.is_finite() || !right.is_finite() || !top.is_finite() || !bottom.is_finite() {
        return;
    }
    let mask = match primitive {
        Primitive::Quad(value) => &mut value.content_mask,
        Primitive::Shadow(value) => &mut value.content_mask,
        Primitive::MonochromeSprite(value) => &mut value.content_mask,
        Primitive::PolychromeSprite(value) => &mut value.content_mask,
        Primitive::BackdropBlur(value) => &mut value.content_mask,
        Primitive::Blur(value) => &mut value.content_mask,
        _ => return,
    };
    let clip_left = crate::ScaledPixels(left.min(right));
    let clip_right = crate::ScaledPixels(left.max(right));
    let clip_top = crate::ScaledPixels(top.min(bottom));
    let clip_bottom = crate::ScaledPixels(top.max(bottom));
    let clipped_left = mask.bounds.left().max(clip_left);
    let clipped_right = mask.bounds.right().min(clip_right);
    let clipped_top = mask.bounds.top().max(clip_top);
    let clipped_bottom = mask.bounds.bottom().min(clip_bottom);
    mask.bounds.origin.x = clipped_left;
    mask.bounds.origin.y = clipped_top;
    mask.bounds.size.width = (clipped_right - clipped_left).max(crate::ScaledPixels(0.0));
    mask.bounds.size.height = (clipped_bottom - clipped_top).max(crate::ScaledPixels(0.0));
}

fn apply_scale(
    primitive: &mut Primitive,
    scale: f32,
    origin: Option<crate::Point<crate::ScaledPixels>>,
) {
    let scale = if scale.is_finite() {
        scale.max(0.0)
    } else {
        1.0
    };
    let bounds = *primitive.bounds();
    let origin = origin.unwrap_or_else(|| bounds.center());
    let scale_bounds = |bounds: crate::Bounds<crate::ScaledPixels>| crate::Bounds {
        origin: origin + (bounds.origin - origin) * scale,
        size: bounds.size.map(|value| value * scale),
    };
    let scale_mask = |mask: &mut crate::ContentMask<crate::ScaledPixels>| {
        mask.bounds = scale_bounds(mask.bounds);
        mask.corner_bounds = scale_bounds(mask.corner_bounds);
        mask.corner_radii = mask.corner_radii.map(|value| *value * scale);
    };

    match primitive {
        Primitive::Quad(quad) => {
            quad.bounds = scale_bounds(quad.bounds);
            scale_mask(&mut quad.content_mask);
            quad.corner_radii = quad.corner_radii.map(|value| *value * scale);
            quad.border_widths = quad.border_widths.map(|value| *value * scale);
        }
        Primitive::Shadow(shadow) => {
            shadow.bounds = scale_bounds(shadow.bounds);
            scale_mask(&mut shadow.content_mask);
            shadow.corner_radii = shadow.corner_radii.map(|value| *value * scale);
            shadow.blur_radius *= scale;
        }
        Primitive::MonochromeSprite(sprite) => {
            sprite.bounds = scale_bounds(sprite.bounds);
            scale_mask(&mut sprite.content_mask);
        }
        Primitive::PolychromeSprite(sprite) => {
            sprite.bounds = scale_bounds(sprite.bounds);
            scale_mask(&mut sprite.content_mask);
            sprite.corner_radii = sprite.corner_radii.map(|value| *value * scale);
        }
        Primitive::BackdropBlur(blur) => {
            blur.bounds = scale_bounds(blur.bounds);
            scale_mask(&mut blur.content_mask);
            blur.corner_radii = blur.corner_radii.map(|value| *value * scale);
        }
        Primitive::Blur(blur) => {
            blur.bounds = scale_bounds(blur.bounds);
            scale_mask(&mut blur.content_mask);
        }
        _ => {}
    }
}

fn apply_opacity(primitive: &mut Primitive, opacity: f32) {
    match primitive {
        Primitive::Quad(quad) => {
            quad.background = quad.background.opacity(opacity);
            quad.border_color = quad.border_color.opacity(opacity);
        }
        Primitive::Shadow(shadow) => shadow.color = shadow.color.opacity(opacity),
        Primitive::MonochromeSprite(sprite) => sprite.color = sprite.color.opacity(opacity),
        Primitive::PolychromeSprite(sprite) => sprite.opacity *= opacity,
        Primitive::BackdropBlur(blur) => blur.opacity *= opacity,
        Primitive::Blur(blur) => blur.opacity *= opacity,
        _ => {}
    }
}
