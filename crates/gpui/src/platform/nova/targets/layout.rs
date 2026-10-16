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
        // When a popup temporarily disables Root Color, the physical owner
        // remains allocated. Reintroducing Root must find that same Texture,
        // never relabel an arbitrary scratch target as the Root allocation.
        let previous_root_owner = old_root.or_else(|| {
            let native = *self.root_binding.as_ref()?;
            self.variants.iter().position(|variant| {
                variant.levels.len() == 1
                    && variant.levels[0].texture == native.texture
                    && variant.levels[0].texture_view == native.texture_view
            })
        });
        if let Some(root) = self.root_binding.as_ref() {
            let Some(owner_slot) = previous_root_owner else {
                // Never let a Filter-only descriptor impersonate an orphaned
                // Root native allocation.
                return false;
            };
            let Some(level) = self.variants.get(owner_slot)
                .and_then(|variant| variant.levels.first())
            else {
                return false;
            };
            if level.texture != root.texture
                || level.texture_view != root.texture_view
            {
                return false;
            }
            if new_root.is_none() && owner_slot < next.len() {
                // Root was removed from the logical DAG. Park its borrowed
                // Texture/View in a spare slot so no ordinary Capture can
                // sample it with empty (Root-owned) descriptor mirrors.
                let Some(spare) = (next.len()..self.variants.len())
                    .find(|&slot| slot != owner_slot)
                else {
                    return false; // requires a fresh Filter chain
                };
                self.variants.swap(owner_slot, spare);
            } else if let Some(current) = new_root {
                self.variants.swap(owner_slot, current);
            }
            if new_root.is_none() {
                // AutoStatic can still relocate independently while a former
                // Root allocation is parked in an inactive physical slot.
                if let Some((old, new)) = auto_static_slots {
                    self.variants.swap(old, new);
                }
            }
        } else if let (Some(previous), Some(current)) = (previous_root_owner, new_root) {
            // Legacy unowned fixtures never have Root residency.
            self.variants.swap(previous, current);
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
        let previous_root_owner = self.retained_root_variant.or_else(|| {
            let native = *self.root_binding.as_ref()?;
            self.variants.iter().position(|variant| {
                variant.levels.len() == 1
                    && variant.levels[0].texture == native.texture
                    && variant.levels[0].texture_view == native.texture_view
            })
        });
        // Physical Root storage must not be reassigned to a non-Root
        // Capture while its logical role is absent. Its descriptor arrays
        // intentionally remain empty: only RootColorTarget owns those sets.
        let native_root_slot = self.root_binding.as_ref().and_then(|root| {
            self.variants.iter().position(|variant| {
                variant.levels.len() == 1
                    && variant.levels[0].texture == root.texture
                    && variant.levels[0].texture_view == root.texture_view
            })
        });
        if self.root_binding.is_some() && native_root_slot.is_none() {
            return false;
        }
        let pinned = if let (Some(previous), Some(current)) =
            (previous_root_owner, new_root)
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

        // Plan ALL physical owners before mutating a live GPU descriptor. Reserve
        // the stable native Root/AutoStatic view, then prefer every exact target
        // identity. A later exact match must never be consumed by a preceding
        // zero-radius fallback (otherwise a safe permutation can spuriously fail).
        let mut permutation = smallvec::SmallVec::<[usize; 8]>::new();
        permutation.resize(next.len(), usize::MAX);
        if let Some((old, at)) = pinned {
            let valid = self.variants.get(old).is_some_and(|variant| {
                next[at].radius() == 0.0
                    && variant.config.radius() == 0.0
                    && variant.levels.len() == 1
            });
            if !valid {
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI filter layout reuse rejected: reason=pinned_target_incompatible old_slot={} new_slot={}",
                    old, at,
                );
                return false;
            }
            permutation[at] = old;
        }
        for (new_index, config) in next.iter().copied().enumerate() {
            if permutation[new_index] != usize::MAX {
                continue;
            }
            if let Some(source) = self.variants.iter().enumerate().find_map(|(old, variant)| {
                (!permutation.contains(&old)
                    && (new_root.is_some() || native_root_slot != Some(old))
                    && variant.config.same_target_slot(config)
                    && variant.levels.len() == blur_variant_level_count(config))
                    .then_some(old)
            }) {
                permutation[new_index] = source;
            }
        }

        // A zero-radius Capture always uses a native-size one-level target.
        // Its flattened filter member index is NOT the physical texture's
        // format, extent or descriptor layout. When an already resident Root
        // is pinned, spare one-level targets of the same native size can serve
        // changed popup indices. This does not apply to Gaussian (two-pass)
        // targets or AutoStatic-only identity relocation. The caller will
        // invalidate unproven captured pixels before any later sampling.
        let root_physical = self.retained_root_variant
            .filter(|_| new_root.is_some())
            .and_then(|_| self.root_binding.as_ref());
        let mut rebound_zero_radius = 0usize;
        for (new_index, config) in next.iter().copied().enumerate() {
            if permutation[new_index] != usize::MAX {
                continue;
            }
            if config.radius() == 0.0 {
                if let Some(root) = root_physical {
                    if let Some(source) = self.variants.iter().enumerate().find_map(|(old, variant)| {
                        (!permutation.contains(&old)
                            && (new_root.is_some() || native_root_slot != Some(old))
                            && variant.config.radius() == 0.0
                            && variant.levels.len() == 1
                            && variant.levels[0].byte_size == root.byte_size
                            && !variant.target_resource_sets.is_empty()
                            && variant.levels[0].pass_resource_sets.len()
                                == variant.target_resource_sets.len())
                            .then_some(old)
                    }) {
                        permutation[new_index] = source;
                        rebound_zero_radius += 1;
                        continue;
                    }
                }
            }
            // A Gaussian scratch allocation is keyed physically by the
            // downsample/level geometry, not its ephemeral flattened member
            // index or blur radius. When the Root has an independent pinned
            // owner, an unmatched Gaussian may borrow an unused SAME-SIZE
            // two-pass target. Its previous filtered pixels remain untrusted;
            // the renderer's topology invalidation must rebuild the output.
            //
            // Never use Root's one-level Texture/View, and never substitute a
            // different downsample or frame-slot descriptor capacity.
            if config.radius() > 0.0 && new_root.is_some()
                && pinned.is_some() && self.root_binding.is_some()
            {
                if let Some(source) = self.variants.iter().enumerate().find_map(|(old, variant)| {
                    (!permutation.contains(&old)
                        && native_root_slot != Some(old)
                        && variant.config.radius() > 0.0
                        && variant.config.downsample() == config.downsample()
                        && variant.levels.len() == blur_variant_level_count(config)
                        && variant.levels.iter().all(|level| {
                            level.pass_resource_sets.len() == variant.target_resource_sets.len()
                        })
                        && !variant.target_resource_sets.is_empty())
                        .then_some(old)
                }) {
                    permutation[new_index] = source;
                    log::trace!(
                        target: "gpui::diagnostics",
                        "GPUI Gaussian scratch target rebound: new_slot={} old_slot={} downsample={} levels={}",
                        new_index, source, config.downsample(), blur_variant_level_count(config),
                    );
                    continue;
                }
            }
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI filter layout reuse rejected: reason=target_slot_mismatch new_slot={} radius={} levels={} old_active={} old_allocated={} pinned={:?}",
                new_index, config.radius(), blur_variant_level_count(config),
                self.active_variant_count, self.variants.len(), pinned,
            );
            return false;
        }
        if rebound_zero_radius != 0 {
            log::debug!(
                target: "gpui::diagnostics",
                "GPUI zero-radius mixed layout permutation: rebound={} active={} allocated={} root_old={:?} root_new={:?}",
                rebound_zero_radius, next.len(), self.variants.len(),
                self.retained_root_variant, new_root,
            );
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
