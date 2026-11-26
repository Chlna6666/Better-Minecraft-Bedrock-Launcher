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
use crate::layout::{ClaimBlockMap, LayoutClaimBlocks, SpanRelocations};

/// State values remain uniquely owned by the frame map. These blocks only
/// describe which access-log entries keep those values alive on publication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StateAccessBlocks(smallvec::SmallVec<[Range<usize>; 1]>);

impl StateAccessBlocks {
    pub(crate) fn from_range(range: Range<usize>) -> Self {
        let mut result = Self::default();
        result.push(range);
        result
    }

    pub(crate) fn push(&mut self, range: Range<usize>) {
        if !range.is_empty() { self.0.push(range); }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Range<usize>> { self.0.iter() }

    pub(crate) fn resolve(&self, moves: &SpanRelocations) -> Self {
        let mut result = self.clone();
        result.resolve_in_place(moves);
        result
    }

    fn resolve_in_place(&mut self, moves: &SpanRelocations) {
        for span in &mut self.0 {
            *span = moves.resolve(span);
        }
    }

    fn normalize(&mut self) {
        if self.0.len() <= 1 { return; }
        self.0.sort_unstable_by_key(|span| span.start);
        let mut len = 0;
        for index in 0..self.0.len() {
            let span = self.0[index].clone();
            if len > 0 && self.0[len - 1].end >= span.start {
                self.0[len - 1].end = self.0[len - 1].end.max(span.end);
            } else {
                self.0[len] = span;
                len += 1;
            }
        }
        self.0.truncate(len);
    }

    pub(crate) fn covers(&self, child: &Self) -> bool {
        child.iter().all(|span| {
            let index = self.0.partition_point(|block| block.end <= span.start);
            self.0.get(index).is_some_and(|block| block.start <= span.start && block.end >= span.end)
        })
    }

    pub(crate) fn project(&self, map: &ClaimBlockMap, complete: bool) -> Option<Self> {
        let mut result = Self::default();
        for span in self.iter() {
            map.project_range(span, complete, |range| result.push(range))?;
        }
        Some(result)
    }

    fn between(&self, target: &Self) -> Option<ClaimBlockMap> {
        let mut map = ClaimBlockMap::default();
        let mut source = self.0.iter();
        let mut dest = target.0.iter();
        let (mut s, mut t) = (source.next(), dest.next());
        let (mut a, mut b) = (0, 0);
        while let (Some(old), Some(new)) = (s, t) {
            let len = (old.len() - a).min(new.len() - b);
            map.record(old.start + a..old.start + a + len, new.start + b);
            a += len; b += len;
            if a == old.len() { s = source.next(); a = 0; }
            if b == new.len() { t = dest.next(); b = 0; }
        }
        if s.is_some() || t.is_some() { return None; }
        map.sort_by_source();
        Some(map)
    }
}

/// Identity and effective inherited values at RequestLayout, distinct from Prepaint.
pub(crate) struct ViewRebuild {
    pub(super) view: crate::AnyWeakView,
    pub(super) application_id: GlobalElementId,
    pub(super) parent_layout_key: Option<u64>,
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
    pub(crate) layout_state_range: StateAccessBlocks,
}

impl RetainedViewRecord {
    fn own_scene_segment<'a>(
        &self,
        owner: EntityId,
        segments: &'a [RetainedSceneSegment],
    ) -> Option<&'a RetainedSceneSegment> {
        let range = self.paint_range.start.scene_index..self.paint_range.end.scene_index;
        if range.is_empty() {
            return None;
        }
        segments.get(self.scene_segments.end)
            .filter(|segment| segment.entity_id == owner && segment.scene_range == range)
    }
}

/// Prepaint order is also record order. A reservation without a paint commit
/// remains a hole, and cannot be selected as a rebuild/replay source.
pub(crate) struct ViewRecordSlot {
    pub(crate) key: u64,
    pub(crate) owner: EntityId,
    pub(crate) record: Option<RetainedViewRecord>,
}

/// Where the view's own record is inserted once it has painted.
pub(crate) struct ViewRetentionSlot(usize);

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
            parent_layout_key: crate::fast::layout_key::parent_layout_key(self),
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
        let mut result = Vec::new();
        let mut skip_until = anchor.nested.start;
        let mut skipped_clean = false;
        for (offset, entry) in entries.iter().enumerate() {
            let record = entry.record.as_ref()?;
            let rebuild = record.rebuild.as_ref()?;
            let owner = rebuild.view.entity_id();
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

            // Validate every record, but plan only the outermost clean/direct
            // subtrees. Descendants of a clean subtree must also be clean;
            // descendants of a direct gap will be rebuilt with that gap.
            if index < skip_until {
                if skipped_clean && self.view_dirty_scope(owner).is_some() {
                    return None;
                }
                continue;
            }
            let scope = self.view_dirty_scope(owner);
            if scope.is_some() && !rebuild.splice_supported {
                return None;
            }
            result.push((index, scope));
            if scope != Some(ViewDirtyScope::TraversalAncestor) {
                skip_until = record.nested.end;
                skipped_clean = scope.is_none();
            }
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
        &mut self, key: u64, record: &RetainedViewRecord,
    ) -> (ViewRetentionSlot, Range<usize>) {
        let owner = record.rebuild.as_ref().expect("validated clean subtree").view.entity_id();
        let slot = self.reserve_view_retention(key, owner);
        self.next_frame.view_retention_records.extend(
            self.rendered_frame.view_retention_records[record.nested.clone()].iter().map(|entry| {
                ViewRecordSlot { key: entry.key, owner: entry.owner, record: None }
            }),
        );
        let nested = self.finish_view_retention_scope(&slot);
        (slot, nested)
    }

    /// Replay an admitted view and carry its authoritative fragment bounds
    /// into the active paint scope. The replay may rebind draw order, but
    /// plain primitives preserve visual geometry as long as none were dropped.
    pub(crate) fn replay_retained_view_paint(
        &mut self,
        owner: EntityId,
        source: &RetainedViewRecord,
    ) {
        let range = source.paint_range.start.scene_index..source.paint_range.end.scene_index;
        let start = self.next_frame.painter.len();
        let bounds = if !self.view_bounds_stack.is_empty()
            && !self.next_frame.painter.is_capturing_blur()
            && self.rendered_frame.scene().can_reuse_range_bounds(range.clone())
        {
            source.own_scene_segment(owner, &self.rendered_frame.retained_scene_segments)
                .map(|segment| segment.bounds)
        } else {
            None
        };
        self.replay_validated_paint(source.paint_range.clone());
        let end = self.next_frame.painter.len();
        if let Some(bounds) = bounds
            && end - start == range.len()
        {
            // In test builds compare the carried geometry with the reference
            // scanner; release replay pays no additional primitive traversal.
            #[cfg(test)]
            assert_eq!(self.next_frame.scene().bounds_for_range(start..end), Some(bounds));
            self.fold_view_bounds(start..end, Some(bounds));
        }
    }

    /// A clean subtree's own damage entry is emitted after its descendant
    /// record is committed. Unlike ordinary view replay, no wrapper is painted
    /// here, so the original own entry must also be carried.
    pub(super) fn commit_spliced_clean(
        &mut self, slot: &ViewRetentionSlot, nested: Range<usize>,
        source: &RetainedViewRecord, claims: LayoutClaimBlocks,
        prepaint: Range<PrepaintStateIndex>, paint: Range<PaintIndex>,
        states: StateAccessBlocks,
    ) {
        let owner = source.rebuild.as_ref().expect("admitted clean subtree").view.entity_id();
        let old_scene = source.paint_range.start.scene_index..source.paint_range.end.scene_index;
        let new_scene = paint.start.scene_index..paint.end.scene_index;
        let own = source.own_scene_segment(owner, &self.rendered_frame.retained_scene_segments).cloned();
        let scene_segments_start = self.next_frame.retained_scene_segments.len();
        self.commit_view_retention(slot, RetainedViewRecord {
            layout_root: source.layout_root, layout_claims: claims,
            prepaint_range: prepaint, paint_range: paint, bounds: source.bounds,
            context: source.context.clone(), rebuild: source.rebuild.clone(),
            splice_safe: source.splice_safe, nested,
            scene_segments: scene_segments_start..self.next_frame.retained_scene_segments.len(),
            layout_state_range: states,
        }, Some(source));
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
        key: u64,
        view_id: EntityId,
        application_id: Option<&GlobalElementId>,
        cx: &App,
    ) -> Option<(RetainedViewRecord, LayoutClaimBlocks)> {
        if !self.view_retention_eligible(view_id, cx) {
            return None;
        }
        let index = *self.rendered_frame.view_retentions.get(&key)?;
        let record = self.rendered_frame.view_retention_records.get(index)?.record.as_ref()?;
        let rebuild = record.rebuild.as_ref()?;
        if rebuild.view.entity_id() != view_id
            || crate::fast::layout_key::parent_layout_key(self) != rebuild.parent_layout_key
            || Some(&rebuild.application_id) != application_id
            || rebuild.splice_supported != self.splice_context_supported()
            || !self.retained_paint_context_matches(&rebuild.layout)
        {
            return None;
        }
        if record.prepaint_range.start.deferred_draws_index != record.prepaint_range.end.deferred_draws_index
            || record.prepaint_range.start.tooltips_index != record.prepaint_range.end.tooltips_index
            || !record.layout_state_range.iter().all(|range| self.rendered_frame.accessed_element_states.get(range.clone()).is_some())
            || self.rendered_frame.view_retention_records.get(record.nested.clone()).is_none()
            || self.rendered_frame.retained_scene_segments.get(record.scene_segments.clone()).is_none()
        {
            return None;
        }
        let claims = self.layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .try_keep_retained(&record.layout_claims, record.layout_root)?;
        Some((record.clone(), claims))
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
                ViewRecordSlot { key: slot.key, owner: slot.owner, record: None }
            }),
        );
        Some(start..end)
    }

    /// Marks where this view's element-state accesses start.
    pub(crate) fn begin_view_element_states(&self) -> usize {
        self.next_frame.accessed_element_states.len()
    }

    /// The element states taken since `start`, in access order.
    pub(crate) fn view_layout_state_range(&self, start: usize) -> StateAccessBlocks {
        StateAccessBlocks::from_range(start..self.next_frame.accessed_element_states.len())
    }

    /// Keep state values in the frame map and replay only ownership references.
    pub(crate) fn replay_view_layout_states(&mut self, source: &StateAccessBlocks) -> StateAccessBlocks {
        let start = self.next_frame.accessed_element_states.len();
        for span in source.iter() {
            self.next_frame.accessed_element_states.extend_from_slice(
                &self.rendered_frame.accessed_element_states[span.clone()],
            );
        }
        self.view_layout_state_range(start)
    }

    /// Cancel speculative ownership without moving other owners' offsets.
    pub(crate) fn cancel_view_layout_states(&mut self, blocks: &StateAccessBlocks) {
        for span in blocks.resolve(&self.prebuilt_state_relocations).iter() {
            self.next_frame.accessed_element_states[span.clone()].fill(None);
        }
    }

    /// Reserves this frame's record position for a view before its subtree runs.
    ///
    /// Positions are allocated in prepaint order, so everything a view prepaints takes a later
    /// position than the view itself and forms one span.
    pub(crate) fn reserve_view_retention(&mut self, key: u64, owner: EntityId) -> ViewRetentionSlot {
        let index = self.next_frame.view_retention_records.len();
        self.next_frame.view_retention_records.push(ViewRecordSlot { key, owner, record: None });
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
        mut record: RetainedViewRecord,
        reused: Option<&RetainedViewRecord>,
    ) {
        self.layout_engine.as_ref().expect("layout engine installed")
            .resolve_claim_blocks_in_place(&mut record.layout_claims);
        record.layout_claims.normalize();
        record.layout_state_range.resolve_in_place(&self.prebuilt_state_relocations);
        record.layout_state_range.normalize();
        let index = slot.0;
        if let Some(source) = reused {
            let source_slots = &self.rendered_frame.view_retention_records[source.nested.clone()];
            for entry in std::iter::once(&self.next_frame.view_retention_records[index]).chain(source_slots) {
                self.replayed_view_owners.insert(entry.owner);
            }
            let source_scene = source.paint_range.start.scene_index..source.paint_range.end.scene_index;
            let target_scene = record.paint_range.start.scene_index..record.paint_range.end.scene_index;
            for segment in &self.rendered_frame.retained_scene_segments[source.scene_segments.clone()] {
                if let Some(scene_range) = rebase_span(&segment.scene_range, &source_scene, &target_scene) {
                    self.next_frame.push_retained_scene_segment(RetainedSceneSegment::new(
                        segment.entity_id, segment.bounds, scene_range,
                    ));
                }
            }
            let scene_segments = record.scene_segments.start..self.next_frame.retained_scene_segments.len();
            if scene_segments.len() == source.scene_segments.len() {
                self.copy_nested_view_records(source, &record.nested, &record.layout_claims, &record.prepaint_range, &record.paint_range, &record.layout_state_range, &scene_segments);
            }
        }
        record.scene_segments.end = self.next_frame.retained_scene_segments.len();
        let entry = &mut self.next_frame.view_retention_records[index];
        self.next_frame.view_retentions.entry(entry.key).and_modify(|slot| *slot = usize::MAX).or_insert(index);
        entry.record = Some(record);
    }

    fn copy_nested_view_records(
        &mut self,
        source: &RetainedViewRecord,
        target_nested: &Range<usize>,
        target_claims: &LayoutClaimBlocks,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
        target_states: &StateAccessBlocks,
        target_scene_segments: &Range<usize>,
    ) {
        let Some(claim_map) = ClaimBlockMap::between(&source.layout_claims, target_claims) else { return; };
        let Some(state_map) = source.layout_state_range.between(target_states) else { return; };
        let source_prepaint_states = source.prepaint_range.start.accessed_element_states_index
            ..source.prepaint_range.end.accessed_element_states_index;
        let target_prepaint_states = target_prepaint.start.accessed_element_states_index
            ..target_prepaint.end.accessed_element_states_index;
        for (offset, entry) in self.rendered_frame.view_retention_records[source.nested.clone()].iter().enumerate() {
            let Some(nested) = entry.record.as_ref() else {
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
            let mut layout_states = StateAccessBlocks::default();
            let mut valid_states = true;
            for span in nested.layout_state_range.iter() {
                let mut mapped = StateAccessBlocks::default();
                if state_map.project_range(span, true, |range| mapped.push(range)).is_some() {
                    for range in mapped.iter() { layout_states.push(range.clone()); }
                } else if let Some(range) = rebase_span(span, &source_prepaint_states, &target_prepaint_states) {
                    layout_states.push(range);
                } else {
                    valid_states = false;
                    break;
                }
            }
            if !valid_states { continue; }
            layout_states.normalize();
            let Some(descendants) = rebase_span(&nested.nested, &source.nested, target_nested) else {
                continue;
            };
            let Some(mut layout_claims) = claim_map.project(&nested.layout_claims, true) else {
                continue;
            };
            layout_claims.normalize();
            let Some(scene_segments) = rebase_span(&nested.scene_segments, &source.scene_segments, target_scene_segments) else {
                continue;
            };
            let index = target_nested.start + offset;
            let entry = &mut self.next_frame.view_retention_records[index];
            self.next_frame.view_retentions.entry(entry.key).and_modify(|slot| *slot = usize::MAX).or_insert(index);
            entry.record = Some(RetainedViewRecord {
                    prepaint_range: prepaint,
                    paint_range: paint,
                    layout_state_range: layout_states,
                    nested: descendants,
                    scene_segments,
                    layout_claims,
                    layout_root: nested.layout_root,
                    bounds: nested.bounds,
                    context: nested.context.clone(),
                    rebuild: nested.rebuild.clone(),
                    splice_safe: nested.splice_safe,
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


