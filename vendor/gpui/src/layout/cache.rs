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
        let Some(cached_nodes) = self.previous_layout_roots.get(root_key).cloned() else {
            return false;
        };
        let current_nodes = self.subtree_nodes(id);
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
        for (current_id, cached_node) in current_nodes.into_iter().zip(cached_nodes) {
            self.retained_layout_bounds
                .insert(current_id, cached_node.bounds);
            if let Some(node_context) = self.taffy.get_node_context_mut(current_id.into()) {
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
        let mut stack = SmallVec::<[LayoutId; 64]>::new();
        stack.push(root_id);
        while let Some(id) = stack.pop() {
            nodes.push(id);
            let children = self.taffy.children(id.into()).expect(EXPECT_MESSAGE);
            stack.extend(children.into_iter().rev().map(Into::into));
        }
        nodes
    }
}
