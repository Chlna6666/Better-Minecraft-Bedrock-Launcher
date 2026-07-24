use super::node::{TabStopNode, TabStopOperation, TabStopPath};
use ::sum_tree::SumTree;
use collections::FxHashMap;
use sum_tree::Bias;

use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
#[derive(Debug)]
pub(crate) struct TabStopMap {
    current_path: TabStopPath,
    pub(crate) insertion_history: Vec<TabStopOperation>,
    by_id: FxHashMap<FocusId, TabStopNode>,
    order: SumTree<TabStopNode>,
}

impl Default for TabStopMap {
    fn default() -> Self {
        Self {
            current_path: TabStopPath::default(),
            insertion_history: Vec::new(),
            by_id: FxHashMap::default(),
            order: SumTree::new(()),
        }
    }
}

impl TabStopMap {
    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
        let mut path = self.current_path.clone();
        path.0.push(focus_handle.tab_index);
        let order = TabStopNode {
            node_insertion_index: self.insertion_history.len() - 1,
            tab_stop: focus_handle.tab_stop,
            path,
        };
        self.by_id.insert(focus_handle.id, order.clone());
        self.order.insert_or_replace(order, ());
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
        *self = Self::default();
        self.current_path.0.clear();
        self.insertion_history.clear();
        self.by_id.clear();
        self.order = SumTree::new(());
    }

    pub fn next(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let first = self.order.first()?;
            if first.tab_stop {
                return self.focus_handle_for_order(first);
            } else {
                return self
                    .next_inner(first)
                    .and_then(|order| self.focus_handle_for_order(order));
            }
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.next(None);
        };
        let item = self.next_inner(node);

        if let Some(item) = item {
            self.focus_handle_for_order(item)
        } else {
            self.next(None)
        }
    }

    fn next_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        let mut cursor = self.order.cursor::<TabStopNode>(());
        cursor.seek(node, Bias::Left);
        cursor.next();
        while let Some(item) = cursor.item()
            && !item.tab_stop
        {
            cursor.next();
        }

        cursor.item()
    }

    pub fn prev(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let last = self.order.last()?;
            if last.tab_stop {
                return self.focus_handle_for_order(last);
            } else {
                return self
                    .prev_inner(last)
                    .and_then(|order| self.focus_handle_for_order(order));
            }
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.prev(None);
        };
        let item = self.prev_inner(node);

        if let Some(item) = item {
            self.focus_handle_for_order(item)
        } else {
            self.prev(None)
        }
    }

    fn prev_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        let mut cursor = self.order.cursor::<TabStopNode>(());
        cursor.seek(node, Bias::Left);
        cursor.prev();
        while let Some(item) = cursor.item()
            && !item.tab_stop
        {
            cursor.prev();
        }

        cursor.item()
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

    fn tab_node_for_focus_id(&self, focused_id: &FocusId) -> Option<&TabStopNode> {
        let Some(order) = self.by_id.get(focused_id) else {
            return None;
        };
        Some(order)
    }
}
