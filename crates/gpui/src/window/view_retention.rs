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
use crate::layout::LayoutClaimSpans;

/// Everything needed to draw one view subtree again without building it.
#[derive(Clone)]
pub(crate) struct RetainedViewRecord {
    /// The view's committed layout root, still owned by the layered layout engine.
    pub(crate) layout_root: LayoutId,
    /// Every persistent node the subtree claimed, in claim order.
    pub(crate) layout_claims: LayoutClaimSpans,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) bounds: Bounds<Pixels>,
    /// The inherited context the ranges were recorded under.
    pub(crate) context: Rc<RetainedPaintContext>,
    /// Descendant span in the frame's prepaint-order view table.
    pub(crate) nested: Range<usize>,
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
    pub(crate) layout_claims: LayoutClaimSpans,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) context: Rc<RetainedPaintContext>,
    pub(crate) layout_state_range: Range<usize>,
    /// The retained source this frame copied, when the view was reused.
    pub(crate) reused: Option<RetainedViewRecord>,
}

impl Window {
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

    /// Claims the committed layout of `key` and returns its record, or `None` when the view has to
    /// be built.
    ///
    /// The returned record is the one admitted for this frame. Callers that decide against reuse
    /// later must release it with [`Window::release_retained_view`].
    pub(crate) fn admit_retained_view(
        &mut self,
        key: &GlobalElementId,
        view_id: EntityId,
        cx: &App,
    ) -> Option<(RetainedViewRecord, LayoutClaimSpans)> {
        if !self.view_retention_eligible(view_id, cx) {
            return None;
        }
        let index = *self.rendered_frame.view_retentions.get(key)?;
        let record = self.rendered_frame.view_retention_records.get(index)?.record.as_ref()?.clone();
        if record.prepaint_range.start.deferred_draws_index != record.prepaint_range.end.deferred_draws_index
            || record.prepaint_range.start.tooltips_index != record.prepaint_range.end.tooltips_index
            || self.rendered_frame.accessed_element_states.get(record.layout_state_range.clone()).is_none()
            || self.rendered_frame.view_retention_records.get(record.nested.clone()).is_none()
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
    pub(crate) fn release_retained_view(&mut self, claims: &LayoutClaimSpans) {
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
        self.next_frame.accessed_element_states[range.clone()].fill(None);
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
        commit: ViewRetentionCommit,
    ) {
        let index = slot.0;
        if let Some(source) = commit.reused.as_ref() {
            let source_slots = &self.rendered_frame.view_retention_records[source.nested.clone()];
            for entry in std::iter::once(&self.next_frame.view_retention_records[index]).chain(source_slots) {
                if let Some(ElementId::View(owner)) = entry.key.0.last() {
                    self.replayed_view_owners.insert(*owner);
                }
            }
            self.copy_nested_view_records(source, &nested, &commit.layout_claims, &commit.prepaint_range, &commit.paint_range, &commit.layout_state_range);
        }
        let entry = &mut self.next_frame.view_retention_records[index];
        self.next_frame.view_retentions.insert(entry.key.clone(), index);
        entry.record = Some(RetainedViewRecord {
                layout_root: commit.layout_root,
                layout_claims: commit.layout_claims,
                prepaint_range: commit.prepaint_range,
                paint_range: commit.paint_range,
                bounds: commit.bounds,
                context: commit.context,
                nested,
                layout_state_range: commit.layout_state_range,
        });
    }

    fn copy_nested_view_records(
        &mut self,
        source: &RetainedViewRecord,
        target_nested: &Range<usize>,
        target_claims: &LayoutClaimSpans,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
        target_states: &Range<usize>,
    ) {
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
            let Some(layout_claims) = rebase_claims(&nested.layout_claims, &source.layout_claims, target_claims) else {
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


fn rebase_claims(claims: &LayoutClaimSpans, source: &LayoutClaimSpans, target: &LayoutClaimSpans) -> Option<LayoutClaimSpans> {
    let rebase = |range: &Range<usize>| {
        if range.is_empty() {
            return Some(target[0].start..target[0].start);
        }
        rebase_span(range, &source[0], &target[0])
            .or_else(|| rebase_span(range, &source[1], &target[1]))
    };
    Some([rebase(&claims[0])?, rebase(&claims[1])?])
}
