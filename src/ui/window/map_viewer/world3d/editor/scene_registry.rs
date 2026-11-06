//! Stable chunk node identities across immutable scene forks.
use super::*;
use gpui_3d::{NodeId, SceneError};

#[derive(Default)]
pub(super) struct ChunkScene {
     lights: BTreeMap<(ChunkPos, [i32; 4]), (NodeId, super::super::types::World3dLocalLight)>,
    nodes: BTreeMap<(ChunkPos, i32), (NodeId, Arc<World3dChunkMesh>)>,
    // Two neighbouring generated Sections are one GPU mesh and therefore
    // one material-part draw per shared texture instead of two. The original
    // Section nodes remain as stable identities and occluder owners.
    fused: BTreeMap<(ChunkPos, i32), (NodeId, [Arc<World3dChunkMesh>; 2])>,
}

#[cfg(test)]
mod tests {
    use super::{Arc, BTreeMap, BTreeSet, ChunkPos, ChunkScene, Dimension, World3dChunkMesh};
    use gpui_3d::Mesh;
    #[test]
    fn invisible_emitters_keep_stable_nodes_without_geometry() {
        let pos = ChunkPos {
            x: 2,
            z: -3,
            dimension: Dimension::Overworld,
        };
        let source = super::super::super::types::World3dLocalLight {
            key: [0; 4],
            light: gpui_3d::Light::Spherical {
                radius: 0.5,
                diffuse_only: true,
                position: gpui_3d::Vec3::ZERO,
                color: [1., 0., 0.],
                intensity: 1.,
                range: 8.,
            },
        };
        let mut registry = ChunkScene::default();
        let mut scene = registry
            .update(None, &BTreeMap::new(), None, [0; 3])
            .unwrap();
        let lights = BTreeMap::from([(pos, Arc::from([source]))]);
        registry
            .update_lights(&mut scene, &lights, None, [0; 3])
            .unwrap();
        let id = registry.lights[&(pos, [0; 4])].0;
        let mut next = registry
            .update(Some(&scene), &BTreeMap::new(), None, [0; 3])
            .unwrap();
        registry
            .update_lights(&mut next, &lights, None, [0; 3])
            .unwrap();
        assert_eq!(registry.lights[&(pos, [0; 4])].0, id);
        assert_eq!(scene.version(), next.version());
        registry
            .update_lights(&mut next, &BTreeMap::new(), Some(&BTreeSet::new()), [0; 3])
            .unwrap();
        assert!(next.node(id).is_some());
        registry
            .update_lights(
                &mut next,
                &BTreeMap::new(),
                Some(&BTreeSet::from([pos])),
                [0; 3],
            )
            .unwrap();
        assert!(next.node(id).is_none());
        assert!(scene.node(id).is_some());
    }
    #[test]
    fn chunk_replacement_keeps_node_ids_and_old_scene_snapshots() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let chunk = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [0; 3],
            mesh: Arc::new(Mesh::cube()),
            materials: Arc::from([]),
            textures: Arc::from([]),
            triangles: 12,
            faces: 6,
            missing_texture_faces: 0,
        });
        let mut registry = ChunkScene::default();
        let mut meshes = BTreeMap::from([((pos, 0), chunk.clone())]);
        let before = registry.update(None, &meshes, None, [0; 3]).unwrap();
        let unchanged = registry
            .update(Some(&before), &meshes, None, [0; 3])
            .unwrap();
        assert_eq!(unchanged.version(), before.version());
        let id = registry.nodes[&(pos, 0)].0;
        let mut replacement = (*chunk).clone();
        replacement.mesh = Arc::new(Mesh::cube());
        meshes.insert((pos, 0), Arc::new(replacement));
        let after = registry
            .update(Some(&before), &meshes, None, [0; 3])
            .unwrap();
        assert_eq!(registry.nodes[&(pos, 0)].0, id);
        assert_eq!(
            before.node(id).unwrap().mesh.as_ref().unwrap().id(),
            chunk.mesh.id()
        );
        assert_ne!(
            before.node(id).unwrap().mesh.as_ref().unwrap().id(),
            after.node(id).unwrap().mesh.as_ref().unwrap().id()
        );
        let outside = after.fork();
        assert!(outside.node(id).unwrap().visible);
        // Moving the render view's range must not rewrite the immutable scene.
        // Camera-relative range filtering is performed by the SceneView culler.
        assert_eq!(outside.version(), after.version());
        let mut replacement_meshes = meshes.clone();
        replacement_meshes.insert((pos, 0), chunk.clone());
        let refreshed = registry
            .update(Some(&outside), &replacement_meshes, None, [0; 3])
            .unwrap();
        assert!(refreshed.node(id).unwrap().visible);
        assert_eq!(registry.nodes[&(pos, 0)].0, id);
        assert_ne!(
            refreshed.node(id).unwrap().mesh.as_ref().unwrap().id(),
            outside.node(id).unwrap().mesh.as_ref().unwrap().id(),
        );
        meshes.clear();
        let removed = registry
            .update(Some(&after), &meshes, None, [0; 3])
            .unwrap();
        assert!(removed.node(id).is_none());
        assert!(after.node(id).is_some());
    }

    #[test]
    fn section_delta_updates_only_changed_section() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mesh = Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [0; 3],
            mesh: Arc::new(Mesh::cube()),
            materials: Arc::from([]),
            textures: Arc::from([]),
            triangles: 12,
            faces: 6,
            missing_texture_faces: 0,
        });
        let mut meshes = BTreeMap::from([((pos, 0), mesh.clone())]);
        let mut registry = ChunkScene::default();
        let before = registry.update(None, &meshes, None, [0; 3]).unwrap();
        let original = registry.nodes[&(pos, 0)].0;
        let next = Arc::new(World3dChunkMesh {
            section_y: 1,
            ..mesh.as_ref().clone()
        });
        meshes.insert((pos, 1), next);
        let changed = BTreeSet::from([(pos, 1)]);
        let added = registry
            .update(Some(&before), &meshes, Some(&changed), [0; 3])
            .unwrap();
        assert_eq!(registry.nodes[&(pos, 0)].0, original);
        assert!(added.node(original).is_some());
        let added_id = registry.nodes[&(pos, 1)].0;
        meshes.remove(&(pos, 1));
        let removed = registry
            .update(Some(&added), &meshes, Some(&changed), [0; 3])
            .unwrap();
        assert_eq!(registry.nodes[&(pos, 0)].0, original);
        assert!(removed.node(original).is_some());
        assert!(removed.node(added_id).is_none());
        assert!(added.node(added_id).is_some());
    }
}

impl ChunkScene {
     pub(super) fn update(
        &mut self,
        base: Option<&Scene>,
        meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
        changed: Option<&BTreeSet<(ChunkPos, i32)>>,
        origin: [i32; 3],
    ) -> Result<Scene, SceneError> {
        let mut scene = base.map_or_else(Scene::new, Scene::fork);
        if base.is_none() {
            self.nodes.clear();
            self.lights.clear();
            self.fused.clear();
        } else if let Some(changed) = changed {
            let mut touched = BTreeSet::new();
            for key in changed {
                touched.insert(fusion_key(*key));
                if let Some(mesh) = meshes.get(key) {
                    self.update_mesh(&mut scene, *key, mesh, origin)?;
                } else {
                    self.remove_mesh(&mut scene, key)?;
                }
            }
            for key in touched {
                self.sync_fusion(&mut scene, meshes, key, origin)?;
            }
            return Ok(scene);
        }
        let removed = self
            .nodes
            .keys()
            .filter(|key| !meshes.contains_key(key))
            .copied()
            .collect::<Vec<_>>();
        for key in removed {
            self.remove_mesh(&mut scene, &key)?;
        }
        for (key, mesh) in meshes {
            self.update_mesh(&mut scene, *key, mesh, origin)?;
        }
        let all_groups = meshes.keys().copied().map(fusion_key)
            .chain(self.fused.keys().copied())
            .collect::<BTreeSet<_>>();
        for key in all_groups {
            self.sync_fusion(&mut scene, meshes, key, origin)?;
        }
        Ok(scene)
    }

    fn sync_fusion(
        &mut self,
        scene: &mut Scene,
        meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
        key: (ChunkPos, i32),
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        let left = meshes.get(&key);
        let right_key = ((ChunkPos { x: key.0.x + 1, ..key.0 }), key.1);
        let right = meshes.get(&right_key);
        let candidate = left.zip(right).filter(|(left, right)| {
            // Sections are fused only when both have a GPU-generated source
            // and no blended material. Transparent depth order is not
            // commutative and must remain independently sorted.
            left.mesh.gpu_voxels().is_some()
                && right.mesh.gpu_voxels().is_some()
                && left.materials.iter().chain(right.materials.iter())
                    .all(|material| material.alpha_mode != gpui_3d::AlphaMode::Blend)
        });
        if let Some((left, right)) = candidate {
            if self.fused.get(&key).is_some_and(|(_, sources)| {
                Arc::ptr_eq(&sources[0], left) && Arc::ptr_eq(&sources[1], right)
            }) {
                return Ok(());
            }
            if let Some(fused) = fused_section_pair(left, right) {
                let node = fused_render_node(&fused, origin);
                let id = if let Some((id, _)) = self.fused.get(&key) {
                    *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = node;
                    *id
                } else {
                    scene.insert(None, node)?
                };
                self.fused.insert(key, (id, [left.clone(), right.clone()]));
                // Keep the original nodes, lighting and occluder geometry
                // intact. Only their mesh submission is replaced by the
                // shared GPU-generated source.
                for member in [key, right_key] {
                    if let Some((id, _)) = self.nodes.get(&member) {
                        scene.node_mut(*id).ok_or(SceneError::StaleNode)?.mesh = None;
                    }
                }
                return Ok(());
            }
        }
        if let Some((id, _)) = self.fused.remove(&key) {
            scene.remove(id)?;
            // Restore each original draw when its peer is removed, becomes
            // transparent, or no longer has a compatible GPU source.
            for member in [key, right_key] {
                if let Some((id, mesh)) = self.nodes.get(&member) {
                    *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = chunk_node(mesh, origin);
                }
            }
        }
        Ok(())
    }

    fn remove_mesh(&mut self, scene: &mut Scene, key: &(ChunkPos, i32)) -> Result<(), SceneError> {
        if let Some((id, _)) = self.nodes.remove(key) {
            scene.remove(id)?;
        }
        Ok(())
    }

    fn update_mesh(
        &mut self,
        scene: &mut Scene,
        key: (ChunkPos, i32),
        mesh: &Arc<World3dChunkMesh>,
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        if self
            .nodes
            .get(&key)
            .is_some_and(|(_, old)| Arc::ptr_eq(old, mesh))
        {
            return Ok(());
        }
        // Do not mutate per-Section Scene::visible during camera motion.
        // SceneView carries camera-relative culling bounds; GPU cs_cull
        // performs range/frustum/Hi-Z rejection of resident Sections.
        // CPU fallback uses the same culling bounds in PreparedScene.
        let node = chunk_node(mesh, origin);
        let id = if let Some((id, _)) = self.nodes.get(&key) {
            *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = node;
            *id
        } else {
            scene.insert(None, node)?
        };
        self.nodes.insert(key, (id, mesh.clone()));
        Ok(())
    }

    pub(super) fn update_lights(
        &mut self,
        scene: &mut Scene,
        lights: &BTreeMap<ChunkPos, Arc<[super::super::types::World3dLocalLight]>>,
        changed: Option<&BTreeSet<ChunkPos>>,
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        let all_positions;
        let positions = if let Some(changed) = changed {
            changed
        } else {
            all_positions = lights
                .keys()
                .copied()
                .chain(self.lights.keys().map(|(pos, _)| *pos))
                .collect::<BTreeSet<_>>();
            &all_positions
        };
        for &pos in positions {
            let mut active = HashSet::new();
            for source in lights
                .get(&pos)
                .into_iter()
                .flat_map(|sources| sources.iter())
            {
                let key = (pos, source.key);
                active.insert(key);
                if self.lights.get(&key).is_some_and(|(_, old)| old == source) {
                    continue;
                }
                let node = Node::new()
                    .with_light(source.light)
                    .with_transform(Transform {
                        translation: Vec3::new(
                            (pos.x * 16 - origin[0]) as f32,
                            -origin[1] as f32,
                            (pos.z * 16 - origin[2]) as f32,
                        ),
                        ..Transform::IDENTITY
                    });
                let id = if let Some((id, _)) = self.lights.get(&key) {
                    *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = node;
                    *id
                } else {
                    scene.insert(None, node)?
                };
                self.lights.insert(key, (id, source.clone()));
            }
            let removed = self
                .lights
                .range((pos, [i32::MIN; 4])..=(pos, [i32::MAX; 4]))
                .filter_map(|(key, _)| (!active.contains(key)).then_some(*key))
                .collect::<Vec<_>>();
            for key in removed {
                if let Some((id, _)) = self.lights.remove(&key) {
                    scene.remove(id)?;
                }
            }
        }
        Ok(())
    }
}

// A stable even-X pairing limits a changed Section to one neighbouring
// remesh; it does not rebuild the entire resident map or invalidate unrelated
// GPU Section resources.
fn fusion_key((pos, section_y): (ChunkPos, i32)) -> (ChunkPos, i32) {
    (ChunkPos { x: pos.x.div_euclid(2) * 2, ..pos }, section_y)
}

fn fused_section_pair(
    left: &World3dChunkMesh,
    right: &World3dChunkMesh,
) -> Option<World3dChunkMesh> {
    let first = left.mesh.gpu_voxels()?;
    let second = right.mesh.gpu_voxels()?;
    // Adjacent 16-block Section interiors have a one-cell halo around
    // them. The merged grid spans 32 blocks plus the same outer halo.
    if first.size() != second.size() { return None; }
    let mut materials = Vec::<Arc<gpui_3d::Material>>::new();
    let mut index = HashMap::<gpui_3d::MaterialId, u32>::new();
    let mut mappings = [Vec::new(), Vec::new()];
    for (source, mapping) in [(left, &mut mappings[0]), (right, &mut mappings[1])] {
        for material in source.materials.iter() {
            let slot = if let Some(&slot) = index.get(&material.id()) {
                slot
            } else {
                let slot = u32::try_from(materials.len()).ok()?;
                index.insert(material.id(), slot);
                materials.push(material.clone());
                slot
            };
            mapping.push(slot);
        }
    }
    let fused_source = gpui_3d::VoxelMeshSource::fuse_sections(&[
        gpui_3d::VoxelFusionInput {
            source: first, offset: [0, 0, 0], material_slots: &mappings[0],
        },
        gpui_3d::VoxelFusionInput {
            source: second, offset: [16, 0, 0], material_slots: &mappings[1],
        },
    ]).ok()?;
    let mut textures = BTreeMap::<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>::new();
    for texture in left.textures.iter().chain(right.textures.iter()) {
        textures.entry(texture.id()).or_insert_with(|| texture.clone());
    }
    let mesh = Arc::new(gpui_3d::Mesh::from_gpu_voxels(fused_source));
    let faces = (mesh.index_count() / 6) as usize;
    Some(World3dChunkMesh {
        occluders: Arc::from([]),
        pos: left.pos,
        section_y: left.section_y,
        origin: left.origin,
        mesh,
        materials: Arc::from(materials),
        textures: Arc::from(textures.into_values().collect::<Vec<_>>()),
        faces,
        triangles: faces * 2,
        missing_texture_faces: left.missing_texture_faces + right.missing_texture_faces,
    })
}

fn fused_render_node(mesh: &World3dChunkMesh, origin: [i32; 3]) -> Node {
    // No single culling_origin: the fused node covers both Sections, and
    // assigning just the left origin would wrongly cull the right half at
    // distance boundaries. Camera frustum / union AABB remain conservative.
    Node::new()
        .with_mesh(mesh.mesh.clone())
        .with_materials(mesh.materials.iter().cloned())
        .with_transform(Transform {
            translation: Vec3::new(
                (mesh.origin[0] - origin[0]) as f32,
                mesh.origin[1] as f32,
                (mesh.origin[2] - origin[2]) as f32,
            ),
            ..Transform::IDENTITY
        })
}

fn chunk_node(mesh: &World3dChunkMesh, origin: [i32; 3]) -> Node {
    let mut node = Node::new();
    node.occluders = mesh.occluders.clone();
    node.with_mesh(mesh.mesh.clone())
        .with_materials(mesh.materials.iter().cloned())
        .with_culling_origin()
        .with_transform(Transform {
            translation: Vec3::new(
                (mesh.origin[0] - origin[0]) as f32,
                mesh.origin[1] as f32,
                (mesh.origin[2] - origin[2]) as f32,
            ),
            ..Transform::IDENTITY
        })
}
