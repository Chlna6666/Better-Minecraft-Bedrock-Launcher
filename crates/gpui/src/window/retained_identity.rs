use super::*;
use crate::element::RetainedDivSelfSceneStyle;
use std::ops::Range;

/// Previous-frame scene spans that belong to a parent element itself rather than to its children.
///
/// GPUI paints a styled `Div` as `shadow/background -> children -> border`. Keeping the prefix and
/// suffix separate lets a structural ancestor route traversal to one dirty child without regenerating
/// its own stable primitives.
pub(crate) struct RetainedSelfSceneRanges {
    pub(crate) prefix: Range<usize>,
    pub(crate) suffix: Range<usize>,
}

impl Window {
    /// Execute lazy child construction with a fresh parent-local retained slot namespace.
    ///
    /// Normal elements allocate all children during `request_layout`, while virtualized elements
    /// such as [`crate::List`] may create their visible children later during prepaint. At that
    /// point the original `begin_retained_element` child counter has already been popped, so lazy
    /// children must establish an equivalent scope explicitly. Without this, every lazy child can
    /// fall back to slot zero (or consume an unrelated ancestor counter), causing retained ranges,
    /// hitboxes and listeners to be reconciled against the wrong element after list expansion or
    /// recycling.
    ///
    /// Callers that retry a transactional prepaint must create a new scope for every attempt so the
    /// same logical children receive the same slots after rollback.
    pub(crate) fn with_retained_child_scope<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.retained_child_slot_stack.push(0);
        let result = f(self);
        self.retained_child_slot_stack.pop();
        result
    }

    /// Execute a lazily materialized child subtree under a stable retained-only key.
    ///
    /// Virtualized containers must not identify children by materialization order: measuring an
    /// overdraw item, filling upward with `push_front`, or changing the visible range would shift
    /// positional slots and could reconcile a previous hitbox/listener with a different logical
    /// item. The namespace is deliberately separate from `element_id_stack`, so it has no effect on
    /// application element state or user-visible IDs.
    pub(crate) fn with_retained_child_key<R>(
        &mut self,
        key: impl Into<ElementId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.retained_element_id_stack.push(key.into());
        self.retained_child_slot_stack.push(0);
        let result = f(self);
        self.retained_child_slot_stack.pop();
        self.retained_element_id_stack.pop();
        result
    }

    /// Returns the previous scene ranges owned by the currently painted `Div` itself.
    ///
    /// Unlike full subtree replay, this path is safe on generic application-dirty frames because it
    /// never replays hitboxes/listeners or child lifecycle state. The current `Div` must provide an
    /// exact shadow/background/border key matching the previous retained range, and its bounds must
    /// be unchanged. The child split is recorded directly during paint, so child insertion/removal
    /// cannot move a retained border to the wrong side of the current child scene.
    pub(crate) fn retained_self_scene_ranges_for_current(
        &self,
        bounds: Bounds<Pixels>,
        current_style: &RetainedDivSelfSceneStyle,
    ) -> Option<RetainedSelfSceneRanges> {
        if self.force_view_cache_refresh() {
            return None;
        }

        let retained_id = self.current_retained_element_id()?;
        let parent = self.rendered_frame.retained_element_ranges.get(&retained_id)?;
        let self_scene = parent.div_self_scene.as_ref()?;
        if parent.bounds != bounds || &self_scene.style != current_style {
            return None;
        }

        let parent_start = parent.paint_range.start.scene_index;
        let parent_end = parent.paint_range.end.scene_index;
        let child_start = self_scene.child_scene_range.start;
        let child_end = self_scene.child_scene_range.end;
        if parent_start > child_start
            || child_start > child_end
            || child_end > parent_end
            || parent_end > self.rendered_frame.scene.len()
        {
            return None;
        }

        Some(RetainedSelfSceneRanges {
            prefix: parent_start..child_start,
            suffix: child_end..parent_end,
        })
    }

    /// Replay a validated previous-frame scene span into the current frame without replaying
    /// hitboxes/listeners or any other lifecycle side effects.
    pub(crate) fn replay_retained_scene_range(&mut self, range: Range<usize>) {
        debug_assert!(range.start <= range.end);
        debug_assert!(range.end <= self.rendered_frame.scene.len());
        if range.is_empty() {
            return;
        }
        self.next_frame.scene.replay(range, &self.rendered_frame.scene);
    }

    /// Bound the persistent focus-target lookup when applications create and discard many unique
    /// focus handles over time. The common path pays only one length comparison; pruning happens
    /// only after the table becomes unusually large.
    pub(crate) fn prune_focus_retained_targets_if_needed(&mut self) {
        const PRUNE_THRESHOLD: usize = 1_024;
        if self.focus_retained_targets.len() <= PRUNE_THRESHOLD {
            return;
        }

        let rendered = &self.rendered_frame.dispatch_tree;
        let next = &self.next_frame.dispatch_tree;
        self.focus_retained_targets.retain(|focus_id, _| {
            rendered.focusable_node_id(*focus_id).is_some()
                || next.focusable_node_id(*focus_id).is_some()
        });
    }
}
