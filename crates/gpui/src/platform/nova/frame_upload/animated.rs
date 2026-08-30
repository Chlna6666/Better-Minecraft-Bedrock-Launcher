use super::*;
use crate::{Primitive, SceneAnimationId, SceneAnimationValue, TransitionProperty};

/// A retained primitive and its small, independently uploadable animated range.
/// Nova's current shaders consume primitive buffers, not the animation metadata.
pub(in crate::platform::nova) struct AnimatedUpload {
    pub(in crate::platform::nova) kind: AnimatedPrimitiveKind,
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) bytes: Vec<u8>,
    primitive: Primitive,
}

#[derive(Clone, Copy)]
struct BackdropBlurAnimationSample {
    index: u32,
    animation_id: Option<SceneAnimationId>,
    order: u32,
    base_bounds: crate::Bounds<crate::ScaledPixels>,
    base_mask_bounds: crate::Bounds<crate::ScaledPixels>,
    sampled_bounds: crate::Bounds<crate::ScaledPixels>,
    sampled_mask_bounds: crate::Bounds<crate::ScaledPixels>,
    radius: crate::ScaledPixels,
}

impl BackdropBlurAnimationSample {
    fn can_use_base_filter(self) -> bool {
        bounds_contains(self.base_bounds, self.sampled_bounds)
            && bounds_contains(self.base_mask_bounds, self.sampled_mask_bounds)
    }

    fn base_source_region(self) -> crate::Bounds<crate::ScaledPixels> {
        let sigma = self.radius.0.abs();
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
    pub(super) fn new(primitive: Primitive, kind: AnimatedPrimitiveKind, index: u32) -> Self {
        Self {
            primitive,
            kind,
            index,
            bytes: Vec::new(),
        }
    }

    pub(in crate::platform::nova) fn offset(&self) -> u64 {
        let stride = match self.kind {
            AnimatedPrimitiveKind::Quad => PACKED_QUAD_BYTES,
            AnimatedPrimitiveKind::Shadow => PACKED_SHADOW_BYTES,
            AnimatedPrimitiveKind::MonochromeSprite => PACKED_MONO_SPRITE_BYTES,
            AnimatedPrimitiveKind::PolychromeSprite => PACKED_POLY_SPRITE_BYTES,
            AnimatedPrimitiveKind::BackdropBlur => PACKED_BACKDROP_BLUR_BYTES,
        };
        u64::from(self.index) * stride as u64
    }

    fn sample(
        &mut self,
        values: &[SceneAnimationValue],
        size: DrawableSize,
    ) -> Option<BackdropBlurAnimationSample> {
        let mut primitive = self.primitive.clone();
        if let Some(value) = values
            .iter()
            .find(|value| Some(value.animation_id) == primitive.animation_id())
        {
            apply_value(&mut primitive, value);
        }
        let blur_sample = match (&self.primitive, &primitive) {
            (Primitive::BackdropBlur(base), Primitive::BackdropBlur(sampled)) => {
                Some(BackdropBlurAnimationSample {
                    index: self.index,
                    animation_id: base.animation_id,
                    order: base.order,
                    base_bounds: base.bounds,
                    base_mask_bounds: base.content_mask.bounds,
                    sampled_bounds: sampled.bounds,
                    sampled_mask_bounds: sampled.content_mask.bounds,
                    radius: base.radius,
                })
            }
            _ => None,
        };
        self.bytes.clear();
        match primitive {
            Primitive::Quad(quad) => write_quad(&mut self.bytes, &quad),
            Primitive::Shadow(shadow) => write_shadow(&mut self.bytes, &shadow),
            Primitive::MonochromeSprite(sprite) => {
                write_monochrome_sprite(&mut self.bytes, &sprite)
            }
            Primitive::PolychromeSprite(sprite) => {
                write_polychrome_sprite(&mut self.bytes, &sprite)
            }
            Primitive::BackdropBlur(blur) => write_backdrop_blur(&mut self.bytes, &blur, size),
            _ => {}
        }
        blur_sample
    }

    fn animation_id(&self) -> Option<SceneAnimationId> {
        self.primitive.animation_id()
    }

    fn order(&self) -> u32 {
        self.primitive.order()
    }

    fn sampled_visual_bounds(
        &self,
        values: &[SceneAnimationValue],
    ) -> crate::Bounds<crate::ScaledPixels> {
        let mut primitive = self.primitive.clone();
        if let Some(value) = values
            .iter()
            .find(|value| Some(value.animation_id) == primitive.animation_id())
        {
            apply_value(&mut primitive, value);
        }
        primitive.visual_bounds()
    }

    pub(in crate::platform::nova) fn base_backdrop_blur(
        &self,
    ) -> Option<&crate::PaintBackdropBlur> {
        match &self.primitive {
            Primitive::BackdropBlur(blur) => Some(blur),
            _ => None,
        }
    }
}

impl FrameUpload {
    pub(in crate::platform::nova) fn sample_animated_primitives(&mut self, size: DrawableSize) {
        let previous_filter_dirty = self.backdrop_blur_filter_dirty_indices.clone();
        self.backdrop_blur_use_base_filter_indices.clear();
        self.backdrop_blur_ignore_animation_damage_indices.clear();
        self.backdrop_blur_passes_dirty_this_frame = false;

        let current_animation_ids: FxHashSet<_> = self
            .sampled_animation_values
            .iter()
            .map(|value| value.animation_id)
            .collect();
        let mut relevant_animation_ids = self.backdrop_blur_previous_animation_ids.clone();
        relevant_animation_ids.extend(current_animation_ids.iter().copied());

        let mut blur_samples = Vec::new();
        for primitive in &mut self.animated_primitives {
            let blur_sample = primitive.sample(&self.sampled_animation_values, size);
            // GPU composite state always receives the sampled primitive. Filter planning does not
            // have to use these same bytes: backdrop blur configs can independently select the
            // retained base geometry for composite-only animations.
            let buffer = match primitive.kind {
                AnimatedPrimitiveKind::Quad => &mut self.quads,
                AnimatedPrimitiveKind::Shadow => &mut self.shadows,
                AnimatedPrimitiveKind::MonochromeSprite => &mut self.mono_sprites,
                AnimatedPrimitiveKind::PolychromeSprite => &mut self.poly_sprites,
                AnimatedPrimitiveKind::BackdropBlur => &mut self.backdrop_blurs,
            };
            let offset = primitive.index as usize * primitive.bytes.len();
            buffer[offset..offset + primitive.bytes.len()].copy_from_slice(&primitive.bytes);
            if let Some(sample) = blur_sample {
                blur_samples.push(sample);
            }
        }

        let mut current_filter_dirty = FxHashSet::default();
        for sample in &blur_samples {
            if sample.can_use_base_filter() {
                self.backdrop_blur_use_base_filter_indices
                    .insert(sample.index);
            } else {
                current_filter_dirty.insert(sample.index);
            }
        }

        // A filter that temporarily escaped the retained base footprint may have cleared pixels
        // needed by the base result. The first frame that re-enters the base footprint therefore
        // performs one restoring refresh using the base filter geometry.
        let mut filter_refresh_indices = current_filter_dirty.clone();
        for index in previous_filter_dirty.difference(&current_filter_dirty) {
            filter_refresh_indices.insert(*index);
        }
        self.backdrop_blur_filter_dirty_indices = current_filter_dirty;

        if !filter_refresh_indices.is_empty() {
            // Keep ignore-self-damage empty while canonical configs are rebuilt so retained target
            // identity continues to carry real draw orders. Dynamic draw-time configs apply the
            // sentinel only after this refresh has completed.
            self.refresh_backdrop_blur_configs();
            self.rebuild_backdrop_blur_passes_for_current_frame();
            self.backdrop_blur_passes_dirty_this_frame = true;
        }

        if self.retained_static_reused {
            for sample in &blur_samples {
                if !self
                    .backdrop_blur_use_base_filter_indices
                    .contains(&sample.index)
                    || filter_refresh_indices.contains(&sample.index)
                {
                    continue;
                }
                let source_region = sample.base_source_region();
                let blocked_by_other_animation = self.animated_primitives.iter().any(|other| {
                    if other.kind == AnimatedPrimitiveKind::BackdropBlur
                        && other.index == sample.index
                    {
                        return false;
                    }
                    let Some(other_animation_id) = other.animation_id() else {
                        return false;
                    };
                    if Some(other_animation_id) == sample.animation_id
                        || !relevant_animation_ids.contains(&other_animation_id)
                        || other.order() >= sample.order
                    {
                        return false;
                    }
                    other
                        .sampled_visual_bounds(&self.sampled_animation_values)
                        .intersects(&source_region)
                });
                if !blocked_by_other_animation {
                    self.backdrop_blur_ignore_animation_damage_indices
                        .insert(sample.index);
                }
            }
        }

        self.backdrop_blur_previous_animation_ids = current_animation_ids;
    }

    pub(in crate::platform::nova) fn animated_upload_bytes(&self) -> usize {
        let primitives: usize = self
            .animated_primitives
            .iter()
            .map(|primitive| primitive.bytes.len())
            .sum();
        primitives
            + if self.has_animated_backdrop_blurs() {
                self.backdrop_blur_passes.len()
            } else {
                0
            }
    }

    /// Returns whether animated backdrop state changed Gaussian pass/config data this frame.
    /// Composite-only backdrop animation still uploads its tiny primitive range, but it does not
    /// rewrite the pass buffer and does not imply offscreen filter work.
    pub(in crate::platform::nova) fn has_animated_backdrop_blurs(&self) -> bool {
        self.backdrop_blur_passes_dirty_this_frame
    }

    pub(in crate::platform::nova) fn base_animated_backdrop_blur(
        &self,
        index: u32,
    ) -> Option<&crate::PaintBackdropBlur> {
        self.animated_primitives
            .iter()
            .find(|primitive| {
                primitive.kind == AnimatedPrimitiveKind::BackdropBlur && primitive.index == index
            })
            .and_then(AnimatedUpload::base_backdrop_blur)
    }
}

fn bounds_contains(
    outer: crate::Bounds<crate::ScaledPixels>,
    inner: crate::Bounds<crate::ScaledPixels>,
) -> bool {
    inner.left() >= outer.left()
        && inner.top() >= outer.top()
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn apply_value(primitive: &mut Primitive, value: &SceneAnimationValue) {
    let progress = if value.progress.is_finite() {
        value.progress
    } else {
        0.0
    };
    let sampled = std::array::from_fn::<_, 4, _>(|index| {
        value.from[index] + (value.to[index] - value.from[index]) * progress
    });
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
                _ => {}
            }
        }
        TransitionProperty::Rotation => {
            if let Primitive::MonochromeSprite(sprite) = primitive {
                let center = sprite.bounds.center();
                let rotation = crate::TransformationMatrix::unit()
                    .translate(center)
                    .rotate(crate::radians(sampled[0]))
                    .translate(crate::point(
                        crate::ScaledPixels(-center.x.0),
                        crate::ScaledPixels(-center.y.0),
                    ));
                sprite.transformation = sprite.transformation.compose(rotation);
            }
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
        _ => {}
    }
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
            // Visual scale is a composite transform. It must not alter Gaussian sigma; changing
            // sigma would rebuild H/V filter coefficients every animation frame even though the
            // already-filtered backdrop texture can simply be sampled by a smaller composite quad.
            blur.bounds = scale_bounds(blur.bounds);
            scale_mask(&mut blur.content_mask);
            blur.corner_radii = blur.corner_radii.map(|value| *value * scale);
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
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_translation_preserves_overshoot_without_accumulating_deltas() {
        let id = crate::SceneAnimationId(1);
        let quad = Quad {
            animation_id: Some(id),
            ..Default::default()
        };
        let mut upload = AnimatedUpload::new(Primitive::Quad(quad), AnimatedPrimitiveKind::Quad, 3);
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
        upload.sample(&[value], size);
        assert_eq!(
            f32::from_le_bytes(upload.bytes[8..12].try_into().unwrap()),
            12.0
        );
        assert_eq!(upload.offset(), (3 * PACKED_QUAD_BYTES) as u64);
        value.progress = 0.5;
        upload.sample(&[value], size);
        assert_eq!(
            f32::from_le_bytes(upload.bytes[8..12].try_into().unwrap()),
            5.0
        );
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
        assert_eq!(
            &frame.quads[3 * PACKED_QUAD_BYTES..],
            frame.animated_primitives[0].bytes
        );
        assert_eq!(frame.animated_upload_bytes(), PACKED_QUAD_BYTES);
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
    fn retained_rotation_keeps_the_sprite_center_fixed() {
        let mut primitive = Primitive::MonochromeSprite(MonochromeSprite {
            order: 0,
            pad: 0,
            animation_id: None,
            bounds: crate::bounds(
                crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
                crate::size(crate::ScaledPixels(30.0), crate::ScaledPixels(40.0)),
            ),
            content_mask: crate::ContentMask {
                bounds: crate::bounds(
                    crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                    crate::size(crate::ScaledPixels(100.0), crate::ScaledPixels(100.0)),
                ),
                ..Default::default()
            },
            color: crate::Hsla::default().into(),
            tile: crate::AtlasTile {
                texture_id: crate::AtlasTextureId {
                    index: 0,
                    kind: crate::AtlasTextureKind::Monochrome,
                },
                tile_id: crate::TileId(0),
                padding: 1,
                bounds: crate::bounds(
                    crate::point(crate::DevicePixels(0), crate::DevicePixels(0)),
                    crate::size(crate::DevicePixels(1), crate::DevicePixels(1)),
                ),
            },
            transformation: crate::TransformationMatrix::unit(),
        });
        apply_value(
            &mut primitive,
            &SceneAnimationValue {
                animation_id: crate::SceneAnimationId(1),
                property: TransitionProperty::Rotation,
                progress: 1.0,
                from: [0.0; 4],
                to: [std::f32::consts::FRAC_PI_2, 0.0, 0.0, 0.0],
            },
        );
        let Primitive::MonochromeSprite(sprite) = primitive else {
            panic!("monochrome sprite");
        };
        let center = crate::point(crate::px(25.0), crate::px(40.0));
        let transformed = sprite.transformation.apply(center);
        assert!((transformed.x.0 - center.x.0).abs() < 0.0001);
        assert!((transformed.y.0 - center.y.0).abs() < 0.0001);
    }
}
