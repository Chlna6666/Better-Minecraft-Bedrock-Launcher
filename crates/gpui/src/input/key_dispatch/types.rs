use crate::{
    ActionRegistry, App, DispatchPhase, EntityId, FocusId, KeyBinding, KeyContext, Keymap,
    Keystroke, ModifiersChangedEvent, Window,
};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    ops::Range,
    rc::Rc,
};

/// ID of a node within `DispatchTree`. Note that these are **not** stable between frames, and so a
/// `DispatchNodeId` should only be used with the `DispatchTree` that provided it.
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
        debug_assert!(
            self.old_range.contains(&node_id.0),
            "node {} was not part of the reused subtree {:?}",
            node_id.0,
            self.old_range
        );
        DispatchNodeId((node_id.0 - self.old_range.start) + self.new_range.start)
    }

    pub fn contains_focus(&self) -> bool {
        self.contains_focus
    }
}

#[derive(Default, Debug)]
pub(crate) struct Replay {
    pub(crate) keystroke: Keystroke,
    pub(crate) bindings: SmallVec<[KeyBinding; 1]>,
}

#[derive(Default, Debug)]
pub(crate) struct DispatchResult {
    pub(crate) pending: SmallVec<[Keystroke; 1]>,
    pub(crate) bindings: SmallVec<[KeyBinding; 1]>,
    pub(crate) to_replay: SmallVec<[Replay; 1]>,
    pub(crate) context_stack: Vec<KeyContext>,
}

pub(crate) type KeyListener = Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>;
pub(crate) type ModifiersChangedListener =
    Rc<dyn Fn(&ModifiersChangedEvent, &mut Window, &mut App)>;

#[derive(Clone)]
pub(crate) struct DispatchActionListener {
    pub(crate) action_type: TypeId,
    pub(crate) listener: Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>,
}
