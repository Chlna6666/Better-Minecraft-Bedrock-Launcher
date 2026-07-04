use std::fmt::Debug;

use crate::FocusHandle;

pub(crate) type TabIndex = isize;

#[derive(Debug, Clone)]
pub(crate) enum TabStopOperation {
    Insert(FocusHandle),
    Group(TabIndex),
    GroupEnd,
}

impl TabStopOperation {
    pub(super) fn focus_handle(&self) -> Option<&FocusHandle> {
        match self {
            TabStopOperation::Insert(focus_handle) => Some(focus_handle),
            _ => None,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub(super) struct TabStopPath(pub(super) smallvec::SmallVec<[TabIndex; 6]>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TabStopNode {
    /// Path to access the node in the tree
    /// The final node in the list is a leaf node corresponding to an actual focus handle,
    /// all other nodes are group nodes
    pub(super) path: TabStopPath,
    /// index into the backing array of nodes. Corresponds to insertion order
    pub(super) node_insertion_index: usize,

    /// Whether this node is a tab stop
    pub(super) tab_stop: bool,
}

impl Ord for TabStopNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then(self.node_insertion_index.cmp(&other.node_insertion_index))
    }
}

impl PartialOrd for TabStopNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
