//! Single-gap frame splicing, adapted from GPUI Fast's layout/gap transaction
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
}

pub(crate) struct ViewSplice {
    pub(crate) anchor: RetainedViewRecord,
    pub(crate) claims: LayoutClaimSpans,
    pub(crate) states: Range<usize>,
    kept: LayoutClaimSpans,
    gap: PrebuiltGap,
    gap_node: Option<DispatchNodeId>,
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
        self.prebuilt_gaps.insert(splice.gap.key.clone(), splice.gap);
        splice.anchor
    }

    /// Initial proof: one dirty leaf, matching inheritance, no ancestor-owned
    /// element state or lazy layout, no mutation of native layout. General
    /// multi-gap or changed-geometry splices need a broader placement proof.
    pub(crate) fn try_splice_view(
        &mut self, key: &GlobalElementId, owner: EntityId,
        id: Option<&GlobalElementId>, cx: &mut App,
    ) -> Option<ViewSplice> {
        if self.view_dirty_scope(owner) != Some(ViewDirtyScope::TraversalAncestor)
            || self.direct_dirty_views.len() != 1
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
        {
            return None;
        }
        let gap_index = self.single_dirty_gap(&anchor)?;
        let slot = &self.rendered_frame.view_retention_records[gap_index];
        let old = slot.record.as_ref()?.clone();
        let gap_key = slot.key.clone();
        if self.rendered_frame.view_retentions.get(&gap_key).copied() != Some(gap_index) {
            return None;
        }
        let gap_rebuild = old.rebuild.as_ref()?.clone();
        if !old.splice_safe || !old.layout_state_range.is_empty()
            || gap_rebuild.layout != rebuild.layout || old.context != anchor.context
            || anchor.prepaint_range.start.accessed_element_states_index != old.prepaint_range.start.accessed_element_states_index
            || anchor.prepaint_range.end.accessed_element_states_index != old.prepaint_range.end.accessed_element_states_index
            || anchor.paint_range.start.accessed_element_states_index != old.paint_range.start.accessed_element_states_index
            || anchor.paint_range.end.accessed_element_states_index != old.paint_range.end.accessed_element_states_index
        {
            return None;
        }
        if !self.can_reuse_prepaint(&anchor.prepaint_range)
            || !self.can_reuse_prepaint(&old.prepaint_range)
            || !self.can_reuse_prepaint(&(anchor.prepaint_range.start.clone()..old.prepaint_range.start.clone()))
            || !self.can_reuse_prepaint(&(old.prepaint_range.end.clone()..anchor.prepaint_range.end.clone()))
            || !self.can_reuse_paint(&anchor.paint_range)
            || !self.can_reuse_paint(&(anchor.paint_range.start.clone()..old.paint_range.start.clone()))
            || !self.can_reuse_paint(&(old.paint_range.end.clone()..anchor.paint_range.end.clone()))
        {
            return None;
        }
        let outer_dispatch = anchor.prepaint_range.start.dispatch_tree_index..anchor.prepaint_range.end.dispatch_tree_index;
        let gap_dispatch = old.prepaint_range.start.dispatch_tree_index..old.prepaint_range.end.dispatch_tree_index;
        if !self.rendered_frame.dispatch_tree.can_splice_gap(outer_dispatch, gap_dispatch, gap_rebuild.view.entity_id()) {
            return None;
        }
        let mut view = gap_rebuild.view.upgrade()?.into_element();
        let engine = self.layout_engine.as_mut()?;
        let stamp = engine.splice_layout_stamp();
        if stamp.3 != 0 {
            return None;
        }
        let start = engine.claim_log_len();
        let kept = engine.try_keep_around_gap(&anchor.layout_claims, &old.layout_claims, anchor.layout_root, old.layout_root)?;
        let gap_start = self.claim_log_len();
        let state_start = self.begin_view_element_states();
        self.splice_building = true;
        let (layout_id, state) = self.with_gap_paths(&gap_key, &gap_rebuild.application_id, |window| {
            window.with_retained_child_scope(|window| {
                view.request_layout(Some(&gap_rebuild.application_id), None, window, cx)
            })
        });
        self.splice_building = false;
        let end = self.claim_log_len();
        let states = self.view_layout_state_range(state_start);
        let gap = PrebuiltGap {
            key: gap_key, old, view, layout_id, state,
            claims: [gap_start..end, 0..0], states: states.clone(),
        };
        if layout_id != gap.old.layout_root
            || self.layout_engine.as_ref().expect("layout engine").splice_layout_stamp() != stamp
            || self.force_view_cache_refresh || !states.is_empty()
        {
            self.release_retained_view(&kept);
            self.prebuilt_gaps.insert(gap.key.clone(), gap);
            return None;
        }
        Some(ViewSplice {
            anchor, kept, gap, claims: [start..end, 0..0], states, gap_node: None,
        })
    }

    pub(crate) fn can_prepaint_view_splice(&mut self, splice: &ViewSplice, bounds: Bounds<Pixels>) -> bool {
        !self.force_view_cache_refresh && self.splice_context_supported()
            && self.retained_view_context_matches(&splice.anchor, bounds)
            && self.layout_bounds(splice.gap.layout_id) == splice.gap.old.bounds
    }

    pub(crate) fn prepaint_view_splice(&mut self, splice: &mut ViewSplice, cx: &mut App) {
        let gap = &mut splice.gap;
        let old = &gap.old;
        let prefix = splice.anchor.prepaint_range.start.clone()..old.prepaint_range.start.clone();
        let suffix = old.prepaint_range.end.clone()..splice.anchor.prepaint_range.end.clone();
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let source_start = prefix.start.dispatch_tree_index;
        let source_end = prefix.end.dispatch_tree_index;
        let target_start = self.next_frame.dispatch_tree.len();
        self.replay_prepaint_segment(prefix, |_| Some(parent));
        // The view wrapper was copied in the prefix. Do not push another.
        let node = DispatchNodeId(target_start + source_end - source_start - 1);
        self.next_frame.dispatch_tree.set_active_node(node);
        let rebuild = old.rebuild.as_ref().expect("validated gap");
        self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
            gap.view.prepaint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, window, cx);
        });
        self.next_frame.dispatch_tree.set_active_node(parent);
        self.replay_prepaint_segment(suffix, |source| match source {
            Some(source) if (source_start..source_end).contains(&source.0) =>
                Some(DispatchNodeId(target_start + source.0 - source_start)),
            _ => Some(parent),
        });
        splice.gap_node = Some(node);
    }

    pub(crate) fn paint_view_splice(&mut self, splice: &mut ViewSplice, owner: EntityId, cx: &mut App) {
        let gap = &mut splice.gap;
        let old = &gap.old;
        self.replay_validated_paint(splice.anchor.paint_range.start.clone()..old.paint_range.start.clone());
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let node = splice.gap_node.expect("prepainted gap");
        self.next_frame.dispatch_tree.set_active_node(node);
        let rebuild = old.rebuild.as_ref().expect("validated gap");
        self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
            gap.view.paint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, &mut (), window, cx);
        });
        self.next_frame.dispatch_tree.set_active_node(parent);
        self.replay_validated_paint(old.paint_range.end.clone()..splice.anchor.paint_range.end.clone());
        self.replayed_view_owners.insert(owner);
    }
}
