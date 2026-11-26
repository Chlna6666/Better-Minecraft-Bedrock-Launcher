//! Multi-gap frame splicing, adapted from GPUI Fast's layout/gap transaction
//! (Apache-2.0; Zed Industries and Longbridge). Recompute native layout when
//! needed and prove clean placement before replaying the retained fragments.

use super::*;
use crate::{Element, IntoElement, ViewElement, ViewRequestLayout};
use crate::layout::{LayoutClaimBlocks, NativePlacementProof};

pub(super) struct PrebuiltGap {
    key: GlobalElementId,
    old: RetainedViewRecord,
    view: ViewElement,
    layout_id: LayoutId,
    state: ViewRequestLayout,
    claims: LayoutClaimBlocks,
    states: Range<usize>,
    node: Option<DispatchNodeId>,
}

pub(crate) struct ViewSplice {
    pub(crate) anchor: RetainedViewRecord,
    pub(crate) claims: LayoutClaimBlocks,
    pub(crate) states: Range<usize>,
    kept: LayoutClaimBlocks,
    placement: NativePlacementProof,
    parts: Vec<SplicePart>,
}

enum SplicePart {
    Clean(CleanPart),
    Dirty(PrebuiltGap),
}

struct CleanPart {
    key: GlobalElementId,
    old: RetainedViewRecord,
    claims: LayoutClaimBlocks,
    states: Range<usize>,
    prepaint: Option<Range<PrepaintStateIndex>>,
    reservation: Option<(ViewRetentionSlot, Range<usize>)>,
}

impl SplicePart {
    fn old(&self) -> &RetainedViewRecord {
        match self {
            Self::Clean(part) => &part.old,
            Self::Dirty(part) => &part.old,
        }
    }
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
        if !compatible || !gap.state.adopt_prebuilt_layout(self, &gap.claims, &gap.states) {
            self.release_retained_view(&gap.claims);
            self.cancel_view_layout_states(&gap.states);
            return None;
        }
        Some((gap.layout_id, gap.state))
    }

    pub(crate) fn adopt_gap_layout_spans(
        &mut self, root: LayoutId, claims: &LayoutClaimBlocks, states: &Range<usize>,
    ) -> Option<(LayoutClaimBlocks, Range<usize>)> {
        let states = self.prebuilt_state_relocations.resolve(states);
        if !claims.prepaint_blocks().is_empty() || claims.request_blocks().len() != 1
            || self.next_frame.accessed_element_states.get(states.clone()).is_none()
        {
            return None;
        }
        let claimed = self.layout_engine.as_mut()?.adopt_claim_span(&claims.request_blocks()[0], root)?;
        let accesses = &mut self.next_frame.accessed_element_states;
        let start = accesses.len();
        accesses.reserve(states.len());
        for index in states.clone() {
            let state = accesses[index].take();
            accesses.push(state);
        }
        let end = accesses.len();
        self.prebuilt_state_relocations.record(states, start);
        Some((LayoutClaimBlocks::request(claimed), start..end))
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
        self.stash_prebuilt_gaps(splice.parts);
        splice.anchor
    }

    fn stash_prebuilt_gaps(&mut self, parts: Vec<SplicePart>) {
        for part in parts {
            if let SplicePart::Dirty(gap) = part {
                self.prebuilt_gaps.insert(gap.key.clone(), gap);
            }
        }
    }

    /// Select disjoint dirty and clean subtrees, validate the partition before
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
        let indices = self.splice_subtrees(&anchor)?;
        let mut plan = Vec::with_capacity(indices.len());
        let mut claims = smallvec::SmallVec::<[(LayoutClaimBlocks, LayoutId); 4]>::new();
        let mut dispatch = smallvec::SmallVec::<[(Range<usize>, EntityId); 4]>::new();
        let mut prepaint = anchor.prepaint_range.start.clone();
        let mut paint = anchor.paint_range.start.clone();
        for (gap_index, dirty) in indices {
            let slot = &self.rendered_frame.view_retention_records[gap_index];
            let old = slot.record.as_ref()?.clone();
            let gap_rebuild = old.rebuild.as_ref()?;
            if !old.layout_state_range.is_empty() || !old.layout_claims.prepaint_blocks().is_empty()
                || (dirty && (!old.splice_safe || gap_rebuild.layout != rebuild.layout || old.context != anchor.context))
                || self.rendered_frame.retained_scene_segments.get(old.scene_segments.clone()).is_none()
                // Stretches outside all immediate subtrees cannot contain mutable state.
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
            let view = if dirty {
                dispatch.push((old.prepaint_range.start.dispatch_tree_index..old.prepaint_range.end.dispatch_tree_index, gap_rebuild.view.entity_id()));
                claims.push((old.layout_claims.clone(), old.layout_root));
                Some(gap_rebuild.view.upgrade()?.into_element())
            } else {
                None
            };
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
        let start = engine.claim_log_len();
        let (kept, claim_map) = engine.try_keep_around_gaps(&anchor.layout_claims, &claims, anchor.layout_root)?;
        let Some(placement) = engine.capture_splice_placement(&kept, &claims) else {
            engine.release_kept(&kept);
            return None;
        };
        let states_start = self.begin_view_element_states();
        // The engine's block map is the only source of retained claim relocation.
        let mut parts = Vec::with_capacity(plan.len());
        for (gap_key, old, view) in plan {
            let Some(mut view) = view else {
                let Some(claims) = claim_map.project(&old.layout_claims, true) else {
                    self.layout_engine.as_mut().expect("layout engine").discard_splice_placement(placement);
                    self.release_retained_view(&kept);
                    self.stash_prebuilt_gaps(parts);
                    return None;
                };
                parts.push(SplicePart::Clean(CleanPart {
                    key: gap_key, old, claims, states: states_start..states_start,
                    prepaint: None, reservation: None,
                }));
                continue;
            };
            self.splice_building = true;
            let gap_start = self.claim_log_len();
            let state_start = self.begin_view_element_states();
            let rebuild = old.rebuild.as_ref().expect("validated gap");
            let (layout_id, state) = self.with_gap_paths(&gap_key, &rebuild.application_id, |window| {
                window.with_retained_child_scope(|window| {
                    view.request_layout(Some(&rebuild.application_id), None, window, cx)
                })
            });
            self.splice_building = false;
            let states = self.view_layout_state_range(state_start);
            let compatible = layout_id == old.layout_root
                && !self.force_view_cache_refresh && states.is_empty();
            parts.push(SplicePart::Dirty(PrebuiltGap {
                key: gap_key, old, view, layout_id, state,
                claims: LayoutClaimBlocks::request(gap_start..self.claim_log_len()), states, node: None,
            }));
            if !compatible {
                self.layout_engine.as_mut().expect("layout engine").discard_splice_placement(placement);
                self.release_retained_view(&kept);
                self.stash_prebuilt_gaps(parts);
                return None;
            }
        }
        if !self.layout_engine.as_ref().expect("layout engine").splice_topology_matches(placement) {
            self.layout_engine.as_mut().expect("layout engine").discard_splice_placement(placement);
            self.release_retained_view(&kept);
            self.stash_prebuilt_gaps(parts);
            return None;
        }
        Some(ViewSplice {
            anchor, kept, placement, parts, claims: LayoutClaimBlocks::request(start..self.claim_log_len()),
            states: self.view_layout_state_range(states_start),
        })
    }

    pub(crate) fn can_prepaint_view_splice(&mut self, splice: &ViewSplice, bounds: Bounds<Pixels>) -> bool {
        let placement_matches = self.layout_engine.as_mut()
            .is_some_and(|engine| engine.finish_splice_placement(splice.placement));
        placement_matches && !self.force_view_cache_refresh && self.splice_context_supported()
            && self.retained_view_context_matches(&splice.anchor, bounds)
            && splice.parts.iter().all(|part| match part {
                SplicePart::Dirty(gap) => self.layout_bounds(gap.layout_id) == gap.old.bounds,
                SplicePart::Clean(_) => true,
            })
    }

    pub(crate) fn prepaint_view_splice(&mut self, splice: &mut ViewSplice, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let mut cursor = splice.anchor.prepaint_range.start.clone();
        // Only copied intervals have a valid affine dispatch mapping. A
        // removed interval is never a valid external parent of clean nodes.
        let mut relocated = smallvec::SmallVec::<[(Range<usize>, usize); 5]>::new();
        for part in &mut splice.parts {
            let old_start = part.old().prepaint_range.start.clone();
            replay_prepaint(self, cursor..old_start, parent, &mut relocated);
            match part {
                SplicePart::Clean(clean) => {
                    let start = self.prepaint_index();
                    clean.reservation = Some(self.reserve_spliced_clean(clean.key.clone(), &clean.old));
                    replay_prepaint(self, clean.old.prepaint_range.clone(), parent, &mut relocated);
                    clean.prepaint = Some(start..self.prepaint_index());
                    cursor = clean.old.prepaint_range.end.clone();
                }
                SplicePart::Dirty(gap) => {
                    let old = &gap.old;
                    let wrapper = DispatchNodeId(old.prepaint_range.start.dispatch_tree_index - 1);
                    let node = relocate_dispatch(Some(wrapper), parent, &relocated).expect("validated gap wrapper");
                    self.next_frame.dispatch_tree.set_active_node(node);
                    let rebuild = old.rebuild.as_ref().expect("validated gap");
                    self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
                        gap.view.prepaint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, window, cx);
                    });
                    self.next_frame.dispatch_tree.set_active_node(parent);
                    gap.node = Some(node);
                    cursor = old.prepaint_range.end.clone();
                }
            }
        }
        replay_prepaint(self, cursor..splice.anchor.prepaint_range.end.clone(), parent, &mut relocated);
    }

    pub(crate) fn paint_view_splice(&mut self, splice: &mut ViewSplice, owner: EntityId, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let mut cursor = splice.anchor.paint_range.start.clone();
        for part in &mut splice.parts {
            self.replay_validated_paint(cursor..part.old().paint_range.start.clone());
            match part {
                SplicePart::Clean(clean) => {
                    let start = self.paint_index();
                    self.replay_validated_paint(clean.old.paint_range.clone());
                    let paint = start..self.paint_index();
                    let (slot, nested) = clean.reservation.take().expect("prepainted clean subtree");
                    self.commit_spliced_clean(&slot, nested, &clean.old, clean.claims.clone(),
                        clean.prepaint.take().expect("prepaint range"), paint, clean.states.clone());
                    cursor = clean.old.paint_range.end.clone();
                }
                SplicePart::Dirty(gap) => {
                    let old = &gap.old;
                    self.next_frame.dispatch_tree.set_active_node(gap.node.expect("prepainted gap"));
                    let rebuild = old.rebuild.as_ref().expect("validated gap");
                    self.with_gap_paths(&gap.key, &rebuild.application_id, |window| {
                        gap.view.paint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, &mut (), window, cx);
                    });
                    self.next_frame.dispatch_tree.set_active_node(parent);
                    cursor = old.paint_range.end.clone();
                }
            }
        }
        self.replay_validated_paint(cursor..splice.anchor.paint_range.end.clone());
        self.replayed_view_owners.insert(owner);
    }
}

fn replay_prepaint(
    window: &mut Window, range: Range<PrepaintStateIndex>, parent: DispatchNodeId,
    relocated: &mut SmallVec<[(Range<usize>, usize); 5]>,
) {
    let source = range.start.dispatch_tree_index..range.end.dispatch_tree_index;
    let target = window.next_frame.dispatch_tree.len();
    window.replay_prepaint_segment(range, |id| relocate_dispatch(id, parent, relocated));
    if !source.is_empty() {
        relocated.push((source, target));
    }
}

fn relocate_dispatch(
    id: Option<DispatchNodeId>, anchor: DispatchNodeId, segments: &[(Range<usize>, usize)],
) -> Option<DispatchNodeId> {
    Some(id.and_then(|id| {
        let index = segments.partition_point(|(source, _)| source.end <= id.0);
        let (source, target) = segments.get(index)?;
        source.contains(&id.0).then(|| DispatchNodeId(target + id.0 - source.start))
    }).unwrap_or(anchor))
}
