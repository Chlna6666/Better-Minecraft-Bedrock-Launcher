use super::*;

impl BackdropBlurTargets {
    /// Rebind full-resolution zero-radius capture textures when a popup changes
    /// the flattened capture order/count but no actual Gaussian target is needed.
    ///
    /// All previous source pixels become untrusted: the caller MUST invalidate
    /// filter residency and force a full first refresh after this mapping change.
    /// This path only avoids reallocating textures and GPU resource bindings.
    pub(in crate::platform::nova) fn rebind_zero_radius_layout(
        &mut self,
        next: &[BackdropBlurConfig],
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
        if needs_root_source
            || self.source.is_some()
            || !self.isolated_sources.is_empty()
            || self.shared_horizontal_scratch
            || next.len() > self.variants.len()
            || self.variants.iter().any(|variant| {
                variant.config.radius() != 0. || variant.levels.len() != 1
            })
            || next.iter().any(|config| config.radius() != 0.)
        {
            return false;
        }

        // Every admitted target has exactly one native-resolution color texture.
        // No per-slot source or Gaussian dimensions change with the flattened index.
        self.variants.swap(old_root, new_root);
        for (variant, config) in self.variants.iter_mut().zip(next) {
            variant.config = *config;
        }
        self.retained_root_variant = Some(new_root);
        self.active_variant_count = next.len();
        true
    }

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
