//! Reference counts preserve hidden assets without rewalking unchanged nodes.
use super::*;

#[derive(Default)]
pub(super) struct AssetCounts {
    stamp: Option<(u64, u64)>,
    meshes: HashMap<MeshId, usize>,
    textures: HashMap<TextureAssetId, usize>,
    // Only IDs that lost their final reference in the last committed delta.
    // Avoid rebuilding the whole resident GPU resource set for one Section removal.
    removed_meshes: Vec<MeshId>,
    removed_textures: Vec<TextureAssetId>,
    full_rescan: bool,
}

impl AssetCounts {
    /// Return true if a resource could have lost its last scene reference.
    /// New-section-only publications do not require a global GPU eviction walk.
    /// A lost delta lineage conservatively requests a full residency sweep.
    pub(super) fn sync(&mut self, previous: Option<&Scene>, scene: &Scene) -> bool {
        let stamp = (scene.instance_id(), scene.version());
        if self.stamp == Some(stamp) {
            return false;
        }
        self.removed_meshes.clear();
        self.removed_textures.clear();
        self.full_rescan = false;
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
            // Unrelated history or first scene: do not assume cached GPU
            // bindings are still referenced by the new scene.
            *self = Self::default();
            self.full_rescan = true;
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
        // A mesh can disappear and be re-added within one atomic scene delta.
        // Only retire IDs absent from the final reference-count snapshot.
        self.removed_meshes
            .retain(|id| !self.meshes.contains_key(id));
        self.removed_textures
            .retain(|id| !self.textures.contains_key(id));
        self.stamp = Some(stamp);
        self.full_rescan || !self.removed_meshes.is_empty() || !self.removed_textures.is_empty()
    }

    pub(super) fn removed_ids(&self) -> (&[MeshId], &[TextureAssetId]) {
        (&self.removed_meshes, &self.removed_textures)
    }

    pub(super) fn needs_full_rescan(&self) -> bool {
        self.full_rescan
    }

    fn record(&mut self, node: &crate::Node, add: bool) {
        for mesh in node
            .mesh
            .iter()
            .chain(node.lod.iter().map(|level| &level.mesh))
        {
            if adjust(&mut self.meshes, mesh.id(), add) {
                self.removed_meshes.push(mesh.id());
            }
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
                if adjust(&mut self.textures, texture, add) {
                    self.removed_textures.push(texture);
                }
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
    fn additions_do_not_require_residency_retirement() {
        let first = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let mut counts = AssetCounts::default();
        assert!(
            counts.sync(None, &first),
            "initial residency is conservative"
        );
        let mut second = first.fork();
        second
            .insert(None, Node::new().with_mesh(mesh.clone()))
            .unwrap();
        assert!(
            !counts.sync(Some(&first), &second),
            "adding a section must not retire existing assets"
        );
        assert!(
            !counts.sync(Some(&first), &second),
            "same revision must be a no-op"
        );
        let mut third = second.fork();
        let new_mesh = Arc::new(Mesh::cube());
        third
            .insert(None, Node::new().with_mesh(new_mesh.clone()))
            .unwrap();
        assert!(
            !counts.sync(Some(&second), &third),
            "adding another mesh is monotonic"
        );
        assert_eq!(counts.meshes.get(&mesh.id()), Some(&1));
        assert_eq!(counts.meshes.get(&new_mesh.id()), Some(&1));
    }

    #[test]
    fn removing_a_last_reference_requests_retirement() {
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        let a = scene
            .insert(None, Node::new().with_mesh(mesh.clone()))
            .unwrap();
        let b = scene
            .insert(None, Node::new().with_mesh(mesh.clone()))
            .unwrap();
        let mut counts = AssetCounts::default();
        assert!(counts.sync(None, &scene));
        let mut next = scene.fork();
        next.remove(a).unwrap();
        assert!(
            !counts.sync(Some(&scene), &next),
            "one shared mesh reference remains"
        );
        assert_eq!(counts.meshes.get(&mesh.id()), Some(&1));
        let mut last = next.fork();
        last.remove(b).unwrap();
        assert!(
            counts.sync(Some(&next), &last),
            "last reference must trigger cleanup"
        );
        assert!(counts.meshes.is_empty());
    }

    #[test]
    fn atomic_replacement_does_not_retire_readded_shared_mesh() {
        let mesh = Arc::new(Mesh::cube());
        let mut before = Scene::new();
        let id = before
            .insert(None, Node::new().with_mesh(mesh.clone()))
            .unwrap();
        let mut counts = AssetCounts::default();
        assert!(counts.sync(None, &before));
        let mut next = before.fork();
        next.remove(id).unwrap();
        next.insert(None, Node::new().with_mesh(mesh)).unwrap();
        assert!(!counts.sync(Some(&before), &next));
        assert!(counts.removed_ids().0.is_empty());
        assert!(!counts.needs_full_rescan());
    }

    #[test]
    fn retirement_delta_contains_only_last_removed_mesh() {
        let one = Arc::new(Mesh::cube());
        let two = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        let old = scene
            .insert(None, Node::new().with_mesh(one.clone()))
            .unwrap();
        let stay = scene
            .insert(None, Node::new().with_mesh(two.clone()))
            .unwrap();
        let mut counts = AssetCounts::default();
        assert!(counts.sync(None, &scene));
        let mut next = scene.fork();
        next.remove(old).unwrap();
        assert!(counts.sync(Some(&scene), &next));
        assert_eq!(counts.removed_ids().0, &[one.id()]);
        assert_eq!(counts.removed_ids().1.len(), 0);
        assert!(next.node(stay).is_some());
        assert!(!counts.needs_full_rescan());
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
        assert!(
            counts.sync(Some(&before), &next),
            "mesh replacement loses one active reference"
        );
        assert!(
            !counts.sync(Some(&before), &next),
            "retry does not invalidate again"
        );
        assert_eq!(counts.meshes.get(&b.id()), Some(&1));
        assert!(!counts.meshes.contains_key(&a.id()));
        let alternative = before.fork();
        assert!(
            counts.sync(Some(&before), &alternative),
            "diverging history forces full validation"
        );
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
