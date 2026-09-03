use super::*;

impl Window {
    /// Compatibility name for the retained rendering identity captured by interaction handlers.
    ///
    /// This is deliberately separate from the state-bearing `GlobalElementId`: anonymous elements
    /// acquire synthetic positional segments only in the retained identity stack.
    pub(crate) fn current_instance_path(&self) -> Option<GlobalElementId> {
        self.current_retained_element_id()
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
