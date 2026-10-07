//! Per-window retained filter variants and cache validity, owned by the GPU consumer.
//! Painter-order source damage stays in BackdropBlurDamagePlan; full/partial redraw eligibility
//! does not replace that provenance with a global cache invalidation.
use super::*;

pub(super) struct FilterRegistry {
    pub(super) targets: Option<BackdropBlurTargets>,
    pub(super) atlas_generation: u64,
    pub(super) quality: Option<BackdropBlurQuality>,
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
