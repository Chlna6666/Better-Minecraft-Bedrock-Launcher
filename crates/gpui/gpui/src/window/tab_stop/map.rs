use super::node::{TabStopNode, TabStopOperation, TabStopPath};
use collections::FxHashMap;

use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
#[derive(Debug, Default)]
pub(crate) struct TabStopMap {
    current_path: TabStopPath,
    pub(crate) insertion_history: Vec<TabStopOperation>,
    /// Position in `nodes` of each handle's latest insertion.
    by_id: FxHashMap<FocusId, usize>,
    nodes: Vec<TabStopNode>,
    sorted: bool,
}

impl TabStopMap {
    const MIN_RETAINED_CAPACITY: usize = 16;
    const TRIM_WATERMARK_MULTIPLIER: usize = 4;

    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
        let mut path = self.current_path.clone();
        path.0.push(focus_handle.tab_index);
        self.by_id.insert(focus_handle.id, self.nodes.len());
        self.sorted = false;
        self.nodes.push(TabStopNode {
            node_insertion_index: self.insertion_history.len() - 1,
            tab_stop: focus_handle.tab_stop,
            path,
        });
    }

    pub fn begin_group(&mut self, tab_index: isize) {
        self.insertion_history
            .push(TabStopOperation::Group(tab_index));
        self.current_path.0.push(tab_index);
    }

    pub fn end_group(&mut self) {
        self.insertion_history.push(TabStopOperation::GroupEnd);
        self.current_path.0.pop();
    }

    pub fn clear(&mut self) {
        // This map is rebuilt every frame. Preserve the Vec/HashMap allocations so focus-heavy
        // pages do not pay an allocator round-trip on every draw; pressure/working-set trimming
        // below returns excess capacity when the page actually becomes smaller.
        self.current_path.0.clear();
        self.insertion_history.clear();
        self.by_id.clear();
        self.nodes.clear();
        self.sorted = false;
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.insertion_history.capacity() + self.by_id.capacity() + self.nodes.capacity()
    }

    pub(crate) fn trim_for_reuse(&mut self, current: &Self) {
        let history_target = Self::MIN_RETAINED_CAPACITY.max(current.insertion_history.len());
        if self.insertion_history.capacity()
            > history_target.saturating_mul(Self::TRIM_WATERMARK_MULTIPLIER)
        {
            self.insertion_history.shrink_to(history_target);
        }

        let by_id_target = Self::MIN_RETAINED_CAPACITY.max(current.by_id.len());
        if self.by_id.capacity() > by_id_target.saturating_mul(Self::TRIM_WATERMARK_MULTIPLIER) {
            self.by_id.shrink_to(by_id_target);
        }
        let nodes_target = Self::MIN_RETAINED_CAPACITY.max(current.nodes.len());
        if self.nodes.capacity() > nodes_target.saturating_mul(Self::TRIM_WATERMARK_MULTIPLIER) {
            self.nodes.shrink_to(nodes_target);
        }
    }

    pub(crate) fn trim_retained_capacity(&mut self, aggressive: bool) {
        let floor = if aggressive {
            0
        } else {
            Self::MIN_RETAINED_CAPACITY
        };
        self.insertion_history
            .shrink_to(floor.max(self.insertion_history.len()));
        self.by_id.shrink_to(floor.max(self.by_id.len()));
        self.nodes.shrink_to(floor.max(self.nodes.len()));
    }

    pub fn next(&mut self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.ensure_sorted();
        let start = focused_id
            .and_then(|id| self.by_id.get(id))
            .map_or(0, |position| position + 1);
        let node = self.nodes[start..]
            .iter()
            .chain(&self.nodes[..start])
            .find(|node| node.tab_stop)?;
        self.focus_handle_for_order(node)
    }

    pub fn prev(&mut self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.ensure_sorted();
        let end = focused_id
            .and_then(|id| self.by_id.get(id).copied())
            .unwrap_or(self.nodes.len());
        let node = self.nodes[..end]
            .iter()
            .rev()
            .chain(self.nodes[end..].iter().rev())
            .find(|node| node.tab_stop)?;
        self.focus_handle_for_order(node)
    }

    fn ensure_sorted(&mut self) {
        if self.sorted {
            return;
        }
        // Insertion indices break ties, preserving paint order without a stable sort buffer.
        self.nodes.sort_unstable();
        self.by_id.clear();
        for (position, node) in self.nodes.iter().enumerate() {
            let handle = self.insertion_history[node.node_insertion_index]
                .focus_handle()
                .expect("tab stop nodes correspond to insertions");
            let latest = self.by_id.entry(handle.id).or_insert(position);
            if self.nodes[*latest].node_insertion_index < node.node_insertion_index {
                *latest = position;
            }
        }
        self.sorted = true;
    }

    pub fn replay(&mut self, nodes: &[TabStopOperation]) {
        for node in nodes {
            match node {
                TabStopOperation::Insert(focus_handle) => self.insert(focus_handle),
                TabStopOperation::Group(tab_index) => self.begin_group(*tab_index),
                TabStopOperation::GroupEnd => self.end_group(),
            }
        }
    }

    pub fn paint_index(&self) -> usize {
        self.insertion_history.len()
    }

    fn focus_handle_for_order(&self, order: &TabStopNode) -> Option<FocusHandle> {
        let handle = self.insertion_history[order.node_insertion_index].focus_handle();
        debug_assert!(
            handle.is_some(),
            "The order node did not correspond to an element, this is a GPUI bug"
        );
        handle.cloned()
    }
}
