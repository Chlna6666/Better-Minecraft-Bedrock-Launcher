use crate::{App, Window};
use smallvec::SmallVec;

use super::{
    engine::{EXPECT_MESSAGE, TaffyLayoutEngine},
    metrics::{LayoutId, RetainedLayoutNode, RetainedViewLayout},
};

impl TaffyLayoutEngine {
    pub(crate) fn capture_view_layout(&self, id: LayoutId) -> Option<RetainedViewLayout> {
        let fingerprint = self.retained_layout_fingerprint(id)?;
        let mut ids = Vec::new();
        self.collect_subtree_nodes_into(id, &mut ids);
        let nodes = ids
            .into_iter()
            .map(|id| {
                Some(RetainedLayoutNode {
                    bounds: *self.absolute_layout_bounds.get(&id)?,
                    measure_input: self
                        .taffy
                        .get_node_context(id.into())
                        .and_then(|context| context.last_measure_input),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(RetainedViewLayout {
            fingerprint,
            nodes: nodes.into(),
        })
    }

    pub(crate) fn restore_view_layout(
        &mut self,
        id: LayoutId,
        retained: &RetainedViewLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if self.retained_layout_fingerprint(id) != Some(retained.fingerprint) {
            return false;
        }
        let mut ids = std::mem::take(&mut self.subtree_scratch);
        ids.clear();
        self.collect_subtree_nodes_into(id, &mut ids);
        let matches = ids.len() == retained.nodes.len();
        if matches {
            for (id, node) in ids.iter().copied().zip(retained.nodes.iter()) {
                self.absolute_layout_bounds.insert(id, node.bounds);
                if let Some(context) = self.taffy.get_node_context_mut(id.into()) {
                    context.last_measure_input = node.measure_input;
                    if !context.is_pure
                        && let Some((known, available)) = node.measure_input
                    {
                        (context.measure)(known, available, window, cx);
                    }
                }
            }
            self.layout_cache_saved_roots = self
                .layout_cache_saved_roots
                .saturating_add(ids.len().saturating_sub(1));
        }
        ids.clear();
        self.subtree_scratch = ids;
        matches
    }

    pub(super) fn collect_subtree_nodes_into(&self, root_id: LayoutId, nodes: &mut Vec<LayoutId>) {
        let mut stack = SmallVec::<[LayoutId; 64]>::new();
        stack.push(root_id);
        while let Some(id) = stack.pop() {
            nodes.push(id);
            let children = self.taffy.children(id.into()).expect(EXPECT_MESSAGE);
            stack.extend(children.into_iter().rev().map(Into::into));
        }
    }
}
