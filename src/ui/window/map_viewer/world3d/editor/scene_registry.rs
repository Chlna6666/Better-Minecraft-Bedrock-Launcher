//! Stable chunk node identities across immutable scene forks.
use super::*;
use gpui_3d::{NodeId, SceneError};

#[derive(Default)]
pub(super) struct ChunkScene {
    range: Option<(Dimension, i32, i32, i32)>,
    lights: BTreeMap<(ChunkPos, [i32; 4]), (NodeId, super::super::types::World3dLocalLight)>,
    nodes: BTreeMap<(ChunkPos, i32), (NodeId, Arc<World3dChunkMesh>)>,
}

#[cfg(test)]
mod tests {
    use super::{Arc, BTreeMap, ChunkPos, ChunkScene, Dimension, World3dChunkMesh};
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
        let mut scene = registry.update(None, &BTreeMap::new(), [0; 3]).unwrap();
        let lights = BTreeMap::from([(pos, Arc::from([source]))]);
        registry.update_lights(&mut scene, &lights, [0; 3]).unwrap();
        let id = registry.lights[&(pos, [0; 4])].0;
        let mut next = registry
            .update(Some(&scene), &BTreeMap::new(), [0; 3])
            .unwrap();
        registry.update_lights(&mut next, &lights, [0; 3]).unwrap();
        assert_eq!(registry.lights[&(pos, [0; 4])].0, id);
        assert_eq!(scene.version(), next.version());
        registry
            .update_lights(&mut next, &BTreeMap::new(), [0; 3])
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
        let before = registry.update(None, &meshes, [0; 3]).unwrap();
        let unchanged = registry.update(Some(&before), &meshes, [0; 3]).unwrap();
        assert_eq!(unchanged.version(), before.version());
        let id = registry.nodes[&(pos, 0)].0;
        let mut replacement = (*chunk).clone();
        replacement.mesh = Arc::new(Mesh::cube());
        meshes.insert((pos, 0), Arc::new(replacement));
        let after = registry.update(Some(&before), &meshes, [0; 3]).unwrap();
        assert_eq!(registry.nodes[&(pos, 0)].0, id);
        assert_eq!(
            before.node(id).unwrap().mesh.as_ref().unwrap().id(),
            chunk.mesh.id()
        );
        assert_ne!(
            before.node(id).unwrap().mesh.as_ref().unwrap().id(),
            after.node(id).unwrap().mesh.as_ref().unwrap().id()
        );
        let mut outside = after.fork();
        registry.update_range(&mut outside, (Dimension::Overworld, 1, 1, 1));
        assert!(!outside.node(id).unwrap().visible);
        assert!(after.node(id).unwrap().visible);
        let mut replacement_meshes = meshes.clone();
        replacement_meshes.insert((pos, 0), chunk.clone());
        let mut refreshed = registry
            .update(Some(&outside), &replacement_meshes, [0; 3])
            .unwrap();
        registry.update_range(&mut refreshed, (Dimension::Overworld, 1, 1, 1));
        assert!(!refreshed.node(id).unwrap().visible);
        let mut boundary = outside.fork();
        registry.update_range(&mut boundary, (Dimension::Overworld, 1, 0, 1));
        assert!(boundary.node(id).unwrap().visible);
        assert_eq!(registry.nodes[&(pos, 0)].0, id);
        let version = boundary.version();
        registry.update_range(&mut boundary, (Dimension::Overworld, 1, 0, 1));
        assert_eq!(boundary.version(), version);
        meshes.clear();
        let removed = registry.update(Some(&after), &meshes, [0; 3]).unwrap();
        assert!(removed.node(id).is_none());
        assert!(after.node(id).is_some());
    }
}

impl ChunkScene {
    pub(super) fn update_range(&mut self, scene: &mut Scene, range: (Dimension, i32, i32, i32)) {
        if self.range == Some(range) {
            return;
        }
        self.range = Some(range);
        let (dimension, x, z, radius) = range;
        for (&(pos, _), &(id, _)) in &self.nodes {
            let visible = super::super::chunk_service::inside_view(pos, dimension, x, z, radius);
            if scene.node(id).is_some_and(|node| node.visible != visible) {
                scene.set_visible(id, visible);
            }
        }
    }

    pub(super) fn update(
        &mut self,
        base: Option<&Scene>,
        meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
        origin: [i32; 3],
    ) -> Result<Scene, SceneError> {
        let mut scene = base.map_or_else(Scene::new, Scene::fork);
        if base.is_none() {
            self.nodes.clear();
            self.lights.clear();
        }
        let removed = self
            .nodes
            .keys()
            .filter(|key| !meshes.contains_key(key))
            .copied()
            .collect::<Vec<_>>();
        for key in removed {
            if let Some((id, _)) = self.nodes.remove(&key) {
                scene.remove(id)?;
            }
        }
        for (key, mesh) in meshes {
            if self
                .nodes
                .get(key)
                .is_some_and(|(_, old)| Arc::ptr_eq(old, mesh))
            {
                continue;
            }
            let visible = self.range.is_none_or(|(dimension, x, z, radius)| {
                super::super::chunk_service::inside_view(mesh.pos, dimension, x, z, radius)
            });
            let node = chunk_node(mesh, origin).with_visible(visible);
            let id = if let Some((id, _)) = self.nodes.get(key) {
                *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = node;
                *id
            } else {
                scene.insert(None, node)?
            };
            self.nodes.insert(*key, (id, mesh.clone()));
        }
        Ok(scene)
    }

    pub(super) fn update_lights(
        &mut self,
        scene: &mut Scene,
        lights: &BTreeMap<ChunkPos, Arc<[super::super::types::World3dLocalLight]>>,
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        let mut active = HashSet::new();
        for (&pos, sources) in lights {
            for source in sources.iter() {
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
        }
        let removed = self
            .lights
            .keys()
            .filter(|key| !active.contains(key))
            .copied()
            .collect::<Vec<_>>();
        for key in removed {
            if let Some((id, _)) = self.lights.remove(&key) {
                scene.remove(id)?;
            }
        }
        Ok(())
    }
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
