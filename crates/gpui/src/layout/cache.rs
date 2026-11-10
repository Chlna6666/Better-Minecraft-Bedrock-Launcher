use smallvec::SmallVec;
use taffy::TraversePartialTree as _;

use super::{
    engine::TaffyLayoutEngine,
    metrics::LayoutId,
};

impl TaffyLayoutEngine {
    pub(super) fn collect_subtree_nodes_into(&self, root_id: LayoutId, nodes: &mut Vec<LayoutId>) {
        let mut stack = SmallVec::<[LayoutId; 64]>::new();
        stack.push(root_id);
        while let Some(id) = stack.pop() {
            nodes.push(id);
            // Preserve pre-order traversal without allocating a Vec of
            // child IDs at every node of a retained layout subtree.
            let count = self.taffy.child_count(id.into());
            for index in (0..count).rev() {
                stack.push(self.taffy.get_child_id(id.into(), index).into());
            }
        }
    }
}
