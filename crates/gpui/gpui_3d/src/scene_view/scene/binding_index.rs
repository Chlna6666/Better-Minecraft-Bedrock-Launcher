//! Reverse references for GPU draw BindGroups.
//! An unloaded Section invalidates only the bindings that reference its mesh
//! or textures. A changed device buffer is handled by the full rebind path.
use super::*;

#[derive(Default)]
pub(super) struct BindingIndex {
    by_mesh: HashMap<MeshId, HashSet<DrawResourceKey>>,
    by_page: HashMap<BufferId, HashSet<DrawResourceKey>>,
    by_texture: HashMap<TextureAssetId, HashSet<DrawResourceKey>>,
}

fn remove_reference<K: Eq + std::hash::Hash + Copy>(
    index: &mut HashMap<K, HashSet<DrawResourceKey>>,
    identity: K,
    key: DrawResourceKey,
) {
    let empty = if let Some(keys) = index.get_mut(&identity) {
        keys.remove(&key);
        keys.is_empty()
    } else {
        false
    };
    if empty {
        index.remove(&identity);
    }
}

impl BindingIndex {
    pub(super) fn clear(&mut self) {
        self.by_mesh.clear();
        self.by_page.clear();
        self.by_texture.clear();
    }

    pub(super) fn insert(&mut self, key: DrawResourceKey) {
        match key.mesh {
            DrawMeshKey::Mesh { id, .. } => {
                self.by_mesh.entry(id).or_default().insert(key);
            }
            DrawMeshKey::GeneratedPage { faces, templates } => {
                self.by_page.entry(faces).or_default().insert(key);
                self.by_page.entry(templates).or_default().insert(key);
            }
            DrawMeshKey::VoxelPage(page) => {
                self.by_page.entry(page).or_default().insert(key);
            }
        }
        for id in [
            key.albedo_id,
            key.normal_id,
            key.occlusion_id,
            key.metallic_roughness_id,
        ]
        .into_iter()
        .flatten()
        {
            self.by_texture.entry(id).or_default().insert(key);
        }
    }

    pub(super) fn remove(&mut self, key: DrawResourceKey) {
        match key.mesh {
            DrawMeshKey::Mesh { id, .. } => {
                remove_reference(&mut self.by_mesh, id, key);
            }
            DrawMeshKey::GeneratedPage { faces, templates } => {
                remove_reference(&mut self.by_page, faces, key);
                remove_reference(&mut self.by_page, templates, key);
            }
            DrawMeshKey::VoxelPage(page) => {
                remove_reference(&mut self.by_page, page, key);
            }
        }
        for id in [
            key.albedo_id,
            key.normal_id,
            key.occlusion_id,
            key.metallic_roughness_id,
        ]
        .into_iter()
        .flatten()
        {
            remove_reference(&mut self.by_texture, id, key);
        }
    }

    pub(super) fn affected(
        &self,
        meshes: &[MeshId],
        pages: &HashSet<BufferId>,
        textures: &[TextureAssetId],
    ) -> HashSet<DrawResourceKey> {
        let mut changed = HashSet::new();
        for id in meshes {
            if let Some(keys) = self.by_mesh.get(id) {
                changed.extend(keys.iter().copied());
            }
        }
        for id in pages {
            if let Some(keys) = self.by_page.get(id) {
                changed.extend(keys.iter().copied());
            }
        }
        for id in textures {
            if let Some(keys) = self.by_texture.get(id) {
                changed.extend(keys.iter().copied());
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: MeshId, texture: Option<TextureAssetId>) -> DrawResourceKey {
        DrawResourceKey {
            shadow: None,
            mesh: DrawMeshKey::Mesh { id, generation: 1 },
            visible_instances: None,
            albedo_id: texture,
            normal_id: None,
            occlusion_id: None,
            metallic_roughness_id: None,
        }
    }

    #[test]
    fn removes_only_the_referenced_mesh_bindings() {
        let mut index = BindingIndex::default();
        let a = key(MeshId(1), None);
        let b = key(MeshId(2), None);
        index.insert(a);
        index.insert(b);
        assert_eq!(
            index.affected(&[MeshId(1)], &HashSet::new(), &[]),
            HashSet::from([a])
        );
        index.remove(a);
        assert!(
            index
                .affected(&[MeshId(1)], &HashSet::new(), &[])
                .is_empty()
        );
        assert_eq!(
            index.affected(&[MeshId(2)], &HashSet::new(), &[]),
            HashSet::from([b])
        );
    }

    #[test]
    fn generated_page_lookup_deduplicates_both_resources() {
        let faces = BufferId::from_parts(5, 1);
        let templates = BufferId::from_parts(6, 1);
        let page_key = DrawResourceKey {
            mesh: DrawMeshKey::GeneratedPage { faces, templates },
            ..key(MeshId(11), None)
        };
        let mut index = BindingIndex::default();
        index.insert(page_key);
        assert_eq!(
            index.affected(&[], &HashSet::from([faces, templates]), &[]),
            HashSet::from([page_key])
        );
        index.remove(page_key);
        assert!(
            index
                .affected(&[], &HashSet::from([faces, templates]), &[])
                .is_empty()
        );
    }

    #[test]
    fn texture_replacement_selects_all_users_once() {
        let mut index = BindingIndex::default();
        let texture = TextureAssetId(17);
        let a = key(MeshId(1), Some(texture));
        let b = key(MeshId(2), Some(texture));
        index.insert(a);
        index.insert(b);
        assert_eq!(
            index.affected(&[MeshId(1)], &HashSet::new(), &[texture]),
            HashSet::from([a, b])
        );
    }
}
