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
    /// Element states the subtree took while its layout was requested and prepainted.
    pub(crate) element_state_range: Range<usize>,
    /// The subtree contains an explicit cached view, whose replay keeps its own committed
    /// element metadata. Such a view is not drawn again as a whole.
    pub(crate) contains_cached_view: bool,
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
    pub(crate) element_state_range: Range<usize>,
    pub(crate) contains_cached_view: bool,
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
        !self.force_view_cache_refresh
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
        if record.contains_cached_view
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

    /// Monotonic count of explicit cached views laid out so far.
    pub(crate) fn cached_view_mark(&self) -> u64 {
        self.cached_views_laid_out
    }

    /// Whether an explicit cached view was laid out since `mark`.
    pub(crate) fn cached_view_seen_since(&self, mark: u64) -> bool {
        self.cached_views_laid_out != mark
    }

    pub(crate) fn note_cached_view_layout(&mut self) {
        self.cached_views_laid_out = self.cached_views_laid_out.wrapping_add(1);
    }

    /// Marks where this view's element-state accesses start.
    pub(crate) fn begin_view_element_states(&self) -> usize {
        self.next_frame.accessed_element_states.len()
    }

    /// The element states taken since `start`, in access order.
    pub(crate) fn view_element_state_range(&self, start: usize) -> Range<usize> {
        start..self.next_frame.accessed_element_states.len()
    }

    /// Carries the states a reused view's subtree took while it was built.
    ///
    /// The prepaint copy only replays the states taken during prepaint; a text layout takes its
    /// state while its layout is requested, before the copied range begins.
    fn keep_reused_view_element_states(&mut self, range: &Range<usize>) {
        let Some(source) = self
            .rendered_frame
            .accessed_element_states
            .get(range.clone())
        else {
            return;
        };
        self.next_frame
            .accessed_element_states
            .extend(source.iter().map(|(id, type_id)| (id.clone(), *type_id)));
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
        let element_state_range = if let Some(source) = commit.reused.as_ref() {
            self.copy_reused_element_metadata(index, source, &commit.prepaint_range, &commit.paint_range);
            let start = self.next_frame.accessed_element_states.len();
            self.keep_reused_view_element_states(&source.element_state_range);
            let states = start..self.next_frame.accessed_element_states.len();
            self.copy_nested_view_records(source, &commit.prepaint_range, &commit.paint_range, &states);
            states
        } else {
            commit.element_state_range
        };
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
                element_state_range,
                contains_cached_view: commit.contains_cached_view,
            },
        );
    }

    /// Carries the element-level records of a view drawn again into the new frame.
    ///
    /// A reused view still has to leave the committed frame in a state where a later rebuild of
    /// one of its ancestors can replay the nested views inside it. Those records live in the
    /// view's own post-order metadata span, which is copied and rebased as one unit.
    fn copy_reused_element_metadata(
        &mut self,
        index: usize,
        admission: &RetainedViewRecord,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
    ) {
        let key = &self.next_frame.view_retention_order[index];
        let Some(source) = self.rendered_frame.retained_element_ranges.get(key) else {
            return;
        };
        // The enclosing Drawable records its own metadata after View::paint.
        // Only its descendants belong to the copied inner prepaint/paint ranges.
        let mut source_metadata = source.metadata_range.clone();
        if source_metadata.is_empty() {
            return;
        }
        source_metadata.end -= 1;
        if !self.replay_retained_element_metadata(
            &admission.prepaint_range,
            &admission.paint_range,
            &source_metadata,
            target_prepaint,
            target_paint,
        ) {
            self.force_view_cache_refresh = true;
            self.degrade_current_draw();
        }
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
            let old_states = &nested.element_state_range;
            if old_states.start < source.element_state_range.start
                || old_states.end > source.element_state_range.end
                || old_states.start > old_states.end
            {
                continue;
            }
            let start = target_states.start + (old_states.start - source.element_state_range.start);
            let end = target_states.start + (old_states.end - source.element_state_range.start);
            if end > target_states.end {
                continue;
            }
            self.next_frame.view_retentions.insert(
                key.clone(),
                RetainedViewRecord {
                    prepaint_range: prepaint,
                    paint_range: paint,
                    element_state_range: start..end,
                    ..nested
                },
            );
        }
    }
}
