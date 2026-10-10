//! Per-window retained filter variants and cache validity, owned by the GPU consumer.
//! Painter-order source damage stays in BackdropBlurDamagePlan; full/partial redraw eligibility
//! does not replace that provenance with a global cache invalidation.
use super::*;

#[derive(Clone)]
pub(super) struct RetainedElementFilterSource {
    index: u32,
    blur: crate::PaintBlur,
    /// Last successful OFFSCREEN refresh for this particular filter.
    animation_values: Vec<crate::SceneAnimationValue>,
}

pub(super) struct FilterRegistry {
    pub(super) targets: Option<BackdropBlurTargets>,
    pub(super) atlas_generation: u64,
    pub(super) quality: Option<BackdropBlurQuality>,
    /// Sources sampled in the most recent successfully presented GPU frame.
    /// Snapshotting only on success prevents a deferred/failed present from
    /// validating an output texture that was never updated.
    pub(super) element_blur_inputs: Vec<RetainedElementFilterSource>,
    valid: bool,
}

impl FilterRegistry {
    pub(super) fn is_valid(&self) -> bool {
        self.valid
    }
    pub(super) fn new(targets: Option<BackdropBlurTargets>) -> Self {
        Self {
            targets,
            valid: false,
            atlas_generation: 0,
            quality: None,
            element_blur_inputs: Vec::new(),
        }
    }

    pub(super) fn refresh_required(
        &self,
        quality: BackdropBlurQuality,
        source_atlas_dirty: bool,
    ) -> bool {
        !self.valid || source_atlas_dirty || self.quality != Some(quality)
    }

    pub(super) fn begin_refresh(&mut self) {
        self.valid = false;
    }

    pub(super) fn invalidate(&mut self) {
        self.valid = false;
        self.quality = None;
        self.element_blur_inputs.clear();
    }

    pub(super) fn source_unchanged(
        &self,
        index: u32,
        current: &crate::PaintBlur,
        animation_values: &[crate::SceneAnimationValue],
    ) -> bool {
        let Some(previous) = self
            .element_blur_inputs
            .iter()
            .find(|previous| previous.index == index)
        else {
            return false;
        };
        current.bounds == previous.blur.bounds
            && current.content_mask == previous.blur.content_mask
            && current.radius == previous.blur.radius
            && current.content.retained_filter_source_matches(
                &previous.blur.content,
                animation_values,
                &previous.animation_values,
            )
    }

    pub(super) fn record_element_blur_inputs(
        &mut self,
        sources: &[(u32, crate::PaintBlur)],
        animation_values: &[crate::SceneAnimationValue],
        refreshed_indices: impl IntoIterator<Item = u32>,
    ) {
        // Only texture passes actually executed are allowed to advance their
        // source snapshot. A global Present can skip several unaffected blurs.
        for index in refreshed_indices {
            let Some((_, current)) = sources.iter().find(|(source_index, _)| *source_index == index)
            else {
                continue;
            };
            let snapshot = RetainedElementFilterSource {
                index,
                blur: current.clone(),
                animation_values: animation_values.to_vec(),
            };
            if let Some(previous) = self
                .element_blur_inputs
                .iter_mut()
                .find(|previous| previous.index == index)
            {
                *previous = snapshot;
            } else {
                self.element_blur_inputs.push(snapshot);
            }
        }
    }

    pub(super) fn record_submission(
        &mut self,
        quality: BackdropBlurQuality,
        atlas_generation: u64,
    ) {
        self.valid = true;
        self.quality = Some(quality);
        self.atlas_generation = atlas_generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_filters_reuse_until_source_quality_or_targets_change() {
        let mut registry = FilterRegistry::new(None);
        assert!(registry.refresh_required(BackdropBlurQuality::Full, false));
        registry.record_submission(BackdropBlurQuality::Full, 3);
        assert!(!registry.refresh_required(BackdropBlurQuality::Full, false));
        assert!(registry.refresh_required(BackdropBlurQuality::Full, true));
        assert!(registry.refresh_required(BackdropBlurQuality::Interactive, false));
        registry.invalidate();
        assert!(registry.refresh_required(BackdropBlurQuality::Full, false));
    }

    #[test]
    fn isolated_blur_reuses_source_when_only_composite_opacity_changes() {
        let mut registry = FilterRegistry::new(None);
        let scene = std::sync::Arc::new(crate::Scene::default());
        let blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(8.0),
            opacity: 1.0,
            content: scene,
        };
        registry.record_element_blur_inputs(&[(3, blur.clone())], &[], [3]);
        let mut changed_composite = blur.clone();
        changed_composite.opacity = 0.5;
        assert!(registry.source_unchanged(3, &changed_composite, &[]));
        changed_composite.radius = crate::ScaledPixels(12.0);
        assert!(!registry.source_unchanged(3, &changed_composite, &[]));
        assert!(!registry.source_unchanged(4, &blur, &[]));
        registry.invalidate();
        assert!(!registry.source_unchanged(3, &blur, &[]));
    }

    #[test]
    fn deferred_refresh_does_not_validate_retained_filters() {
        let mut registry = FilterRegistry::new(None);
        registry.record_submission(BackdropBlurQuality::Full, 3);
        registry.begin_refresh();
        assert!(registry.refresh_required(BackdropBlurQuality::Full, false));
        registry.record_submission(BackdropBlurQuality::Full, 4);
        assert!(!registry.refresh_required(BackdropBlurQuality::Full, false));
        assert_eq!(registry.atlas_generation, 4);
    }
}
