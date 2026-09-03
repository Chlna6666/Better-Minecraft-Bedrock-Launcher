use super::*;

impl Window {
    /// Compatibility name for the retained rendering identity captured by interaction handlers.
    ///
    /// This is deliberately separate from the state-bearing `GlobalElementId`: anonymous elements
    /// acquire synthetic positional segments only in the retained identity stack.
    pub(crate) fn current_instance_path(&self) -> Option<GlobalElementId> {
        self.current_retained_element_id()
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
    pub(crate) fn with_retained_child_scope<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.retained_child_slot_stack.push(0);
        let result = f(self);
        self.retained_child_slot_stack.pop();
        result
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
