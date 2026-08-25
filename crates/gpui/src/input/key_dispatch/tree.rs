use super::DispatchActionListener;
use crate::{
    ActionRegistry, App, DispatchPhase, EntityId, FocusId, KeyContext, Keymap,
    ModifiersChangedEvent, Window,
};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    mem,
    ops::Range,
    rc::Rc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct DispatchNodeId(pub(super) usize);

pub(crate) struct DispatchTree {
    pub(super) node_stack: Vec<DispatchNodeId>,
    pub(crate) context_stack: Vec<KeyContext>,
    pub(super) view_stack: Vec<EntityId>,
    pub(super) nodes: Vec<DispatchNode>,
    pub(super) focusable_node_ids: FxHashMap<FocusId, DispatchNodeId>,
    pub(super) view_node_ids: FxHashMap<EntityId, DispatchNodeId>,
    pub(super) keymap: Rc<RefCell<Keymap>>,
    pub(super) action_registry: Rc<ActionRegistry>,
}

#[derive(Default)]
pub(crate) struct DispatchNode {
    pub(crate) key_listeners: Vec<KeyListener>,
    pub(crate) action_listeners: Vec<DispatchActionListener>,
    pub(crate) modifiers_changed_listeners: Vec<ModifiersChangedListener>,
    pub(crate) context: Option<KeyContext>,
    pub(super) focus_id: Option<FocusId>,
    pub(super) view_id: Option<EntityId>,
    pub(super) parent: Option<DispatchNodeId>,
}

pub(crate) struct ReusedSubtree {
    pub(super) old_range: Range<usize>,
    pub(super) new_range: Range<usize>,
    pub(super) contains_focus: bool,
}

impl ReusedSubtree {
    pub fn refresh_node_id(&self, node_id: DispatchNodeId) -> DispatchNodeId {
        debug_assert!(self.old_range.contains(&node_id.0));
        DispatchNodeId((node_id.0 - self.old_range.start) + self.new_range.start)
    }

    pub fn contains_focus(&self) -> bool {
        self.contains_focus
    }
}

pub(crate) type KeyListener = Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>;
pub(crate) type ModifiersChangedListener =
    Rc<dyn Fn(&ModifiersChangedEvent, &mut Window, &mut App)>;

impl DispatchTree {
    const MIN_RETAINED_CAPACITY: usize = 32;

    pub fn new(keymap: Rc<RefCell<Keymap>>, action_registry: Rc<ActionRegistry>) -> Self {
        Self {
            node_stack: Vec::new(),
            context_stack: Vec::new(),
            view_stack: Vec::new(),
            nodes: Vec::new(),
            focusable_node_ids: FxHashMap::default(),
            view_node_ids: FxHashMap::default(),
            keymap,
            action_registry,
        }
    }

    pub fn clear(&mut self) {
        self.node_stack.clear();
        self.context_stack.clear();
        self.view_stack.clear();
        self.nodes.clear();
        self.focusable_node_ids.clear();
        self.view_node_ids.clear();
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.node_stack.capacity()
            + self.context_stack.capacity()
            + self.view_stack.capacity()
            + self.nodes.capacity()
            + self.focusable_node_ids.capacity()
            + self.view_node_ids.capacity()
    }

    pub(crate) fn trim_retained_capacity(&mut self, aggressive: bool) {
        let floor = if aggressive {
            0
        } else {
            Self::MIN_RETAINED_CAPACITY
        };
        self.node_stack.shrink_to(floor.max(self.node_stack.len()));
        self.context_stack
            .shrink_to(floor.max(self.context_stack.len()));
        self.view_stack.shrink_to(floor.max(self.view_stack.len()));
        self.nodes.shrink_to(floor.max(self.nodes.len()));
        self.focusable_node_ids
            .shrink_to(floor.max(self.focusable_node_ids.len()));
        self.view_node_ids
            .shrink_to(floor.max(self.view_node_ids.len()));
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn push_node(&mut self) -> DispatchNodeId {
        let parent = self.node_stack.last().copied();
        let node_id = DispatchNodeId(self.nodes.len());

        self.nodes.push(DispatchNode {
            parent,
            ..Default::default()
        });
        self.node_stack.push(node_id);
        node_id
    }

    pub fn set_active_node(&mut self, node_id: DispatchNodeId) {
        let next_node_parent = self.nodes[node_id.0].parent;
        while self.node_stack.last().copied() != next_node_parent && !self.node_stack.is_empty() {
            self.pop_node();
        }

        if self.node_stack.last().copied() == next_node_parent {
            self.node_stack.push(node_id);
            let active_node = &self.nodes[node_id.0];
            if let Some(view_id) = active_node.view_id {
                self.view_stack.push(view_id)
            }
            if let Some(context) = active_node.context.clone() {
                self.context_stack.push(context);
            }
        } else {
            debug_assert_eq!(self.node_stack.len(), 0);

            let mut current_node_id = Some(node_id);
            while let Some(node_id) = current_node_id {
                let node = &self.nodes[node_id.0];
                if let Some(context) = node.context.clone() {
                    self.context_stack.push(context);
                }
                if let Some(view_id) = node.view_id {
                    self.view_stack.push(view_id);
                }
                self.node_stack.push(node_id);
                current_node_id = node.parent;
            }

            self.context_stack.reverse();
            self.view_stack.reverse();
            self.node_stack.reverse();
        }
    }

    pub fn set_key_context(&mut self, context: KeyContext) {
        self.active_node().context = Some(context.clone());
        self.context_stack.push(context);
    }

    pub fn set_focus_id(&mut self, focus_id: FocusId) {
        let node_id = *self.node_stack.last().unwrap();
        self.nodes[node_id.0].focus_id = Some(focus_id);
        self.focusable_node_ids.insert(focus_id, node_id);
    }

    pub fn set_view_id(&mut self, view_id: EntityId) {
        if self.view_stack.last().copied() != Some(view_id) {
            let node_id = *self.node_stack.last().unwrap();
            self.nodes[node_id.0].view_id = Some(view_id);
            self.view_node_ids.insert(view_id, node_id);
            self.view_stack.push(view_id);
        }
    }

    pub fn pop_node(&mut self) {
        let node = &self.nodes[self.active_node_id().unwrap().0];
        if node.context.is_some() {
            self.context_stack.pop();
        }
        if node.view_id.is_some() {
            self.view_stack.pop();
        }
        self.node_stack.pop();
    }

    fn move_node(&mut self, source: &mut DispatchNode) {
        self.push_node();
        if let Some(context) = source.context.clone() {
            self.set_key_context(context);
        }
        if let Some(focus_id) = source.focus_id {
            self.set_focus_id(focus_id);
        }
        if let Some(view_id) = source.view_id {
            self.set_view_id(view_id);
        }

        let target = self.active_node();
        target.key_listeners = mem::take(&mut source.key_listeners);
        target.action_listeners = mem::take(&mut source.action_listeners);
        target.modifiers_changed_listeners = mem::take(&mut source.modifiers_changed_listeners);
    }

    pub fn reuse_subtree(
        &mut self,
        old_range: Range<usize>,
        source: &mut Self,
        focus: Option<FocusId>,
    ) -> ReusedSubtree {
        let new_range = self.nodes.len()..self.nodes.len() + old_range.len();

        let mut contains_focus = false;
        let mut source_stack = vec![];
        for (source_node_id, source_node) in source
            .nodes
            .iter_mut()
            .enumerate()
            .skip(old_range.start)
            .take(old_range.len())
        {
            let source_node_id = DispatchNodeId(source_node_id);
            while let Some(source_ancestor) = source_stack.last() {
                if source_node.parent == Some(*source_ancestor) {
                    break;
                } else {
                    source_stack.pop();
                    self.pop_node();
                }
            }

            source_stack.push(source_node_id);
            if source_node.focus_id.is_some() && source_node.focus_id == focus {
                contains_focus = true;
            }
            self.move_node(source_node);
        }

        while !source_stack.is_empty() {
            source_stack.pop();
            self.pop_node();
        }

        ReusedSubtree {
            old_range,
            new_range,
            contains_focus,
        }
    }

    pub fn truncate(&mut self, index: usize) {
        for node in &self.nodes[index..] {
            if let Some(focus_id) = node.focus_id {
                self.focusable_node_ids.remove(&focus_id);
            }

            if let Some(view_id) = node.view_id {
                self.view_node_ids.remove(&view_id);
            }
        }
        self.nodes.truncate(index);
    }

    pub fn on_key_event(&mut self, listener: KeyListener) {
        self.active_node().key_listeners.push(listener);
    }

    pub fn on_modifiers_changed(&mut self, listener: ModifiersChangedListener) {
        self.active_node()
            .modifiers_changed_listeners
            .push(listener);
    }

    pub fn on_action(
        &mut self,
        action_type: TypeId,
        listener: Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>,
    ) {
        self.active_node()
            .action_listeners
            .push(DispatchActionListener {
                action_type,
                listener,
            });
    }

    pub fn focus_contains(&self, parent: FocusId, child: FocusId) -> bool {
        if parent == child {
            return true;
        }

        if let Some(parent_node_id) = self.focusable_node_ids.get(&parent) {
            let mut current_node_id = self.focusable_node_ids.get(&child).copied();
            while let Some(node_id) = current_node_id {
                if node_id == *parent_node_id {
                    return true;
                }
                current_node_id = self.nodes[node_id.0].parent;
            }
        }
        false
    }

    pub fn dispatch_path(&self, target: DispatchNodeId) -> SmallVec<[DispatchNodeId; 32]> {
        let mut dispatch_path: SmallVec<[DispatchNodeId; 32]> = SmallVec::new();
        let mut current_node_id = Some(target);
        while let Some(node_id) = current_node_id {
            dispatch_path.push(node_id);
            current_node_id = self.nodes.get(node_id.0).and_then(|node| node.parent);
        }
        dispatch_path.reverse(); // Reverse the path so it goes from the root to the focused node.
        dispatch_path
    }

    pub fn focus_path(&self, focus_id: FocusId) -> SmallVec<[FocusId; 8]> {
        let mut focus_path: SmallVec<[FocusId; 8]> = SmallVec::new();
        let mut current_node_id = self.focusable_node_ids.get(&focus_id).copied();
        while let Some(node_id) = current_node_id {
            let node = self.node(node_id);
            if let Some(focus_id) = node.focus_id {
                focus_path.push(focus_id);
            }
            current_node_id = node.parent;
        }
        focus_path.reverse(); // Reverse the path so it goes from the root to the focused node.
        focus_path
    }

    pub fn view_path(&self, view_id: EntityId) -> SmallVec<[EntityId; 8]> {
        let mut view_path: SmallVec<[EntityId; 8]> = SmallVec::new();
        let mut current_node_id = self.view_node_ids.get(&view_id).copied();
        while let Some(node_id) = current_node_id {
            let node = self.node(node_id);
            if let Some(view_id) = node.view_id {
                view_path.push(view_id);
            }
            current_node_id = node.parent;
        }
        view_path.reverse(); // Reverse the path so it goes from the root to the view node.
        view_path
    }

    pub fn node(&self, node_id: DispatchNodeId) -> &DispatchNode {
        &self.nodes[node_id.0]
    }

    fn active_node(&mut self) -> &mut DispatchNode {
        let active_node_id = self.active_node_id().unwrap();
        &mut self.nodes[active_node_id.0]
    }

    pub fn focusable_node_id(&self, target: FocusId) -> Option<DispatchNodeId> {
        self.focusable_node_ids.get(&target).copied()
    }

    pub fn root_node_id(&self) -> DispatchNodeId {
        debug_assert!(!self.nodes.is_empty());
        DispatchNodeId(0)
    }

    pub fn active_node_id(&self) -> Option<DispatchNodeId> {
        self.node_stack.last().copied()
    }
}
