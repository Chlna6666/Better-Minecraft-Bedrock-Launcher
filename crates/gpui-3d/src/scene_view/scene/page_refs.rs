//! Compact GPU Page residency counts, independent of the backend API.
//! Section removal only examines the pages referenced by changed Mesh IDs.
//! Physical page reuse remains the MeshArena's deferred-fence responsibility.

use super::*;

#[derive(Default)]
pub(super) struct PageRefs {
    users: HashMap<BufferId, usize>,
    /// Resident CPU-packed procedural quad meshes requiring native instance
    /// grouping. Generated Meshes are already independent indirect draws.
    ungenerated_quads: usize,
    /// Covers both compact and ordinary meshes: only when every resident
    /// resource is GPU-generated can every singleton indirect batch use the
    /// constant [generated=1, instances=1, first_instance=index] ABI.
    generated_meshes: usize,
    ordinary_meshes: usize,
}

impl PageRefs {
    fn visit(mesh: &MeshResources, mut action: impl FnMut(BufferId)) {
        action(mesh.vertices);
        if let Some(generated) = &mesh.generated {
            action(generated.templates);
            action(generated.arguments);
        }
    }

    pub(super) fn insert(&mut self, mesh: &MeshResources, compact: bool) {
        if mesh.generated.is_some() {
            self.generated_meshes += 1;
        } else {
            self.ordinary_meshes += 1;
        }
        if mesh.procedural_quads && mesh.generated.is_none() {
            self.ungenerated_quads += 1;
        }
        if compact {
            Self::visit(mesh, |page| *self.users.entry(page).or_default() += 1);
        }
    }

    pub(super) fn remove(&mut self, mesh: &MeshResources, compact: bool) {
        if mesh.generated.is_some() {
            debug_assert!(self.generated_meshes > 0);
            self.generated_meshes -= 1;
        } else {
            debug_assert!(self.ordinary_meshes > 0);
            self.ordinary_meshes -= 1;
        }
        if mesh.procedural_quads && mesh.generated.is_none() {
            debug_assert!(self.ungenerated_quads > 0);
            self.ungenerated_quads -= 1;
        }
        if compact {
            Self::visit(mesh, |page| {
                match self.users.entry(page) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let count = *entry.get();
                        debug_assert!(count > 0);
                        if count <= 1 {
                            entry.remove();
                        } else {
                            *entry.get_mut() -= 1;
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(_) => {
                        debug_assert!(false, "untracked voxel page removed");
                    }
                }
            });
        }
    }

    pub(super) fn clear(&mut self) {
        self.users.clear();
        self.ungenerated_quads = 0;
        self.generated_meshes = 0;
        self.ordinary_meshes = 0;
    }

    /// All currently live GPU MeshResources are generated. This property
    /// can be queried on every Section Delta without scanning the mesh map.
    pub(super) fn only_generated_meshes(&self) -> bool {
        self.generated_meshes > 0 && self.ordinary_meshes == 0
    }

    /// Read-only O(1) grouping decision, even for 100K+ resident Meshes.
    pub(super) fn has_non_generated_quads(&self) -> bool {
        self.ungenerated_quads != 0
    }

    pub(super) fn active_pages(&self) -> HashSet<BufferId> {
        self.users.keys().copied().collect()
    }

    /// A physical page becomes orphaned only if every live compact-Mesh
    /// reference belongs to the removed IDs. Duplicate IDs are ignored.
    pub(super) fn orphaned(
        &self,
        removed_ids: &[MeshId],
        resident: &HashMap<(MeshId, bool), MeshResources>,
    ) -> HashSet<BufferId> {
        let mut removed_uses = HashMap::<BufferId, usize>::new();
        let mut seen = HashSet::with_capacity(removed_ids.len());
        for &id in removed_ids {
            if !seen.insert(id) {
                continue;
            }
            if let Some(mesh) = resident.get(&(id, true)) {
                Self::visit(mesh, |page| {
                    *removed_uses.entry(page).or_default() += 1;
                });
            }
        }
        removed_uses.into_iter()
            .filter_map(|(page, count)| {
                (self.users.get(&page).copied() == Some(count)).then_some(page)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::IndexFormat;

    fn mesh(page: u32) -> MeshResources {
        let buffer = BufferId::from_parts(page, 1);
        MeshResources {
            generated: None,
            generation: 1,
            vertices: buffer,
            vertex_binding_bytes: 16,
            vertex_stride: 16,
            vertex_base: 0,
            tangents: None,
            tangent_bytes: 0,
            indices: buffer,
            index_format: IndexFormat::Uint16,
            index_offset: 0,
            procedural_quads: true,
            allocation: None,
        }
    }

    #[test]
    fn retiring_one_section_does_not_retire_a_shared_page() {
        let mut refs = PageRefs::default();
        let mut meshes = HashMap::new();
        for (id, page) in [(1, 7), (2, 7), (3, 9)] {
            let value = mesh(page);
            refs.insert(&value, true);
            meshes.insert((MeshId(id), true), value);
        }
        assert!(refs.orphaned(&[MeshId(1)], &meshes).is_empty());
        assert_eq!(
            refs.orphaned(&[MeshId(1), MeshId(2)], &meshes),
            HashSet::from([BufferId::from_parts(7, 1)])
        );
        assert!(refs.orphaned(&[MeshId(1), MeshId(1)], &meshes).is_empty());
        let removed = meshes.remove(&(MeshId(1), true)).unwrap();
        refs.remove(&removed, true);
        assert_eq!(
            refs.orphaned(&[MeshId(2)], &meshes),
            HashSet::from([BufferId::from_parts(7, 1)])
        );
        assert_eq!(refs.active_pages().len(), 2);
    }

    #[test]
    fn quad_grouping_tracks_upload_replacement_and_last_retirement() {
        let mut refs = PageRefs::default();
        let a = mesh(5);
        let b = mesh(6);
        assert!(!refs.has_non_generated_quads());
        refs.insert(&a, true);
        refs.insert(&b, true);
        assert!(refs.has_non_generated_quads());
        refs.remove(&a, true);
        assert!(refs.has_non_generated_quads());
        refs.remove(&b, true);
        assert!(!refs.has_non_generated_quads());
        refs.insert(&a, false);
        assert!(refs.has_non_generated_quads());
        refs.clear();
        assert!(!refs.has_non_generated_quads());
    }

    #[test]
    fn conservative_generated_only_proof_falls_back_on_any_other_mesh() {
        let mut refs = PageRefs::default();
        assert!(!refs.only_generated_meshes());
        let mut generated = mesh(8);
        generated.generated = Some(crate::scene_view::generated::GeneratedMesh {
            templates: BufferId::from_parts(9, 1),
            arguments: BufferId::from_parts(10, 1),
            template_bytes: 0,
            template_allocation: None,
            template_base: 0,
            template_offset: 0,
            template_page_bytes: 0,
            argument_allocation: None,
            argument_base: 0,
            argument_offset: 0,
            argument_page_bytes: 0,
        });
        refs.insert(&generated, true);
        assert!(refs.only_generated_meshes());
        let plain = mesh(11);
        refs.insert(&plain, false);
        assert!(!refs.only_generated_meshes());
        refs.remove(&plain, false);
        assert!(refs.only_generated_meshes());
        refs.remove(&generated, true);
        assert!(!refs.only_generated_meshes());
    }

    #[test]
    fn ordinary_mesh_does_not_keep_voxel_pages_alive() {
        let mut refs = PageRefs::default();
        let resource = mesh(5);
        refs.insert(&resource, false);
        assert!(refs.active_pages().is_empty());
        refs.remove(&resource, false);
        assert!(refs.active_pages().is_empty());
    }
}
