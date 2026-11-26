//! Multi-gap frame splicing, adapted from GPUI Fast's layout/gap transaction
//! (Apache-2.0; Zed Industries and Longbridge). A native layout change is a
//! fallback, not permission to retain stale ancestor geometry.

use super::*;
use crate::{Element, IntoElement, ViewElement, ViewRequestLayout};
use crate::layout::LayoutClaimSpans;

pub(super) struct PrebuiltGap {
    key: GlobalElementId,
    old: RetainedViewRecord,
    view: ViewElement,
    layout_id: LayoutId,
    state: ViewRequestLayout,
    claims: LayoutClaimSpans,
    states: Range<usize>,
    node: Option<DispatchNodeId>,
}

pub(crate) struct ViewSplice {
    pub(crate) anchor: RetainedViewRecord,
    pub(crate) claims: LayoutClaimSpans,
    pub(crate) states: Range<usize>,
    kept: LayoutClaimSpans,
    gaps: Vec<PrebuiltGap>,
}

impl Window {
    // Restore the view's existing path, including its own segment, without
    // mounting an extra wrapper. The caller supplies the original layout scope.
    fn with_gap_paths<R>(
        &mut self, key: &GlobalElementId, application_id: &GlobalElementId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        fn stack(id: &GlobalElementId) -> ElementIdStack {
            let mut stack = ElementIdStack::default();
            for segment in id.0.iter() {
                stack.push(segment.clone());
            }
            stack
        }
        let application = mem::replace(&mut self.element_id_stack, stack(application_id));
        let native = mem::replace(&mut self.retained_element_id_stack, stack(key));
        let result = f(self);
        self.element_id_stack = application;
        self.retained_element_id_stack = native;
        result
    }

    /// An abandoned gap can hand off its already-requested layout to the normal
    /// traversal. The original spans remain valid, including nested layouts.
    pub(crate) fn take_prebuilt_gap(
        &mut self, key: &GlobalElementId, id: Option<&GlobalElementId>, owner: EntityId,
    ) -> Option<(LayoutId, ViewRequestLayout)> {
        if self.prebuilt_gaps.is_empty() {
            return None;
        }
        let mut gap = self.prebuilt_gaps.remove(key)?;
        let compatible = gap.old.rebuild.as_ref().is_some_and(|rebuild| {
            rebuild.view.entity_id() == owner
                && id == Some(&rebuild.application_id)
                && self.splice_context_supported()
                && self.retained_paint_context_matches(&rebuild.layout)
        });
        if !compatible {
            self.release_retained_view(&gap.claims);
            self.cancel_view_layout_states(&gap.states);
            return None;
        }
        // Ancestors taking over an earlier span must not publish a false
        // contiguous claim proof. A later normal frame will rebuild it.
        self.prebuilt_adoption_serial = self.prebuilt_adoption_serial.wrapping_add(1);
        gap.state.adoption_serial = self.prebuilt_adoption_serial;
        Some((gap.layout_id, gap.state))
    }

    /// Arena-backed elements and their candidate claims never survive a frame.
    pub(super) fn discard_prebuilt_gaps(&mut self) {
        if self.prebuilt_gaps.is_empty() {
            return;
        }
        let mut gaps = mem::take(&mut self.prebuilt_gaps);
        for (_, gap) in gaps.drain() {
            self.release_retained_view(&gap.claims);
            self.cancel_view_layout_states(&gap.states);
        }
        self.prebuilt_gaps = gaps;
    }

    pub(crate) fn abandon_view_splice(&mut self, splice: ViewSplice) -> RetainedViewRecord {
        self.release_retained_view(&splice.kept);
        self.stash_prebuilt_gaps(splice.gaps);
        splice.anchor
    }

    fn stash_prebuilt_gaps(&mut self, gaps: Vec<PrebuiltGap>) {
        for gap in gaps {
            self.prebuilt_gaps.insert(gap.key.clone(), gap);
        }
    }

    /// Select disjoint leaf gaps, validate the complete partition before
    /// mutating native layout, then build them under their original paths.
    pub(crate) fn try_splice_view(
        &mut self, key: &GlobalElementId, owner: EntityId,
        id: Option<&GlobalElementId>, cx: &mut App,
    ) -> Option<ViewSplice> {
        if self.view_dirty_scope(owner) != Some(ViewDirtyScope::TraversalAncestor)
            || self.splice_building || !self.prebuilt_gaps.is_empty()
            || cfg!(gpui_hot_reload) || self.force_view_cache_refresh
            || cx.has_active_drag() || self.is_inspector_picking(cx)
            || !self.splice_context_supported()
        {
            return None;
        }
        let index = *self.rendered_frame.view_retentions.get(key)?;
        let anchor = self.rendered_frame.view_retention_records.get(index)?.record.as_ref()?.clone();
        let rebuild = anchor.rebuild.as_ref()?;
        if !anchor.splice_safe || !rebuild.splice_supported
            || rebuild.view.entity_id() != owner || id != Some(&rebuild.application_id)
            || !self.retained_paint_context_matches(&rebuild.layout)
            || !anchor.layout_state_range.is_empty()
            || !self.can_reuse_prepaint(&anchor.prepaint_range)
            || !self.can_reuse_paint(&anchor.paint_range)
        {
            return None;
        }
        let indices = self.dirty_leaf_gaps(&anchor)?;
        let mut plan = Vec::with_capacity(indices.len());
        let mut claims = smallvec::SmallVec::<[(LayoutClaimSpans, LayoutId); 4]>::new();
        let mut dispatch = smallvec::SmallVec::<[(Range<usize>, EntityId); 4]>::new();
        let mut prepaint = anchor.prepaint_range.start.clone();
        let mut paint = anchor.paint_range.start.clone();
        for gap_index in indices {
            let slot = &self.rendered_frame.view_retention_records[gap_index];
            let old = slot.record.as_ref()?.clone();
            let gap_rebuild = old.rebuild.as_ref()?;
            if !old.splice_safe || !old.layout_state_range.is_empty()
                || gap_rebuild.layout != rebuild.layout || old.context != anchor.context
                // Clean stretches cannot contain ancestor-owned mutable state.
                || prepaint.accessed_element_states_index != old.prepaint_range.start.accessed_element_states_index
                || paint.accessed_element_states_index != old.paint_range.start.accessed_element_states_index
                || !self.can_reuse_prepaint(&(prepaint..old.prepaint_range.start.clone()))
                || !self.can_reuse_prepaint(&old.prepaint_range)
                || !self.can_reuse_paint(&(paint..old.paint_range.start.clone()))
                || !self.can_reuse_paint(&old.paint_range)
            {
                return None;
            }
            prepaint = old.prepaint_range.end.clone();
            paint = old.paint_range.end.clone();
            dispatch.push((old.prepaint_range.start.dispatch_tree_index..old.prepaint_range.end.dispatch_tree_index, gap_rebuild.view.entity_id()));
            claims.push((old.layout_claims.clone(), old.layout_root));
            let view = gap_rebuild.view.upgrade()?.into_element();
            plan.push((slot.key.clone(), old, view));
        }
        if prepaint.accessed_element_states_index != anchor.prepaint_range.end.accessed_element_states_index
            || paint.accessed_element_states_index != anchor.paint_range.end.accessed_element_states_index
            || !self.can_reuse_prepaint(&(prepaint..anchor.prepaint_range.end.clone()))
            || !self.can_reuse_paint(&(paint..anchor.paint_range.end.clone()))
            || !self.rendered_frame.dispatch_tree.can_splice_gaps(
                anchor.prepaint_range.start.dispatch_tree_index..anchor.prepaint_range.end.dispatch_tree_index,
                &dispatch,
            )
        {
            return None;
        }
        let engine = self.layout_engine.as_mut()?;
        let stamp = engine.splice_layout_stamp();
        if stamp.3 != 0 {
            return None;
        }
        let start = engine.claim_log_len();
        let kept = engine.try_keep_around_gaps(&anchor.layout_claims, &claims, anchor.layout_root)?;
        let states_start = self.begin_view_element_states();
        let mut gaps = Vec::with_capacity(plan.len());
        self.splice_building = true;
        for (gap_key, old, mut view) in plan {
            let gap_start = self.claim_log_len();
            let state_start = self.begin_view_element_states();
            let rebuild = old.rebuild.as_ref().expect("validated gap");
            let (layout_id, state) = self.with_gap_paths(&gap_key, &rebuild.application_id, |window| {
                window.with_retained_child_scope(|window| {
                    view.request_layout(Some(&rebuild.application_id), None, window, cx)
                })
            });
            let states = self.view_layout_state_range(state_start);
            let compatible = layout_id == old.layout_root
                && self.layout_engine.as_ref().expect("layout engine").splice_layout_stamp() == stamp
                && !self.force_view_cache_refresh && states.is_empty();
            gaps.push(PrebuiltGap {
                key: gap_key, old, view, layout_id, state,
                claims: [gap_start..self.claim_log_len(), 0..0], states, node: None,
            });
            if !compatible {
                self.splice_building = false;
                self.release_retained_view(&kept);
                self.stash_prebuilt_gaps(gaps);
                return None;
            }
        }
        self.splice_building = false;
        Some(ViewSplice {
            anchor, kept, gaps, claims: [start..self.claim_log_len(), 0..0],
            states: self.view_layout_state_range(states_start),
        })
    }

    pub(crate) fn can_prepaint_view_splice(&mut self, splice: &ViewSplice, bounds: Bounds<Pixels>) -> bool {
        !self.force_view_cache_refresh && self.splice_context_supported()
            && self.retained_view_context_matches(&splice.anchor, bounds)
            && splice.gaps.iter().all(|gap| self.layout_bounds(gap.layout_id) == gap.old.bounds)
    }

    pub(crate) fn prepaint_view_splice(&mut self, splice: &mut ViewSplice, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let mut cursor = splice.anchor.prepaint_range.start.clone();
        // Only copied intervals have a valid affine dispatch mapping. A
        // removed interval is never a valid external parent of clean nodes.
        let mut relocated = smallvec::SmallVec::<[(Range<usize>, usize); 5]>::new();
        for gap in &mut splice.gaps {
            let old = &gap.old;
            let range = cursor..old.prepaint_range.start.clone();
            let source = range.start.dispatch_tree_index..range.end.dispatch_tree_index;
            let target = self.next_frame.dispatch_tree.len();
            self.replay_prepaint_segment(range, |id| relocate_dispatch(id, parent, &relocated));
            relocated.push((source.clone(), target));
            let node = DispatchNodeId(target + source.len() - 1);
            self.next_frame.dispatch_tree.set_active_node(node);
            let rebuild = old.rebuild.as_ref().expect("validated gap");
            self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
                gap.view.prepaint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, window, cx);
            });
            self.next_frame.dispatch_tree.set_active_node(parent);
            gap.node = Some(node);
            cursor = old.prepaint_range.end.clone();
        }
        self.replay_prepaint_segment(cursor..splice.anchor.prepaint_range.end.clone(), |id| {
            relocate_dispatch(id, parent, &relocated)
        });
    }

    pub(crate) fn paint_view_splice(&mut self, splice: &mut ViewSplice, owner: EntityId, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let mut cursor = splice.anchor.paint_range.start.clone();
        for gap in &mut splice.gaps {
            let old = &gap.old;
            self.replay_validated_paint(cursor..old.paint_range.start.clone());
            self.next_frame.dispatch_tree.set_active_node(gap.node.expect("prepainted gap"));
            let rebuild = old.rebuild.as_ref().expect("validated gap");
            self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
                gap.view.paint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, &mut (), window, cx);
            });
            self.next_frame.dispatch_tree.set_active_node(parent);
            cursor = old.paint_range.end.clone();
        }
        self.replay_validated_paint(cursor..splice.anchor.paint_range.end.clone());
        self.replayed_view_owners.insert(owner);
    }
}

fn relocate_dispatch(
    id: Option<DispatchNodeId>, anchor: DispatchNodeId, segments: &[(Range<usize>, usize)],
) -> Option<DispatchNodeId> {
    Some(id.and_then(|id| segments.iter().find_map(|(source, target)| {
        source.contains(&id.0).then(|| DispatchNodeId(target + id.0 - source.start))
    })).unwrap_or(anchor))
}
