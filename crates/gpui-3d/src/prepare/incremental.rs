//! Rebuild changed root leaves; hierarchical, animated and LOD edits use the full walker.
use super::*;
use std::collections::HashSet;

impl PreparedScene {
    pub(crate) fn update_nodes(
        &mut self,
        scene: &Scene,
        nodes: &[NodeId],
        transform: Mat4,
    ) -> Option<usize> {
        let camera = self.camera?;
        if nodes.is_empty() {
            return Some(0);
        }
        if nodes.iter().any(|&id| {
            scene.node(id).is_some_and(|node| {
                scene.parent(id).is_some()
                    || scene.children(id).is_some_and(|c| !c.is_empty())
                    || node.light.is_some()
                    || !node.lod.is_empty()
            })
        }) {
            return None;
        }
        let changed = nodes.iter().copied().collect::<HashSet<_>>();
        let mut partial = Self::default();
        partial
            .walk_stack
            .extend(changed.iter().copied().map(|id| (id, transform, true)));
        partial
            .walk_pending(scene, |_, node| node.transform, camera)
            .ok()?;
        let mut replacements = std::collections::HashMap::<NodeId, Vec<PreparedDraw>>::new();
        for draw in partial.candidates {
            replacements.entry(draw.node).or_default().push(draw);
        }
        let groups = self
            .candidate_groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| {
                let id = self.candidates[group.start].node;
                changed
                    .contains(&id)
                    .then_some((index, group.start..group.end, id))
            })
            .collect::<Vec<_>>();
        let mut inserted = replacements.keys().copied().collect::<HashSet<_>>();
        for (_, _, id) in &groups {
            inserted.remove(id);
        }
        // New roots are visited first by the full walker. Existing roots without
        // candidates (e.g. a hidden node becoming visible) can require a full walk.
        let mut prefix = Vec::new();
        let mut next_groups = Vec::with_capacity(self.candidate_groups.len() + inserted.len());
        for &id in scene.roots().iter().rev() {
            if !inserted.remove(&id) {
                break;
            }
            if let Some(draws) = replacements.remove(&id) {
                if let Some(group) = candidate_group(&draws, prefix.len()) {
                    next_groups.push(group);
                }
                prefix.extend(draws);
            }
        }
        if !inserted.is_empty() {
            return None;
        }
        let mut next_start = prefix.len();
        let mut changed_groups = groups.iter().peekable();
        for (index, group) in self.candidate_groups.iter().enumerate() {
            if changed_groups
                .peek()
                .is_some_and(|(changed_index, _, _)| *changed_index == index)
            {
                let (_, _, id) = changed_groups.next()?;
                if let Some(draws) = replacements.get(id)
                    && let Some(next) = candidate_group(draws, next_start)
                {
                    next_start = next.end;
                    next_groups.push(next);
                }
            } else {
                let mut next = group.clone();
                next.end = next_start + group.end - group.start;
                next.start = next_start;
                next_start = next.end;
                next_groups.push(next);
            }
        }
        for (_, range, id) in groups.into_iter().rev() {
            self.candidates
                .splice(range, replacements.remove(&id).unwrap_or_default());
        }
        if !prefix.is_empty() {
            self.candidates.splice(0..0, prefix);
        }
        self.occluders.retain(|id, _| !changed.contains(id));
        self.occluders.extend(partial.occluders);
        self.candidate_groups = next_groups;
        self.visible_candidates.clear();
        self.gpu_candidates_dirty = true;
        Some(changed.len())
    }
}

fn candidate_group(draws: &[PreparedDraw], start: usize) -> Option<CandidateGroup> {
    let first = draws.first()?;
    let mut bounds = first.bounds;
    for draw in &draws[1..] {
        bounds.min = bounds.min.min(draw.bounds.min);
        bounds.max = bounds.max.max(draw.bounds.max);
    }
    Some(CandidateGroup {
        start,
        end: start + draws.len(),
        bounds,
        culling_origin: first.culling_origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_replacement_prepares_only_changed_nodes_and_preserves_snapshots() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 10.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let ids = (0..128)
            .map(|_| {
                scene
                    .insert(None, Node::new().with_mesh(mesh.clone()))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        let mut next = scene.fork();
        next.node_mut(ids[65]).unwrap().transform.translation.x = 1.;
        next.remove(ids[3]).unwrap();
        next.insert(None, Node::new().with_mesh(mesh)).unwrap();
        let changed = next.changed_nodes(&scene).unwrap();
        assert_eq!(
            prepared.update_nodes(&next, changed, Mat4::IDENTITY),
            Some(3)
        );
        prepared.update_camera(camera, 1.).unwrap();
        let complete = PreparedScene::new(&next, camera, 1.).unwrap();
        assert_eq!(prepared.draws.len(), complete.draws.len());
        assert_eq!(
            prepared
                .candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>(),
            complete
                .candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| draw.node)
                .collect::<Vec<_>>(),
            complete
                .draws
                .iter()
                .map(|draw| draw.node)
                .collect::<Vec<_>>()
        );
        for draw in &prepared.draws {
            let expected = complete
                .draws
                .iter()
                .find(|other| other.node == draw.node)
                .unwrap();
            assert_eq!(draw.world, expected.world);
            assert_eq!(draw.bounds, expected.bounds);
        }
        assert_eq!(scene.node(ids[65]).unwrap().transform.translation.x, 0.);
        assert!(scene.node(ids[3]).is_some());
        assert!(next.fork().changed_nodes(&scene).is_none());
    }
}
