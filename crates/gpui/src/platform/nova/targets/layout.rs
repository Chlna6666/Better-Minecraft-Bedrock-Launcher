use super::*;

impl BackdropBlurTargets {
    /// Reuse a warmed popup layout without discarding resident window color.
    /// Only existing slots are reused; growth, filters with shared scratch and
    /// changed source layouts retain the complete allocation/initialization path.
    pub(in crate::platform::nova) fn reuse_layout(
        &mut self,
        next: &[BackdropBlurConfig],
        isolated_indices: &[u32],
        needs_root_source: bool,
        root_index: Option<u32>,
    ) -> bool {
        let Some(old_root) = self.retained_root_variant else {
            return false;
        };
        let Some(new_root) = root_index.and_then(|index| {
            next.iter()
                .position(|config| config.contains_member_index(index))
        }) else {
            return false;
        };
        if self.shared_horizontal_scratch
            || next.len() > self.variants.len()
            || self.source.is_some() != needs_root_source
            || !self
                .isolated_sources
                .iter()
                .map(|source| source.index)
                .eq(isolated_indices
                    .iter()
                    .copied()
                    .filter(|index| blur_requires_isolated_source(next, *index)))
        {
            return false;
        }
        // Validate the entire permutation before mutating any live metadata.
        for (index, config) in next.iter().enumerate() {
            let previous = if index == new_root {
                old_root
            } else if index == old_root {
                new_root
            } else {
                index
            };
            let variant = &self.variants[previous];
            if variant.levels.len() != blur_variant_level_count(*config)
                || (index == new_root && config.radius() != 0.)
                || (index != new_root && !variant.config.same_target_slot(*config))
            {
                return false;
            }
        }
        self.variants.swap(old_root, new_root);
        for (variant, config) in self.variants.iter_mut().zip(next) {
            variant.config = *config;
        }
        self.active_variant_count = next.len();
        self.retained_root_variant = Some(new_root);
        true
    }
}
