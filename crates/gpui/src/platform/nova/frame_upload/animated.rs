use super::*;
use crate::{Primitive, SceneAnimationId, SceneAnimationValue, TransitionProperty};
use smallvec::SmallVec;

const BLUR_SOURCE_BOUNDS_OFFSET: usize = 16;
const BLUR_DISPLAY_BOUNDS_OFFSET: usize = 96;
const ENGINE_ANIMATION_ID_BASE: u32 = 1 << 31;
const DENSE_LOOKUP_MAX_SPAN: usize = 4096;
const DENSE_LOOKUP_DENSITY_FACTOR: usize = 4;

#[derive(Clone, Copy)]
struct ResolvedAnimationValue {
    property: TransitionProperty,
    sampled: [f32; 4],
}

struct ResolvedAnimationValues {
    scene: SmallVec<[Option<ResolvedAnimationValue>; 16]>,
    engine_base: u32,
    engine: SmallVec<[Option<ResolvedAnimationValue>; 16]>,
    sparse: FxHashMap<SceneAnimationId, ResolvedAnimationValue>,
}

impl ResolvedAnimationValue {
    #[inline]
    fn new(value: &SceneAnimationValue) -> Self {
        let progress = if value.progress.is_finite() {
            value.progress
        } else {
            0.0
        };
        Self {
            property: value.property,
            sampled: std::array::from_fn(|index| {
                value.from[index] + (value.to[index] - value.from[index]) * progress
            }),
        }
    }
}

impl ResolvedAnimationValues {
    fn new(values: &[SceneAnimationValue]) -> Self {
        let mut scene_count = 0usize;
        let mut scene_max = None::<u32>;
        let mut engine_count = 0usize;
        let mut engine_min = None::<u32>;
        let mut engine_max = None::<u32>;
        for value in values {
            let id = value.animation_id.0;
            if id < ENGINE_ANIMATION_ID_BASE {
                scene_count += 1;
                scene_max = Some(scene_max.map_or(id, |max| max.max(id)));
            } else {
                engine_count += 1;
                engine_min = Some(engine_min.map_or(id, |min| min.min(id)));
                engine_max = Some(engine_max.map_or(id, |max| max.max(id)));
            }
        }

        let scene_len = scene_max
            .and_then(|max| dense_span_len(scene_count, 0, max))
            .unwrap_or(0);
        let engine_base = engine_min.unwrap_or(ENGINE_ANIMATION_ID_BASE);
        let engine_len = engine_max
            .and_then(|max| dense_span_len(engine_count, engine_base, max))
            .unwrap_or(0);
        let sparse_count = if scene_len == 0 { scene_count } else { 0 }
            + if engine_len == 0 { engine_count } else { 0 };

        let mut scene = SmallVec::new();
        scene.resize(scene_len, None);
        let mut engine = SmallVec::new();
        engine.resize(engine_len, None);
        let mut sparse = FxHashMap::default();
        if sparse_count > 0 {
            sparse.reserve(sparse_count);
        }

        let mut resolved = Self {
            scene,
            engine_base,
            engine,
            sparse,
        };
        for value in values {
            resolved.insert_first(value.animation_id, ResolvedAnimationValue::new(value));
        }
        resolved
    }

    #[inline]
    fn get(&self, animation_id: &SceneAnimationId) -> Option<&ResolvedAnimationValue> {
        let id = animation_id.0;
        if id < ENGINE_ANIMATION_ID_BASE {
            if let Some(value) = self.scene.get(id as usize).and_then(Option::as_ref) {
                return Some(value);
            }
        } else if let Some(offset) = id.checked_sub(self.engine_base)
            && let Some(value) = self.engine.get(offset as usize).and_then(Option::as_ref)
        {
            return Some(value);
        }
        self.sparse.get(animation_id)
    }

    fn insert_first(&mut self, animation_id: SceneAnimationId, value: ResolvedAnimationValue) {
        let id = animation_id.0;
        if id < ENGINE_ANIMATION_ID_BASE {
            if let Some(slot) = self.scene.get_mut(id as usize) {
                if slot.is_none() {
                    *slot = Some(value);
                }
                return;
            }
        } else if let Some(offset) = id.checked_sub(self.engine_base)
            && let Some(slot) = self.engine.get_mut(offset as usize)
        {
            if slot.is_none() {
                *slot = Some(value);
            }
            return;
        }
        self.sparse.entry(animation_id).or_insert(value);
    }
}

fn dense_span_len(count: usize, min: u32, max: u32) -> Option<usize> {
    let span = max.checked_sub(min)?.checked_add(1)? as usize;
    let density_limit = count
        .saturating_mul(DENSE_LOOKUP_DENSITY_FACTOR)
        .max(16);
    (span <= density_limit && span <= DENSE_LOOKUP_MAX_SPAN).then_some(span)
}

fn resolve_animation_values(values: &[SceneAnimationValue]) -> ResolvedAnimationValues {
    ResolvedAnimationValues::new(values)
}

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

    #[inline]
    pub(in crate::platform::nova) const fn capacity(self) -> usize {
        0
    }
}

/// A retained primitive and its small, independently uploadable animated range.
/// Nova's current shaders consume primitive buffers, not the animation metadata.
pub(in crate::platform::nova) struct AnimatedUpload {
    pub(in crate::platform::nova) kind: AnimatedPrimitiveKind,
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) bytes: AnimatedByteMetadata,
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

#[derive(Clone, Copy)]
struct AnimatedPrimitiveSample {
    visual_bounds: crate::Bounds<crate::ScaledPixels>,
    backdrop_blur: Option<BackdropBlurAnimationSample>,
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
            bytes: AnimatedByteMetadata::new(kind),
        }
    }

    pub(in crate::platform::nova) fn offset(&self) -> u64 {
        u64::from(self.index) * self.kind.stride() as u64
    }

    #[cfg(test)]
    fn sample(
        &self,
        values: &[SceneAnimationValue],
        size: DrawableSize,
        bytes: &mut Vec<u8>,
    ) -> Option<BackdropBlurAnimationSample> {
        let resolved = resolve_animation_values(values);
        self.sample_resolved(&resolved, size, bytes).backdrop_blur
    }

    fn sample_resolved(
        &self,
        values: &ResolvedAnimationValues,
        size: DrawableSize,
        bytes: &mut Vec<u8>,
    ) -> AnimatedPrimitiveSample {
        let mut primitive = self.primitive.clone();
        if let Some(value) = primitive
            .animation_id()
            .and_then(|animation_id| values.get(&animation_id))
        {
            apply_resolved_value(&mut primitive, *value);
        }
        let visual_bounds = primitive.visual_bounds();
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
                    radius: base.radius,
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
                // Element/composite records deliberately use two geometries in one 136-byte record:
                // the normal bounds slot remains the immutable source/filter footprint, while the
                // auxiliary slot written by write_paint_blur contains the sampled display bounds.
                if let Primitive::Blur(base) = &self.primitive {
                    write_packed_bounds_at(bytes, BLUR_SOURCE_BOUNDS_OFFSET, base.bounds);
                }
            }
            _ => {}
        }
        debug_assert_eq!(bytes.len(), self.bytes.len());
        AnimatedPrimitiveSample {
            visual_bounds,
            backdrop_blur,
        }
    }

    fn animation_id(&self) -> Option<SceneAnimationId> {
        self.primitive.animation_id()
    }

    fn order(&self) -> u32 {
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

impl FrameUpload {
    pub(in crate::platform::nova) fn sample_animated_primitives(&mut self, size: DrawableSize) {
        self.backdrop_blur_use_base_filter_indices.clear();
        self.backdrop_blur_ignore_animation_damage_indices.clear();
        self.backdrop_blur_passes_dirty_this_frame = false;

        // Resolve each animation value once per frame. Animated primitives can heavily outnumber
        // animation timelines (for example hundreds of glyphs sharing one retained transform), so
        // the old per-primitive linear search and interpolation scaled as O(primitives * values).
        let resolved_animation_values = resolve_animation_values(&self.sampled_animation_values);

        let mut current_animation_ids =
            std::mem::take(&mut self.backdrop_blur_current_animation_ids_scratch);
        current_animation_ids.clear();
        current_animation_ids.extend(
            self.sampled_animation_values
                .iter()
                .map(|value| value.animation_id),
        );

        let mut blur_samples: SmallVec<[BackdropBlurAnimationSample; 4]> = SmallVec::new();
        let mut sampled_visual_bounds = std::mem::take(&mut self.animated_visual_bounds_scratch);
        sampled_visual_bounds.clear();
        sampled_visual_bounds.reserve(self.animated_primitives.len());
        let mut staging = std::mem::take(&mut self.animated_primitive_staging);
        staging.clear();
        staging.reserve(PACKED_BACKDROP_BLUR_BYTES);

        for primitive in &self.animated_primitives {
            let sample = primitive.sample_resolved(&resolved_animation_values, size, &mut staging);
            sampled_visual_bounds.push(sample.visual_bounds);
            // GPU composite state always receives the sampled primitive. Filter planning does not
            // have to use these same bytes: root backdrop configs can independently select retained
            // base geometry, while element blur records keep base source bounds inside the record.
            let buffer = match primitive.kind {
                AnimatedPrimitiveKind::Quad => &mut self.quads,
                AnimatedPrimitiveKind::Shadow => &mut self.shadows,
                AnimatedPrimitiveKind::MonochromeSprite => &mut self.mono_sprites,
                AnimatedPrimitiveKind::PolychromeSprite => &mut self.poly_sprites,
                AnimatedPrimitiveKind::BackdropBlur => &mut self.backdrop_blurs,
            };
            let byte_len = primitive.bytes.len();
            let offset = primitive.index as usize * byte_len;
            debug_assert_eq!(staging.len(), byte_len);
            buffer[offset..offset + byte_len].copy_from_slice(&staging);
            if let Some(sample) = sample.backdrop_blur {
                blur_samples.push(sample);
            }
        }
        self.animated_primitive_staging = staging;

        let mut current_filter_dirty =
            std::mem::take(&mut self.backdrop_blur_filter_dirty_scratch);
        current_filter_dirty.clear();
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
        let mut filter_refresh_indices =
            std::mem::take(&mut self.backdrop_blur_filter_refresh_scratch);
        filter_refresh_indices.clear();
        filter_refresh_indices.extend(current_filter_dirty.iter().copied());
        for index in self
            .backdrop_blur_filter_dirty_indices
            .difference(&current_filter_dirty)
        {
            filter_refresh_indices.insert(*index);
        }

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
                let blocked_by_other_animation = self
                    .animated_primitives
                    .iter()
                    .zip(&sampled_visual_bounds)
                    .any(|(other, other_bounds)| {
                        if other.base_backdrop_blur().is_some() && other.index == sample.index {
                            return false;
                        }
                        let Some(other_animation_id) = other.animation_id() else {
                            return false;
                        };
                        if Some(other_animation_id) == sample.animation_id
                            || !(current_animation_ids.contains(&other_animation_id)
                                || self
                                    .backdrop_blur_previous_animation_ids
                                    .contains(&other_animation_id))
                            || other.order() >= sample.order
                        {
                            return false;
                        }
                        other_bounds.intersects(&source_region)
                    });
                if !blocked_by_other_animation {
                    self.backdrop_blur_ignore_animation_damage_indices
                        .insert(sample.index);
                }
            }
        }

        std::mem::swap(
            &mut self.backdrop_blur_filter_dirty_indices,
            &mut current_filter_dirty,
        );
        self.backdrop_blur_filter_dirty_scratch = current_filter_dirty;
        self.backdrop_blur_filter_refresh_scratch = filter_refresh_indices;
        self.animated_visual_bounds_scratch = sampled_visual_bounds;

        std::mem::swap(
            &mut self.backdrop_blur_previous_animation_ids,
            &mut current_animation_ids,
        );
        self.backdrop_blur_current_animation_ids_scratch = current_animation_ids;
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

    /// Returns whether animated root-backdrop state changed Gaussian pass/config data this frame.
    /// Composite-only root or element blur animation still uploads its tiny primitive range, but it
    /// does not rewrite the pass buffer and does not imply offscreen filter work.
    pub(in crate::platform::nova) fn has_animated_backdrop_blurs(&self) -> bool {
        self.backdrop_blur_passes_dirty_this_frame
    }

    pub(in crate::platform::nova) fn base_animated_backdrop_blur(
        &self,
        index: u32,
    ) -> Option<&crate::PaintBackdropBlur> {
        self.animated_primitives
            .iter()
            .find(|primitive| primitive.index == index && primitive.base_backdrop_blur().is_some())
            .and_then(AnimatedUpload::base_backdrop_blur)
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

fn read_packed_bounds_at(bytes: &[u8], offset: usize) -> [f32; 4] {
    std::array::from_fn(|index| {
        let start = offset + index * 4;
        f32::from_ne_bytes(bytes[start..start + 4].try_into().unwrap())
    })
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
        Primitive::Blur(blur) => {
            // Same contract as a retained compositor layer: only final display geometry changes.
            // Source/filter bounds and Gaussian sigma are restored/kept from the base primitive.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_animation_values_preserve_first_duplicate() {
        let id = crate::SceneAnimationId(5);
        let values = [
            SceneAnimationValue {
                animation_id: id,
                property: TransitionProperty::Translation,
                progress: 0.5,
                from: [0.0; 4],
                to: [10.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: id,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [99.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&id).unwrap().sampled[0], 5.0);
    }

    #[test]
    fn resolved_animation_values_handle_dense_scene_and_engine_namespaces() {
        let scene_id = crate::SceneAnimationId(3);
        let engine_id = crate::SceneAnimationId(ENGINE_ANIMATION_ID_BASE + 7);
        let values = [
            SceneAnimationValue {
                animation_id: scene_id,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [3.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: engine_id,
                property: TransitionProperty::Opacity,
                progress: 0.5,
                from: [0.0; 4],
                to: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&scene_id).unwrap().sampled[0], 3.0);
        assert_eq!(resolved.get(&engine_id).unwrap().sampled[0], 0.5);
    }

    #[test]
    fn resolved_animation_values_fall_back_for_sparse_ids() {
        let first = crate::SceneAnimationId(1);
        let sparse = crate::SceneAnimationId(10_000);
        let values = [
            SceneAnimationValue {
                animation_id: first,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [1.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: sparse,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [2.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&first).unwrap().sampled[0], 1.0);
        assert_eq!(resolved.get(&sparse).unwrap().sampled[0], 2.0);
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
        assert_eq!(f32::from_ne_bytes(bytes[112..116].try_into().unwrap()), 14.0);
        assert_eq!(f32::from_ne_bytes(bytes[128..132].try_into().unwrap()), 0.5);
        assert_eq!(upload.offset(), (4 * PACKED_BACKDROP_BLUR_BYTES) as u64);
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
