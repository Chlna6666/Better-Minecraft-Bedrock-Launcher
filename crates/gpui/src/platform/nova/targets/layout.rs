use super::*;

impl BackdropBlurTargets {
    /// Rebind only after pending GPU readers retire. All old logical source versions
    /// must be revalidated by the FilterRegistry against the resulting physical views.
    pub(in crate::platform::nova) fn reuse_layout(
        &mut self,
        next: &[BackdropBlurConfig],
        isolated_indices: &[u32],
        needs_root_source: bool,
        auto_static_indices: Option<(u32, u32)>,
    ) -> bool {
        let requested_sources: smallvec::SmallVec<[u32; 8]> = isolated_indices
            .iter()
            .copied()
            .filter(|index| blur_requires_isolated_source(next, *index))
            .collect();
        if next.len() > self.variants.len()
            || requested_sources.len() > self.isolated_sources.len()
            || self.source.is_some() != needs_root_source
        {
            return false;
        }
        let Some(permutation) = self.plan_layout(next, auto_static_indices) else {
            return false;
        };
        // The complete Texture/View/BindGroup owner moves together. This temporary
        // permutation has no content version or resource-lifetime authority.
        let mut original_at_position: Vec<_> = (0..self.variants.len()).collect();
        for (destination, (config, original)) in next.iter().zip(permutation).enumerate() {
            let source = original_at_position
                .iter()
                .position(|index| *index == original)
                .expect("layout preflight selected an existing target");
            self.variants.swap(destination, source);
            original_at_position.swap(destination, source);
            self.variants[destination].config = *config;
        }
        for (source, index) in self.isolated_sources.iter_mut().zip(&requested_sources) {
            source.index = *index;
        }
        self.active_isolated_source_count = requested_sources.len();
        self.active_variant_count = next.len();
        true
    }

    fn plan_layout(
        &self,
        next: &[BackdropBlurConfig],
        auto_static_indices: Option<(u32, u32)>,
    ) -> Option<smallvec::SmallVec<[usize; 8]>> {
        let mut permutation = smallvec::SmallVec::new();
        permutation.resize(next.len(), usize::MAX);
        if let Some((old_index, new_index)) = auto_static_indices {
            let old = self
                .variants
                .iter()
                .take(self.active_variant_count)
                .position(|variant| {
                    variant.config.exclusively_contains_member_index(old_index)
                        && variant.config.radius() == 0.0
                })?;
            let new = next.iter().position(|config| {
                config.exclusively_contains_member_index(new_index) && config.radius() == 0.0
            })?;
            if !target_fits(&self.variants[old], next[new]) {
                return None;
            }
            permutation[new] = old;
        }
        // Reserve all exact identities before consuming spare same-size targets.
        // A popup insertion must not steal a later unchanged source's texture.
        for (new_index, config) in next.iter().enumerate() {
            if permutation[new_index] != usize::MAX {
                continue;
            }
            if let Some(old) = self.variants.iter().enumerate().find_map(|(old, variant)| {
                (!permutation.contains(&old)
                    && variant.config.same_target_slot(*config)
                    && target_fits(variant, *config))
                .then_some(old)
            }) {
                permutation[new_index] = old;
            }
        }
        for (new_index, config) in next.iter().enumerate() {
            if permutation[new_index] == usize::MAX {
                permutation[new_index] =
                    self.variants
                        .iter()
                        .enumerate()
                        .find_map(|(old, variant)| {
                            (!permutation.contains(&old) && target_fits(variant, *config))
                                .then_some(old)
                        })?;
            }
        }
        Some(permutation)
    }
}

/// Targets share their window's extent/format and FrameSlot count. Gaussian
/// geometry additionally depends on downsample; direct color targets stay native-size.
fn target_fits(variant: &BackdropBlurVariantTargets, config: BackdropBlurConfig) -> bool {
    let gaussian = config.radius() > 0.0;
    gaussian == (variant.config.radius() > 0.0)
        && (!gaussian || variant.config.downsample() == config.downsample())
        && variant.levels.len() == blur_variant_level_count(config)
        && !variant.target_resource_sets.is_empty()
        && variant
            .levels
            .iter()
            .all(|level| level.pass_resource_sets.len() == variant.target_resource_sets.len())
}
