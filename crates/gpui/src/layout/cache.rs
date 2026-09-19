use crate::{App, Size, Window, size};
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
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let mut current_nodes = std::mem::take(&mut self.subtree_scratch);
        current_nodes.clear();
        self.collect_subtree_nodes_into(id, &mut current_nodes);

        let retained = self.try_retain_layout_from_nodes(&current_nodes, root_key, window, cx);

        current_nodes.clear();
        self.subtree_scratch = current_nodes;
        retained
    }

    fn try_retain_layout_from_nodes(
        &mut self,
        current_nodes: &[LayoutId],
        root_key: &LayoutRootCacheKey,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(cached_nodes) = self.previous_layout_roots.get(root_key) else {
            return false;
        };
        if current_nodes.len() != cached_nodes.len() {
            return false;
        }
        let reused_node_count = current_nodes.len();
        let absolute_layout_bounds = &mut self.absolute_layout_bounds;
        let taffy = &mut self.taffy;
        for (current_id, cached_node) in current_nodes.iter().copied().zip(cached_nodes.iter()) {
            absolute_layout_bounds.insert(current_id, cached_node.bounds);
            if let Some(node_context) = taffy.get_node_context_mut(current_id.into()) {
                node_context.last_measure_input = cached_node.measure_input;
                if !node_context.is_pure
                    && let Some((known_dimensions, available_space)) = cached_node.measure_input
                {
                    (node_context.measure)(known_dimensions, available_space, window, cx);
                }
            }
        }
        self.layout_cache_saved_roots = self
            .layout_cache_saved_roots
            .saturating_add(reused_node_count.saturating_sub(1));
        true
    }

    pub(super) fn save_retained_layout_roots(&mut self) {
        // Keep scratch vectors and per-root retained-node buffers hot across frames. Stable layout
        // roots should not rebuild the same heap allocations on every frame.
        let mut computed_root_keys = std::mem::take(&mut self.computed_root_keys);
        let mut subtree_scratch = std::mem::take(&mut self.subtree_scratch);

        for (root_key, root_id) in computed_root_keys.iter().copied() {
            let mut retained_nodes = self
                .previous_layout_roots
                .remove(&root_key)
                .unwrap_or_default();
            retained_nodes.clear();
            subtree_scratch.clear();
            self.collect_subtree_nodes_into(root_id, &mut subtree_scratch);

            let mut complete = true;
            for id in subtree_scratch.iter().copied() {
                let Some(bounds) = self.absolute_layout_bounds.get(&id).copied() else {
                    complete = false;
                    break;
                };
                retained_nodes.push(RetainedLayoutNode {
                    bounds,
                    measure_input: self
                        .taffy
                        .get_node_context(id.into())
                        .and_then(|node_context| node_context.last_measure_input),
                });
            }

            if complete {
                let target = 32usize.max(retained_nodes.len());
                if retained_nodes.capacity() > target.saturating_mul(4) {
                    retained_nodes.shrink_to(target);
                }
                self.previous_layout_roots.insert(root_key, retained_nodes);
            }
        }

        self.previous_layout_roots.retain(|key, _| {
            computed_root_keys
                .iter()
                .any(|(current_key, _)| current_key == key)
        });
        let root_target = 8usize.max(self.previous_layout_roots.len());
        if self.previous_layout_roots.capacity() > root_target.saturating_mul(4) {
            self.previous_layout_roots.shrink_to(root_target);
        }

        computed_root_keys.clear();
        subtree_scratch.clear();
        self.computed_root_keys = computed_root_keys;
        self.subtree_scratch = subtree_scratch;
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
