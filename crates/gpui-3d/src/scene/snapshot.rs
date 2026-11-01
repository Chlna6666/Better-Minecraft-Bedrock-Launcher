//! Snapshot lineage is local to a fork; skipped revisions safely fall back to a full walk.
use super::*;

impl Scene {
    /// Forks an immutable snapshot while preserving node handles and sharing node pages.
    /// Only pages touched by subsequent mutations are copied. The page index costs
    /// O(node_count / 64); root and free-slot lists are shared until modified.
    #[must_use]
    pub fn fork(&self) -> Self {
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
        }
    }

    pub(crate) fn changed_nodes(&self, previous: &Scene) -> Option<&[NodeId]> {
        (self.base == Some((previous.instance_id, previous.version)))
            .then_some(self.changes.as_deref())
            .flatten()
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
