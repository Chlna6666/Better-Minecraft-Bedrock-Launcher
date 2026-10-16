//! Per-window retained filter variants and cache validity, owned by the GPU consumer.
//! Painter-order source damage stays in BackdropBlurDamagePlan; full/partial redraw eligibility
//! does not replace that provenance with a global cache invalidation.
use super::*;

#[cfg(test)]
mod root_lifecycle_tests;

/// Logical content version of an offscreen GPU target, advanced only when a
/// successful Present made the freshly rendered texels eligible for reuse.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TextureContentVersion(pub(super) u64);

#[derive(Clone)]
pub(super) struct RetainedFilterSource {
    pub(super) index: u32,
    source: FilterSource,
    /// Last successful OFFSCREEN refresh for this particular filter.
    // All filters refreshed in the same Present share one immutable sample
    // snapshot; untouched filters retain the samples from their earlier refresh.
    // None represents an empty sample list without allocating an Arc.
    animation_values: Option<std::sync::Arc<[crate::SceneAnimationValue]>>,
    /// Content version is independent of compositor transform/opacity samples.
    content_version: TextureContentVersion,
}

/// The single authoritative content identity of the existing native Root
/// Color target. This is metadata for the GPU texture already owned by
/// BackdropBlurTargets, NOT another retained copy or texture allocation.
/// The flattened filter slot can change without changing native ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootColorResidency {
    texture: TextureId,
    view: TextureViewId,
    content_version: TextureContentVersion,
}

pub(super) struct FilterRegistry {
    pub(super) targets: Option<BackdropBlurTargets>,
    pub(super) atlas_generation: u64,
    pub(super) quality: Option<BackdropBlurQuality>,
    /// Sources sampled in the most recent successfully presented GPU frame.
    /// Snapshotting only on success prevents a deferred/failed present from
    /// validating an output texture that was never updated.
    pub(super) element_blur_inputs: Vec<RetainedFilterSource>,
    /// Last successfully submitted shader animation samples. A GPU-side
    /// retained target must be invalidated against this precise snapshot,
    /// not merely the latest UI View invalidation.
    pub(super) last_gpu_animation_values: Vec<crate::SceneAnimationValue>,
    next_content_version: u64,
    /// Resident window color has an independent content epoch. A popup's
    /// flattened filter-index change must not reset this epoch while the
    /// physical root texture and its pixels remain the same.
    root_color_residency: Option<RootColorResidency>,
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
            last_gpu_animation_values: Vec::new(),
            next_content_version: 0,
            root_color_residency: None,
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
        self.root_color_residency = None;
        self.valid = false;
        self.quality = None;
        self.element_blur_inputs.clear();
        self.last_gpu_animation_values.clear();
    }

    /// Only the packed content fingerprint establishes AutoStatic identity.
    /// A matching target is additionally required before its pixels can be reused.
    fn same_auto_static_source(current: &FilterSource, previous: &FilterSource) -> bool {
        match (current, previous) {
            (
                FilterSource::AutoStatic { bounds, content_mask, content_hash, content_bytes },
                FilterSource::AutoStatic {
                    bounds: old_bounds,
                    content_mask: old_mask,
                    content_hash: old_hash,
                    content_bytes: old_bytes,
                },
            ) => {
                bounds == old_bounds
                    && content_mask == old_mask
                    && content_hash == old_hash
                    && content_bytes == old_bytes
            }
            _ => false,
        }
    }

    /// Offer the sole successfully presented AutoStatic source for physical
    /// relocation across flattened filter indices. A new fingerprint must
    /// rebuild its texture and is never eligible for this fast path.
    pub(super) fn matching_auto_static_indices(
        &self,
        new_sources: &[(u32, FilterSource)],
    ) -> Option<(u32, u32)> {
        let mut old = self.element_blur_inputs.iter().filter(|entry| {
            matches!(entry.source, FilterSource::AutoStatic { .. })
        });
        let previous = old.next()?;
        if old.next().is_some() || previous.content_version.0 == 0 {
            return None;
        }
        let mut next = new_sources.iter().filter(|(_, source)| {
            matches!(source, FilterSource::AutoStatic { .. })
        });
        let (index, source) = next.next()?;
        if next.next().is_some() || !Self::same_auto_static_source(source, &previous.source) {
            return None;
        }
        Some((previous.index, *index))
    }

    /// Migrate resident snapshots only when original zero-radius TextureViews
    /// survive an index reorder. Elements use their stable Capture ID; the
    /// single AutoStatic source uses its packed fingerprint instead.
    /// Source content equivalence is still checked by source_unchanged().
    /// The per-rebind view lists are temporary, not another residency cache.
    pub(super) fn remap_identity_capture_sources(
        &mut self,
        old_views: &[(u32, TextureViewId)],
        new_views: &[(u32, TextureViewId)],
        new_sources: &[(u32, FilterSource)],
    ) -> usize {
        let previous = std::mem::take(&mut self.element_blur_inputs);
        let unique_auto_static = new_sources
            .iter()
            .filter(|(_, source)| matches!(source, FilterSource::AutoStatic { .. }))
            .count() == 1;
        for (index, incoming) in new_sources {
            let Some((_, target_view)) = new_views.iter().find(|(next, _)| next == index) else {
                continue;
            };
            let mut candidates = previous.iter().filter(|old| {
                if !old_views.iter().any(|(old_index, old_view)| {
                    *old_index == old.index && old_view == target_view
                }) {
                    return false;
                }
                match (incoming, &old.source) {
                    (FilterSource::AutoStatic { .. }, FilterSource::AutoStatic { .. }) => {
                        unique_auto_static && Self::same_auto_static_source(incoming, &old.source)
                    }
                    (FilterSource::Element(new_layer), FilterSource::Element(old_layer)) => {
                        new_layer.layer_id.is_some()
                            && new_layer.layer_id == old_layer.layer_id
                            && new_layer.radius.0 == 0.0
                            && old_layer.radius.0 == 0.0
                    }
                    _ => false,
                }
            });
            let Some(snapshot) = candidates.next() else { continue };
            // Multiple old matches cannot prove uniquely initialized texels.
            if candidates.next().is_some() { continue }
            let mut resident = snapshot.clone();
            resident.index = *index;
            self.element_blur_inputs.push(resident);
        }
        self.element_blur_inputs.sort_unstable_by_key(|source| source.index);
        self.element_blur_inputs.len()
    }

    pub(super) fn root_color_content_version(&self) -> Option<TextureContentVersion> {
        self.root_color_residency.map(|resident| resident.content_version)
    }

    /// Root Color content is independently valid only on its original native
    /// Texture and View. Flattened filter indices have no ownership authority.
    pub(super) fn root_color_resident_on(&self, targets: &BackdropBlurTargets) -> bool {
        self.root_color_residency
            .zip(targets.root_color_texture_identity())
            .is_some_and(|(resident, (texture, view))| {
                resident.texture == texture && resident.view == view
            })
    }

    /// Called only AFTER a successful GPU submission. When a root source pass
    /// did not run, keep the old version only if the exact native allocation
    /// and its view survive; otherwise its pixels are unproven. No new cache
    /// or GPU resource is created here.
    pub(super) fn record_root_color_present(
        &mut self,
        native_root: Option<(TextureId, TextureViewId)>,
        refreshed: bool,
    ) {
        match native_root {
            None => self.root_color_residency = None,
            Some((texture, view)) if refreshed => {
                self.next_content_version = self.next_content_version.wrapping_add(1).max(1);
                self.root_color_residency = Some(RootColorResidency {
                    texture,
                    view,
                    content_version: TextureContentVersion(self.next_content_version),
                });
            }
            Some((texture, view)) => {
                if !self.root_color_residency.is_some_and(|resident| {
                    resident.texture == texture && resident.view == view
                }) {
                    // Rebinding an output Texture without repainting it cannot
                    // import a previous Filter's content version.
                    self.root_color_residency = None;
                }
            }
        }
    }

    /// The GPU owns one resident source snapshot per flattened filter index.
    /// Keep the snapshots sorted so unchanged-layer queries need no frame-local
    /// hash table or repeated linear scan.
    fn source_at(&self, index: u32) -> Option<&RetainedFilterSource> {
        let position = self
            .element_blur_inputs
            .binary_search_by_key(&index, |source| source.index)
            .ok()?;
        self.element_blur_inputs.get(position)
    }

    pub(super) fn source_unchanged(
        &self,
        index: u32,
        current: &FilterSource,
        animation_values: &[crate::SceneAnimationValue],
    ) -> bool {
        let Some(previous) = self
            .source_at(index)
            .filter(|previous| previous.content_version.0 != 0)
        else {
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI filter source invalidated: index={} reason=missing_presented_content_version",
                index,
            );
            return false;
        };
        match (current, &previous.source) {
            (FilterSource::AutoStatic { .. }, FilterSource::AutoStatic { .. }) => {
                Self::same_auto_static_source(current, &previous.source)
            }
            (FilterSource::Element(current), FilterSource::Element(old)) => {
                let previous_values = previous.animation_values.as_deref().unwrap_or(&[]);
                if current.layer_id != old.layer_id
                    || current.bounds != old.bounds
                    || current.content_mask != old.content_mask
                    || current.radius != old.radius
                {
                    log::trace!(
                        target: "gpui::diagnostics",
                        "GPUI filter source invalidated: index={} reason=element_identity_or_geometry layer_changed={} bounds_changed={} mask_changed={} radius_changed={}",
                        index,
                        current.layer_id != old.layer_id,
                        current.bounds != old.bounds,
                        current.content_mask != old.content_mask,
                        current.radius != old.radius,
                    );
                    return false;
                }

                if crate::paint::retention::filter_source_matches(
                    &current.content,
                    &old.content,
                    animation_values,
                    previous_values,
                ) {
                    return true;
                }

                // A changed GPU sample outside an identical identity-capture
                // subtree does not invalidate its resident texture.
                let same_scene = std::sync::Arc::ptr_eq(&current.content, &old.content);
                let old_coordinates_valid = crate::paint::capture_preserves_source_coordinates(
                    current.radius, current.animation_id, previous_values.iter(),
                );
                let new_coordinates_valid = crate::paint::capture_preserves_source_coordinates(
                    current.radius, current.animation_id, animation_values.iter(),
                );
                if !same_scene || !old_coordinates_valid || !new_coordinates_valid {
                    log::trace!(
                        target: "gpui::diagnostics",
                        "GPUI filter source invalidated: index={} reason=nested_paint_or_animation_changed same_scene={} old_coordinates_valid={} new_coordinates_valid={} animation_samples={} old_samples={}",
                        index, same_scene, old_coordinates_valid, new_coordinates_valid,
                        animation_values.len(), previous_values.len(),
                    );
                    return false;
                }
                let visible_source = current.bounds.intersect(&current.content_mask.bounds);
                let damage = crate::paint::gpu_sample_damage(&current.content, previous_values, animation_values);
                let affected = damage.iter().any(|area| area.intersects(&visible_source));
                if affected {
                    log::trace!(
                        target: "gpui::diagnostics",
                        "GPUI filter source invalidated: index={} reason=sampled_child_pixels_changed regions={}",
                        index, damage.len(),
                    );
                }
                !affected
            }
            // A slot changing between root auto-cache and element blur can
            // never inherit an unrelated texture content version.
            _ => {
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI filter source invalidated: index={} reason=capture_source_kind_changed",
                    index,
                );
                false
            },
        }
    }

    /// Partial source damage is meaningful only for a real Element subtree.
    /// AutoStatic contains no animated content; packed byte fingerprints
    /// determine whether the whole source must be rebuilt.
    pub(super) fn local_source_damage(
        &self,
        index: u32,
        current: &FilterSource,
        animation_values: &[crate::SceneAnimationValue],
    ) -> Option<crate::DirtyRegion> {
        let previous = self
            .source_at(index)
            .filter(|source| source.content_version.0 != 0)?;
        let (FilterSource::Element(current), FilterSource::Element(old)) =
            (current, &previous.source)
        else {
            return None;
        };
        // The snapshot of the last *successful refresh for this layer* owns
        // the pixels we are updating. The global last-presented GPU sample may
        // have advanced while this particular filter was skipped.
        let previous_values = previous.animation_values.as_deref().unwrap_or(&[]);
        if current.layer_id != old.layer_id
            || current.bounds != old.bounds
            || current.content_mask != old.content_mask
            || current.radius != old.radius
            || !crate::paint::capture_preserves_source_coordinates(
                current.radius, current.animation_id, previous_values.iter(),
            )
            || !crate::paint::capture_preserves_source_coordinates(
                current.radius, current.animation_id, animation_values.iter(),
            )
        {
            return None;
        }
        // A UI commit may rebuild an equivalent child Scene into another Arc.
        // Prove *all* recursive static paint operations identical before
        // reusing any of the old texture. Passing empty sample arrays here
        // intentionally checks structural paint, then gpu_sample_damage below
        // accounts for every actual animation sample difference.
        let same_paint = std::sync::Arc::ptr_eq(&current.content, &old.content)
            || crate::paint::retention::filter_source_matches(&current.content, &old.content, &[], &[]);
        if !same_paint {
            return None;
        }
        let bounds = current.bounds.intersect(&current.content_mask.bounds);
        let mut damage = crate::DirtyRegion::empty();
        for region in crate::paint::gpu_sample_damage(&current.content, previous_values, animation_values) {
            damage.push(region.intersect(&bounds));
        }
        let result = (!damage.is_empty()).then_some(damage);
        if result.is_some() && !std::sync::Arc::ptr_eq(&current.content, &old.content) {
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI isolated source partial damage reused across equivalent Scene revisions: index={} changed_rects={}",
                index,
                result.as_ref().map_or(0, crate::DirtyRegion::rect_count),
            );
        }
        result
    }

    pub(super) fn record_element_blur_inputs(
        &mut self,
        sources: &[(u32, FilterSource)],
        animation_values: &[crate::SceneAnimationValue],
        refreshed_indices: impl IntoIterator<Item = u32>,
    ) {
        // The encoder allocates flattened filter indices monotonically, even
        // through nested captures. The source list is therefore ordered by index.
        // A successful Present is also the ownership boundary: discard snapshots
        // for filters absent from this frame, releasing their child Scene Arcs.
        // Keep present-but-unrefreshed sources at their previous GPU version.
        debug_assert!(sources.windows(2).all(|pair| pair[0].0 < pair[1].0));
        self.element_blur_inputs.retain(|previous| {
            sources
                .binary_search_by_key(&previous.index, |(index, _)| *index)
                .is_ok()
        });
        // One allocation per nonempty refresh generation, not per filter:
        // independent targets refreshed on later frames still keep their own
        // earlier immutable sample snapshots.
        let mut shared_samples: Option<std::sync::Arc<[crate::SceneAnimationValue]>> = None;
        // Only texture passes actually executed may advance their source version.
        for index in refreshed_indices {
            let Ok(source_position) =
                sources.binary_search_by_key(&index, |(source_index, _)| *source_index)
            else {
                continue;
            };
            let current = &sources[source_position].1;
            self.next_content_version = self.next_content_version.wrapping_add(1).max(1);
            let snapshot = RetainedFilterSource {
                index,
                source: current.clone(),
                animation_values: if matches!(current, FilterSource::AutoStatic { .. })
                    || animation_values.is_empty()
                {
                    None
                } else {
                    Some(std::sync::Arc::clone(shared_samples.get_or_insert_with(|| {
                        std::sync::Arc::from(animation_values)
                    })))
                },
                content_version: TextureContentVersion(self.next_content_version),
            };
            match self
                .element_blur_inputs
                .binary_search_by_key(&index, |previous| previous.index)
            {
                Ok(position) => self.element_blur_inputs[position] = snapshot,
                Err(position) => self.element_blur_inputs.insert(position, snapshot),
            }
        }
    }

    /// Texture contents become resident only through record_element_blur_inputs
    /// after a successful presentation. A retained layer skipped by the Render
    /// Graph keeps its previous version.
    pub(super) fn layer_content_version(&self, index: u32) -> Option<TextureContentVersion> {
        self.source_at(index).map(|source| source.content_version)
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
    fn root_color_version_survives_only_texture_preserving_topology_rebind() {
        let mut registry = FilterRegistry::new(None);
        assert_eq!(registry.root_color_content_version(), None);
        registry.record_submission(BackdropBlurQuality::Full, 9);
        let root1 = (TextureId::from_parts(10, 1), TextureViewId::from_parts(11, 1));
        let root2 = (TextureId::from_parts(12, 1), TextureViewId::from_parts(13, 1));
        registry.record_root_color_present(Some(root1), true);
        let resident = registry.root_color_content_version().expect("resident color");
        let source = FilterSource::AutoStatic {
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            content_hash: 0xbeef,
            content_bytes: 64,
        };
        registry.record_element_blur_inputs(&[(3, source.clone())], &[], [3]);
        assert!(registry.source_unchanged(3, &source, &[]));

        registry.remap_identity_capture_sources(&[], &[], &[(3, source.clone())]);
        assert!(
            !registry.source_unchanged(3, &source, &[]),
            "a remapped popup must rebuild even when an old index had valid pixels"
        );
        assert_eq!(registry.root_color_content_version(), Some(resident));
        assert!(!registry.refresh_required(BackdropBlurQuality::Full, false));
        registry.record_root_color_present(Some(root1), false);
        assert_eq!(registry.root_color_content_version(), Some(resident));
        // A target rebind without a source draw must never inherit old pixels.
        registry.record_root_color_present(Some(root2), false);
        assert_eq!(registry.root_color_content_version(), None);
        registry.record_root_color_present(Some(root2), true);
        assert_ne!(registry.root_color_content_version(), Some(resident));

        // A native target retirement, a resize, or atlas invalidation still
        // discards the physical root residency as well as filter snapshots.
        registry.record_root_color_present(None, false);
        assert!(registry.root_color_content_version().is_none());
        registry.record_root_color_present(Some(root1), true);
        registry.invalidate();
        assert!(registry.root_color_content_version().is_none());
        assert!(registry.refresh_required(BackdropBlurQuality::Full, false));
    }

    #[test]
    fn zero_filter_target_reorder_preserves_only_same_layer_and_texture() {
        let mut registry = FilterRegistry::new(None);
        let make = |name: &'static str| FilterSource::Element(crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: Some(crate::GlobalElementId::from_path(&[name.into()])),
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        });
        let first = make("first");
        let second = make("second");
        registry.record_element_blur_inputs(&[(2, first.clone()), (4, second.clone())], &[], [2, 4]);
        let version1 = registry.layer_content_version(2);
        let version2 = registry.layer_content_version(4);
        let v1 = TextureViewId::new(101);
        let v2 = TextureViewId::new(102);
        assert_eq!(registry.remap_identity_capture_sources(
            &[(2, v1), (4, v2)],
            &[(5, v1), (7, v2)],
            &[(5, first.clone()), (7, second.clone())],
        ), 2);
        assert_eq!(registry.layer_content_version(5), version1);
        assert_eq!(registry.layer_content_version(7), version2);
        assert!(registry.source_unchanged(5, &first, &[]));
        assert!(registry.source_unchanged(7, &second, &[]));

        let replacement = make("replacement");
        assert_eq!(registry.remap_identity_capture_sources(
            &[(5, v1), (7, v2)],
            &[(5, v1), (7, TextureViewId::new(103))],
            &[(5, replacement.clone()), (7, second.clone())],
        ), 0);
        assert!(registry.layer_content_version(5).is_none());
        assert!(registry.layer_content_version(7).is_none());
        assert!(!registry.source_unchanged(5, &replacement, &[]));

        // A Gaussian filter cannot inherit a direct-compositor target.
        registry.record_element_blur_inputs(&[(5, first.clone())], &[], [5]);
        let mut gaussian = match first { FilterSource::Element(layer) => layer, _ => unreachable!() };
        gaussian.radius = crate::ScaledPixels(3.0);
        assert_eq!(registry.remap_identity_capture_sources(
            &[(5, v1)], &[(9, v1)],
            &[(9, FilterSource::Element(gaussian))],
        ), 0);
    }

    #[test]
    fn remapped_filter_source_still_rejects_changed_content_geometry() {
        let mut registry = FilterRegistry::new(None);
        let mut blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: Some(crate::GlobalElementId::from_path(&["stable".into()])),
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        registry.record_element_blur_inputs(&[(1, FilterSource::Element(blur.clone()))], &[], [1]);
        blur.bounds = crate::bounds(
            crate::point(crate::ScaledPixels(8.0), crate::ScaledPixels(8.0)),
            crate::size(crate::ScaledPixels(16.0), crate::ScaledPixels(16.0)),
        );
        let v = TextureViewId::new(303);
        assert_eq!(registry.remap_identity_capture_sources(
            &[(1, v)], &[(3, v)], &[(3, FilterSource::Element(blur.clone()))],
        ), 1);
        assert!(!registry.source_unchanged(3, &FilterSource::Element(blur), &[]));
    }

    #[test]
    fn auto_static_rebind_preserves_only_submitted_identical_pixels_on_same_view() {
        let mut registry = FilterRegistry::new(None);
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
        );
        let source = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 0xdead_beef,
            content_bytes: 4096,
        };
        let view = TextureViewId::new(28);
        assert_eq!(registry.matching_auto_static_indices(&[(5, source.clone())]), None);
        registry.record_element_blur_inputs(&[(2, source.clone())], &[], [2]);
        registry.record_submission(BackdropBlurQuality::Full, 1);
        let resident = registry.layer_content_version(2);
        assert_eq!(registry.matching_auto_static_indices(&[(5, source.clone())]), Some((2, 5)));
        assert_eq!(registry.remap_identity_capture_sources(
            &[(2, view)], &[(5, view)], &[(5, source.clone())],
        ), 1);
        assert_eq!(registry.layer_content_version(5), resident);
        assert!(registry.source_unchanged(5, &source, &[]));

        assert_eq!(registry.remap_identity_capture_sources(
            &[(5, view)], &[(3, TextureViewId::new(29))],
            &[(3, source.clone())],
        ), 0);
        assert!(registry.layer_content_version(3).is_none());
    }

    #[test]
    fn auto_static_rebind_rejects_fingerprint_or_geometry_change() {
        let mut registry = FilterRegistry::new(None);
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(2.0), crate::ScaledPixels(3.0)),
            crate::size(crate::ScaledPixels(128.0), crate::ScaledPixels(64.0)),
        );
        let old = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 12345,
            content_bytes: 8192,
        };
        registry.record_element_blur_inputs(&[(1, old.clone())], &[], [1]);
        let changed = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 12346,
            content_bytes: 8192,
        };
        assert_eq!(registry.matching_auto_static_indices(&[(4, changed.clone())]), None);
        let view = TextureViewId::new(37);
        assert_eq!(registry.remap_identity_capture_sources(
            &[(1, view)], &[(4, view)], &[(4, changed.clone())],
        ), 0);
        assert!(!registry.source_unchanged(4, &changed, &[]));

        registry.record_element_blur_inputs(&[(1, old.clone())], &[], [1]);
        let moved_bounds = crate::bounds(
            crate::point(crate::ScaledPixels(6.0), crate::ScaledPixels(3.0)),
            crate::size(crate::ScaledPixels(128.0), crate::ScaledPixels(64.0)),
        );
        let moved = FilterSource::AutoStatic {
            bounds: moved_bounds,
            content_mask: crate::ContentMask::new(moved_bounds),
            content_hash: 12345,
            content_bytes: 8192,
        };
        assert_eq!(registry.matching_auto_static_indices(&[(5, moved.clone())]), None);
        assert_eq!(registry.remap_identity_capture_sources(
            &[(1, view)], &[(5, view)], &[(5, moved)],
        ), 0);
    }

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
    fn auto_static_source_is_resident_only_after_successful_target_refresh() {
        let mut registry = FilterRegistry::new(None);
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(256.0), crate::ScaledPixels(128.0)),
        );
        let source = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 0x123456789abcdef0,
            content_bytes: 4096,
        };
        assert!(!registry.source_unchanged(0, &source, &[]));
        registry.record_element_blur_inputs(&[(0, source.clone())], &[], []);
        assert!(!registry.source_unchanged(0, &source, &[]));
        registry.record_element_blur_inputs(&[(0, source.clone())], &[], [0]);
        assert!(registry.source_unchanged(0, &source, &[]));
        let version = registry.layer_content_version(0).expect("resident texture");
        let changed = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 0x123456789abcdef1,
            content_bytes: 4096,
        };
        assert!(!registry.source_unchanged(0, &changed, &[]));
        assert_eq!(registry.layer_content_version(0), Some(version));
        registry.record_element_blur_inputs(&[(0, changed.clone())], &[], [0]);
        assert!(registry.source_unchanged(0, &changed, &[]));
        assert_ne!(registry.layer_content_version(0), Some(version));

        let real = FilterSource::Element(crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        });
        assert!(!registry.source_unchanged(0, &real, &[]));
        registry.invalidate();
        assert!(!registry.source_unchanged(0, &changed, &[]));
    }

    #[test]
    fn simultaneous_filter_refreshes_share_samples_but_skipped_sources_keep_old_generation() {
        let mut registry = FilterRegistry::new(None);
        let source = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        let sources = [
            (2, FilterSource::Element(source.clone())),
            (4, FilterSource::Element(source)),
        ];
        let sample = |progress| crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: crate::TransitionProperty::Opacity,
            progress,
            from: [0.0; 4],
            to: [1.0, 0.0, 0.0, 0.0],
        };
        registry.record_element_blur_inputs(&sources, &[sample(0.2)], [2, 4]);
        let first = registry.element_blur_inputs[0]
            .animation_values
            .as_ref()
            .expect("first sample snapshot")
            .clone();
        let second = registry.element_blur_inputs[1]
            .animation_values
            .as_ref()
            .expect("second sample snapshot");
        assert!(std::sync::Arc::ptr_eq(&first, second));
        registry.record_element_blur_inputs(&sources, &[sample(0.8)], [4]);
        assert!(std::sync::Arc::ptr_eq(
            &first,
            registry.element_blur_inputs[0].animation_values.as_ref().unwrap(),
        ));
        assert!(!std::sync::Arc::ptr_eq(
            &first,
            registry.element_blur_inputs[1].animation_values.as_ref().unwrap(),
        ));
        assert_eq!(registry.element_blur_inputs[0].animation_values.as_deref().unwrap()[0].progress, 0.2);
        assert_eq!(registry.element_blur_inputs[1].animation_values.as_deref().unwrap()[0].progress, 0.8);
        registry.invalidate();
        assert!(registry.element_blur_inputs.is_empty());
    }

    #[test]
    fn removed_filter_releases_nested_scene_without_invalidating_live_source() {
        let mut registry = FilterRegistry::new(None);
        let bounds = crate::Bounds::default();
        let live = FilterSource::AutoStatic {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            content_hash: 123,
            content_bytes: 4096,
        };
        let scene = std::sync::Arc::new(crate::Scene::default());
        let removed = FilterSource::Element(crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(4.0),
            opacity: 1.0,
            content: std::sync::Arc::clone(&scene),
        });
        // A reversed refresh order must not disturb the sorted resident index.
        registry.record_element_blur_inputs(&[(2, live.clone()), (7, removed)], &[], [7, 2]);
        assert_eq!(registry.element_blur_inputs.iter().map(|source| source.index).collect::<Vec<_>>(), vec![2, 7]);
        assert_eq!(std::sync::Arc::strong_count(&scene), 2);
        let live_version = registry.layer_content_version(2);
        registry.record_element_blur_inputs(&[(2, live.clone())], &[], []);
        assert_eq!(registry.layer_content_version(2), live_version);
        assert!(registry.source_unchanged(2, &live, &[]));
        assert_eq!(registry.layer_content_version(7), None);
        assert_eq!(std::sync::Arc::strong_count(&scene), 1);
    }

    #[test]
    fn isolated_blur_reuses_source_when_only_composite_opacity_changes() {
        let mut registry = FilterRegistry::new(None);
        let scene = std::sync::Arc::new(crate::Scene::default());
        let blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(8.0),
            opacity: 1.0,
            content: scene,
        };
        registry.record_element_blur_inputs(&[(3, FilterSource::Element(blur.clone()))], &[], [3]);
        let mut changed_composite = blur.clone();
        changed_composite.opacity = 0.5;
        assert!(registry.source_unchanged(3, &FilterSource::Element(changed_composite.clone()), &[]));
        let first_version = registry.layer_content_version(3).expect("first resident texture");
        registry.record_element_blur_inputs(&[(3, FilterSource::Element(blur.clone()))], &[], []);
        assert_eq!(registry.layer_content_version(3), Some(first_version));
        registry.record_element_blur_inputs(&[(3, FilterSource::Element(blur.clone()))], &[], [3]);
        assert_ne!(registry.layer_content_version(3), Some(first_version));
        changed_composite.radius = crate::ScaledPixels(12.0);
        assert!(!registry.source_unchanged(3, &FilterSource::Element(changed_composite.clone()), &[]));
        assert!(!registry.source_unchanged(4, &FilterSource::Element(blur.clone()), &[]));
        registry.invalidate();
        assert!(!registry.source_unchanged(3, &FilterSource::Element(blur.clone()), &[]));
    }

    #[test]
    fn distinct_stable_layer_identity_prevents_slot_alias_reuse() {
        let mut registry = FilterRegistry::new(None);
        let mut source = crate::PaintBlur {
            order: 0,
            layer_id: Some(crate::GlobalElementId::from_path(&["page-one".into()])),
            animation_id: None,
            bounds: crate::Bounds::default(),
            content_mask: crate::ContentMask::default(),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        registry.record_element_blur_inputs(&[(4, FilterSource::Element(source.clone()))], &[], [4]);
        let previous_version = registry.layer_content_version(4).expect("resident source");
        assert!(registry.source_unchanged(4, &FilterSource::Element(source.clone()), &[]));
        source.layer_id = Some(crate::GlobalElementId::from_path(&["page-two".into()]));
        assert!(!registry.source_unchanged(4, &FilterSource::Element(source.clone()), &[]));
        // A reused flattened index must never imply that the old GPU texture
        // already holds the new layer's pixels.
        registry.record_element_blur_inputs(&[(4, FilterSource::Element(source.clone()))], &[], [4]);
        assert_ne!(registry.layer_content_version(4), Some(previous_version));
        assert!(registry.source_unchanged(4, &FilterSource::Element(source.clone()), &[]));
    }

    #[test]
    fn identity_compositor_preserves_only_unchanged_immutable_source_geometry() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(10.), crate::ScaledPixels(20.)),
            crate::size(crate::ScaledPixels(16.), crate::ScaledPixels(16.)),
        );
        let mut scene = crate::Scene::default();
        scene.insert_animated_primitive(
            crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            },
            crate::SceneAnimationId(1),
        );
        let sample = |progress| crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: crate::TransitionProperty::Opacity,
            progress,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        };
        let mut source = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
            content: std::sync::Arc::new(scene),
        };
        let mut registry = FilterRegistry::new(None);
        registry.record_element_blur_inputs(&[(1, FilterSource::Element(source.clone()))], &[sample(0.2)], [1]);
        registry.record_submission(BackdropBlurQuality::Full, 0);
        registry.last_gpu_animation_values.push(sample(0.2));
        registry.begin_refresh();
        assert_eq!(
            registry
                .local_source_damage(1, &FilterSource::Element(source.clone()), &[sample(0.8)])
                .expect("local source damage")
                .union_bounds(),
            Some(bounds)
        );
        assert!(
            registry
                .local_source_damage(1, &FilterSource::Element(source.clone()), &[sample(0.2)])
                .is_none()
        );
        source.radius = crate::ScaledPixels(8.);
        assert!(
            registry
                .local_source_damage(1, &FilterSource::Element(source.clone()), &[sample(0.8)])
                .is_none()
        );
        source.radius = crate::ScaledPixels(0.);
        let mut replacement = crate::Scene::default();
        replacement.replay(0..source.content.len(), &source.content);
        source.content = std::sync::Arc::new(replacement);
        assert_eq!(
            registry
                .local_source_damage(1, &FilterSource::Element(source.clone()), &[sample(0.8)])
                .expect("equivalent Arc-rebuilt content must retain local damage")
                .union_bounds(),
            Some(bounds)
        );
    }

    #[test]
    fn isolated_source_uses_last_resident_samples_not_last_submitted_frame() {
        let area = crate::bounds(
            crate::point(crate::ScaledPixels(12.), crate::ScaledPixels(14.)),
            crate::size(crate::ScaledPixels(100.), crate::ScaledPixels(80.)),
        );
        let mut scene = crate::Scene::default();
        scene.insert_animated_primitive(
            crate::Quad {
                bounds: area,
                content_mask: crate::ContentMask::new(area),
                ..Default::default()
            },
            crate::SceneAnimationId(21),
        );
        let old_scene = std::sync::Arc::new(scene);
        let sample = |progress| crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(21),
            property: crate::TransitionProperty::Opacity,
            progress,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        };
        let old_source = crate::PaintBlur {
            order: 0, animation_id: None, layer_id: None,
            bounds: area, content_mask: crate::ContentMask::new(area),
            radius: crate::ScaledPixels(0.0), opacity: 1.0,
            content: old_scene.clone(),
        };
        let mut registry = FilterRegistry::new(None);
        registry.record_element_blur_inputs(
            &[(1, FilterSource::Element(old_source.clone()))],
            &[sample(0.2)], [1],
        );
        // A newer frame can be submitted without refreshing this texture.
        // Its resident pixels must still be compared against sample(0.2).
        registry.last_gpu_animation_values = vec![sample(0.7)];
        let mut equivalent = crate::Scene::default();
        equivalent.replay(0..old_scene.len(), &old_scene);
        let mut rebuilt = old_source;
        rebuilt.content = std::sync::Arc::new(equivalent);
        assert_eq!(
            registry.local_source_damage(
                1, &FilterSource::Element(rebuilt.clone()), &[sample(0.7)],
            ).expect("old resident sample must cause damage").union_bounds(),
            Some(area),
        );
        // A genuinely changed static child may not inherit old source texels.
        let mut changed = crate::Scene::default();
        changed.insert_animated_primitive(
            crate::Quad {
                bounds: crate::bounds(
                    crate::point(crate::ScaledPixels(20.), crate::ScaledPixels(14.)),
                    crate::size(crate::ScaledPixels(100.), crate::ScaledPixels(80.)),
                ),
                content_mask: crate::ContentMask::new(area),
                ..Default::default()
            },
            crate::SceneAnimationId(21),
        );
        rebuilt.content = std::sync::Arc::new(changed);
        assert!(registry.local_source_damage(
            1, &FilterSource::Element(rebuilt), &[sample(0.7)],
        ).is_none(), "changing static painter geometry must force full capture");
    }

    #[test]
    fn identity_capture_ignores_child_animation_entirely_outside_its_visible_source() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(16.), crate::ScaledPixels(16.)),
        );
        let far_bounds = crate::bounds(
            crate::point(crate::ScaledPixels(100.), crate::ScaledPixels(100.)),
            crate::size(crate::ScaledPixels(8.), crate::ScaledPixels(8.)),
        );
        let mut child = crate::Scene::default();
        child.insert_animated_primitive(
            crate::Quad {
                bounds: far_bounds,
                content_mask: crate::ContentMask::new(far_bounds),
                ..Default::default()
            },
            crate::SceneAnimationId(7),
        );
        let sample = |progress: f32| crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(7),
            property: crate::TransitionProperty::Opacity,
            progress,
            from: [0., 0., 0., 0.],
            to: [1., 0., 0., 0.],
        };
        let source = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
            content: std::sync::Arc::new(child),
        };
        let mut registry = FilterRegistry::new(None);
        registry.record_element_blur_inputs(&[(2, FilterSource::Element(source.clone()))], &[sample(0.1)], [2]);
        assert!(registry.source_unchanged(2, &FilterSource::Element(source.clone()), &[sample(0.9)]));
        let mut filtered = source.clone();
        filtered.radius = crate::ScaledPixels(8.);
        assert!(!registry.source_unchanged(2, &FilterSource::Element(filtered.clone()), &[sample(0.9)]));
        let mut shifted = source.clone();
        shifted.bounds = far_bounds;
        shifted.content_mask = crate::ContentMask::new(far_bounds);
        assert!(!registry.source_unchanged(2, &FilterSource::Element(shifted.clone()), &[sample(0.9)]));
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
