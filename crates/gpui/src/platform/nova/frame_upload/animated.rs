use super::*;
use crate::SceneAnimationId;
use collections::FxHashSet;
use smallvec::SmallVec;

mod primitive;
#[cfg(test)]
mod tests;
mod values;

use primitive::BackdropBlurAnimationSample;
pub(super) use primitive::{AnimatedByteMetadata, AnimatedUpload};

#[derive(Default)]
struct AnimatedPrimitiveSamples {
    blur_samples: SmallVec<[BackdropBlurAnimationSample; 4]>,
    filter_parameters_changed: bool,
    visual_bounds: Vec<crate::Bounds<crate::ScaledPixels>>,
}

struct BackdropFilterSamples {
    dirty_indices: FxHashSet<u32>,
    refresh_indices: FxHashSet<u32>,
}

impl FrameUpload {
    pub(in crate::platform::nova) fn sample_animated_primitives(&mut self, size: DrawableSize) {
        self.backdrop_blur_use_base_filter_indices.clear();
        self.backdrop_blur_ignore_animation_damage_indices.clear();
        self.backdrop_blur_passes_dirty_this_frame = false;

        // Ordinary quads, shadows and sprites are promoted to the indexed GPU animation table
        // during encode. If no CPU-sampled primitive remains, there is no primitive buffer to
        // rewrite or resolve; only rotate the small animation-id scratch set used by blur history.
        // Keep the fallback path when a previous filtered primitive still needs its dirty state
        // retired.
        if self.animated_primitives.is_empty() && self.backdrop_blur_filter_dirty_indices.is_empty()
        {
            let current_animation_ids = self.current_animation_ids();
            self.store_animation_ids(current_animation_ids);
            return;
        }

        let resolved_animation_values =
            values::resolve_animation_values(&self.sampled_animation_values);

        let current_animation_ids = self.current_animation_ids();

        let samples = self.sample_primitive_records(&resolved_animation_values, size);
        let filter_samples = self.sample_backdrop_filters(&samples);

        self.suppress_backdrop_blur_damage(
            &samples.blur_samples,
            &samples.visual_bounds,
            &current_animation_ids,
            &filter_samples.refresh_indices,
        );

        self.store_backdrop_filters(filter_samples);
        self.animated_visual_bounds_scratch = samples.visual_bounds;

        self.store_animation_ids(current_animation_ids);
    }

    fn current_animation_ids(&mut self) -> FxHashSet<SceneAnimationId> {
        let mut ids = std::mem::take(&mut self.backdrop_blur_current_animation_ids_scratch);
        ids.clear();
        ids.extend(
            self.sampled_animation_values
                .iter()
                .map(|value| value.animation_id),
        );
        ids
    }

    fn store_animation_ids(&mut self, mut ids: FxHashSet<SceneAnimationId>) {
        std::mem::swap(&mut self.backdrop_blur_previous_animation_ids, &mut ids);
        self.backdrop_blur_current_animation_ids_scratch = ids;
    }

    fn sample_backdrop_filters(
        &mut self,
        samples: &AnimatedPrimitiveSamples,
    ) -> BackdropFilterSamples {
        let mut dirty_indices = std::mem::take(&mut self.backdrop_blur_filter_dirty_scratch);
        dirty_indices.clear();
        for sample in &samples.blur_samples {
            if sample.can_use_base_filter() {
                self.backdrop_blur_use_base_filter_indices
                    .insert(sample.index);
            } else {
                dirty_indices.insert(sample.index);
            }
        }

        let mut refresh_indices = std::mem::take(&mut self.backdrop_blur_filter_refresh_scratch);
        refresh_indices.clear();
        refresh_indices.extend(dirty_indices.iter().copied());
        refresh_indices.extend(
            self.backdrop_blur_filter_dirty_indices
                .difference(&dirty_indices)
                .copied(),
        );

        if samples.filter_parameters_changed || !refresh_indices.is_empty() {
            self.refresh_backdrop_blur_configs();
            self.rebuild_backdrop_blur_passes();
            self.backdrop_blur_passes_dirty_this_frame = true;
        }

        BackdropFilterSamples {
            dirty_indices,
            refresh_indices,
        }
    }

    fn store_backdrop_filters(&mut self, mut samples: BackdropFilterSamples) {
        std::mem::swap(
            &mut self.backdrop_blur_filter_dirty_indices,
            &mut samples.dirty_indices,
        );
        self.backdrop_blur_filter_dirty_scratch = samples.dirty_indices;
        self.backdrop_blur_filter_refresh_scratch = samples.refresh_indices;
    }

    fn sample_primitive_records(
        &mut self,
        values: &values::ResolvedAnimationValues,
        size: DrawableSize,
    ) -> AnimatedPrimitiveSamples {
        let mut samples = AnimatedPrimitiveSamples::default();
        samples.visual_bounds = std::mem::take(&mut self.animated_visual_bounds_scratch);
        samples.visual_bounds.clear();
        samples
            .visual_bounds
            .reserve(self.animated_primitives.len());
        let mut staging = std::mem::take(&mut self.animated_primitive_staging);
        staging.clear();
        staging.reserve(PACKED_BACKDROP_BLUR_BYTES);

        for primitive in &self.animated_primitives {
            let sample = primitive.sample_resolved(values, size, &mut staging);
            samples.filter_parameters_changed |= sample.filter_parameters_changed;
            samples.visual_bounds.push(sample.visual_bounds);
            let byte_len = primitive.bytes.len();
            let offset = primitive.index as usize * byte_len;
            debug_assert_eq!(staging.len(), byte_len);
            let range = offset..offset + byte_len;
            let bytes = match primitive.kind {
                AnimatedPrimitiveKind::Quad => self.quads.slice_mut(range),
                AnimatedPrimitiveKind::Shadow => &mut self.shadows[range],
                AnimatedPrimitiveKind::MonochromeSprite => &mut self.mono_sprites[range],
                AnimatedPrimitiveKind::PolychromeSprite => &mut self.poly_sprites[range],
                AnimatedPrimitiveKind::BackdropBlur => &mut self.backdrop_blurs[range],
            };
            bytes.copy_from_slice(&staging);
            if let Some(sample) = sample.backdrop_blur {
                samples.blur_samples.push(sample);
            }
        }
        self.animated_primitive_staging = staging;
        samples
    }

    fn suppress_backdrop_blur_damage(
        &mut self,
        blur_samples: &[BackdropBlurAnimationSample],
        sampled_visual_bounds: &[crate::Bounds<crate::ScaledPixels>],
        current_animation_ids: &FxHashSet<SceneAnimationId>,
        filter_refresh_indices: &FxHashSet<u32>,
    ) {
        if !self.retained_static_reused {
            return;
        }
        for sample in blur_samples {
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
                .zip(sampled_visual_bounds)
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
