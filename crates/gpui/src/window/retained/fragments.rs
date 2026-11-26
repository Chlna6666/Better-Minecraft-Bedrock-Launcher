use super::super::*;
impl Window {
    /// Returns whether the current view may use retained-path selective replay in this frame.
    ///
    /// A generic application notification is conservative for the notified view and every nested
    /// view below it, because the parent may have changed inherited layout/paint context. A generic
    /// dirty child does not poison its ancestors: they still rerender to route traversal to the
    /// child, but proven-stable retained elements in those ancestors remain replayable.
    pub(crate) fn retained_replay_is_targeted(&self) -> bool {
        if !self.invalidator.active_targeted_replay() {
            return false;
        }
        let Some(view_id) = self.current_view_or_root() else {
            return false;
        };
        if !self.invalidator.has_active_generic_dirty_views() {
            return true;
        }
        if self.invalidator.active_generic_view_is_dirty(view_id) {
            return false;
        }
        !self
            .rendered_frame
            .dispatch_tree
            .view_path_reversed(view_id)
            .any(|ancestor| self.invalidator.active_generic_view_is_dirty(ancestor))
    }

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
    pub(crate) fn with_retained_child_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
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

    /// Bound the persistent focus-target lookup when applications create and discard many unique
    /// focus handles over time. The common path pays only one length comparison; pruning happens
    /// only after the table becomes unusually large.
    pub(crate) fn prune_focus_retained_targets(&mut self) {
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
