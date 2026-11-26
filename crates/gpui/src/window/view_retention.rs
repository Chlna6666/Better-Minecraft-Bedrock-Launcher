//! Whole-view retained replay.
//!
//! A view that was not notified, read nothing that changed, and inherits the same context can be
//! drawn again from the committed frame: its persistent Taffy nodes are claimed as they are, and
//! its prepaint and paint ranges are copied into the new frame. Nothing in the view's subtree is
//! built, measured, prepainted or painted again.
//!
//! This is the retention boundary the element-level proofs cannot provide. A view's contents are
//! derived by its own `Render`; when that did not have to run, no element inside it can have a
//! different answer than last frame, no matter how small the elements are.

use super::draw_reuse::{rebase_paint_range, rebase_prepaint_range};
use super::*;
use crate::layout::{ClaimBlockMap, LayoutClaimBlocks};

/// Identity and effective inherited values at RequestLayout, distinct from Prepaint.
pub(crate) struct ViewRebuild {
    pub(super) view: crate::AnyWeakView,
    pub(super) application_id: GlobalElementId,
    pub(super) layout: RetainedPaintContext,
    pub(super) splice_supported: bool,
}

/// Everything needed to draw one view subtree again without building it.
#[derive(Clone)]
pub(crate) struct RetainedViewRecord {
    /// The view's committed layout root, still owned by the layered layout engine.
    pub(crate) layout_root: LayoutId,
    /// Every persistent node the subtree claimed, in claim order.
    pub(crate) layout_claims: LayoutClaimBlocks,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) bounds: Bounds<Pixels>,
    /// The inherited context the ranges were recorded under.
    pub(crate) context: Rc<RetainedPaintContext>,
    pub(crate) rebuild: Option<Rc<ViewRebuild>>,
    pub(crate) splice_safe: bool,
    /// Descendant span in the frame's prepaint-order view table.
    pub(crate) nested: Range<usize>,
    /// Descendant damage segments in paint order (the enclosing view's own segment follows).
    pub(crate) scene_segments: Range<usize>,
    /// Only states taken during request layout. Prepaint and paint replay their own spans.
    pub(crate) layout_state_range: Range<usize>,
}

/// Prepaint order is also record order. A reservation without a paint commit
/// remains a hole, and cannot be selected as a rebuild/replay source.
pub(crate) struct ViewRecordSlot {
    pub(crate) key: GlobalElementId,
    pub(crate) record: Option<RetainedViewRecord>,
}

/// Where the view's own record is inserted once it has painted.
pub(crate) struct ViewRetentionSlot(usize);

pub(crate) struct ViewRetentionCommit {
    pub(crate) layout_root: LayoutId,
    pub(crate) layout_claims: LayoutClaimBlocks,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) context: Rc<RetainedPaintContext>,
    pub(crate) rebuild: Option<Rc<ViewRebuild>>,
    pub(crate) splice_safe: bool,
    pub(crate) scene_segments_start: usize,
    pub(crate) layout_state_range: Range<usize>,
    /// The retained source this frame copied, when the view was reused.
    pub(crate) reused: Option<RetainedViewRecord>,
}

impl Window {
    pub(crate) fn splice_context_supported(&self) -> bool {
        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.inspector_enabled() {
            return false;
        }
        self.image_cache_stack.is_empty() && self.scene_animation.is_none()
    }

    pub(crate) fn splice_paint_context_matches(&self, context: &RetainedPaintContext) -> bool {
        self.splice_context_supported() && self.retained_paint_context_matches(context)
    }

    /// Keep a weak view handle, so a committed frame does not extend entity lifetimes.
    pub(crate) fn capture_view_rebuild(
        &self,
        view: &crate::AnyView,
        application_id: Option<&GlobalElementId>,
    ) -> Option<Rc<ViewRebuild>> {
        Some(Rc::new(ViewRebuild {
            view: view.downgrade(),
            application_id: application_id?.clone(),
            layout: self.current_retained_paint_context(),
            splice_supported: self.splice_context_supported(),
        }))
    }

    /// Whether a view may be drawn again from the committed frame at all.
    ///
    /// The per-view dirty scope already carries every dependency edge: a notified view, a view
    /// that read an entity whose generation moved, and a traversal ancestor on the path to either
    /// are all in it.
    pub(crate) fn view_retention_eligible(&self, view_id: EntityId, cx: &App) -> bool {
        !cfg!(gpui_hot_reload)
            && !self.force_view_cache_refresh
            && !cx.has_active_drag()
            && !self.is_inspector_picking(cx)
            && self.view_dirty_scope(view_id).is_none()
    }

    /// Walk retained records in prepaint order. Only directly dirty views
    /// become gaps; traversal ancestors remain open around their descendants.
    pub(super) fn splice_subtrees(&self, anchor: &RetainedViewRecord) -> Option<Vec<(usize, Option<ViewDirtyScope>)>> {
        let table = &self.rendered_frame.view_retention_records;
        let entries = table.get(anchor.nested.clone())?;
        let mut owners = FxHashSet::default();
        let mut ancestors = smallvec::SmallVec::<[usize; 8]>::new();
        for (offset, entry) in entries.iter().enumerate() {
            let record = entry.record.as_ref()?;
            let owner = record.rebuild.as_ref()?.view.entity_id();
            let index = anchor.nested.start + offset;
            while ancestors.last().is_some_and(|end| *end == index) { ancestors.pop(); }
            if ancestors.last().is_some_and(|end| record.nested.end > *end)
                || !owners.insert(owner)
                || self.rendered_frame.view_retentions.get(&entry.key).copied() != Some(index)
                || record.nested.start != index + 1 || record.nested.end > anchor.nested.end
                || record.nested.end < record.nested.start
            {
                return None;
            }
            if !record.nested.is_empty() { ancestors.push(record.nested.end); }
        }
        let mut result = Vec::new();
        let mut index = anchor.nested.start;
        while index < anchor.nested.end {
            let record = table[index].record.as_ref()?;
            let rebuild = record.rebuild.as_ref()?;
            let scope = self.view_dirty_scope(rebuild.view.entity_id());
            if scope.is_some() && !rebuild.splice_supported { return None; }
            if scope.is_none() && table[record.nested.clone()].iter().any(|entry| {
                entry.record.as_ref().and_then(|record| record.rebuild.as_ref())
                    .is_none_or(|child| self.view_dirty_scope(child.view.entity_id()).is_some())
            }) {
                return None;
            }
            result.push((index, scope));
            index = if scope == Some(ViewDirtyScope::TraversalAncestor) {
                index + 1
            } else {
                record.nested.end
            };
        }
        // A missing dirty descendant is not evidence that the traversal
        // ancestor can be retained. Fall back instead of losing invalidation.
        let mut next_dirty = None;
        for &(index, scope) in result.iter().rev() {
            match scope {
                Some(ViewDirtyScope::Direct) => next_dirty = Some(index),
                Some(ViewDirtyScope::TraversalAncestor) => {
                    if next_dirty.is_none_or(|dirty| dirty >= table[index].record.as_ref().unwrap().nested.end) {
                        return None;
                    }
                }
                None => {}
            }
        }
        next_dirty.map(|_| result)
    }

    /// Reserve the entire clean subtree in the candidate table at its new
    /// prepaint position. Its actual records are installed during paint.
    pub(super) fn reserve_spliced_clean(
        &mut self, key: GlobalElementId, record: &RetainedViewRecord,
    ) -> (ViewRetentionSlot, Range<usize>) {
        let slot = self.reserve_view_retention(key);
        self.next_frame.view_retention_records.extend(
            self.rendered_frame.view_retention_records[record.nested.clone()].iter().map(|entry| {
                ViewRecordSlot { key: entry.key.clone(), record: None }
            }),
        );
        let nested = self.finish_view_retention_scope(&slot);
        (slot, nested)
    }

    /// A clean subtree's own damage entry is emitted after its descendant
    /// record is committed. Unlike ordinary view replay, no wrapper is painted
    /// here, so the original own entry must also be carried.
    pub(super) fn commit_spliced_clean(
        &mut self, slot: &ViewRetentionSlot, nested: Range<usize>,
        source: &RetainedViewRecord, claims: LayoutClaimBlocks,
        prepaint: Range<PrepaintStateIndex>, paint: Range<PaintIndex>,
        states: Range<usize>,
    ) {
        let owner = source.rebuild.as_ref().expect("admitted clean subtree").view.entity_id();
        let old_scene = source.paint_range.start.scene_index..source.paint_range.end.scene_index;
        let new_scene = paint.start.scene_index..paint.end.scene_index;
        let own = self.rendered_frame.retained_scene_segments.get(source.scene_segments.end)
            .filter(|entry| entry.entity_id == owner && entry.scene_range == old_scene)
            .cloned();
        let scene_segments_start = self.next_frame.retained_scene_segments.len();
        self.commit_view_retention(slot, nested, ViewRetentionCommit {
            layout_root: source.layout_root, layout_claims: claims,
            prepaint_range: prepaint, paint_range: paint, bounds: source.bounds,
            context: source.context.clone(), rebuild: source.rebuild.clone(),
            splice_safe: source.splice_safe, scene_segments_start,
            layout_state_range: states, reused: Some(source.clone()),
        });
        if let Some(mut own) = own {
            if let Some(range) = rebase_span(&own.scene_range, &old_scene, &new_scene) {
                own.scene_range = range;
                self.next_frame.push_retained_scene_segment(own);
            }
        }
    }

    /// Claims the committed layout of `key` and returns its record, or `None` when the view has to
    /// be built.
    ///
    /// The returned record is the one admitted for this frame. Callers that decide against reuse
    /// later must release it with [`Window::release_retained_view`].
    pub(crate) fn admit_retained_view(
        &mut self,
        key: &GlobalElementId,
        view_id: EntityId,
        application_id: Option<&GlobalElementId>,
        cx: &App,
    ) -> Option<(RetainedViewRecord, LayoutClaimBlocks)> {
        if !self.view_retention_eligible(view_id, cx) {
            return None;
        }
        let index = *self.rendered_frame.view_retentions.get(key)?;
        let record = self.rendered_frame.view_retention_records.get(index)?.record.as_ref()?.clone();
        let rebuild = record.rebuild.as_ref()?;
        if rebuild.view.entity_id() != view_id
            || Some(&rebuild.application_id) != application_id
            || rebuild.splice_supported != self.splice_context_supported()
            || !self.retained_paint_context_matches(&rebuild.layout)
        {
            return None;
        }
        if record.prepaint_range.start.deferred_draws_index != record.prepaint_range.end.deferred_draws_index
            || record.prepaint_range.start.tooltips_index != record.prepaint_range.end.tooltips_index
            || self.rendered_frame.accessed_element_states.get(record.layout_state_range.clone()).is_none()
            || self.rendered_frame.view_retention_records.get(record.nested.clone()).is_none()
            || self.rendered_frame.retained_scene_segments.get(record.scene_segments.clone()).is_none()
        {
            return None;
        }
        let claims = self.layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .try_keep_retained(&record.layout_claims, record.layout_root)?;
        Some((record, claims))
    }

    /// Gives the admitted layout back so the view can be built normally.
    pub(crate) fn release_retained_view(&mut self, claims: &LayoutClaimBlocks) {
        self.layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .release_kept(claims);
    }

    /// The admitted record still describes this view's inherited context.
    pub(crate) fn retained_view_context_matches(
        &self,
        record: &RetainedViewRecord,
        bounds: Bounds<Pixels>,
    ) -> bool {
        record.bounds == bounds && self.retained_paint_context_matches(&record.context)
    }

    /// Copies the committed prepaint and paint ranges, returning the new prepaint range.
    ///
    /// Both ranges are validated before anything is moved, so a failure leaves the frame as it
    /// was and the caller builds the view normally.
    pub(crate) fn reuse_retained_view_prepaint(
        &mut self,
        record: &RetainedViewRecord,
    ) -> Option<Range<PrepaintStateIndex>> {
        if !self.can_reuse_prepaint(&record.prepaint_range)
            || !self.can_reuse_paint(&record.paint_range)
        {
            return None;
        }
        let start = self.prepaint_index();
        self.replay_validated_prepaint(record.prepaint_range.clone());
        let end = self.prepaint_index();
        if record
            .prepaint_range
            .end
            .rebased_from(&record.prepaint_range.start, &start)
            != Some(end.clone())
        {
            // The copied span changed length, so nested records cannot be rebased onto it. The
            // copy itself is positional and still valid; only deeper future reuse is lost.
            log::debug!("gpui retained view prepaint changed length");
        }
        // Reserve nested view positions during prepaint, not paint. Ancestors
        // finish collecting their descendant order before paint begins.
        self.next_frame.view_retention_records.extend(
            self.rendered_frame.view_retention_records[record.nested.clone()].iter().map(|slot| {
                ViewRecordSlot { key: slot.key.clone(), record: None }
            }),
        );
        Some(start..end)
    }

    /// Marks where this view's element-state accesses start.
    pub(crate) fn begin_view_element_states(&self) -> usize {
        self.next_frame.accessed_element_states.len()
    }

    /// The element states taken since `start`, in access order.
    pub(crate) fn view_layout_state_range(&self, start: usize) -> Range<usize> {
        start..self.next_frame.accessed_element_states.len()
    }

    /// Request-layout replay happens at admission, while the parent's layout
    /// span is still open. This keeps nested layout state ranges contiguous.
    pub(crate) fn replay_view_layout_states(&mut self, source: &Range<usize>) -> Range<usize> {
        let start = self.next_frame.accessed_element_states.len();
        self.next_frame.accessed_element_states.extend_from_slice(
            &self.rendered_frame.accessed_element_states[source.clone()],
        );
        start..self.next_frame.accessed_element_states.len()
    }

    /// An admitted view that rebuilds in prepaint no longer owns its
    /// speculative layout accesses. Keep the offsets stable for its ancestors.
    pub(crate) fn cancel_view_layout_states(&mut self, range: &Range<usize>) {
        let range = self.prebuilt_state_relocations.resolve(range);
        self.next_frame.accessed_element_states[range].fill(None);
    }

    /// Reserves this frame's record position for a view before its subtree runs.
    ///
    /// Positions are allocated in prepaint order, so everything a view prepaints takes a later
    /// position than the view itself and forms one span.
    pub(crate) fn reserve_view_retention(&mut self, key: GlobalElementId) -> ViewRetentionSlot {
        let index = self.next_frame.view_retention_records.len();
        self.next_frame.view_retention_records.push(ViewRecordSlot { key, record: None });
        ViewRetentionSlot(index)
    }

    /// The contiguous descendants this view prepainted, without copying their keys.
    pub(crate) fn finish_view_retention_scope(
        &self,
        slot: &ViewRetentionSlot,
    ) -> Range<usize> {
        slot.0 + 1..self.next_frame.view_retention_records.len()
    }

    pub(crate) fn claim_log_len(&self) -> usize {
        self.layout_engine
            .as_ref()
            .expect("layout engine is installed while a frame is drawn")
            .claim_log_len()
    }

    /// Stores this view's record once its paint range is known.
    pub(crate) fn commit_view_retention(
        &mut self,
        slot: &ViewRetentionSlot,
        nested: Range<usize>,
        mut commit: ViewRetentionCommit,
    ) {
        commit.layout_claims = self.layout_engine.as_ref().expect("layout engine installed")
            .resolve_claim_blocks(&commit.layout_claims);
        commit.layout_claims.normalize();
        commit.layout_state_range = self.prebuilt_state_relocations.resolve(&commit.layout_state_range);
        let index = slot.0;
        if let Some(source) = commit.reused.as_ref() {
            let source_slots = &self.rendered_frame.view_retention_records[source.nested.clone()];
            for entry in std::iter::once(&self.next_frame.view_retention_records[index]).chain(source_slots) {
                if let Some(ElementId::View(owner)) = entry.key.0.last() {
                    self.replayed_view_owners.insert(*owner);
                }
            }
            let source_scene = source.paint_range.start.scene_index..source.paint_range.end.scene_index;
            let target_scene = commit.paint_range.start.scene_index..commit.paint_range.end.scene_index;
            for segment in &self.rendered_frame.retained_scene_segments[source.scene_segments.clone()] {
                if let Some(scene_range) = rebase_span(&segment.scene_range, &source_scene, &target_scene) {
                    self.next_frame.push_retained_scene_segment(RetainedSceneSegment {
                        scene_range,
                        ..segment.clone()
                    });
                }
            }
            let scene_segments = commit.scene_segments_start..self.next_frame.retained_scene_segments.len();
            if scene_segments.len() == source.scene_segments.len() {
                self.copy_nested_view_records(source, &nested, &commit.layout_claims, &commit.prepaint_range, &commit.paint_range, &commit.layout_state_range, &scene_segments);
            }
        }
        let scene_segments = commit.scene_segments_start..self.next_frame.retained_scene_segments.len();
        let entry = &mut self.next_frame.view_retention_records[index];
        self.next_frame.view_retentions.insert(entry.key.clone(), index);
        entry.record = Some(RetainedViewRecord {
                layout_root: commit.layout_root,
                layout_claims: commit.layout_claims,
                prepaint_range: commit.prepaint_range,
                paint_range: commit.paint_range,
                bounds: commit.bounds,
                context: commit.context,
                rebuild: commit.rebuild,
                splice_safe: commit.splice_safe,
                nested,
                scene_segments,
                layout_state_range: commit.layout_state_range,
        });
    }

    fn copy_nested_view_records(
        &mut self,
        source: &RetainedViewRecord,
        target_nested: &Range<usize>,
        target_claims: &LayoutClaimBlocks,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
        target_states: &Range<usize>,
        target_scene_segments: &Range<usize>,
    ) {
        let Some(claim_map) = ClaimBlockMap::between(&source.layout_claims, target_claims) else { return; };
        for (offset, entry) in self.rendered_frame.view_retention_records[source.nested.clone()].iter().enumerate() {
            let Some(nested) = entry.record.clone() else {
                continue;
            };
            let Some(prepaint) = rebase_prepaint_range(
                &nested.prepaint_range,
                &source.prepaint_range,
                target_prepaint,
            ) else {
                continue;
            };
            let Some(paint) =
                rebase_paint_range(&nested.paint_range, &source.paint_range, target_paint)
            else {
                continue;
            };
            let old_states = &nested.layout_state_range;
            let source_prepaint_states = source.prepaint_range.start.accessed_element_states_index
                ..source.prepaint_range.end.accessed_element_states_index;
            let target_prepaint_states = target_prepaint.start.accessed_element_states_index
                ..target_prepaint.end.accessed_element_states_index;
            // Lazy view layout can run inside the enclosing view's prepaint.
            // Rebase it against that phase, not against the initial layout span.
            let layout_states = if old_states.is_empty() {
                target_states.start..target_states.start
            } else if let Some(range) = rebase_span(old_states, &source.layout_state_range, target_states)
                .or_else(|| rebase_span(old_states, &source_prepaint_states, &target_prepaint_states))
            {
                range
            } else {
                continue;
            };
            let Some(descendants) = rebase_span(&nested.nested, &source.nested, target_nested) else {
                continue;
            };
            let Some(layout_claims) = claim_map.project(&nested.layout_claims, true) else {
                continue;
            };
            let Some(scene_segments) = rebase_span(&nested.scene_segments, &source.scene_segments, target_scene_segments) else {
                continue;
            };
            let index = target_nested.start + offset;
            let entry = &mut self.next_frame.view_retention_records[index];
            self.next_frame.view_retentions.insert(entry.key.clone(), index);
            entry.record = Some(RetainedViewRecord {
                    prepaint_range: prepaint,
                    paint_range: paint,
                    layout_state_range: layout_states,
                    nested: descendants,
                    scene_segments,
                    layout_claims,
                    ..nested
            });
        }
    }
}

fn rebase_span(range: &Range<usize>, source: &Range<usize>, target: &Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end || range.start < source.start || range.end > source.end {
        return None;
    }
    let start = target.start.checked_add(range.start - source.start)?;
    let end = target.start.checked_add(range.end - source.start)?;
    (end <= target.end).then_some(start..end)
}


