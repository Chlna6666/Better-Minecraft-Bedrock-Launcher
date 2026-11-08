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
            self.changed_asset_draws.clear();
            self.resident_delta_draws = self.opaque_resident_order.then(Vec::new);
            return Some(0);
        }
        // Until all delta preconditions are established, an early-return
        // failure must never reuse an old resident GPU dirty-index list.
        self.resident_delta_draws = None;
        self.structural_delta = None;
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
        // The bounded RenderJob traversal now takes a cancellation guard.
        // Incremental in-place Section updates are synchronous and must
        // complete; never publish a partially walked replacement.
        if !partial
            .walk_pending(scene, |_, node| node.transform, camera, || false)
            .ok()?
        {
            return None;
        }
        let mut replacements = std::collections::HashMap::<NodeId, Vec<PreparedDraw>>::new();
        for draw in partial.candidates.iter().cloned() {
            replacements.entry(draw.node).or_default().push(draw);
        }
        // Resolve only changed roots by stable NodeId index instead of
        // scanning all resident candidate groups. Sort the small delta by
        // its original position to retain the full walk's ordering.
        let mut groups = changed
            .iter()
            .filter_map(|&id| {
                let index = *self.candidate_group_index.get(&id)?;
                let group = self.candidate_groups.get(index)?;
                Some((index, group.start..group.end, id))
            })
            .collect::<Vec<_>>();
        groups.sort_unstable_by_key(|(index, _, _)| *index);
        let mut inserted = replacements.keys().copied().collect::<HashSet<_>>();
        for (_, _, id) in &groups {
            inserted.remove(id);
        }
        // Editor transforms and resource changes normally preserve the
        // number of mesh parts for each affected root. In this case every
        // surviving candidate retains its position; update only those
        // slices instead of moving the entire scene through a scratch Vec.
        // Use the partial walk's exact group bounds (not a union of part
        // bounds), so the result matches a complete scene traversal.
        let fixed_layout = inserted.is_empty()
            && groups.iter().all(|(_, range, id)| {
                replacements
                    .get(id)
                    .is_some_and(|draws| draws.len() == range.len())
                    && partial.candidate_group_index.contains_key(id)
            });
        if fixed_layout {
            // Stable GPU Draw Slots can be updated directly only if their
            // opaque candidate ordering and per-part identity are unchanged.
            // A material replacement, mesh topology change, or transparency
            // transition falls back to the existing full GPU layout planner.
            let stable_gpu_layout = self.gpu_culling
                && self.opaque_resident_order
                && self.draws.len() == self.candidates.len()
                && groups.iter().all(|(_, range, id)| {
                    replacements.get(id).is_some_and(|draws| {
                        self.draws[range.clone()]
                            .iter()
                            .zip(draws)
                            .all(|(old, new)| {
                                old.node == new.node
                                    && old.mesh.id() == new.mesh.id()
                                    && old.part == new.part
                                    && old.material.id() == new.material.id()
                                    && old.material.alpha_mode != AlphaMode::Blend
                                    && new.material.alpha_mode != AlphaMode::Blend
                            })
                    })
                });
            let mut dirty = stable_gpu_layout
                .then(|| Vec::with_capacity(groups.iter().map(|(_, range, _)| range.len()).sum()));
            self.changed_asset_draws.clear();
            self.changed_asset_draws.extend(partial.candidates);
            for (index, range, id) in groups {
                let new_group = &partial.candidate_groups[partial.candidate_group_index[&id]];
                let mut group = new_group.clone();
                group.start = range.start;
                group.end = range.end;
                self.candidate_groups[index] = group;
                let replacement = replacements
                    .remove(&id)
                    .expect("equal-length group must retain its replacement");
                if let Some(dirty) = &mut dirty {
                    dirty.extend(range.clone());
                    self.draws[range.clone()].clone_from_slice(&replacement);
                }
                self.candidates[range].clone_from_slice(&replacement);
            }
            self.occluders.retain(|id, _| !changed.contains(id));
            self.occluders.extend(partial.occluders);
            if stable_gpu_layout {
                // apply_camera() sees an already-valid GPU opaque candidate
                // order and only refreshes camera uniforms; the changed
                // resident slots are published to SceneResources.
                self.resident_delta_draws = dirty;
            } else {
                self.visible_candidates.clear();
                self.gpu_candidates_dirty = true;
                self.opaque_resident_order = false;
                self.resident_prefix_order = false;
            }
            return Some(changed.len());
        }
        // New roots are visited first by the full walker. Streaming
        // publications interleave insertions, removals and fused-node
        // replacements, and removed slots are reused by Scene::insert, so
        // new roots are not necessarily a contiguous tail of Scene::roots().
        // Resolve inserted roots by their actual roots order instead of
        // requiring an uninterrupted reverse prefix.
        let mut prefix = Vec::new();
        let mut next_groups = Vec::with_capacity(self.candidate_groups.len() + inserted.len());
        let inserted_roots = scene
            .roots()
            .iter()
            .rev()
            .filter(|id| inserted.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in inserted_roots {
            if let Some(draws) = replacements.remove(&id) {
                if let Some(group) = candidate_group(&draws, prefix.len()) {
                    next_groups.push(group);
                }
                prefix.extend(draws);
                inserted.remove(&id);
            }
        }
        if !inserted.is_empty() {
            return None;
        }
        // The committed draw list is the previous full publication's
        // [candidate-opaque prefix, depth-sorted blend tail]. Structural
        // deltas are therefore expressed in draw space: every candidate
        // edit maps to its opaque-prefix interval, and one final whole-tail
        // edit covers blend removals, additions and camera re-sorting.
        // This keeps deltas valid for blend-heavy worlds and moving cameras.
        let structural_ready = self.gpu_culling
            && self.resident_prefix_order
            && self.draws.len() == self.candidates.len();
        let is_blended = |draw: &PreparedDraw| draw.material.alpha_mode == AlphaMode::Blend;
        let prefix_blends = prefix.iter().filter(|draw| is_blended(draw)).count();
        let prefix_opaques = prefix.len() - prefix_blends;
        let mut draw_edits = Vec::<StructuralDrawEdit>::with_capacity(groups.len() + 2);
        if !prefix.is_empty() {
            draw_edits.push(StructuralDrawEdit {
                old: 0..0,
                new: 0..prefix_opaques,
            });
        }
        // Blends among candidates before the current group on each side.
        let mut old_blends = 0usize;
        let mut new_blends = prefix_blends;
        let mut next_start = prefix.len();
        let mut changed_groups = groups.iter().peekable();
        for (index, group) in self.candidate_groups.iter().enumerate() {
            if changed_groups
                .peek()
                .is_some_and(|(changed_index, _, _)| *changed_index == index)
            {
                let (_, old_range, id) = changed_groups.next()?;
                let new_start = next_start;
                let mut replacement_blends = 0usize;
                if let Some(draws) = replacements.get(id) {
                    replacement_blends = draws.iter().filter(|draw| is_blended(draw)).count();
                    if let Some(next) = candidate_group(draws, next_start) {
                        next_start = next.end;
                        next_groups.push(next);
                    }
                }
                let old_opaques = old_range.len() - group.blends as usize;
                let new_opaques = (next_start - new_start) - replacement_blends;
                if structural_ready && (old_opaques > 0 || new_opaques > 0) {
                    draw_edits.push(StructuralDrawEdit {
                        old: (old_range.start - old_blends)
                            ..(old_range.start - old_blends + old_opaques),
                        new: (new_start - new_blends)..(new_start - new_blends + new_opaques),
                    });
                }
                old_blends += group.blends as usize;
                new_blends += replacement_blends;
            } else {
                let mut next = group.clone();
                next.end = next_start + group.end - group.start;
                next.start = next_start;
                next_start = next.end;
                next_groups.push(next);
                old_blends += group.blends as usize;
                new_blends += group.blends as usize;
            }
        }
        if structural_ready {
            let old_opaque_len = self.candidates.len() - old_blends;
            let new_opaque_len = next_start - new_blends;
            if old_opaque_len < self.draws.len() || new_opaque_len < next_start {
                draw_edits.push(StructuralDrawEdit {
                    old: old_opaque_len..self.draws.len(),
                    new: new_opaque_len..next_start,
                });
            }
        }
        // Repeated Vec::splice shifts the entire suffix for every changed section.
        // Merge the retained candidates once instead, moving unchanged draws and
        // replacement draws in O(total_draws + changed_nodes). Keep the spare vector
        // allocated across updates so section streaming does not churn the allocator.
        self.changed_asset_draws.clear();
        self.changed_asset_draws.extend(partial.candidates);
        self.candidate_scratch.clear();
        self.candidate_scratch.reserve(next_start);
        self.candidate_scratch.extend(prefix);
        let mut previous = self.candidates.drain(..);
        let mut changed_groups = groups.into_iter().peekable();
        for (index, group) in self.candidate_groups.iter().enumerate() {
            let current = previous.by_ref().take(group.end - group.start);
            if changed_groups
                .peek()
                .is_some_and(|(changed_index, _, _)| *changed_index == index)
            {
                let (_, _, id) = changed_groups.next()?;
                // The old root has been replaced or removed.
                for _ in current {}
                self.candidate_scratch
                    .extend(replacements.remove(&id).unwrap_or_default());
            } else {
                self.candidate_scratch.extend(current);
            }
        }
        debug_assert!(previous.next().is_none());
        drop(previous);
        std::mem::swap(&mut self.candidates, &mut self.candidate_scratch);
        self.occluders.retain(|id, _| !changed.contains(id));
        self.occluders.extend(partial.occluders);
        self.candidate_groups = next_groups;
        // Inserted/removed Sections shift offsets. Rebuild the lookup once
        // after the merged candidate list has been committed.
        self.candidate_group_index.clear();
        for (index, group) in self.candidate_groups.iter().enumerate() {
            self.candidate_group_index
                .insert(self.candidates[group.start].node, index);
        }
        self.visible_candidates.clear();
        self.gpu_candidates_dirty = true;
        self.opaque_resident_order = false;
        self.resident_prefix_order = false;
        if structural_ready && !draw_edits.is_empty() {
            self.structural_delta = Some(StructuralDrawDelta {
                previous_count: self.draws.len(),
                edits: draw_edits,
            });
        }
        Some(changed.len())
    }
}

fn candidate_group(draws: &[PreparedDraw], start: usize) -> Option<CandidateGroup> {
    let first = draws.first()?;
    let mut bounds = first.bounds;
    let mut blends = u32::from(first.material.alpha_mode == AlphaMode::Blend);
    for draw in &draws[1..] {
        bounds.min = bounds.min.min(draw.bounds.min);
        bounds.max = bounds.max.max(draw.bounds.max);
        blends += u32::from(draw.material.alpha_mode == AlphaMode::Blend);
    }
    Some(CandidateGroup {
        start,
        end: start + draws.len(),
        blends,
        bounds,
        culling_origin: first.culling_origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;

    #[test]
    fn exact_opaque_section_delta_keeps_gpu_logical_slot_indices() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 30.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let ids = (0..64)
            .map(|_| {
                scene
                    .insert(None, Node::new().with_mesh(mesh.clone()))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        assert!(prepared.set_gpu_culling(true));
        prepared.update_camera(camera, 1.).unwrap();
        assert!(prepared.opaque_resident_order);
        let mut next = scene.fork();
        next.node_mut(ids[13]).unwrap().transform.translation.x = 2.;
        next.node_mut(ids[38]).unwrap().transform.translation.z = -1.;
        let changed = next.changed_nodes(&scene).unwrap();
        assert_eq!(
            prepared.update_nodes(&next, changed.as_ref(), Mat4::IDENTITY),
            Some(2)
        );
        let dirty = prepared.resident_delta_draws().unwrap().to_vec();
        assert_eq!(dirty.len(), 2);
        assert!(
            dirty
                .iter()
                .all(|&index| changed.contains(&prepared.draws[index].node))
        );
        prepared.update_camera(camera, 1.).unwrap();
        assert_eq!(prepared.resident_delta_draws().unwrap(), dirty.as_slice());
        let full = PreparedScene::new(&next, camera, 1.).unwrap();
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| (draw.node, draw.bounds, draw.world))
                .collect::<Vec<_>>(),
            full.draws
                .iter()
                .map(|draw| (draw.node, draw.bounds, draw.world))
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn unchanged_part_count_updates_only_affected_candidate_storage() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 30.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let ids = (0..256)
            .map(|_| {
                scene
                    .insert(None, Node::new().with_mesh(mesh.clone()))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        let before = prepared.candidates.as_ptr();
        let unaffected = prepared.candidates[17].clone();
        let mut edited = scene.fork();
        edited.node_mut(ids[88]).unwrap().transform.translation.x = 3.5;
        let changed = edited.changed_nodes(&scene).unwrap();
        assert_eq!(
            prepared.update_nodes(&edited, changed.as_ref(), Mat4::IDENTITY),
            Some(1)
        );
        assert_eq!(
            prepared.candidates.as_ptr(),
            before,
            "same-size Section Delta must reuse the existing candidate storage"
        );
        assert_eq!(prepared.candidates[17].node, unaffected.node);
        assert_eq!(prepared.candidates[17].world, unaffected.world);
        assert_eq!(
            prepared.candidate_group_index.len(),
            prepared.candidate_groups.len()
        );
        prepared.update_camera(camera, 1.).unwrap();
        let full = PreparedScene::new(&edited, camera, 1.).unwrap();
        assert_eq!(
            prepared
                .candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>(),
            full.candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| (draw.node, draw.world))
                .collect::<Vec<_>>(),
            full.draws
                .iter()
                .map(|draw| (draw.node, draw.world))
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn many_changed_sections_match_a_full_walk_without_repeated_splicing() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 10.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let ids = (0..512)
            .map(|_| {
                scene
                    .insert(None, Node::new().with_mesh(mesh.clone()))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        let step = 0;
        {
            let mut next = scene.fork();
            for index in (step..512).step_by(7) {
                if index % 5 == 0 {
                    next.remove(ids[index]).unwrap();
                } else {
                    next.node_mut(ids[index]).unwrap().transform.translation.x = step as f32 + 1.;
                }
            }
            let changes = next.changed_nodes(&scene).unwrap();
            assert!(changes.len() > 50);
            assert_eq!(
                prepared.update_nodes(&next, changes.as_ref(), Mat4::IDENTITY),
                Some(changes.len())
            );
            prepared.update_camera(camera, 1.).unwrap();
            let full = PreparedScene::new(&next, camera, 1.).unwrap();
            assert_eq!(
                prepared
                    .candidates
                    .iter()
                    .map(|draw| draw.node)
                    .collect::<Vec<_>>(),
                full.candidates
                    .iter()
                    .map(|draw| draw.node)
                    .collect::<Vec<_>>(),
            );
            assert_eq!(
                prepared
                    .candidate_groups
                    .iter()
                    .map(|group| (group.start, group.end, group.bounds))
                    .collect::<Vec<_>>(),
                full.candidate_groups
                    .iter()
                    .map(|group| (group.start, group.end, group.bounds))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(prepared.draws.len(), full.draws.len());
            assert!(prepared.candidate_scratch.capacity() >= full.candidates.len());
            assert!(
                prepared
                    .changed_asset_draws
                    .iter()
                    .all(|draw| changes.contains(&draw.node))
            );
            assert!(prepared.changed_asset_draws.len() < full.candidates.len());
        }
    }

    #[test]
    fn candidate_group_index_survives_remove_insert_then_followup_edit() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 25.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
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

        let mut inserted = scene.fork();
        inserted.remove(ids[6]).unwrap();
        inserted.remove(ids[33]).unwrap();
        inserted
            .insert(None, Node::new().with_mesh(mesh.clone()))
            .unwrap();
        let delta = inserted.changed_nodes(&scene).unwrap();
        assert_eq!(
            prepared.update_nodes(&inserted, delta.as_ref(), Mat4::IDENTITY),
            Some(delta.len())
        );
        prepared.update_camera(camera, 1.).unwrap();
        assert_eq!(
            prepared.candidate_group_index.len(),
            prepared.candidate_groups.len()
        );
        for (index, group) in prepared.candidate_groups.iter().enumerate() {
            assert_eq!(
                prepared
                    .candidate_group_index
                    .get(&prepared.candidates[group.start].node),
                Some(&index)
            );
        }

        let original_storage = prepared.candidates.as_ptr();
        let mut edited = inserted.fork();
        edited.node_mut(ids[90]).unwrap().transform.translation.x = 2.;
        edited.node_mut(ids[11]).unwrap().transform.translation.y = 1.;
        let delta = edited.changed_nodes(&inserted).unwrap();
        assert_eq!(
            prepared.update_nodes(&edited, delta.as_ref(), Mat4::IDENTITY),
            Some(delta.len())
        );
        assert_eq!(
            prepared.candidates.as_ptr(),
            original_storage,
            "non-structural followup must not reconstruct the candidate Vec"
        );
        prepared.update_camera(camera, 1.).unwrap();
        let full = PreparedScene::new(&edited, camera, 1.).unwrap();
        assert_eq!(
            prepared
                .candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>(),
            full.candidate_groups
                .iter()
                .map(|group| (group.start, group.end, group.bounds))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| (draw.node, draw.world))
                .collect::<Vec<_>>(),
            full.draws
                .iter()
                .map(|draw| (draw.node, draw.world))
                .collect::<Vec<_>>()
        );
    }

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
            prepared.update_nodes(&next, changed.as_ref(), Mat4::IDENTITY),
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

    #[test]
    fn blended_section_splice_publishes_draw_space_structural_delta() {
        let camera =
            Camera::perspective(Vec3::new(0., 0., 30.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let moved_camera =
            Camera::perspective(Vec3::new(2., 1., 28.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let mut scene = Scene::new();
        let opaque = Arc::new(crate::Material::new());
        let mut transparent = crate::Material::new();
        transparent.alpha_mode = AlphaMode::Blend;
        let transparent = Arc::new(transparent);
        let mesh = Arc::new(Mesh::cube());
        let ids = (0..48)
            .map(|index| {
                let material = if index % 4 == 3 {
                    transparent.clone()
                } else {
                    opaque.clone()
                };
                scene
                    .insert(
                        None,
                        Node::new()
                            .with_mesh(mesh.clone())
                            .with_materials([material])
                            .with_transform(Transform {
                                translation: Vec3::new((index % 7) as f32, 0., -(index as f32)),
                                ..Transform::IDENTITY
                            }),
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        assert!(prepared.set_gpu_culling(true));
        prepared.update_camera(camera, 1.).unwrap();
        assert!(prepared.resident_prefix_order);
        assert!(
            !prepared.opaque_resident_order,
            "a blended tail must not disable structural deltas"
        );
        let previous_draws = prepared.draws.clone();
        assert!(previous_draws.len() > prepared.candidates.len() / 2);

        let mut next = scene.fork();
        // A new ocean Section (blended) plus a moved land Section and one
        // removed Section: insertion, replacement and removal in one splice.
        next.insert(
            None,
            Node::new()
                .with_mesh(mesh.clone())
                .with_materials([transparent.clone()]),
        )
        .unwrap();
        next.node_mut(ids[5]).unwrap().transform.translation.x = 5.;
        next.remove(ids[9]).unwrap();
        let changed = next.changed_nodes(&scene).unwrap();
        assert_eq!(
            prepared.update_nodes(&next, changed.as_ref(), Mat4::IDENTITY),
            Some(changed.len())
        );
        let delta = prepared
            .structural_draw_delta()
            .expect("a blended splice must still publish the structural delta");
        assert_eq!(delta.previous_count, previous_draws.len());
        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;
        for edit in &delta.edits {
            assert!(edit.old.start >= old_cursor && edit.new.start >= new_cursor);
            assert_eq!(
                edit.old.start - old_cursor,
                edit.new.start - new_cursor,
                "unchanged opaque gaps must match on both sides"
            );
            old_cursor = edit.old.end;
            new_cursor = edit.new.end;
        }
        assert_eq!(old_cursor, previous_draws.len());
        assert!(
            delta.edits.last().is_some_and(|edit| {
                edit.old.end == delta.previous_count
                    && edit.old.start < edit.old.end
                    && edit.new.end > edit.new.start
            }),
            "the final whole-tail edit covers the depth-sorted blend tail"
        );

        // A moving camera must not clear the delta: the tail edit already
        // covers blend re-sorting, and the opaque prefix keeps its order.
        prepared.update_camera(moved_camera, 1.).unwrap();
        assert!(prepared.structural_draw_delta().is_some());

        let mut full = PreparedScene::new(&next, moved_camera, 1.).unwrap();
        full.set_gpu_culling(true);
        full.update_camera(moved_camera, 1.).unwrap();
        assert_eq!(prepared.draws.len(), full.draws.len());
        let spliced = prepared
            .draws
            .iter()
            .map(|draw| (draw.node, draw.world))
            .collect::<Vec<_>>();
        let walked = full
            .draws
            .iter()
            .map(|draw| (draw.node, draw.world))
            .collect::<Vec<_>>();
        assert_eq!(spliced, walked, "splice must reproduce the full walk order");
    }
}
