use crate::{App, Size, Window, size};
use smallvec::SmallVec;

use super::{
    engine::{EXPECT_MESSAGE, TaffyLayoutEngine},
    metrics::{
        AvailableSpace, AvailableSpaceKey, LayoutId, LayoutRootCacheKey, RetainedLayoutNode,
        RetainedViewLayout,
    },
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
                // Keep the most recent exact layout shapes warm across alternating
                // popup/tab states instead of evicting every non-current shape.
                self.retained_root_recency.retain(|key| *key != root_key);
                self.retained_root_recency.push(root_key);
            }
        }

        self.trim_retained_root_history();
        let root_target = 8usize.max(self.previous_layout_roots.len());
        if self.previous_layout_roots.capacity() > root_target.saturating_mul(4) {
            self.previous_layout_roots.shrink_to(root_target);
        }

        computed_root_keys.clear();
        subtree_scratch.clear();
        self.computed_root_keys = computed_root_keys;
        self.subtree_scratch = subtree_scratch;
    }

    fn trim_retained_root_history(&mut self) {
        // The cache key contains the entire recursive layout fingerprint and
        // root constraints. Keeping prior shapes never bypasses either proof.
        const MAX_ROOTS: usize = 16;
        const MAX_NODES: usize = 8192;

        self.retained_root_recency
            .retain(|key| self.previous_layout_roots.contains_key(key));
        let mut total_nodes: usize = self
            .previous_layout_roots
            .values()
            .map(Vec::len)
            .sum();
        while self.retained_root_recency.len() > MAX_ROOTS
            || (total_nodes > MAX_NODES && self.retained_root_recency.len() > 1)
        {
            let evicted = self.retained_root_recency.remove(0);
            if let Some(nodes) = self.previous_layout_roots.remove(&evicted) {
                total_nodes = total_nodes.saturating_sub(nodes.len());
            }
        }
        // An isolated large page can exceed the nominal node budget, but
        // keeping its only recent shape avoids a guaranteed miss every frame.
        if self.retained_root_recency.capacity()
            > self.retained_root_recency.len().max(8).saturating_mul(4)
        {
            self.retained_root_recency
                .shrink_to(self.retained_root_recency.len().max(8));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternating_exact_layout_shapes_remain_warm_with_bounded_history() {
        let mut engine = TaffyLayoutEngine::new();
        let key = |index| LayoutRootCacheKey {
            root_fingerprint: index,
            available_space: size(
                AvailableSpaceKey::Definite(800.0f32.to_bits()),
                AvailableSpaceKey::Definite(600.0f32.to_bits()),
            ),
        };
        let insert = |engine: &mut TaffyLayoutEngine, index, nodes: usize| {
            let key = key(index);
            engine.previous_layout_roots.insert(
                key,
                vec![
                    RetainedLayoutNode {
                        bounds: Default::default(),
                        measure_input: None,
                    };
                    nodes
                ],
            );
            engine.retained_root_recency.retain(|existing| *existing != key);
            engine.retained_root_recency.push(key);
            engine.trim_retained_root_history();
        };

        insert(&mut engine, 0, 2);
        insert(&mut engine, 1, 2);
        insert(&mut engine, 2, 2);
        // Revisit the first state: its exact fingerprint stays resident.
        insert(&mut engine, 0, 2);
        for index in 3..18 {
            insert(&mut engine, index, 2);
        }
        assert_eq!(engine.previous_layout_roots.len(), 16);
        assert!(engine.previous_layout_roots.contains_key(&key(0)));
        assert!(!engine.previous_layout_roots.contains_key(&key(1)));

        engine.trim_retained_capacity(crate::GpuiMemoryTrimLevel::Aggressive);
        assert!(engine.previous_layout_roots.is_empty());
        assert!(engine.retained_root_recency.is_empty());
    }

    #[test]
    fn retained_layout_history_limits_node_memory_without_losing_latest_shape() {
        let mut engine = TaffyLayoutEngine::new();
        for index in 0..3u64 {
            let key = LayoutRootCacheKey {
                root_fingerprint: index,
                available_space: size(
                    AvailableSpaceKey::MinContent,
                    AvailableSpaceKey::MaxContent,
                ),
            };
            engine.previous_layout_roots.insert(
                key,
                vec![
                    RetainedLayoutNode {
                        bounds: Default::default(),
                        measure_input: None,
                    };
                    4096
                ],
            );
            engine.retained_root_recency.push(key);
        }
        engine.trim_retained_root_history();
        assert_eq!(engine.previous_layout_roots.len(), 2);
        assert!(engine.previous_layout_roots.contains_key(
            engine.retained_root_recency.last().expect("last active root"),
        ));
    }
}
