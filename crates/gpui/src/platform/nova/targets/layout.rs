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
        let old_root = self.retained_root_variant;
        let new_root = root_index.and_then(|index| {
            next.iter()
                .position(|config| config.contains_member_index(index))
        });
        if root_index.is_some() && new_root.is_none() {
            return false;
        }
        if needs_root_source
            || self.source.is_some()
            || !self.isolated_sources.is_empty()
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
        // Preserve a warmed window-root allocation when both layouts contain it.
        // Other layouts may rebind child-only slots, but ALL old pixel residency
        // is invalidated by ensure_backdrop_blur_targets before the next draw.
        if let (Some(old_root), Some(new_root)) = (old_root, new_root) {
            self.variants.swap(old_root, new_root);
        }
        for (variant, config) in self.variants.iter_mut().zip(next) {
            variant.config = *config;
        }
        self.retained_root_variant = new_root;
        self.active_variant_count = next.len();
        true
    }

    /// Reuse an already allocated GPU target prefix when only the active capture
    /// count changed. This is safe for both Gaussian and zero-radius captures:
    /// texture descriptors and their resource sets remain in their original slots.
    ///
    /// Unlike flattened capture rebinding, this path does not reinterpret any
    /// slot's identity. The caller must invalidate filter pixels and draw steps
    /// because adding/removing captures changes the composition topology.
    pub(in crate::platform::nova) fn reuse_stable_prefix_layout(
        &mut self,
        next: &[BackdropBlurConfig],
        isolated_indices: &[u32],
        needs_root_source: bool,
        root_index: Option<u32>,
    ) -> bool {
        let next_root = root_index.and_then(|index| {
            next.iter()
                .position(|config| config.contains_member_index(index))
        });
        if (root_index.is_some() && next_root.is_none())
            || self.retained_root_variant != next_root
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
            || !self.variants.iter().zip(next).all(|(variant, config)| {
                variant.config.same_target_slot(*config)
                    && variant.levels.len() == blur_variant_level_count(*config)
            })
        {
            return false;
        }

        for (variant, config) in self.variants.iter_mut().zip(next) {
            variant.config = *config;
        }
        self.active_variant_count = next.len();
        true
    }

    /// Reuse a warmed popup layout without discarding resident window color.
    /// Only existing slots are reused; growth and
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
        if next.len() > self.variants.len()
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
