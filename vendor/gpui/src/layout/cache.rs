use crate::{Size, size};
use smallvec::SmallVec;

use super::{
    engine::{EXPECT_MESSAGE, TaffyLayoutEngine},
    metrics::{
        AvailableSpace, AvailableSpaceKey, LayoutId, LayoutRootCacheKey, RetainedLayoutNode,
    },
};

impl TaffyLayoutEngine {
    pub(super) fn root_cache_key(
        &self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
    ) -> Option<LayoutRootCacheKey> {
        Some(LayoutRootCacheKey {
            root_fingerprint: self.node_fingerprints.get(&id).copied().flatten()?,
            available_space: size(
                AvailableSpaceKey::from(available_space.width),
                AvailableSpaceKey::from(available_space.height),
            ),
        })
    }

    pub(super) fn try_retain_layout(
        &mut self,
        id: LayoutId,
        root_key: &LayoutRootCacheKey,
    ) -> bool {
        let mut current_nodes = std::mem::take(&mut self.subtree_scratch);
        current_nodes.clear();
        self.collect_subtree_nodes_into(id, &mut current_nodes);

        let retained = self.try_retain_layout_from_nodes(&current_nodes, root_key);

        current_nodes.clear();
        self.subtree_scratch = current_nodes;
        retained
    }

    fn try_retain_layout_from_nodes(
        &mut self,
        current_nodes: &[LayoutId],
        root_key: &LayoutRootCacheKey,
    ) -> bool {
        let Some(cached_nodes) = self.previous_layout_roots.get(root_key) else {
            return false;
        };
        if current_nodes.len() != cached_nodes.len() {
            return false;
        }
        for (current_id, cached_node) in current_nodes.iter().zip(cached_nodes.iter()) {
            let current_fingerprint = self.node_fingerprints.get(current_id).copied().flatten();
            if current_fingerprint != Some(cached_node.fingerprint) {
                return false;
            }
        }

        let reused_node_count = current_nodes.len();
        let retained_layout_bounds = &mut self.retained_layout_bounds;
        let taffy = &mut self.taffy;
        for (current_id, cached_node) in current_nodes.iter().copied().zip(cached_nodes.iter()) {
            retained_layout_bounds.insert(current_id, cached_node.bounds);
            if let Some(node_context) = taffy.get_node_context_mut(current_id.into()) {
                node_context.last_measure_input = cached_node.measure_input;
            }
        }
        self.layout_cache_saved_roots = self
            .layout_cache_saved_roots
            .saturating_add(reused_node_count.saturating_sub(1));
        true
    }

    pub(super) fn save_retained_layout_roots(&mut self) {
        self.previous_layout_roots.clear();
        let computed_root_keys = std::mem::take(&mut self.computed_root_keys);
        for (root_key, root_id) in computed_root_keys {
            if let Some(nodes) = self.retained_layout_nodes(root_id) {
                if let Some(previous_nodes) = self.previous_layout_roots.get_mut(&root_key) {
                    *previous_nodes = nodes;
                } else {
                    self.previous_layout_roots.insert(root_key, nodes);
                }
            }
        }
    }

    pub(super) fn retained_layout_nodes(
        &self,
        root_id: LayoutId,
    ) -> Option<Vec<RetainedLayoutNode>> {
        self.subtree_nodes(root_id)
            .into_iter()
            .map(|id| {
                Some(RetainedLayoutNode {
                    fingerprint: self.node_fingerprints.get(&id).copied().flatten()?,
                    bounds: self.absolute_layout_bounds.get(&id).copied()?,
                    measure_input: self
                        .taffy
                        .get_node_context(id.into())
                        .and_then(|node_context| node_context.last_measure_input),
                })
            })
            .collect()
    }

    pub(super) fn subtree_nodes(&self, root_id: LayoutId) -> Vec<LayoutId> {
        let mut nodes = Vec::new();
        self.collect_subtree_nodes_into(root_id, &mut nodes);
        nodes
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
