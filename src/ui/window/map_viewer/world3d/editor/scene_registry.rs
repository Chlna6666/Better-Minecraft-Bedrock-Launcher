//! Stable chunk node identities across immutable scene forks.
use super::*;
use gpui_3d::{NodeId, SceneError};

#[derive(Default)]
pub(super) struct ChunkScene {
    nodes: BTreeMap<(ChunkPos, i32), (NodeId, Arc<World3dChunkMesh>)>,
}

#[cfg(test)]
mod tests {
    use super::{Arc, BTreeMap, ChunkPos, ChunkScene, Dimension, World3dChunkMesh};
    use gpui_3d::Mesh;
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
        meshes.clear();
        let removed = registry.update(Some(&after), &meshes, [0; 3]).unwrap();
        assert!(removed.node(id).is_none());
        assert!(after.node(id).is_some());
    }
}

impl ChunkScene {
    pub(super) fn update(
        &mut self,
        base: Option<&Scene>,
        meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
        origin: [i32; 3],
    ) -> Result<Scene, SceneError> {
        if meshes.is_empty() {
            self.nodes.clear();
            return Ok(Scene::new());
        }
        let mut scene = base.map_or_else(Scene::new, Scene::fork);
        if base.is_none() {
            self.nodes.clear();
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
            let node = chunk_node(mesh, origin);
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
