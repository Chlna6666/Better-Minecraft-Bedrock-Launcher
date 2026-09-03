use super::*;
use std::{borrow::Borrow, ops::Range};

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

    /// Returns the scene ranges owned by the currently painted structural ancestor itself.
    ///
    /// This path is enabled only when the current element is executing solely to reach one or more
    /// targeted dirty descendants. Immediate-child retained ranges define the exact split even when
    /// a child emitted zero primitives in the previous frame, preserving border ordering when that
    /// child becomes visible in the current targeted frame.
    pub(crate) fn retained_self_scene_ranges_for_current(
        &self,
        bounds: Bounds<Pixels>,
    ) -> Option<RetainedSelfSceneRanges> {
        let retained_id = self.current_retained_element_id()?;
        if !self
            .invalidator
            .retained_path_is_descendant_only(&retained_id)
        {
            return None;
        }

        let parent = self.rendered_frame.retained_element_ranges.get(&retained_id)?;
        if parent.bounds != bounds {
            return None;
        }

        let parent_start = parent.paint_range.start.scene_index;
        let parent_end = parent.paint_range.end.scene_index;
        if parent_start > parent_end || parent_end > self.rendered_frame.scene.len() {
            return None;
        }
        if parent.metadata_range.start > parent.metadata_range.end
            || parent.metadata_range.end > self.rendered_frame.retained_element_order.len()
        {
            return None;
        }

        let parent_depth = retained_id.0.len();
        let mut child_scene_start: Option<usize> = None;
        let mut child_scene_end: Option<usize> = None;

        for metadata_index in parent.metadata_range.clone() {
            let key = &self.rendered_frame.retained_element_order[metadata_index];
            let candidate: &GlobalElementId = key.borrow();
            if candidate.0.len() != parent_depth + 1
                || !retained_path_has_prefix(candidate, &retained_id)
            {
                continue;
            }

            let child = self.rendered_frame.retained_element_ranges.get(candidate)?;
            let start = child.paint_range.start.scene_index;
            let end = child.paint_range.end.scene_index;
            if start > end || start < parent_start || end > parent_end {
                return None;
            }

            child_scene_start = Some(child_scene_start.map_or(start, |current| current.min(start)));
            child_scene_end = Some(child_scene_end.map_or(end, |current| current.max(end)));
        }

        let (child_scene_start, child_scene_end) = child_scene_start.zip(child_scene_end)?;
        if child_scene_start > child_scene_end {
            return None;
        }

        Some(RetainedSelfSceneRanges {
            prefix: parent_start..child_scene_start,
            suffix: child_scene_end..parent_end,
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

fn retained_path_has_prefix(path: &GlobalElementId, prefix: &GlobalElementId) -> bool {
    prefix.0.len() <= path.0.len()
        && prefix
            .0
            .iter()
            .zip(path.0.iter())
            .all(|(prefix, path)| prefix == path)
}
