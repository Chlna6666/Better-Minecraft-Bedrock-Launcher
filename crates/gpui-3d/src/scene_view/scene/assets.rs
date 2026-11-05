//! Reference counts preserve hidden assets without rewalking unchanged nodes.
use super::*;

#[derive(Default)]
pub(super) struct AssetCounts {
    stamp: Option<(u64, u64)>,
    meshes: HashMap<MeshId, usize>,
    textures: HashMap<TextureAssetId, usize>,
}

impl AssetCounts {
    /// Return true if a resource could have lost its last scene reference.
    /// New-section-only publications do not require a global GPU eviction walk.
    /// A lost delta lineage conservatively requests a full residency sweep.
    pub(super) fn sync(&mut self, previous: Option<&Scene>, scene: &Scene) -> bool {
        let mut might_have_removed_assets = false;
        let stamp = (scene.instance_id(), scene.version());
        if self.stamp == Some(stamp) {
            return false;
        }
        if let Some(previous) =
            previous.filter(|old| self.stamp == Some((old.instance_id(), old.version())))
            && let Some(nodes) = scene.changed_nodes(previous)
        {
            let unique = nodes.iter().copied().collect::<HashSet<_>>();
            for id in unique {
                if let Some(node) = previous.node(id) {
                    might_have_removed_assets |= self.record(node, false);
                }
                if let Some(node) = scene.node(id) {
                    self.record(node, true);
                }
            }
        } else {
            // Unrelated history or first scene: do not assume cached GPU
            // bindings are still referenced by the new scene.
            might_have_removed_assets = true;
            *self = Self::default();
            let mut pending = scene.roots().to_vec();
            while let Some(id) = pending.pop() {
                if let Some(node) = scene.node(id) {
                    self.record(node, true);
                }
                if let Some(children) = scene.children(id) {
                    pending.extend_from_slice(children);
                }
            }
        }
        self.stamp = Some(stamp);
        might_have_removed_assets
    }

    fn record(&mut self, node: &crate::Node, add: bool) -> bool {
        let mut removed = false;
        for mesh in node
            .mesh
            .iter()
            .chain(node.lod.iter().map(|level| &level.mesh))
        {
            removed |= adjust(&mut self.meshes, mesh.id(), add);
        }
        for material in node
            .materials
            .iter()
            .chain(node.lod.iter().flat_map(|level| &level.materials))
        {
            for texture in [
                material.albedo_texture,
                material.normal_texture,
                material.occlusion_texture,
                material.metallic_roughness_texture,
            ]
            .into_iter()
            .flatten()
            {
                removed |= adjust(&mut self.textures, texture, add);
            }
        }
        removed
    }

    pub(super) fn ids(&self) -> (HashSet<MeshId>, HashSet<TextureAssetId>) {
        (
            self.meshes.keys().copied().collect(),
            self.textures.keys().copied().collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mesh, Node};
    #[test]
    fn additions_do_not_require_residency_retirement() {
        let first = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let mut counts = AssetCounts::default();
        assert!(counts.sync(None, &first), "initial residency is conservative");
        let mut second = first.fork();
        second.insert(None, Node::new().with_mesh(mesh.clone())).unwrap();
        assert!(!counts.sync(Some(&first), &second), "adding a section must not retire existing assets");
        assert!(!counts.sync(Some(&first), &second), "same revision must be a no-op");
        let mut third = second.fork();
        let new_mesh = Arc::new(Mesh::cube());
        third.insert(None, Node::new().with_mesh(new_mesh.clone())).unwrap();
        assert!(!counts.sync(Some(&second), &third), "adding another mesh is monotonic");
        assert_eq!(counts.meshes.get(&mesh.id()), Some(&1));
        assert_eq!(counts.meshes.get(&new_mesh.id()), Some(&1));
    }

    #[test]
    fn removing_a_last_reference_requests_retirement() {
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        let a = scene.insert(None, Node::new().with_mesh(mesh.clone())).unwrap();
        let b = scene.insert(None, Node::new().with_mesh(mesh.clone())).unwrap();
        let mut counts = AssetCounts::default();
        assert!(counts.sync(None, &scene));
        let mut next = scene.fork();
        next.remove(a).unwrap();
        assert!(!counts.sync(Some(&scene), &next), "one shared mesh reference remains");
        assert_eq!(counts.meshes.get(&mesh.id()), Some(&1));
        let mut last = next.fork();
        last.remove(b).unwrap();
        assert!(counts.sync(Some(&next), &last), "last reference must trigger cleanup");
        assert!(counts.meshes.is_empty());
    }

    #[test]
    fn retries_and_diverging_snapshots_do_not_duplicate_asset_references() {
        let mut before = Scene::new();
        let a = Arc::new(Mesh::cube());
        let b = Arc::new(Mesh::cube());
        let id = before
            .insert(None, Node::new().with_mesh(a.clone()))
            .unwrap();
        let mut counts = AssetCounts::default();
        assert!(counts.sync(Some(&before), &before));
        let mut next = before.fork();
        next.node_mut(id).unwrap().mesh = Some(b.clone());
        assert!(counts.sync(Some(&before), &next), "mesh replacement loses one active reference");
        assert!(!counts.sync(Some(&before), &next), "retry does not invalidate again");
        assert_eq!(counts.meshes.get(&b.id()), Some(&1));
        assert!(!counts.meshes.contains_key(&a.id()));
        let alternative = before.fork();
        assert!(counts.sync(Some(&before), &alternative), "diverging history forces full validation");
        assert_eq!(counts.meshes.get(&a.id()), Some(&1));
        assert!(!counts.meshes.contains_key(&b.id()));
        let mut empty = alternative.fork();
        empty.remove(id).unwrap();
        counts.sync(Some(&alternative), &empty);
        assert!(counts.meshes.is_empty());
    }
}

/// True if a last reference was retired, including conservative transients.
fn adjust<K: Eq + std::hash::Hash>(counts: &mut HashMap<K, usize>, key: K, add: bool) -> bool {
    if add {
        *counts.entry(key).or_default() += 1;
        false
    } else if let std::collections::hash_map::Entry::Occupied(mut entry) = counts.entry(key) {
        if *entry.get() <= 1 {
            entry.remove();
            true
        } else {
            *entry.get_mut() -= 1;
            false
        }
    } else {
        false
    }
}
