use super::DispatchActionListener;
use crate::fast::interactivity::SharedListeners;
use crate::{
    ActionRegistry, App, DispatchPhase, EntityId, FocusId, KeyContext, Keymap,
    ModifiersChangedEvent, Window,
};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    ops::{Index, Range},
    rc::Rc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct DispatchNodeId(pub(crate) usize);

pub(crate) struct DispatchTree {
    pub(super) node_stack: Vec<DispatchNodeId>,
    pub(crate) context_stack: Vec<KeyContext>,
    pub(super) view_stack: Vec<EntityId>,
    nodes: NodeStore,
    // Dense topology is independent of immutable listener/context payload.
    parents: Vec<u32>,
    pub(super) focusable_node_ids: FxHashMap<FocusId, DispatchNodeId>,
    pub(super) view_node_ids: FxHashMap<EntityId, DispatchNodeId>,
    pub(super) keymap: Rc<RefCell<Keymap>>,
    pub(super) action_registry: Rc<ActionRegistry>,
}

#[derive(Clone, Default)]
pub(crate) struct DispatchNode {
    pub(crate) key_listeners: SharedListeners<KeyListener>,
    pub(crate) action_listeners: SharedListeners<DispatchActionListener>,
    pub(crate) modifiers_changed_listeners: SharedListeners<ModifiersChangedListener>,
    pub(crate) context: Option<KeyContext>,
    pub(super) focus_id: Option<FocusId>,
    pub(super) view_id: Option<EntityId>,
}

// One reference per bounded page, never per node. Rc is intentional: event
// callbacks are UI-thread-owned and must not acquire atomic references.
const NODE_PAGE_LEN: usize = 128;
const NO_PARENT: u32 = u32::MAX;

#[derive(Clone)]
struct NodeSpan {
    owner: Rc<Vec<DispatchNode>>,
    start: usize,
    len: usize,
    end: usize,
}

impl NodeSpan {
    fn begin(&self) -> usize { self.end - self.len }
    fn slice(&self, range: Range<usize>) -> &[DispatchNode] {
        let start = self.start + range.start.max(self.begin()) - self.begin();
        let end = self.start + range.end.min(self.end) - self.begin();
        &self.owner[start..end]
    }
}

#[derive(Default)]
struct NodeStore {
    spans: Vec<NodeSpan>,
    pending: Vec<DispatchNode>,
    len: usize,
}

impl NodeStore {
    fn len(&self) -> usize { self.len }
    fn is_empty(&self) -> bool { self.len == 0 }
    fn capacity(&self) -> usize { self.spans.capacity() + self.pending.capacity() }

    fn push(&mut self, node: DispatchNode) {
        self.pending.push(node);
        self.len += 1;
        if self.pending.len() == NODE_PAGE_LEN { self.freeze(); }
    }

    fn freeze(&mut self) {
        if self.pending.is_empty() { return; }
        let nodes = std::mem::take(&mut self.pending);
        let len = nodes.len();
        self.spans.push(NodeSpan { owner: Rc::new(nodes), start: 0, len, end: self.len });
    }

    fn spans_in(&self, range: Range<usize>) -> &[NodeSpan] {
        let start = self.spans.partition_point(|span| span.end <= range.start);
        let end = self.spans.partition_point(|span| span.begin() < range.end);
        &self.spans[start..end.max(start)]
    }

    fn slices(&self, range: Range<usize>) -> impl Iterator<Item = &[DispatchNode]> {
        assert!(range.start <= range.end && range.end <= self.len);
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)];
        self.spans_in(range.clone()).iter().map(move |span| span.slice(range.clone()))
            .chain(std::iter::once(tail))
            .filter(|slice| !slice.is_empty())
    }

    fn iter(&self) -> impl Iterator<Item = &DispatchNode> {
        self.slices(0..self.len).flatten()
    }

    fn share(&mut self, source: &Self, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= source.len);
        if range.is_empty() { return; }
        for span in source.spans_in(range.clone()) {
            let begin = range.start.max(span.begin());
            let end = range.end.min(span.end);
            let start = span.start + begin - span.begin();
            let len = end - begin;
            let adjacent = self.pending.is_empty() && self.spans.last().is_some_and(|last|
                Rc::ptr_eq(&last.owner, &span.owner) && last.start + last.len == start);
            // Avoid pinning a large payload page for isolated replayed nodes.
            if !adjacent && len * 4 < span.owner.len() {
                for node in &span.owner[start..start + len] { self.push(node.clone()); }
                continue;
            }
            self.freeze();
            self.len += len;
            if adjacent {
                let last = self.spans.last_mut().unwrap();
                last.len += len;
                last.end = self.len;
            } else {
                self.spans.push(NodeSpan { owner: span.owner.clone(), start, len, end: self.len });
            }
        }
        let pending_start = source.len - source.pending.len();
        for node in &source.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)] {
            self.push(node.clone());
        }
    }

    fn get(&self, index: usize) -> Option<&DispatchNode> {
        if index >= self.len { return None; }
        let pending_start = self.len - self.pending.len();
        if index >= pending_start { return self.pending.get(index - pending_start); }
        let span = &self.spans[self.spans.partition_point(|span| span.end <= index)];
        span.owner.get(span.start + index - span.begin())
    }

    fn get_mut(&mut self, index: usize) -> &mut DispatchNode {
        assert!(index < self.len);
        let pending_start = self.len - self.pending.len();
        if index >= pending_start { return &mut self.pending[index - pending_start]; }
        let span_index = self.spans.partition_point(|span| span.end <= index);
        let span = &mut self.spans[span_index];
        let local = span.start + index - span.begin();
        // Deferred paint can register listeners on an earlier node. Detach
        // at most one bounded page; committed callbacks remain untouched.
        &mut Rc::make_mut(&mut span.owner)[local]
    }

    fn truncate(&mut self, len: usize) {
        assert!(len <= self.len);
        let pending_start = self.len - self.pending.len();
        if len >= pending_start {
            self.pending.truncate(len - pending_start);
        } else {
            self.pending.clear();
            self.spans.truncate(self.spans.partition_point(|span| span.begin() < len));
            if let Some(last) = self.spans.last_mut() && last.end > len {
                last.len -= last.end - len;
                last.end = len;
            }
        }
        self.len = len;
    }

    fn clear(&mut self) {
        self.pending.clear();
        // Retain one uniquely owned construction page, not a second set of
        // immutable pages. The other frame may still hold shared callbacks.
        for span in self.spans.drain(..) {
            if let Ok(mut nodes) = Rc::try_unwrap(span.owner)
                && nodes.capacity() > self.pending.capacity()
            {
                nodes.clear();
                self.pending = nodes;
            }
        }
        self.len = 0;
    }

    fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.spans.len().max(4);
        if self.spans.capacity() > target.saturating_mul(4) {
            self.spans.shrink_to(target);
        }
    }

    fn trim(&mut self, aggressive: bool) {
        let floor = if aggressive { 0 } else { 4 };
        self.spans.shrink_to(self.spans.len().max(floor));
        if aggressive { self.pending.shrink_to_fit(); }
    }
}

impl Index<usize> for NodeStore {
    type Output = DispatchNode;
    fn index(&self, index: usize) -> &DispatchNode {
        self.get(index).expect("valid dispatch node")
    }
}

fn encode_parent(parent: Option<DispatchNodeId>) -> u32 {
    parent.map_or(NO_PARENT, |id| {
        let value = u32::try_from(id.0).expect("dispatch node index overflow");
        assert!(value != NO_PARENT, "dispatch node index reserved for no parent");
        value
    })
}

fn decode_parent(parent: u32) -> Option<DispatchNodeId> {
    (parent != NO_PARENT).then_some(DispatchNodeId(parent as usize))
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
            nodes: NodeStore::default(),
            parents: Vec::new(),
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
        self.parents.clear();
        self.focusable_node_ids.clear();
        self.view_node_ids.clear();
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.node_stack.capacity()
            + self.context_stack.capacity()
            + self.view_stack.capacity()
            + self.nodes.capacity()
            + self.parents.capacity()
            + self.focusable_node_ids.capacity()
            + self.view_node_ids.capacity()
    }

    pub(crate) fn trim_for_reuse(&mut self, current: &Self) {
        const WATERMARK_MULTIPLIER: usize = 4;
        macro_rules! trim_vec {
            ($field:ident) => {{
                let target = Self::MIN_RETAINED_CAPACITY.max(current.$field.len());
                if self.$field.capacity() > target.saturating_mul(WATERMARK_MULTIPLIER) {
                    self.$field.shrink_to(target);
                }
            }};
        }
        trim_vec!(node_stack);
        trim_vec!(context_stack);
        trim_vec!(view_stack);
        trim_vec!(parents);
        self.nodes.trim_for_reuse(&current.nodes);

        let focusable_target = Self::MIN_RETAINED_CAPACITY.max(current.focusable_node_ids.len());
        if self.focusable_node_ids.capacity()
            > focusable_target.saturating_mul(WATERMARK_MULTIPLIER)
        {
            self.focusable_node_ids.shrink_to(focusable_target);
        }
        let view_target = Self::MIN_RETAINED_CAPACITY.max(current.view_node_ids.len());
        if self.view_node_ids.capacity() > view_target.saturating_mul(WATERMARK_MULTIPLIER) {
            self.view_node_ids.shrink_to(view_target);
        }
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
        self.parents.shrink_to(floor.max(self.parents.len()));
        self.nodes.trim(aggressive);
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

        assert!(node_id.0 < NO_PARENT as usize, "dispatch node index overflow");
        self.parents.push(encode_parent(parent));
        self.nodes.push(DispatchNode::default());
        self.node_stack.push(node_id);
        node_id
    }

    pub fn set_active_node(&mut self, node_id: DispatchNodeId) {
        let next_node_parent = self.parent(node_id);
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
                current_node_id = self.parent(node_id);
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
        self.nodes.get_mut(node_id.0).focus_id = Some(focus_id);
        self.focusable_node_ids.insert(focus_id, node_id);
    }

    pub fn set_view_id(&mut self, view_id: EntityId) {
        if self.view_stack.last().copied() != Some(view_id) {
            let node_id = *self.node_stack.last().unwrap();
            self.nodes.get_mut(node_id.0).view_id = Some(view_id);
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

    /// Validate disjoint removed dispatch intervals and the wrapper of each
    /// gap. Every copied parent must be in the clean partition or the anchor.
    pub(crate) fn can_splice_gaps(&self, outer: Range<usize>, gaps: &[(Range<usize>, EntityId)]) -> bool {
        if outer.start > outer.end || outer.end > self.nodes.len() || gaps.is_empty() {
            return false;
        }
        let anchor = outer.start.checked_sub(1).map(DispatchNodeId);
        let mut cursor = outer.start;
        for (gap, owner) in gaps {
            if cursor >= gap.start || gap.start > gap.end || gap.end > outer.end {
                return false;
            }
            let wrapper = DispatchNodeId(gap.start - 1);
            if self.nodes[wrapper.0].view_id != Some(*owner)
                || self.view_node_ids.get(owner) != Some(&wrapper)
            {
                return false;
            }
            cursor = gap.end;
        }
        let mut excluded = gaps.iter().peekable();
        for index in outer.clone() {
            while excluded.peek().is_some_and(|(gap, _)| gap.end <= index) {
                excluded.next();
            }
            if excluded.peek().is_some_and(|(gap, _)| gap.contains(&index)) {
                continue;
            }
            let parent = self.parent(DispatchNodeId(index));
            if parent != anchor && !parent.is_some_and(|parent| {
                outer.contains(&parent.0) && parent.0 < index
                    && !gaps.get(gaps.partition_point(|(gap, _)| gap.end <= parent.0))
                        .is_some_and(|(gap, _)| gap.contains(&parent.0))
            }) {
                return false;
            }
        }
        true
    }

    /// Append a contiguous dispatch segment, translating in-segment parents
    /// by offset. The caller resolves parents outside the segment (including
    /// across splice gaps). No active key-context or view stack is traversed.
    /// Share immutable listener payloads with the committed source. A failed
    /// candidate cannot consume callbacks from the visible frame.
    pub(crate) fn replay_segment(
        &mut self,
        old_range: Range<usize>,
        source: &Self,
        focus: Option<FocusId>,
        mut external_parent: impl FnMut(Option<DispatchNodeId>) -> Option<DispatchNodeId>,
    ) -> ReusedSubtree {
        let start = self.nodes.len();
        let new_range = start..start + old_range.len();
        let mut contains_focus = false;
        assert!(new_range.end <= NO_PARENT as usize, "dispatch node index overflow");
        self.parents.reserve(old_range.len());
        // Parent indices are the only mandatory per-node replay work.
        // Iterate the compact topology, then inspect route metadata in
        // contiguous slices without cloning listener or context payloads.
        for (offset, &encoded) in source.parents[old_range.clone()].iter().enumerate() {
            let id = DispatchNodeId(start + offset);
            let parent = match decode_parent(encoded) {
                Some(parent) if old_range.contains(&parent.0) => {
                    debug_assert!(parent.0 < old_range.start + offset);
                    Some(DispatchNodeId(start + parent.0 - old_range.start))
                }
                parent => external_parent(parent),
            };
            debug_assert!(parent.is_none_or(|parent| parent.0 < id.0));
            self.parents.push(encode_parent(parent));
        }
        for (offset, node) in source.nodes.slices(old_range.clone()).flatten().enumerate() {
            let id = DispatchNodeId(start + offset);
            if let Some(focus_id) = node.focus_id {
                self.focusable_node_ids.insert(focus_id, id);
                contains_focus |= Some(focus_id) == focus;
            }
            if let Some(view_id) = node.view_id {
                self.view_node_ids.insert(view_id, id);
            }
        }
        self.nodes.share(&source.nodes, old_range.clone());
        ReusedSubtree { old_range, new_range, contains_focus }
    }

    #[cfg(test)]
    pub fn reuse_subtree(
        &mut self,
        old_range: Range<usize>,
        source: &Self,
        focus: Option<FocusId>,
    ) -> ReusedSubtree {
        let parent = self.active_node_id();
        self.replay_segment(old_range, source, focus, |_| parent)
    }

    pub fn truncate(&mut self, index: usize) {
        while self.active_node_id().is_some_and(|node| node.0 >= index) {
            self.pop_node();
        }
        let restore_focus = self.nodes.slices(index..self.nodes.len()).flatten().any(|node| node.focus_id.is_some());
        let restore_views = self.nodes.slices(index..self.nodes.len()).flatten().any(|node| node.view_id.is_some());
        self.nodes.truncate(index);
        self.parents.truncate(index);
        // Removed nodes may shadow earlier occurrences of the same ID.
        if restore_focus {
            self.focusable_node_ids.clear();
            for (index, node) in self.nodes.iter().enumerate() {
                if let Some(id) = node.focus_id {
                    self.focusable_node_ids.insert(id, DispatchNodeId(index));
                }
            }
        }
        if restore_views {
            self.view_node_ids.clear();
            for (index, node) in self.nodes.iter().enumerate() {
                if let Some(id) = node.view_id {
                    self.view_node_ids.insert(id, DispatchNodeId(index));
                }
            }
        }
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
                current_node_id = self.parent(node_id);
            }
        }
        false
    }

    pub fn dispatch_path(&self, target: DispatchNodeId) -> SmallVec<[DispatchNodeId; 32]> {
        let mut dispatch_path: SmallVec<[DispatchNodeId; 32]> = SmallVec::new();
        let mut current_node_id = Some(target);
        while let Some(node_id) = current_node_id {
            dispatch_path.push(node_id);
            current_node_id = self.parents.get(node_id.0).copied().and_then(decode_parent);
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
            current_node_id = self.parent(node_id);
        }
        focus_path.reverse(); // Reverse the path so it goes from the root to the focused node.
        focus_path
    }

    /// A retained owner has a route iff its view node is in this frame.
    pub(crate) fn has_view_route(&self, view_id: EntityId) -> bool {
        self.view_node_ids.contains_key(&view_id)
    }

    /// Iterates the view path from the requested view toward the root without allocating.
    pub fn view_path_reversed(&self, view_id: EntityId) -> impl Iterator<Item = EntityId> + '_ {
        let current_node_id = self.view_node_ids.get(&view_id).copied();
        std::iter::successors(current_node_id, |node_id| self.parent(*node_id))
            .filter_map(|node_id| self.node(node_id).view_id)
    }

    pub fn view_path(&self, view_id: EntityId) -> SmallVec<[EntityId; 8]> {
        let mut view_path: SmallVec<[EntityId; 8]> = self.view_path_reversed(view_id).collect();
        view_path.reverse(); // Reverse the path so it goes from the root to the view node.
        view_path
    }

    fn parent(&self, id: DispatchNodeId) -> Option<DispatchNodeId> {
        decode_parent(self.parents[id.0])
    }

    pub(crate) fn freeze(&mut self) {
        debug_assert_eq!(self.nodes.len(), self.parents.len());
        self.nodes.freeze();
    }

    pub fn node(&self, node_id: DispatchNodeId) -> &DispatchNode {
        &self.nodes[node_id.0]
    }

    fn active_node(&mut self) -> &mut DispatchNode {
        let active_node_id = self.active_node_id().unwrap();
        self.nodes.get_mut(active_node_id.0)
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

#[cfg(test)]
mod retained_dispatch_replay_tests {
    use super::*;

    #[test]
    fn split_dispatch_replay_relocates_suffix_parent() {
        let keymap = Rc::new(RefCell::new(Keymap::default()));
        let actions = Rc::new(ActionRegistry::default());
        let mut source = DispatchTree::new(keymap.clone(), actions.clone());
        let mut target = DispatchTree::new(keymap, actions);
        source.push_node();
        let clean = source.push_node();
        let owner = EntityId::from(7_u64);
        source.push_node();
        source.set_view_id(owner);
        source.push_node();
        source.pop_node();
        source.pop_node();
        source.push_node();
        source.pop_node();
        source.pop_node();
        source.pop_node();
        assert!(source.can_splice_gaps(1..5, &[(3..4, owner)]));
        let root = target.push_node();
        target.replay_segment(1..3, &source, None, |_| Some(root));
        target.set_active_node(DispatchNodeId(2));
        target.push_node();
        target.pop_node();
        target.push_node();
        target.pop_node();
        target.set_active_node(root);
        target.replay_segment(4..5, &source, None, |parent| {
            assert_eq!(parent, Some(clean));
            Some(DispatchNodeId(1))
        });
        assert_eq!(target.parent(DispatchNodeId(5)), Some(DispatchNodeId(1)));
        assert_eq!(target.active_node_id(), Some(root));
    }

    #[test]
    fn retained_dispatch_subtree_replay_keeps_committed_listeners_after_abort() {
        use std::any::TypeId;
        let keymap = Rc::new(RefCell::new(Keymap::default()));
        let actions = Rc::new(ActionRegistry::default());
        let mut committed = DispatchTree::new(keymap.clone(), actions.clone());
        let mut candidate = DispatchTree::new(keymap, actions);
        committed.push_node();
        committed.on_key_event(Rc::new(|_, _, _, _| {}));
        committed.on_modifiers_changed(Rc::new(|_, _, _| {}));
        committed.on_action(TypeId::of::<u8>(), Rc::new(|_, _, _, _| {}));
        committed.pop_node();

        committed.freeze();
        candidate.reuse_subtree(0..1, &committed, None);
        for tree in [&committed, &candidate] {
            assert_eq!(tree.nodes[0].key_listeners.len(), 1);
            assert_eq!(tree.nodes[0].modifiers_changed_listeners.len(), 1);
            assert_eq!(tree.nodes[0].action_listeners.len(), 1);
        }
        assert!(Rc::ptr_eq(
            &committed.nodes[0].key_listeners[0],
            &candidate.nodes[0].key_listeners[0]
        ));
        assert!(std::ptr::eq(candidate.node(DispatchNodeId(0)), committed.node(DispatchNodeId(0))));
        candidate.clear();
        assert_eq!(committed.nodes[0].action_listeners.len(), 1);
        assert_eq!(committed.nodes[0].key_listeners.len(), 1);
    }

}


#[cfg(test)]
mod node_page_tests {
    use super::*;

    #[test]
    fn retained_payload_pages_share_and_detach_without_editing_source() {
        let mut source = NodeStore::default();
        for _ in 0..NODE_PAGE_LEN * 2 + 3 { source.push(DispatchNode::default()); }
        source.freeze();
        let mut target = NodeStore::default();
        target.push(DispatchNode::default());
        target.share(&source, 0..NODE_PAGE_LEN * 2);
        assert!(std::ptr::eq(&source[0], &target[1]));
        assert!(std::ptr::eq(&source[NODE_PAGE_LEN], &target[NODE_PAGE_LEN + 1]));
        target.get_mut(1).view_id = Some(EntityId::from(7_u64));
        assert!(source[0].view_id.is_none());
        assert_eq!(target[1].view_id, Some(EntityId::from(7_u64)));
        assert!(std::ptr::eq(&source[NODE_PAGE_LEN], &target[NODE_PAGE_LEN + 1]));
        target.truncate(NODE_PAGE_LEN + 2);
        assert_eq!(target.iter().count(), NODE_PAGE_LEN + 2);
        source.clear();
        assert_eq!(target[1].view_id, Some(EntityId::from(7_u64)));
    }

    #[test]
    fn sparse_fragments_do_not_pin_large_payload_pages() {
        let mut source = NodeStore::default();
        for _ in 0..NODE_PAGE_LEN * 2 { source.push(DispatchNode::default()); }
        let mut target = NodeStore::default();
        target.share(&source, 0..1);
        target.share(&source, NODE_PAGE_LEN..NODE_PAGE_LEN + 1);
        assert_eq!(target.pending.len(), 2);
        assert!(source.spans.iter().all(|span| Rc::strong_count(&span.owner) == 1));
    }
}
