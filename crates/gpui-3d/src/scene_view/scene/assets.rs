//! Reference counts preserve hidden assets without rewalking unchanged nodes.
use super::*;

#[derive(Default)]
pub(super) struct AssetCounts {
    stamp: Option<(u64, u64)>,
    meshes: HashMap<MeshId, usize>,
    textures: HashMap<TextureAssetId, usize>,
}

impl AssetCounts {
    pub(super) fn sync(&mut self, previous: Option<&Scene>, scene: &Scene) {
        let stamp = (scene.instance_id(), scene.version());
        if self.stamp == Some(stamp) {
            return;
        }
        if let Some(previous) =
            previous.filter(|old| self.stamp == Some((old.instance_id(), old.version())))
            && let Some(nodes) = scene.changed_nodes(previous)
        {
            let unique = nodes.iter().copied().collect::<HashSet<_>>();
            for id in unique {
                if let Some(node) = previous.node(id) {
                    self.record(node, false);
                }
                if let Some(node) = scene.node(id) {
                    self.record(node, true);
                }
            }
        } else {
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
    }

    fn record(&mut self, node: &crate::Node, add: bool) {
        for mesh in node
            .mesh
            .iter()
            .chain(node.lod.iter().map(|level| &level.mesh))
        {
            adjust(&mut self.meshes, mesh.id(), add);
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
                adjust(&mut self.textures, texture, add);
            }
        }
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
    fn retries_and_diverging_snapshots_do_not_duplicate_asset_references() {
        let mut before = Scene::new();
        let a = Arc::new(Mesh::cube());
        let b = Arc::new(Mesh::cube());
        let id = before
            .insert(None, Node::new().with_mesh(a.clone()))
            .unwrap();
        let mut counts = AssetCounts::default();
        counts.sync(Some(&before), &before);
        let mut next = before.fork();
        next.node_mut(id).unwrap().mesh = Some(b.clone());
        counts.sync(Some(&before), &next);
        counts.sync(Some(&before), &next);
        assert_eq!(counts.meshes.get(&b.id()), Some(&1));
        assert!(!counts.meshes.contains_key(&a.id()));
        let alternative = before.fork();
        counts.sync(Some(&before), &alternative);
        assert_eq!(counts.meshes.get(&a.id()), Some(&1));
        assert!(!counts.meshes.contains_key(&b.id()));
        let mut empty = alternative.fork();
        empty.remove(id).unwrap();
        counts.sync(Some(&alternative), &empty);
        assert!(counts.meshes.is_empty());
    }
}

fn adjust<K: Eq + std::hash::Hash>(counts: &mut HashMap<K, usize>, key: K, add: bool) {
    if add {
        *counts.entry(key).or_default() += 1;
    } else if let std::collections::hash_map::Entry::Occupied(mut entry) = counts.entry(key) {
        if *entry.get() <= 1 {
            entry.remove();
        } else {
            *entry.get_mut() -= 1;
        }
    }
}
