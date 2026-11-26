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
use crate::layout::LayoutClaim;

/// Everything needed to draw one view subtree again without building it.
#[derive(Clone)]
pub(crate) struct RetainedViewRecord {
    /// The view's committed layout root, still owned by the layered layout engine.
    pub(crate) layout_root: LayoutId,
    /// Every persistent node the subtree claimed, in claim order.
    pub(crate) layout_claims: Rc<[LayoutClaim]>,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) bounds: Bounds<Pixels>,
    /// The inherited context the ranges were recorded under.
    pub(crate) context: Rc<RetainedPaintContext>,
    /// Views drawn inside this one, in post-order.
    pub(crate) nested: SmallVec<[GlobalElementId; 4]>,
    /// Only states taken during request layout. Prepaint and paint replay their own spans.
    pub(crate) layout_state_range: Range<usize>,
}

/// Where the view's own record is inserted once it has painted.
pub(crate) struct ViewRetentionSlot(usize);

pub(crate) struct ViewRetentionCommit {
    pub(crate) layout_root: LayoutId,
    pub(crate) layout_claims: Rc<[LayoutClaim]>,
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
    ) -> Option<RetainedViewRecord> {
        if !self.view_retention_eligible(view_id, cx) {
            return None;
        }
        let record = self.rendered_frame.view_retentions.get(key)?.clone();
        if record.prepaint_range.start.deferred_draws_index != record.prepaint_range.end.deferred_draws_index
            || record.prepaint_range.start.tooltips_index != record.prepaint_range.end.tooltips_index
            || self.rendered_frame.accessed_element_states.get(record.layout_state_range.clone()).is_none()
            || !record.layout_claims.iter().any(|claim| claim.id == record.layout_root)
        {
            return None;
        }
        if !self
            .layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .try_keep_retained(&record.layout_claims)
        {
            return None;
        }
        Some(record)
    }

    /// Gives the admitted layout back so the view can be built normally.
    pub(crate) fn release_retained_view(&mut self, record: &RetainedViewRecord) {
        self.layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .release_kept(&record.layout_claims);
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
        if !self.reuse_prepaint(record.prepaint_range.clone()) {
            return None;
        }
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
        self.next_frame.view_retention_order.extend(record.nested.iter().cloned());
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
        self.next_frame.view_retention_order.push(key);
        ViewRetentionSlot(self.next_frame.view_retention_order.len() - 1)
    }

    /// The views this view prepainted, in post-order.
    pub(crate) fn finish_view_retention_scope(
        &self,
        slot: &ViewRetentionSlot,
    ) -> SmallVec<[GlobalElementId; 4]> {
        self.next_frame.view_retention_order[slot.0 + 1..]
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) fn begin_view_record(&mut self) -> usize {
        self.layout_engine
            .as_mut()
            .expect("layout engine is installed while a frame is drawn")
            .record_claimed_keys()
    }

    pub(crate) fn finish_view_record_claims(&mut self, claim_start: usize) -> Rc<[LayoutClaim]> {
        Rc::from(
            self.layout_engine
                .as_mut()
                .expect("layout engine is installed while a frame is drawn")
                .finish_recording_claimed_keys(claim_start),
        )
    }

    /// Lazy children (notably List rows) request layout during prepaint.
    /// Merge those claims with the earlier request-layout record. A nested
    /// view may have released and rebuilt some of the earlier claims.
    pub(crate) fn finish_view_prepaint_claims(
        &mut self,
        start: usize,
        claims: &mut Rc<[LayoutClaim]>,
    ) {
        let engine = self.layout_engine.as_mut().expect("layout engine installed");
        let extra = engine.finish_recording_claimed_keys(start);
        if extra.is_empty() {
            return;
        }
        let mut seen = FxHashSet::default();
        let merged: Vec<_> = claims.iter().copied().chain(extra)
            .filter(|claim| engine.claim_is_current(claim) && seen.insert(*claim))
            .collect();
        *claims = Rc::from(merged);
    }

    /// Stores this view's record once its paint range is known.
    pub(crate) fn commit_view_retention(
        &mut self,
        slot: &ViewRetentionSlot,
        nested: SmallVec<[GlobalElementId; 4]>,
        commit: ViewRetentionCommit,
    ) {
        let index = slot.0;
        if let Some(source) = commit.reused.as_ref() {
            for key in std::iter::once(&self.next_frame.view_retention_order[index]).chain(source.nested.iter()) {
                if let Some(ElementId::View(owner)) = key.0.last() {
                    self.replayed_view_owners.insert(*owner);
                }
            }
            self.copy_nested_view_records(source, &commit.prepaint_range, &commit.paint_range, &commit.layout_state_range);
        }
        let key = self.next_frame.view_retention_order[index].clone();
        self.next_frame.view_retentions.insert(
            key,
            RetainedViewRecord {
                layout_root: commit.layout_root,
                layout_claims: commit.layout_claims,
                prepaint_range: commit.prepaint_range,
                paint_range: commit.paint_range,
                bounds: commit.bounds,
                context: commit.context,
                nested,
                layout_state_range: commit.layout_state_range,
            },
        );
    }

    fn copy_nested_view_records(
        &mut self,
        source: &RetainedViewRecord,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
        target_states: &Range<usize>,
    ) {
        for key in &source.nested {
            let Some(nested) = self.rendered_frame.view_retentions.get(key).cloned() else {
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
            } else if let Some(range) = rebase_layout_states(old_states, &source.layout_state_range, target_states)
                .or_else(|| rebase_layout_states(old_states, &source_prepaint_states, &target_prepaint_states))
            {
                range
            } else {
                continue;
            };
            self.next_frame.view_retentions.insert(
                key.clone(),
                RetainedViewRecord {
                    prepaint_range: prepaint,
                    paint_range: paint,
                    layout_state_range: layout_states,
                    ..nested
                },
            );
        }
    }
}

fn rebase_layout_states(range: &Range<usize>, source: &Range<usize>, target: &Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end || range.start < source.start || range.end > source.end {
        return None;
    }
    let start = target.start.checked_add(range.start - source.start)?;
    let end = target.start.checked_add(range.end - source.start)?;
    (end <= target.end).then_some(start..end)
}
