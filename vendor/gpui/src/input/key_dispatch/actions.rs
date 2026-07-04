use super::types::{DispatchActionListener, DispatchNodeId, DispatchTree};
use crate::{Action, KeyBinding, KeyContext, Keymap};

impl DispatchTree {
    pub fn available_actions(&self, target: DispatchNodeId) -> Vec<Box<dyn Action>> {
        let mut actions = Vec::<Box<dyn Action>>::new();
        for node_id in self.dispatch_path(target) {
            let node = &self.nodes[node_id.0];
            for DispatchActionListener { action_type, .. } in &node.action_listeners {
                if let Err(ix) = actions.binary_search_by_key(action_type, |a| a.as_any().type_id())
                {
                    // Intentionally silence these errors without logging.
                    // If an action cannot be built by default, it's not available.
                    let action = self.action_registry.build_action_type(action_type).ok();
                    if let Some(action) = action {
                        actions.insert(ix, action);
                    }
                }
            }
        }
        actions
    }

    pub fn is_action_available(&self, action: &dyn Action, target: DispatchNodeId) -> bool {
        for node_id in self.dispatch_path(target) {
            let node = &self.nodes[node_id.0];
            if node
                .action_listeners
                .iter()
                .any(|listener| listener.action_type == action.as_any().type_id())
            {
                return true;
            }
        }
        false
    }

    /// Returns key bindings that invoke an action on the currently focused element. Bindings are
    /// returned in the order they were added. For display, the last binding should take precedence.
    ///
    /// Bindings are only included if they are the highest precedence match for their keystrokes, so
    /// shadowed bindings are not included.
    pub fn bindings_for_action(
        &self,
        action: &dyn Action,
        context_stack: &[KeyContext],
    ) -> Vec<KeyBinding> {
        // Ideally this would return a `DoubleEndedIterator` to avoid `highest_precedence_*`
        // methods, but this can't be done very cleanly since keymap must be borrowed.
        let keymap = self.keymap.borrow();
        keymap
            .bindings_for_action(action)
            .filter(|binding| {
                Self::binding_matches_predicate_and_not_shadowed(&keymap, binding, context_stack)
            })
            .cloned()
            .collect()
    }

    /// Returns the highest precedence binding for the given action and context stack. This is the
    /// same as the last result of `bindings_for_action`, but more efficient than getting all bindings.
    pub fn highest_precedence_binding_for_action(
        &self,
        action: &dyn Action,
        context_stack: &[KeyContext],
    ) -> Option<KeyBinding> {
        let keymap = self.keymap.borrow();
        keymap
            .bindings_for_action(action)
            .rev()
            .find(|binding| {
                Self::binding_matches_predicate_and_not_shadowed(&keymap, binding, context_stack)
            })
            .cloned()
    }

    fn binding_matches_predicate_and_not_shadowed(
        keymap: &Keymap,
        binding: &KeyBinding,
        context_stack: &[KeyContext],
    ) -> bool {
        let (bindings, _) = keymap.bindings_for_input(&binding.keystrokes, context_stack);
        if let Some(found) = bindings.iter().next() {
            found.action.partial_eq(binding.action.as_ref())
        } else {
            false
        }
    }
}
