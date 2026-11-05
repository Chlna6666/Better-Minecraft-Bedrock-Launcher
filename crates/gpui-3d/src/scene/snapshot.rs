//! Retain a bounded lineage of node-ID deltas so a slow renderer can skip scene
//! publications without losing incremental prepare. Hierarchy/LOD changes still
//! invalidate the chain and fall back to a full scene walk.
use super::*;

// More than the typical scene publication backlog, but bounded even if no
// renderer consumes snapshots for a long period.
const MAX_REVISION_HISTORY: usize = 32;

impl Scene {
    /// Forks an immutable snapshot while preserving node handles and sharing node pages.
    /// Only pages touched by subsequent mutations are copied. The page index costs
    /// O(node_count / 64); root and free-slot lists are shared until modified.
    #[must_use]
    pub fn fork(&self) -> Self {
        let mut history = Vec::new();
        if let (Some(from), Some(changes)) = (self.base, self.changes.as_ref()) {
            history.reserve(self.revision_history.len().min(MAX_REVISION_HISTORY - 1) + 1);
            history.push(SceneRevisionDelta {
                from,
                to: (self.instance_id, self.version),
                nodes: changes.clone(),
            });
            history.extend(
                self.revision_history
                    .iter()
                    .take(MAX_REVISION_HISTORY - 1)
                    .cloned(),
            );
        }
        Self {
            nodes: self.nodes.clone(),
            free_slots: self.free_slots.clone(),
            roots: self.roots.clone(),
            live_nodes: self.live_nodes,
            version: self.version,
            instance_id: NEXT_SCENE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            environment: self.environment.clone(),
            base: Some((self.instance_id, self.version)),
            changes: Some(Vec::new()),
            revision_history: history.into(),
        }
    }

    /// Returns changed root node IDs across one or several consecutive revisions.
    /// A direct fork borrows the delta. Skipped publications collect the bounded
    /// intermediate deltas; a divergent or structurally invalid chain is rejected.
    pub(crate) fn changed_nodes(&self, previous: &Scene) -> Option<Cow<'_, [NodeId]>> {
        let target = (previous.instance_id, previous.version);
        if self.base == Some(target) {
            return self.changes.as_deref().map(Cow::Borrowed);
        }
        let mut cursor = self.base?;
        let mut nodes = self.changes.clone()?;
        for entry in self.revision_history.iter() {
            if entry.to != cursor {
                return None;
            }
            nodes.extend_from_slice(&entry.nodes);
            cursor = entry.from;
            if cursor == target {
                nodes.sort_unstable();
                nodes.dedup();
                return Some(Cow::Owned(nodes));
            }
        }
        None
    }

    pub(super) fn record_node(&mut self, id: NodeId) {
        let local = self.node_slot(id).is_none_or(|slot| {
            slot.parent.is_none()
                && slot.children.is_empty()
                && slot
                    .node
                    .as_ref()
                    .is_none_or(|n| n.light.is_none() && n.lod.is_empty())
        });
        self.version = self.version.wrapping_add(1);
        if !local {
            self.changes = None;
        }
        if let Some(changes) = &mut self.changes {
            changes.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_scene_revisions_keep_root_deltas_without_rewalking_the_scene() {
        let mut original = Scene::new();
        let first = original.insert(None, Node::new()).unwrap();
        let baseline = original.fork();

        let mut second = baseline.fork();
        second.node_mut(first).unwrap().visible = false;
        let mut third = second.fork();
        let added = third.insert(None, Node::new()).unwrap();
        let mut fourth = third.fork();
        fourth.remove(first).unwrap();
        let changes = fourth.changed_nodes(&baseline).unwrap();
        assert_eq!(changes.as_ref(), &[first, added]);
        // Direct descendants must still borrow their small deltas.
        assert_eq!(fourth.changed_nodes(&third).unwrap().as_ref(), &[first]);
        // An independent branch must never accept another branch's revision.
        let alternative = baseline.fork();
        assert!(fourth.changed_nodes(&alternative).is_none());
    }

    #[test]
    fn scene_revision_history_is_bounded_and_rejects_hierarchy_changes() {
        let mut original = Scene::new();
        let node = original.insert(None, Node::new()).unwrap();
        let base = original.fork();
        let mut next = base.fork();
        for _ in 0..(MAX_REVISION_HISTORY + 2) {
            next.node_mut(node).unwrap().visible = true;
            next = next.fork();
        }
        assert!(next.changed_nodes(&base).is_none());
        assert!(next.revision_history.len() <= MAX_REVISION_HISTORY);
        let direct = next.fork();
        assert!(direct.changed_nodes(&next).is_some());

        let mut invalid = next.fork();
        invalid.insert(Some(node), Node::new()).unwrap();
        assert!(invalid.changed_nodes(&next).is_none());
        let continued = invalid.fork();
        assert!(continued.changed_nodes(&base).is_none());
    }

    #[test]
    fn removing_a_light_or_editing_a_parent_requires_full_preparation() {
        let mut scene = Scene::new();
        let light = scene
            .insert(
                None,
                Node::new().with_light(crate::Light::Directional {
                    direction: crate::Vec3::Y,
                    color: [1.; 3],
                    intensity: 1.,
                }),
            )
            .unwrap();
        let parent = scene.insert(None, Node::new()).unwrap();
        scene.insert(Some(parent), Node::new()).unwrap();
        let mut next = scene.fork();
        next.node_mut(light).unwrap().light = None;
        assert!(next.changed_nodes(&scene).is_none());
        assert!(scene.node(light).unwrap().light.is_some());
        let mut next = scene.fork();
        next.node_mut(parent).unwrap().visible = false;
        assert!(next.changed_nodes(&scene).is_none());
    }
}
