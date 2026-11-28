mod parents;

use parents::ParentStore;
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
    parents: ParentStore,
    // Only nodes that actually own a focus or view route. Ordered by
    // logical node index, independent of immutable payload page placement.
    route_nodes: Vec<u32>,
    pub(super) focusable_node_ids: FxHashMap<FocusId, DispatchNodeId>,
    pub(super) view_node_ids: FxHashMap<EntityId, DispatchNodeId>,
    // The normal unique-route case can undo a suffix without rebuilding
    // either map. Collisions (including deferred registrations) retain the
    // original logical-order restoration semantics.
    focus_routes_may_shadow: bool,
    view_routes_may_shadow: bool,
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
    #[cfg(test)]
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
        self.pack_sparse_tail();
        self.push_pending(node);
    }

    #[inline]
    fn push_pending(&mut self, node: DispatchNode) {
        self.pending.push(node);
        self.len += 1;
        if self.pending.len() == NODE_PAGE_LEN { self.seal_pending(); }
    }

    fn pack_sparse_tail(&mut self) {
        if !self.pending.is_empty() { return; }
        let Some(last) = self.spans.last() else { return; };
        if last.len * 4 >= last.owner.len() { return; }
        let last = self.spans.pop().unwrap();
        self.pending.extend_from_slice(&last.owner[last.start..last.start + last.len]);
    }

    fn freeze(&mut self) {
        self.pack_sparse_tail();
        self.seal_pending();
    }

    fn seal_pending(&mut self) {
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

    #[cfg(test)]
    fn slices(&self, range: Range<usize>) -> impl Iterator<Item = &[DispatchNode]> {
        assert!(range.start <= range.end && range.end <= self.len);
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)];
        self.spans_in(range.clone()).iter().map(move |span| span.slice(range.clone()))
            .chain(std::iter::once(tail))
            .filter(|slice| !slice.is_empty())
    }

    #[cfg(test)]
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
            if adjacent {
                self.len += len;
                let last = self.spans.last_mut().unwrap();
                last.len += len;
                last.end = self.len;
                continue;
            }
            self.pack_sparse_tail();
            if !self.pending.is_empty() && len * 4 < span.owner.len() {
                for node in &span.owner[start..start + len] { self.push_pending(node.clone()); }
                continue;
            }
            self.seal_pending();
            self.len += len;
            self.spans.push(NodeSpan { owner: span.owner.clone(), start, len, end: self.len });
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
            parents: ParentStore::default(),
            route_nodes: Vec::new(),
            focusable_node_ids: FxHashMap::default(),
            view_node_ids: FxHashMap::default(),
            focus_routes_may_shadow: false,
            view_routes_may_shadow: false,
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
        self.route_nodes.clear();
        self.focusable_node_ids.clear();
        self.view_node_ids.clear();
        self.focus_routes_may_shadow = false;
        self.view_routes_may_shadow = false;
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.node_stack.capacity()
            + self.context_stack.capacity()
            + self.view_stack.capacity()
            + self.nodes.capacity()
            + self.parents.capacity()
            + self.route_nodes.capacity()
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
        self.parents.trim_for_reuse(&current.parents);
        trim_vec!(route_nodes);
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
        self.parents.trim(aggressive);
        self.route_nodes.shrink_to(floor.max(self.route_nodes.len()));
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
        self.parents.push(parent);
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

    fn record_route_node(&mut self, node_id: DispatchNodeId) {
        let slot = node_id.0 as u32;
        if self.route_nodes.last().is_none_or(|&last| last < slot) {
            self.route_nodes.push(slot);
        } else if let Err(index) = self.route_nodes.binary_search(&slot) {
            // Deferred prepaint may activate an earlier node.
            self.route_nodes.insert(index, slot);
        }
    }

    pub fn set_focus_id(&mut self, focus_id: FocusId) {
        let node_id = *self.node_stack.last().unwrap();
        let node = self.nodes.get_mut(node_id.0);
        let new_route = node.focus_id.is_none() && node.view_id.is_none();
        self.focus_routes_may_shadow |= node.focus_id.is_some_and(|old| old != focus_id);
        node.focus_id = Some(focus_id);
        if new_route { self.record_route_node(node_id); }
        self.focus_routes_may_shadow |= self.focusable_node_ids
            .insert(focus_id, node_id).is_some_and(|old| old != node_id);
    }

    pub fn set_view_id(&mut self, view_id: EntityId) {
        if self.view_stack.last().copied() != Some(view_id) {
            let node_id = *self.node_stack.last().unwrap();
            let node = self.nodes.get_mut(node_id.0);
            let new_route = node.focus_id.is_none() && node.view_id.is_none();
            self.view_routes_may_shadow |= node.view_id.is_some_and(|old| old != view_id);
            node.view_id = Some(view_id);
            if new_route { self.record_route_node(node_id); }
            self.view_routes_may_shadow |= self.view_node_ids
                .insert(view_id, node_id).is_some_and(|old| old != node_id);
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
        // Validate only retained partitions. Most parent edges stay within
        // the same partition and need neither a gap search nor a node lookup.
        let mut start = outer.start;
        for end_and_gap in gaps.iter().map(|(gap, _)| (gap.start, gap.end))
            .chain(std::iter::once((outer.end, outer.end)))
        {
            let (end, next) = end_and_gap;
            if !self.parents.all_external_parents(start..end, |parent| {
                if parent == anchor {
                    return true;
                }
                let Some(parent) = parent else { return false; };
                parent.0 >= outer.start
                    && !gaps.get(gaps.partition_point(|(gap, _)| gap.end <= parent.0))
                        .is_some_and(|(gap, _)| gap.contains(&parent.0))
            }) {
                return false;
            }
            start = next;
        }
        true
    }

    /// Prepare an independent subtree replacement without consuming committed
    /// listener ownership. The new forest must have no parents outside each
    /// replacement range; its roots are attached to an explicit retained node.
    /// Retained external edges must still resolve after all edits.
    #[allow(dead_code)] // Boundary Frame transaction supplies the materialized forest.
    pub(crate) fn with_spliced_records(
        &self,
        replacement: &Self,
        plan: &crate::window::RecordSplicePlan,
        replacement_parent: DispatchNodeId,
    ) -> Option<Self> {
        self.with_spliced_records_under(replacement, plan, replacement_parent, None)
    }

    /// Independent materialization can use a temporary dispatch anchor
    /// rather than reproducing the committed ancestor path. That anchor
    /// is not itself adopted; only its explicitly selected children are.
    pub(crate) fn with_spliced_records_under(
        &self,
        replacement: &Self,
        plan: &crate::window::RecordSplicePlan,
        replacement_parent: DispatchNodeId,
        replacement_anchor: Option<DispatchNodeId>,
    ) -> Option<Self> {
        use crate::window::SpliceSource;
        if !self.node_stack.is_empty() || !replacement.node_stack.is_empty()
            || !self.nodes.pending.is_empty() || !replacement.nodes.pending.is_empty()
            || !Rc::ptr_eq(&self.keymap, &replacement.keymap)
            || !Rc::ptr_eq(&self.action_registry, &replacement.action_registry)
            || self.focus_routes_may_shadow || self.view_routes_may_shadow
            || replacement.focus_routes_may_shadow || replacement.view_routes_may_shadow
            || !plan.matches_sources(self.len(), replacement.len())
            || plan.len() > NO_PARENT as usize
            || plan.retained_range(0..1) != Some(0..1)
        {
            return None;
        }
        let remap = |node: DispatchNodeId| {
            let end = node.0.checked_add(1)?;
            plan.retained_range(node.0..end).map(|range| DispatchNodeId(range.start))
        };
        let parent = remap(replacement_parent)?;
        if replacement_anchor.is_some_and(|anchor| anchor.0 >= replacement.len()
            || replacement.parent(anchor).is_some()) { return None; }
        let mut offset = 0;
        let mut valid = true;
        plan.visit_ranges(|source, range| {
            if !valid { return; }
            valid = match source {
                SpliceSource::Retained => self.parents.all_external_parents(range.clone(), |old| {
                    old.is_none_or(|old| remap(old).is_some_and(|new| new.0 < offset))
                }),
                SpliceSource::Replacement => parent.0 < offset
                    && replacement_anchor.is_none_or(|anchor| !range.contains(&anchor.0))
                    && replacement.parents.all_external_parents(range.clone(), |old| old == replacement_anchor),
            };
            offset += range.len();
        });
        if !valid { return None; }

        let mut result = Self::new(Rc::clone(&self.keymap), Rc::clone(&self.action_registry));
        plan.visit_ranges(|source, range| {
            match source {
                SpliceSource::Retained => {
                    result.replay_segment(range, self, None, |old| old.map(|old| {
                        remap(old).expect("validated retained parent")
                    }));
                }
                SpliceSource::Replacement => {
                    result.replay_segment(range, replacement, None, |_| Some(parent));
                }
            }
        });
        // Do not silently change focus or view route precedence when an
        // independently materialized subtree conflicts with a retained one.
        if result.focus_routes_may_shadow || result.view_routes_may_shadow {
            return None;
        }
        result.freeze();
        debug_assert_eq!(result.len(), plan.len());
        Some(result)
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
        self.parents.replay(&source.parents, old_range.clone(), &mut external_parent);
        let first_route = source.route_nodes.partition_point(|&id| (id as usize) < old_range.start);
        let routes = source.route_nodes[first_route..].iter()
            .copied().take_while(|&id| (id as usize) < old_range.end);
        for old_id in routes {
            let id = DispatchNodeId(start + old_id as usize - old_range.start);
            let node = source.node(DispatchNodeId(old_id as usize));
            self.route_nodes.push(id.0 as u32);
            if let Some(focus_id) = node.focus_id {
                self.focus_routes_may_shadow |= self.focusable_node_ids
                    .insert(focus_id, id).is_some_and(|old| old != id);
                contains_focus |= Some(focus_id) == focus;
            }
            if let Some(view_id) = node.view_id {
                self.view_routes_may_shadow |= self.view_node_ids
                    .insert(view_id, id).is_some_and(|old| old != id);
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
        let first_removed = self.route_nodes.partition_point(|&id| (id as usize) < index);
        let mut restore_focus = false;
        let mut restore_views = false;
        for &id in &self.route_nodes[first_removed..] {
            let node = &self.nodes[id as usize];
            if let Some(focus) = node.focus_id {
                if self.focus_routes_may_shadow {
                    restore_focus = true;
                } else {
                    self.focusable_node_ids.remove(&focus);
                }
            }
            if let Some(view) = node.view_id {
                if self.view_routes_may_shadow {
                    restore_views = true;
                } else {
                    self.view_node_ids.remove(&view);
                }
            }
        }
        self.route_nodes.truncate(first_removed);
        self.nodes.truncate(index);
        self.parents.truncate(index);

        // Only a route kind with possible shadowed IDs needs the original
        // logical-order rebuild. Unique routes never touch the clean prefix.
        if restore_focus {
            self.focusable_node_ids.clear();
            self.focus_routes_may_shadow = false;
        }
        if restore_views {
            self.view_node_ids.clear();
            self.view_routes_may_shadow = false;
        }
        if restore_focus || restore_views {
            for &index in &self.route_nodes {
                let node = &self.nodes[index as usize];
                let node_id = DispatchNodeId(index as usize);
                if restore_focus && let Some(id) = node.focus_id {
                    self.focus_routes_may_shadow |= self.focusable_node_ids.insert(id, node_id).is_some();
                }
                if restore_views && let Some(id) = node.view_id {
                    self.view_routes_may_shadow |= self.view_node_ids.insert(id, node_id).is_some();
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
            current_node_id = self.parents.parent(node_id.0);
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
        self.parents.parent(id.0)
    }

    pub(crate) fn freeze(&mut self) {
        debug_assert_eq!(self.nodes.len(), self.parents.len());
        self.nodes.freeze();
        self.parents.freeze();
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
    fn single_node_replays_share_contiguous_page() {
        let mut source = NodeStore::default();
        for _ in 0..NODE_PAGE_LEN { source.push(DispatchNode::default()); }
        source.freeze();
        let mut target = NodeStore::default();
        for index in 0..NODE_PAGE_LEN { target.share(&source, index..index+1); }
        target.freeze();
        assert_eq!(target.spans.len(), 1);
        assert!(Rc::ptr_eq(&source.spans[0].owner, &target.spans[0].owner));
        assert_eq!(target.len(), NODE_PAGE_LEN);
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


#[cfg(test)]
mod sparse_route_tests {
    use super::*;

    #[test]
    fn replay_relocates_sparse_routes_and_truncate_restores_shadowed_ids() {
        let keymap = Rc::new(RefCell::new(Keymap::default()));
        let actions = Rc::new(ActionRegistry::default());
        let mut source = DispatchTree::new(keymap.clone(), actions.clone());
        let owner = EntityId::from(42_u64);
        source.push_node();
        source.set_view_id(owner);
        for _ in 0..256 {
            source.push_node();
            source.pop_node();
        }
        source.pop_node();
        let last = source.push_node();
        source.set_view_id(owner);
        source.pop_node();
        source.freeze();
        assert_eq!(source.route_nodes, vec![0, last.0 as u32]);
        let mut target = DispatchTree::new(keymap, actions);
        target.push_node();
        target.replay_segment(0..source.len(), &source, None, |_| Some(DispatchNodeId(0)));
        assert_eq!(target.route_nodes, vec![1, last.0 as u32 + 1]);
        assert_eq!(target.view_node_ids.get(&owner), Some(&DispatchNodeId(last.0 + 1)));
        target.truncate(last.0 + 1);
        assert_eq!(target.view_node_ids.get(&owner), Some(&DispatchNodeId(1)));
        assert_eq!(target.route_nodes, vec![1]);
    }

    #[test]
    fn unique_route_suffix_removal_keeps_clean_prefix() {
        let mut tree = DispatchTree::new(
            Rc::new(RefCell::new(Keymap::default())), Rc::new(ActionRegistry::default()));
        for id in 1..=1024_u64 {
            tree.push_node();
            tree.set_view_id(EntityId::from(id));
            tree.pop_node();
        }
        assert!(!tree.view_routes_may_shadow);
        tree.truncate(1022);
        assert!(!tree.view_routes_may_shadow);
        assert_eq!(tree.view_node_ids.len(), 1022);
        assert_eq!(tree.route_nodes.len(), 1022);
        for id in 1..=1022_u64 {
            assert_eq!(tree.view_node_ids.get(&EntityId::from(id)), Some(&DispatchNodeId(id as usize - 1)));
        }
        assert!(!tree.view_node_ids.contains_key(&EntityId::from(1023_u64)));
        tree.clear();
        assert!(!tree.view_routes_may_shadow);
    }

    #[test]
    fn shadowed_route_rebuild_can_return_to_unique_fast_path() {
        let mut tree = DispatchTree::new(
            Rc::new(RefCell::new(Keymap::default())), Rc::new(ActionRegistry::default()));
        let owner = EntityId::from(1_u64);
        for id in [owner, EntityId::from(2_u64), owner] {
            tree.push_node();
            tree.set_view_id(id);
            tree.pop_node();
        }
        assert!(tree.view_routes_may_shadow);
        tree.truncate(2);
        assert!(!tree.view_routes_may_shadow);
        assert_eq!(tree.view_node_ids.get(&owner), Some(&DispatchNodeId(0)));
        tree.truncate(1);
        assert_eq!(tree.view_node_ids.len(), 1);
        assert_eq!(tree.view_node_ids.get(&owner), Some(&DispatchNodeId(0)));
    }

    #[test]
    fn deferred_registration_inserts_route_in_logical_order() {
        let mut tree = DispatchTree::new(
            Rc::new(RefCell::new(Keymap::default())), Rc::new(ActionRegistry::default()));
        let root = tree.push_node();
        let first = tree.push_node();
        tree.pop_node();
        tree.push_node();
        tree.set_view_id(EntityId::from(8_u64));
        tree.pop_node();
        tree.set_active_node(first);
        tree.set_view_id(EntityId::from(7_u64));
        tree.set_active_node(root);
        assert_eq!(tree.route_nodes, vec![1, 2]);
    }
}
