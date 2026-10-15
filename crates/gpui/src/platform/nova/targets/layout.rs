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
        auto_static_indices: Option<(u32, u32)>,
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

        // The synthetic root and the AutoStatic prefix are exclusive cache roles.
        // When no synthetic root exists, move the fingerprint-validated prefix's
        // native Texture/View/ResourceSets together before rebinding indices.
        // Validate all indices BEFORE modifying live target ownership.
        let auto_static_slots = if old_root.is_none() && new_root.is_none() {
            if let Some((old_index, new_index)) = auto_static_indices {
                let old = self.variants.iter().take(self.active_variant_count)
                    .position(|variant| {
                        variant.config.exclusively_contains_member_index(old_index)
                            && variant.config.radius() == 0.0
                    });
                let new = next.iter().position(|config| {
                    config.exclusively_contains_member_index(new_index)
                        && config.radius() == 0.0
                });
                let (Some(old), Some(new)) = (old, new) else {
                    return false;
                };
                Some((old, new))
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(old_root), Some(new_root)) = (old_root, new_root) {
            self.variants.swap(old_root, new_root);
        } else if let Some((old, new)) = auto_static_slots {
            self.variants.swap(old, new);
        }
        for (variant, config) in self.variants.iter_mut().zip(next) {
            variant.config = *config;
        }
        self.retained_root_variant = new_root;
        self.active_variant_count = next.len();
        true
    }

    /// Rebind a warmed filter chain without allocating/destroying native targets.
    ///
    /// Source scratch targets and filtered final targets are separate GPU owners.
    /// After popup shrink, inactive physical sources remain in the spare suffix,
    /// so later growth up to the high-water mark can be rebound at no allocation.
    /// The caller invalidates changed source snapshots and waits for any
    /// pending GPU readers through the existing submission preflight.
    pub(in crate::platform::nova) fn reuse_layout(
        &mut self,
        next: &[BackdropBlurConfig],
        isolated_indices: &[u32],
        needs_root_source: bool,
        root_index: Option<u32>,
        auto_static_indices: Option<(u32, u32)>,
    ) -> bool {
        let new_root = root_index.and_then(|index| {
            next.iter()
                .position(|config| config.contains_member_index(index))
        });
        let requested_isolated: smallvec::SmallVec<[u32; 8]> = isolated_indices
            .iter()
            .copied()
            .filter(|index| blur_requires_isolated_source(next, *index))
            .collect();
        let capacity_rejection = if root_index.is_some() && new_root.is_none() {
            Some("root_capture_not_found")
        } else if next.len() > self.variants.len() {
            Some("variant_capacity")
        } else if requested_isolated.len() > self.isolated_sources.len() {
            Some("isolated_source_capacity")
        } else if self.source.is_some() != needs_root_source {
            Some("root_source_topology")
        } else {
            None
        };
        if let Some(reason) = capacity_rejection {
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI filter layout reuse rejected: reason={} next_variants={} allocated_variants={} required_isolated={} allocated_isolated={} old_root={:?} new_root={:?}",
                reason, next.len(), self.variants.len(), requested_isolated.len(),
                self.isolated_sources.len(), self.retained_root_variant, new_root,
            );
            return false;
        }

        // A synthetic Root Color and a fingerprint-proven AutoStatic prefix must
        // retain the SAME physical output texture across flattened index moves.
        // The source fingerprint check happens in FilterRegistry before this
        // method is called, not by guessing from a matching filter config.
        let pinned = if let (Some(previous), Some(current)) =
            (self.retained_root_variant, new_root)
        {
            Some((previous, current))
        } else if self.retained_root_variant.is_none() && new_root.is_none() {
            if let Some((old_index, new_index)) = auto_static_indices {
                let previous = self
                    .variants
                    .iter()
                    .take(self.active_variant_count)
                    .position(|variant| {
                        variant.config.exclusively_contains_member_index(old_index)
                            && variant.config.radius() == 0.0
                            && variant.levels.len() == 1
                    });
                let current = next.iter().position(|config| {
                    config.exclusively_contains_member_index(new_index)
                        && config.radius() == 0.0
                });
                let (Some(previous), Some(current)) = (previous, current) else {
                    log::trace!(
                        target: "gpui::diagnostics",
                        "GPUI filter layout reuse rejected: reason=auto_static_identity_missing old_index={} new_index={}",
                        old_index, new_index,
                    );
                    return false;
                };
                Some((previous, current))
            } else {
                None
            }
        } else {
            // AutoStatic has no physical root; do not combine two cache roles.
            if auto_static_indices.is_some() {
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI filter layout reuse rejected: reason=conflicting_root_and_auto_static",
                );
                return false;
            }
            None
        };

        // Preflight a one-to-one mapping for ALL requested targets, including
        // inactive spare targets. No CPU/native resource metadata changes before
        // the entire permutation has been proven.
        let mut permutation = smallvec::SmallVec::<[usize; 8]>::new();
        for (new_index, config) in next.iter().copied().enumerate() {
            let source = if let Some((old, at)) = pinned.filter(|(_, at)| *at == new_index) {
                self.variants.get(old).and_then(|variant| {
                    (config.radius() == 0.0
                        && variant.config.radius() == 0.0
                        && variant.levels.len() == 1)
                        .then_some(old)
                })
            } else {
                self.variants.iter().enumerate().find_map(|(old, variant)| {
                    (!permutation.contains(&old)
                        && pinned.is_none_or(|(reserved, _)| reserved != old)
                        && variant.config.same_target_slot(config)
                        && variant.levels.len() == blur_variant_level_count(config))
                        .then_some(old)
                })
            };
            let Some(source) = source else {
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI filter layout reuse rejected: reason=target_slot_mismatch new_slot={} radius={} levels={} old_active={} old_allocated={} pinned={:?}",
                    new_index, config.radius(), blur_variant_level_count(config),
                    self.active_variant_count, self.variants.len(), pinned,
                );
                return false;
            };
            if permutation.contains(&source) {
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI filter layout reuse rejected: reason=duplicate_physical_slot new_slot={} old_slot={}",
                    new_index, source,
                );
                return false;
            }
            permutation.push(source);
        }

        // Update the in-place physical target order using ORIGINAL positions.
        // Each swap moves the complete Texture/TextureView/ResourceSet owner;
        // the slot map is temporary and has no residency or version authority.
        let mut original_at_position: Vec<usize> = (0..self.variants.len()).collect();
        for (destination, (config, &original)) in
            next.iter().copied().zip(permutation.iter()).enumerate()
        {
            let source = original_at_position
                .iter()
                .position(|position| *position == original)
                .expect("preflight mapped an existing target");
            self.variants.swap(destination, source);
            original_at_position.swap(destination, source);
            self.variants[destination].config = config;
        }

        // Scratch is not a retained filter output: only the final variants
        // carry stable per-layer content epochs. Reusing its native allocation
        // under another packed index is safe after the preflight invalidation.
        for (source, index) in self.isolated_sources.iter_mut().zip(requested_isolated.iter()) {
            source.index = *index;
        }
        self.active_isolated_source_count = requested_isolated.len();
        self.active_variant_count = next.len();
        self.retained_root_variant = new_root;
        true
    }

}
