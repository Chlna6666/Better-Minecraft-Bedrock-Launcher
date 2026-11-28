//! Multi-gap frame splicing, adapted from GPUI Fast's layout/gap transaction
//! (Apache-2.0; Zed Industries and Longbridge). Recompute native layout when
//! needed and prove clean placement before replaying the retained fragments.

use super::*;
use crate::{Element, IntoElement, ViewElement, ViewRequestLayout};
use crate::layout::{HeldLayout, LayoutClaimBlocks};

pub(super) struct PrebuiltGap {
    key: u64,
    old: RetainedViewRecord,
    view: ViewElement,
    layout_id: LayoutId,
    state: ViewRequestLayout,
    claims: LayoutClaimBlocks,
    states: StateAccessBlocks,
    node: Option<DispatchNodeId>,
}

pub(crate) struct ViewSplice {
    pub(crate) anchor: RetainedViewRecord,
    pub(crate) claims: LayoutClaimBlocks,
    pub(crate) states: StateAccessBlocks,
    kept: LayoutClaimBlocks,
    kept_states: StateAccessBlocks,
    held: Vec<HeldLayout>,
    gap_parents: smallvec::SmallVec<[(LayoutId, Option<LayoutId>); 4]>,
    transient_count: usize,
    parts: Vec<SplicePart>,
}

enum SplicePart {
    Clean(CleanPart),
    Dirty(PrebuiltGap),
    Ancestor(AncestorPart),
}

struct AncestorPart {
    key: u64,
    old: RetainedViewRecord,
    claims: LayoutClaimBlocks,
    states: StateAccessBlocks,
    // Number of following entries belonging to this retained ancestor.
    descendants: usize,
    prepaint: Option<Range<PrepaintStateIndex>>,
    reservation: Option<(ViewRetentionSlot, Range<usize>)>,
}

struct CleanPart {
    key: u64,
    old: RetainedViewRecord,
    claims: LayoutClaimBlocks,
    states: StateAccessBlocks,
    prepaint: Option<Range<PrepaintStateIndex>>,
    reservation: Option<(ViewRetentionSlot, Range<usize>)>,
}

impl SplicePart {
    fn old(&self) -> &RetainedViewRecord {
        match self {
            Self::Clean(part) => &part.old,
            Self::Dirty(part) => &part.old,
            Self::Ancestor(part) => &part.old,
        }
    }
}

impl Window {
    /// Restore application state identity; layout, prepaint and paint own their
    /// respective Fast key scopes.
    fn with_gap_scope<R>(
        &mut self,
        application_id: &GlobalElementId,
        inherited: &TextStyle,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let application = mem::replace(
            &mut self.element_id_stack,
            ElementIdStack::from_prefix(application_id.clone()),
        );
        let text = (!self.with_text_style_ref(|style| style == inherited)).then(|| {
            mem::replace(
                &mut self.text_style_stack,
                crate::fast::text_style::TextStyleStack::with_inherited(inherited),
            )
        });
        let result = f(self);
        if let Some(text) = text {
            self.text_style_stack = text;
        }
        self.element_id_stack = application;
        result
    }

    /// An abandoned gap can hand off its already-requested layout to the normal
    /// traversal. The original spans remain valid, including nested layouts.
    pub(crate) fn take_prebuilt_gap(
        &mut self, key: u64, id: Option<&GlobalElementId>, owner: EntityId,
    ) -> Option<(LayoutId, ViewRequestLayout)> {
        if self.prebuilt_gaps.is_empty() {
            return None;
        }
        // A colliding key must not consume another subtree's speculative claims.
        let rebuild = self.prebuilt_gaps.get(&key)?.old.rebuild.as_ref()?;
        if crate::fast::layout_key::parent_layout_key(self) != rebuild.parent_layout_key { return None; }
        let mut gap = self.prebuilt_gaps.remove(&key)?;
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
        &mut self, root: LayoutId, claims: &LayoutClaimBlocks, states: &StateAccessBlocks,
    ) -> Option<(LayoutClaimBlocks, StateAccessBlocks)> {
        let states = states.resolve(&self.prebuilt_state_relocations);
        if !claims.prepaint_blocks().is_empty()
            || !states.iter().all(|span| self.next_frame.accessed_element_states.contains(span.clone()))
        {
            return None;
        }
        let claimed = self.layout_engine.as_mut()?.adopt_claim_blocks(claims, root)?;
        let accesses = &mut self.next_frame.accessed_element_states;
        let start = accesses.len();
        for span in states.iter() {
            let target = accesses.len();
            accesses.move_to_end(span.clone());
            self.prebuilt_state_relocations.record(span.clone(), target);
        }
        Some((claimed, StateAccessBlocks::from_range(start..accesses.len())))
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
        self.cancel_view_layout_states(&splice.kept_states);
        self.stash_prebuilt_gaps(splice.parts);
        splice.anchor
    }

    fn stash_prebuilt_gaps(&mut self, parts: Vec<SplicePart>) {
        for part in parts {
            if let SplicePart::Dirty(gap) = part {
                let previous = self.prebuilt_gaps.insert(gap.key, gap);
                debug_assert!(previous.is_none(), "splice plan contains duplicate native keys");
                if let Some(previous) = previous {
                    self.release_retained_view(&previous.claims);
                    self.cancel_view_layout_states(&previous.states);
                }
            }
        }
    }

    /// Select disjoint dirty and clean subtrees, validate the partition before
    /// mutating native layout, then build them under their original paths.
    pub(crate) fn try_splice_view(
        &mut self, key: u64, owner: EntityId,
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
        let index = *self.rendered_frame.view_retentions.get(&key)?;
        let anchor = self.rendered_frame.view_retention_records.get(index)?.record.as_ref()?;
        let rebuild = anchor.rebuild.as_ref()?;
        if !anchor.splice_safe || !rebuild.splice_supported
            || rebuild.view.entity_id() != owner || id != Some(&rebuild.application_id)
            || crate::fast::layout_key::parent_layout_key(self) != rebuild.parent_layout_key
            || !self.retained_paint_context_matches(&rebuild.layout)
            || !anchor.layout_state_range.iter().all(|span| self.rendered_frame.accessed_element_states.contains(span.clone()))
            || !self.can_reuse_prepaint(&anchor.prepaint_range)
            || !self.can_reuse_paint(&anchor.paint_range)
        {
            return None;
        }
        let indices = self.splice_subtrees(anchor)?;
        let anchor = anchor.clone();
        let mut plan = Vec::with_capacity(indices.len());
        let mut claims = smallvec::SmallVec::<[(LayoutClaimBlocks, LayoutId); 4]>::new();
        let mut dispatch = smallvec::SmallVec::<[(Range<usize>, EntityId); 4]>::new();
        let mut prepaint = anchor.prepaint_range.start.clone();
        let mut paint = anchor.paint_range.start.clone();
        let mut open = smallvec::SmallVec::<[RetainedViewRecord; 4]>::new();
        for (gap_index, scope) in indices {
            while open.last().is_some_and(|old| old.nested.end <= gap_index) {
                let old = open.pop()?;
                if !self.splice_fragment_supported(&prepaint, &old.prepaint_range.end, &paint, &old.paint_range.end) {
                    return None;
                }
                prepaint = old.prepaint_range.end;
                paint = old.paint_range.end;
            }
            let slot = &self.rendered_frame.view_retention_records[gap_index];
            let old = slot.record.as_ref()?.clone();
            let gap_rebuild = old.rebuild.as_ref()?;
            let dirty = scope == Some(ViewDirtyScope::Direct);
            let ancestor = scope == Some(ViewDirtyScope::TraversalAncestor);
            if !anchor.layout_state_range.covers(&old.layout_state_range)
                || open.iter().any(|ancestor| !ancestor.layout_state_range.covers(&old.layout_state_range))
                || (scope.is_some() && !old.splice_safe)
                || (dirty && (!same_splice_geometry(&gap_rebuild.layout, &rebuild.layout)
                    || !same_splice_geometry(&old.context, &anchor.context)
                    || open.iter().any(|ancestor| !ancestor.layout_claims.covers(&old.layout_claims))))
                || self.rendered_frame.retained_scene_segments.get(old.scene_segments.clone()).is_none()
                || !self.splice_fragment_supported(&prepaint, &old.prepaint_range.start, &paint, &old.paint_range.start)
                || !self.can_reuse_prepaint(&old.prepaint_range)
                || !self.can_reuse_paint(&old.paint_range)
            {
                return None;
            }
            if ancestor {
                prepaint = old.prepaint_range.start.clone();
                paint = old.paint_range.start.clone();
                open.push(old.clone());
            } else {
                prepaint = old.prepaint_range.end.clone();
                paint = old.paint_range.end.clone();
            }
            let view = if dirty {
                dispatch.push((old.prepaint_range.start.dispatch_tree_index..old.prepaint_range.end.dispatch_tree_index, gap_rebuild.view.entity_id()));
                claims.push((old.layout_claims.clone(), old.layout_root));
                Some(gap_rebuild.view.upgrade()?.into_element())
            } else {
                None
            };
            plan.push((gap_index, slot.key, old, ancestor, view));
        }
        while let Some(old) = open.pop() {
            if !self.splice_fragment_supported(&prepaint, &old.prepaint_range.end, &paint, &old.paint_range.end) {
                return None;
            }
            prepaint = old.prepaint_range.end;
            paint = old.paint_range.end;
        }
        if !self.splice_fragment_supported(&prepaint, &anchor.prepaint_range.end, &paint, &anchor.paint_range.end)
            || !self.rendered_frame.dispatch_tree.can_splice_gaps(
                anchor.prepaint_range.start.dispatch_tree_index..anchor.prepaint_range.end.dispatch_tree_index,
                &dispatch,
            )
        {
            return None;
        }
        // If a dirty gap accessed a state also read outside its request span,
        // rebuilding just that gap cannot establish the retained owner's
        // state lifetime. Leave that unusual shared-state case to traversal.
        let source_states = &self.rendered_frame.accessed_element_states;
        let mut dirty_state_spans: SmallVec<[Range<usize>; 4]> = plan.iter()
            .filter(|(_, _, _, _, view)| view.is_some())
            .flat_map(|(_, _, old, _, _)| old.layout_state_range.iter().cloned())
            .collect();
        dirty_state_spans.sort_unstable_by_key(|span| span.start);
        let mut count = 0;
        for index in 0..dirty_state_spans.len() {
            let span = dirty_state_spans[index].clone();
            if count > 0 && dirty_state_spans[count - 1].end >= span.start {
                dirty_state_spans[count - 1].end = dirty_state_spans[count - 1].end.max(span.end);
            } else {
                dirty_state_spans[count] = span;
                count += 1;
            }
        }
        dirty_state_spans.truncate(count);
        let in_gaps: FxHashSet<_> = dirty_state_spans.iter()
            .flat_map(|span| source_states.keys(span.clone()))
            .collect();
        if !in_gaps.is_empty() && anchor.layout_state_range.iter().any(|span| {
            source_states.any_keys_outside(span.clone(), &dirty_state_spans, |state| in_gaps.contains(state))
        }) {
            return None;
        }
        let engine = self.layout_engine.as_mut()?;
        let start = engine.claim_log_len();
        let (kept, claim_map) = engine.try_keep_around_gaps(&anchor.layout_claims, &claims, anchor.layout_root)?;
        let Some(held) = engine.retained_layouts(&kept) else {
            engine.release_kept(&kept);
            return None;
        };
        let gap_parents: smallvec::SmallVec<[(LayoutId, Option<LayoutId>); 4]> = claims.iter()
            .map(|(_, id)| (*id, engine.layout_parent(*id)))
            .collect();
        let transient_count = engine.transient_count();
        let new_request_start = self.claim_log_len();
        let states_start = self.begin_view_element_states();
        // Fast kept_element_states excludes states accessed by rebuilt gaps.
        // Adapt the source's key set to the current frame-log representation:
        // keep the actual keys, and map contiguous surviving runs so that
        // clean nested records continue pointing into the new frame.
        let mut state_map = crate::layout::ClaimBlockMap::default();
        let target_states = &mut self.next_frame.accessed_element_states;
        for span in anchor.layout_state_range.iter() {
            // The preceding key check proved that no gap key occurs outside
            // its recorded spans. Copy contiguous stretches, as in Fast's
            // kept_keys, instead of hashing every retained state again.
            let mut cursor = span.start;
            let first = dirty_state_spans.partition_point(|gap| gap.end <= cursor);
            for gap in &dirty_state_spans[first..] {
                if gap.start >= span.end { break; }
                let end = gap.start.min(span.end);
                if cursor < end {
                    state_map.record(cursor..end, target_states.len());
                    target_states.share(source_states, cursor..end);
                }
                cursor = cursor.max(gap.end.min(span.end));
            }
            if cursor < span.end {
                state_map.record(cursor..span.end, target_states.len());
                target_states.share(source_states, cursor..span.end);
            }
        }
        state_map.sort_by_source();
        let kept_states = self.view_layout_state_range(states_start);
        // The engine's block map is the only source of retained claim relocation.
        let mut parts: Vec<SplicePart> = Vec::with_capacity(plan.len());
        let mut open = smallvec::SmallVec::<[usize; 8]>::new();
        for (gap_index, gap_key, old, ancestor, view) in plan {
            while open.last().is_some_and(|index| parts[*index].old().nested.end <= gap_index) {
                close_ancestor(&mut parts, open.pop().expect("open ancestor"));
            }
            let Some(mut view) = view else {
                let Some(claims) = claim_map.project(&old.layout_claims, !ancestor) else {
                    self.release_retained_view(&kept);
                    self.cancel_view_layout_states(&kept_states);
                    self.stash_prebuilt_gaps(parts);
                    return None;
                };
                let Some(states) = old.layout_state_range.project(&state_map, !ancestor) else {
                    self.release_retained_view(&kept);
                    self.cancel_view_layout_states(&kept_states);
                    self.stash_prebuilt_gaps(parts);
                    return None;
                };
                if ancestor {
                    open.push(parts.len());
                    parts.push(SplicePart::Ancestor(AncestorPart {
                        key: gap_key, old, claims, states,
                        descendants: 0, prepaint: None, reservation: None,
                    }));
                } else {
                    parts.push(SplicePart::Clean(CleanPart {
                        key: gap_key, old, claims, states,
                        prepaint: None, reservation: None,
                    }));
                }
                continue;
            };
            self.splice_building = true;
            let gap_start = self.claim_log_len();
            let state_start = self.begin_view_element_states();
            let rebuild = old.rebuild.as_ref().expect("validated gap");
            let (layout_id, state) = self.with_gap_scope(&rebuild.application_id, &rebuild.layout.text_style, |window| {
                window.with_parent_layout_key(rebuild.parent_layout_key, |window| {
                    let element_id = rebuild.application_id.last().expect("retained view element id");
                    let key = crate::fast::layout_key::push_layout_key(window, Some(element_id));
                    debug_assert_eq!(key, gap_key);
                    let result = view.request_layout(Some(&rebuild.application_id), None, window, cx);
                    crate::fast::layout_key::pop_layout_key(window);
                    result
                })
            });
            self.splice_building = false;
            let states = self.view_layout_state_range(state_start);
            let compatible = layout_id == old.layout_root
                && !self.force_view_cache_refresh;
            let gap_claims = gap_start..self.claim_log_len();
            for index in &open {
                let SplicePart::Ancestor(ancestor) = &mut parts[*index] else { unreachable!() };
                ancestor.claims.push_request(gap_claims.clone());
                for span in states.iter() {
                    ancestor.states.push(span.clone());
                }
            }
            parts.push(SplicePart::Dirty(PrebuiltGap {
                key: gap_key, old, view, layout_id, state,
                claims: LayoutClaimBlocks::request(gap_claims), states, node: None,
            }));
            if !compatible {
                self.release_retained_view(&kept);
                self.cancel_view_layout_states(&kept_states);
                self.stash_prebuilt_gaps(parts);
                return None;
            }
        }
        while let Some(index) = open.pop() { close_ancestor(&mut parts, index); }
        let engine = self.layout_engine.as_ref().expect("layout engine");
        if engine.transient_count() != transient_count
            || !engine.retained_topology_matches(&held)
            || !gap_parents.iter().all(|(id, parent)| engine.layout_parent(*id) == *parent)
        {
            self.release_retained_view(&kept);
            self.cancel_view_layout_states(&kept_states);
            self.stash_prebuilt_gaps(parts);
            return None;
        }
        let end = self.claim_log_len();
        let claims = if kept.prepaint_blocks().is_empty() {
            LayoutClaimBlocks::request(start..end)
        } else {
            let mut claims = kept.clone();
            claims.push_request(new_request_start..end);
            claims
        };
        Some(ViewSplice {
            anchor, kept, kept_states, held, gap_parents, transient_count, parts, claims,
            states: self.view_layout_state_range(states_start),
        })
    }

    pub(crate) fn can_prepaint_view_splice(&mut self, splice: &ViewSplice, bounds: Bounds<Pixels>) -> bool {
        // Fast splice_layout_holds: compare the actual native layouts after
        // computing the enclosing root. Event closures and clipping are not
        // relocatable yet, so any change still takes ordinary traversal.
        if !self.layout_engine.as_ref().is_some_and(|engine| {
            engine.transient_count() == splice.transient_count
                && engine.layouts_unchanged(&splice.held)
                && splice.gap_parents.iter().all(|(id, parent)| engine.layout_parent(*id) == *parent)
        }) {
            return false;
        }
        if self.force_view_cache_refresh || !self.splice_context_supported()
            || !self.retained_view_context_matches(&splice.anchor, bounds)
        {
            return false;
        }
        // Retained prepaint may have consumed a gap's bounds in arbitrary
        // callbacks. Until the layer path owns those geometry dependencies,
        // use the original placement and let ordinary traversal rebuild a
        // moved slot. No second native-subtree ownership scan is needed.
        splice.parts.iter().all(|part| match part {
            SplicePart::Dirty(gap) => self.layout_bounds(gap.layout_id) == gap.old.bounds,
            SplicePart::Clean(_) | SplicePart::Ancestor(_) => true,
        })
    }

    fn splice_fragment_supported(
        &self, prepaint: &PrepaintStateIndex, prepaint_end: &PrepaintStateIndex,
        paint: &PaintIndex, paint_end: &PaintIndex,
    ) -> bool {
        prepaint.accessed_element_states_index == prepaint_end.accessed_element_states_index
            && paint.accessed_element_states_index == paint_end.accessed_element_states_index
            && self.can_reuse_prepaint(&(prepaint.clone()..prepaint_end.clone()))
            && self.can_reuse_paint(&(paint.clone()..paint_end.clone()))
    }

    pub(crate) fn prepaint_view_splice(&mut self, splice: &mut ViewSplice, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        let mut relocated = smallvec::SmallVec::<[(Range<usize>, usize); 5]>::new();
        self.prepaint_splice_parts(&mut splice.parts, &splice.anchor.prepaint_range, parent, &mut relocated, cx);
    }

    fn prepaint_splice_parts(
        &mut self, mut parts: &mut [SplicePart], range: &Range<PrepaintStateIndex>,
        parent: DispatchNodeId, relocated: &mut SmallVec<[(Range<usize>, usize); 5]>, cx: &mut App,
    ) {
        let mut cursor = range.start.clone();
        while let Some((part, rest)) = parts.split_first_mut() {
            replay_prepaint(self, cursor..part.old().prepaint_range.start.clone(), parent, relocated);
            parts = rest;
            match part {
                SplicePart::Clean(clean) => {
                    let start = self.prepaint_index();
                    clean.reservation = Some(self.reserve_spliced_clean(clean.key, &clean.old));
                    replay_prepaint(self, clean.old.prepaint_range.clone(), parent, relocated);
                    clean.prepaint = Some(start..self.prepaint_index());
                    cursor = clean.old.prepaint_range.end.clone();
                }
                SplicePart::Dirty(gap) => {
                    let old = &gap.old;
                    let wrapper = DispatchNodeId(old.prepaint_range.start.dispatch_tree_index - 1);
                    let node = relocate_dispatch(Some(wrapper), parent, relocated).expect("validated gap wrapper");
                    self.next_frame.dispatch_tree.set_active_node(node);
                    let rebuild = old.rebuild.as_ref().expect("validated gap");
                    self.with_gap_scope(&rebuild.application_id, &old.context.text_style, |window| {
                        let scope = crate::fast::layout_key::enter_prepaint_scope(window, gap.key);
                        gap.view.prepaint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, window, cx);
                        crate::fast::layout_key::exit_prepaint_scope(window, scope);
                    });
                    self.next_frame.dispatch_tree.set_active_node(parent);
                    gap.node = Some(node);
                    cursor = old.prepaint_range.end.clone();
                }
                SplicePart::Ancestor(ancestor) => {
                    let (children, tail) = parts.split_at_mut(ancestor.descendants);
                    parts = tail;
                    let owner = ancestor.old.rebuild.as_ref().expect("validated ancestor").view.entity_id();
                    let slot = self.reserve_view_retention(ancestor.key, owner);
                    let start = self.prepaint_index();
                    let claims_start = self.claim_log_len();
                    self.with_rendered_view(owner, |window| {
                        window.prepaint_splice_parts(children, &ancestor.old.prepaint_range, parent, relocated, cx);
                    });
                    ancestor.claims.push_prepaint(claims_start..self.claim_log_len());
                    ancestor.prepaint = Some(start..self.prepaint_index());
                    let nested = self.finish_view_retention_scope(&slot);
                    ancestor.reservation = Some((slot, nested));
                    cursor = ancestor.old.prepaint_range.end.clone();
                }
            }
        }
        replay_prepaint(self, cursor..range.end.clone(), parent, relocated);
    }

    pub(crate) fn paint_view_splice(&mut self, splice: &mut ViewSplice, owner: EntityId, cx: &mut App) {
        let parent = self.next_frame.dispatch_tree.active_node_id().expect("view dispatch node");
        self.paint_splice_parts(&mut splice.parts, &splice.anchor.paint_range, parent, cx);
        self.replayed_view_owners.insert(owner);
    }

    fn paint_splice_parts(
        &mut self, mut parts: &mut [SplicePart], range: &Range<PaintIndex>,
        parent: DispatchNodeId, cx: &mut App,
    ) {
        let mut cursor = range.start.clone();
        while let Some((part, rest)) = parts.split_first_mut() {
            self.replay_validated_paint(cursor..part.old().paint_range.start.clone());
            parts = rest;
            match part {
                SplicePart::Clean(clean) => {
                    let start = self.paint_index();
                    let owner = clean.old.rebuild.as_ref().expect("validated clean subtree").view.entity_id();
                    self.replay_retained_view_paint(owner, &clean.old);
                    let paint = start..self.paint_index();
                    let (slot, nested) = clean.reservation.take().expect("prepainted clean subtree");
                    self.commit_spliced_clean(&slot, nested, &clean.old, std::mem::take(&mut clean.claims),
                        clean.prepaint.take().expect("prepaint range"), paint, std::mem::take(&mut clean.states));
                    cursor = clean.old.paint_range.end.clone();
                }
                SplicePart::Dirty(gap) => {
                    let old = &gap.old;
                    self.next_frame.dispatch_tree.set_active_node(gap.node.expect("prepainted gap"));
                    let rebuild = old.rebuild.as_ref().expect("validated gap");
                    self.with_gap_scope(&rebuild.application_id, &old.context.text_style, |window| {
                        let previous = window.paint_layout_key.replace(gap.key);
                        window.record_debug_element_paint(old.bounds, cx);
                        gap.view.paint(Some(&rebuild.application_id), None, old.bounds, &mut gap.state, &mut (), window, cx);
                        window.paint_layout_key = previous;
                    });
                    self.next_frame.dispatch_tree.set_active_node(parent);
                    cursor = old.paint_range.end.clone();
                }
                SplicePart::Ancestor(ancestor) => {
                    let (children, tail) = parts.split_at_mut(ancestor.descendants);
                    parts = tail;
                    let owner = ancestor.old.rebuild.as_ref().expect("validated ancestor").view.entity_id();
                    let (slot, nested) = ancestor.reservation.take().expect("prepainted ancestor");
                    let scene_segments_start = self.next_frame.retained_scene_segments.len();
                    let start = self.paint_index();
                    self.with_rendered_view(owner, |window| {
                        window.paint_splice_parts(children, &ancestor.old.paint_range, parent, cx);
                        let end = window.paint_index();
                        window.commit_view_retention(&slot, RetainedViewRecord {
                            layout_root: ancestor.old.layout_root, layout_claims: std::mem::take(&mut ancestor.claims),
                            prepaint_range: ancestor.prepaint.take().expect("prepaint range"),
                            paint_range: start..end, bounds: ancestor.old.bounds,
                            context: ancestor.old.context.clone(), rebuild: ancestor.old.rebuild.clone(),
                            splice_safe: ancestor.old.splice_safe, nested,
                            scene_segments: scene_segments_start..window.next_frame.retained_scene_segments.len(),
                            layout_state_range: std::mem::take(&mut ancestor.states),
                        }, None);
                    });
                    self.replayed_view_owners.insert(owner);
                    cursor = ancestor.old.paint_range.end.clone();
                }
            }
        }
        self.replay_validated_paint(cursor..range.end.clone());
    }
}

fn same_splice_geometry(a: &RetainedPaintContext, b: &RetainedPaintContext) -> bool {
    a.device_scale_factor == b.device_scale_factor
        && a.opacity == b.opacity
        && a.scale == b.scale
        && a.translation == b.translation
        && a.element_offset == b.element_offset
        && a.text_raster_scale == b.text_raster_scale
        && a.content_mask == b.content_mask
        && a.visual_content_mask == b.visual_content_mask
        && a.rem_size == b.rem_size
}

fn close_ancestor(parts: &mut [SplicePart], index: usize) {
    let descendants = parts.len() - index - 1;
    let SplicePart::Ancestor(ancestor) = &mut parts[index] else { unreachable!() };
    ancestor.descendants = descendants;
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
