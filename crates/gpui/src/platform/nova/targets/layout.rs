use super::*;

impl BackdropBlurTargets {
    /// Rebind full-resolution zero-radius capture textures when a popup changes
    /// the flattened capture order/count but no actual Gaussian target is needed.
    ///
    /// Other Capture pixels become untrusted: the caller MUST invalidate their
    /// source versions and fully initialize each remapped Capture on first use.
    /// A previous Root Color can retain its pixels only after the caller proves
    /// the pre/post-rebind GPU TextureView and resident epoch are identical.
    /// This path only reorders existing textures and GPU resource bindings.
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
        // Other layouts may rebind child-only slots; their source residency
        // must be invalidated before the next draw. Root pixels may survive
        // only under the renderer's explicit physical texture identity check.
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

    /// Reuse existing GPU target allocations using one identity-based mapping.
    ///
    /// Stable capture IDs may be reordered or their active count may shrink
    /// without recreating textures or resource sets. The caller invalidates
    /// old filtered pixels and draw steps after every topology change.
    pub(in crate::platform::nova) fn reuse_layout(
        &mut self,
        next: &[BackdropBlurConfig],
        isolated_indices: &[u32],
        needs_root_source: bool,
        root_index: Option<u32>,
    ) -> bool {
        let new_root = root_index.and_then(|index| {
            next.iter()
                .position(|config| config.contains_member_index(index))
        });
        if (root_index.is_some() && new_root.is_none())
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

        // Validate a one-to-one mapping BEFORE touching live native resource
        // metadata. This covers stable prefixes and arbitrary capture reorder.
        let mut permutation = smallvec::SmallVec::<[usize; 8]>::new();
        for config in next {
            let Some((old_index, _)) = self.variants.iter().enumerate().find(
                |(index, variant)| {
                    !permutation.contains(index)
                        && variant.config.same_target_slot(*config)
                        && variant.levels.len() == blur_variant_level_count(*config)
                },
            ) else {
                break;
            };
            permutation.push(old_index);
        }
        let root_identity_preserved = match (self.retained_root_variant, new_root) {
            (Some(previous), Some(current)) => permutation.get(current) == Some(&previous),
            _ => true,
        };
        if permutation.len() == next.len() && root_identity_preserved {
            // Reorder in place: preserve the Vec allocation and every native
            // texture/view/resource-set handle. The validated one-to-one
            // mapping ensures that each remaining capture still has a match
            // in the unplaced suffix after earlier swaps.
            for (target_index, config) in next.iter().enumerate() {
                let source_index = (target_index..self.variants.len())
                    .find(|&index| {
                        let variant = &self.variants[index];
                        variant.config.same_target_slot(*config)
                            && variant.levels.len() == blur_variant_level_count(*config)
                    })
                    .expect("validated target permutation");
                self.variants.swap(target_index, source_index);
                self.variants[target_index].config = *config;
            }
            self.active_variant_count = next.len();
            self.retained_root_variant = new_root;
            return true;
        }

        // Keep the existing root-color relocation behavior when a zero-radius
        // capture changes its flattened member index. This cannot use the
        // stable-ID permutation, but can still preserve physical textures.
        let (Some(old_root), Some(new_root)) = (self.retained_root_variant, new_root)
        else {
            return false;
        };
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
