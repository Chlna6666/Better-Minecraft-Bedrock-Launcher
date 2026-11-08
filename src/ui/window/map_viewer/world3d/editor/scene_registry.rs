//! Stable chunk node identities across immutable scene forks.
use super::*;
use gpui_3d::{MeshId, NodeId, SceneError};
use std::collections::VecDeque;

/// LRU bound for retained fused Section-cluster meshes. One entry holds one
/// fused voxel plan (~tens of KiB per 64-block band), so the cap bounds the
/// retained CPU footprint while covering the local navigation window.
const FUSED_CACHE_CAPACITY: usize = 1024;
/// Memo cap for per-source-mesh content fingerprints; cleared wholesale when
/// exceeded because each entry is recomputed in one pass.
const MESH_FINGERPRINT_MEMO_CAPACITY: usize = 32_768;

struct FusedCacheEntry {
    fingerprint: u64,
    mesh: Arc<World3dChunkMesh>,
}

#[derive(Default)]
pub(in super::super) struct ChunkScene {
    lights: BTreeMap<(ChunkPos, [i32; 4]), (NodeId, super::super::types::World3dLocalLight)>,
    nodes: BTreeMap<(ChunkPos, i32), (NodeId, Arc<World3dChunkMesh>)>,
    // Up to four adjacent generated Sections share a material-part draw
    // source. Original Section nodes remain stable occluder/streaming owners.
    fused: BTreeMap<(ChunkPos, i32), (NodeId, Vec<Arc<World3dChunkMesh>>)>,
    // Retained fused meshes keyed by fusion group: a view return or registry
    // rebuild with unchanged Section content reuses the merged mesh instead of
    // re-fusing it through the per-Section mesh cache.
    fused_cache: HashMap<(ChunkPos, i32), FusedCacheEntry>,
    fused_cache_order: VecDeque<(ChunkPos, i32)>,
    // Content fingerprints memoized by (MeshId, generation). Section meshes are
    // immutable, but a re-mesh legitimately reuses the deterministic per-Section
    // MeshId, so the generation is what identifies one immutable mesh object.
    mesh_fingerprints: HashMap<(MeshId, u64), u64>,
    // Last origin used to build `nodes`/`fused` and `lights` translations.
    // Section-delta updates skip unchanged nodes, so an origin change rebases
    // their translations instead of rebuilding the registry.
    origin: [i32; 3],
    lights_origin: [i32; 3],
    last_fusion_trace: Option<Instant>,
}

#[cfg(test)]
mod tests {
    use super::{Arc, BTreeMap, BTreeSet, ChunkPos, ChunkScene, Dimension, World3dChunkMesh};
    use gpui_3d::Mesh;
    fn generated_chunk(x: i32, material: Arc<gpui_3d::Material>) -> Arc<World3dChunkMesh> {
        let cube = Mesh::cube();
        let templates = cube
            .vertices()
            .chunks_exact(4)
            .map(|corners| gpui_3d::VoxelFaceTemplate {
                vertices: std::array::from_fn(|index| {
                    let mut vertex = corners[index];
                    vertex.position = vertex.position + gpui_3d::Vec3::new(0.5, 0.5, 0.5);
                    vertex
                }),
                material_slot: 0,
                neighbour: [
                    corners[0].normal.x as i32,
                    corners[0].normal.y as i32,
                    corners[0].normal.z as i32,
                ],
            })
            .collect::<Vec<_>>();
        let mut cells = vec![0u32; 4 * 3 * 3];
        cells[17] = 1;
        let source = gpui_3d::VoxelMeshSource::new(
            [4, 3, 3],
            gpui_3d::Vec3::new(-1.0, -1.0, -1.0),
            cells,
            templates,
        )
        .unwrap();
        let pos = ChunkPos {
            x,
            z: 0,
            dimension: Dimension::Overworld,
        };
        Arc::new(World3dChunkMesh {
            occluders: Arc::from([]),
            pos,
            section_y: 0,
            origin: [x * 16, 0, 0],
            mesh: Arc::new(Mesh::from_gpu_voxels(source)),
            materials: Arc::from([material]),
            textures: Arc::from([]),
            triangles: 12,
            faces: 6,
            missing_texture_faces: 0,
        })
    }

    /// A Section with the same placement and materials as `section` but
    /// different occupied cells, standing in for a world-edit re-mesh. The
    /// worker reuses the deterministic per-Section MeshId and bumps the
    /// generation for each re-mesh, so the identity must match.
    fn edited_chunk(section: Arc<World3dChunkMesh>) -> Arc<World3dChunkMesh> {
        let mut cells = vec![0u32; 4 * 3 * 3];
        cells[17] = 0;
        cells[18] = 1;
        remeshed(
            &section,
            cells,
            section.mesh.generation() + 1,
            section.materials[0].clone(),
        )
    }

    /// Same deterministic MeshId and placement as `section`, but a new
    /// generation and new cells — one worker re-mesh of the same Section.
    fn remeshed(
        section: &Arc<World3dChunkMesh>,
        cells: Vec<u32>,
        generation: u64,
        material: Arc<gpui_3d::Material>,
    ) -> Arc<World3dChunkMesh> {
        let source = gpui_3d::VoxelMeshSource::new(
            [4, 3, 3],
            gpui_3d::Vec3::new(-1.0, -1.0, -1.0),
            cells,
            source_templates(),
        )
        .unwrap();
        let mesh = Mesh::from_gpu_voxels(source).with_identity(section.mesh.id(), generation);
        Arc::new(World3dChunkMesh {
            mesh: Arc::new(mesh),
            materials: Arc::from([material]),
            ..section.as_ref().clone()
        })
    }

    /// One interior cell filled — the same geometry [`generated_chunk`] builds.
    fn interior_cells() -> Vec<u32> {
        let mut cells = vec![0u32; 4 * 3 * 3];
        cells[17] = 1;
        cells
    }

    /// An unlit material bound to one albedo texture asset id.
    fn tinted_material(albedo: u64) -> Arc<gpui_3d::Material> {
        let mut material = gpui_3d::Material::new();
        material.shading_model = gpui_3d::ShadingModel::Unlit;
        material.albedo_texture = Some(gpui_3d::TextureAssetId(albedo));
        Arc::new(material)
    }

    fn source_templates() -> Vec<gpui_3d::VoxelFaceTemplate> {
        let cube = Mesh::cube();
        cube.vertices()
            .chunks_exact(4)
            .map(|corners| gpui_3d::VoxelFaceTemplate {
                vertices: std::array::from_fn(|index| {
                    let mut vertex = corners[index];
                    vertex.position = vertex.position + gpui_3d::Vec3::new(0.5, 0.5, 0.5);
                    vertex
                }),
                material_slot: 0,
                neighbour: [
                    corners[0].normal.x as i32,
                    corners[0].normal.y as i32,
                    corners[0].normal.z as i32,
                ],
            })
            .collect()
    }

    #[test]
    fn visibility_hides_fused_group_only_when_all_source_sections_are_hidden() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material);
        let left_key = (left.pos, left.section_y);
        let right_key = (right.pos, right.section_y);
        let meshes = BTreeMap::from([(left_key, left), (right_key, right)]);
        let mut registry = ChunkScene::default();
        let mut scene = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused.len(), 1);
        let fused_id = registry.fused.values().next().unwrap().0;
        registry
            .apply_visibility(&mut scene, &BTreeSet::from([left_key]))
            .unwrap();
        assert!(!scene.node(registry.nodes[&left_key].0).unwrap().visible);
        assert!(scene.node(registry.nodes[&right_key].0).unwrap().visible);
        assert!(scene.node(fused_id).unwrap().visible);
        registry
            .apply_visibility(&mut scene, &BTreeSet::from([left_key, right_key]))
            .unwrap();
        assert!(!scene.node(fused_id).unwrap().visible);
        registry.apply_visibility(&mut scene, &BTreeSet::new()).unwrap();
        assert!(scene.node(fused_id).unwrap().visible);
        assert!(registry.nodes.values().all(|(id, _)| scene.node(*id).unwrap().visible));
    }

    #[test]
    fn adjacent_generated_sections_render_as_one_mesh_part_and_restore_on_removal() {
        let mut material = gpui_3d::Material::new();
        material.shading_model = gpui_3d::ShadingModel::Unlit;
        let material = Arc::new(material);
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material);
        let first = (left.pos, 0);
        let second = (right.pos, 0);
        let mut sections = BTreeMap::from([(first, left.clone()), (second, right.clone())]);
        let mut registry = ChunkScene::default();
        let scene = registry.update(None, &sections, None, [0; 3]).unwrap();
        assert_eq!(
            registry.fused.len(),
            1,
            "neighbouring GPU sources must fuse"
        );
        let id = registry.fused[&first].0;
        assert_eq!(
            scene.node(id).unwrap().mesh.as_ref().unwrap().parts().len(),
            1
        );
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| scene.node(*id).unwrap().mesh.is_none())
        );
        let original_id = registry.nodes[&first].0;

        sections.remove(&second);
        let next = registry
            .update(
                Some(&scene),
                &sections,
                Some(&BTreeSet::from([second])),
                [0; 3],
            )
            .unwrap();
        assert!(registry.fused.is_empty());
        assert!(next.node(id).is_none());
        assert!(
            next.node(original_id).unwrap().mesh.is_some(),
            "removing either peer must restore the remaining original Section mesh"
        );
    }

    #[test]
    fn four_section_tile_preserves_node_ids_and_rebuilds_only_its_own_group() {
        let material = Arc::new(gpui_3d::Material::new());
        let mesh_at = |x, z| {
            let section = generated_chunk(x, material.clone());
            let mut section = (*section).clone();
            section.pos.z = z;
            section.origin[2] = z * 16;
            Arc::new(section)
        };
        let sections = [mesh_at(0, 0), mesh_at(1, 0), mesh_at(0, 1), mesh_at(1, 1)];
        let mut meshes = sections
            .iter()
            .map(|mesh| ((mesh.pos, 0), mesh.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut registry = ChunkScene::default();
        let before = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused.len(), 1);
        let key = super::fusion_key((sections[0].pos, 0));
        let group_id = registry.fused[&key].0;
        assert_eq!(registry.fused[&key].1.len(), 4);
        assert_eq!(
            before
                .node(group_id)
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .parts()
                .len(),
            1,
            "four generated Section materials share one native draw"
        );

        let replacement = edited_chunk(mesh_at(1, 1));
        let changed_key = (replacement.pos, 0);
        meshes.insert(changed_key, replacement);
        let next = registry
            .update(
                Some(&before),
                &meshes,
                Some(&BTreeSet::from([changed_key])),
                [0; 3],
            )
            .unwrap();
        assert_eq!(
            registry.fused[&key].0, group_id,
            "fused Scene node IDs stay stable"
        );
        assert_ne!(
            before.node(group_id).unwrap().mesh.as_ref().unwrap().id(),
            next.node(group_id).unwrap().mesh.as_ref().unwrap().id(),
            "a content-changing Section delta regenerates its owning fused mesh",
        );
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| next.node(*id).unwrap().mesh.is_none())
        );
    }

    #[test]
    fn transparent_member_does_not_disable_opaque_neighbour_fusion() {
        let opaque = Arc::new(gpui_3d::Material::new());
        let mut transparent = gpui_3d::Material::new();
        transparent.alpha_mode = gpui_3d::AlphaMode::Blend;
        let a = generated_chunk(0, opaque.clone());
        let b = generated_chunk(1, opaque.clone());
        let mut c = (*generated_chunk(0, opaque)).clone();
        c.pos.z = 1;
        c.origin[2] = 16;
        let c = Arc::new(c);
        let mut d = (*generated_chunk(1, Arc::new(transparent))).clone();
        d.pos.z = 1;
        d.origin[2] = 16;
        let d = Arc::new(d);
        let key = super::fusion_key((a.pos, 0));
        let mut meshes = [a.clone(), b.clone(), c.clone(), d.clone()]
            .iter()
            .map(|mesh| ((mesh.pos, 0), mesh.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut registry = ChunkScene::default();
        let scene = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused[&key].1.len(), 3);
        let transparent_id = registry.nodes[&(d.pos, 0)].0;
        assert!(scene.node(transparent_id).unwrap().mesh.is_some());

        // Replacing an opaque member with a transparent Section must restore
        // that member's native draw, while the remaining two still fuse.
        let mut replacement = (*generated_chunk(
            1,
            Arc::new({
                let mut m = gpui_3d::Material::new();
                m.alpha_mode = gpui_3d::AlphaMode::Blend;
                m
            }),
        ))
        .clone();
        replacement.pos = b.pos;
        replacement.origin = b.origin;
        let replacement = Arc::new(replacement);
        meshes.insert((b.pos, 0), replacement);
        let after = registry
            .update(
                Some(&scene),
                &meshes,
                Some(&BTreeSet::from([(b.pos, 0)])),
                [0; 3],
            )
            .unwrap();
        assert_eq!(registry.fused[&key].1.len(), 2);
        assert!(
            after
                .node(registry.nodes[&(b.pos, 0)].0)
                .unwrap()
                .mesh
                .is_some()
        );
        assert!(after.node(transparent_id).unwrap().mesh.is_some());
    }

    #[test]
    fn equivalent_gpu_materials_from_different_sections_fuse_without_matching_ids() {
        let one = Arc::new(gpui_3d::Material::new());
        let another = Arc::new(gpui_3d::Material::new());
        assert_ne!(
            one.id(),
            another.id(),
            "workers independently allocate MaterialIds"
        );
        assert!(one.render_content_eq(&another));
        let left = generated_chunk(0, one);
        let right = generated_chunk(1, another);
        let mut registry = ChunkScene::default();
        let scene = registry
            .update(
                None,
                &BTreeMap::from([((left.pos, 0), left), ((right.pos, 0), right)]),
                None,
                [0; 3],
            )
            .unwrap();
        assert_eq!(registry.fused.len(), 1);
        assert_eq!(
            scene
                .node(registry.fused.values().next().unwrap().0)
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .parts()
                .len(),
            1
        );
    }

    #[test]
    fn unrelated_generated_materials_keep_finer_section_culling() {
        // Two material parts remain two even after fusion. Keeping the
        // per-Section GPU culling and draw resources costs less in this case.
        let left = generated_chunk(0, Arc::new(gpui_3d::Material::new()));
        let mut distinct = gpui_3d::Material::new();
        distinct.base_color = [0.25, 0.5, 0.75, 1.0];
        let right = generated_chunk(1, Arc::new(distinct));
        let mut registry = ChunkScene::default();
        let scene = registry
            .update(
                None,
                &BTreeMap::from([((left.pos, 0), left), ((right.pos, 0), right)]),
                None,
                [0; 3],
            )
            .unwrap();
        assert!(registry.fused.is_empty());
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| { scene.node(*id).unwrap().mesh.is_some() })
        );
    }

    #[test]
    fn blended_gpu_section_is_never_fused_across_transparency_order() {
        let plain = Arc::new(gpui_3d::Material::new());
        let mut blend = gpui_3d::Material::new();
        blend.alpha_mode = gpui_3d::AlphaMode::Blend;
        let left = generated_chunk(0, plain);
        let right = generated_chunk(1, Arc::new(blend));
        let mut registry = ChunkScene::default();
        let scene = registry
            .update(
                None,
                &BTreeMap::from([((left.pos, 0), left), ((right.pos, 0), right)]),
                None,
                [0; 3],
            )
            .unwrap();
        assert!(registry.fused.is_empty());
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| scene.node(*id).unwrap().mesh.is_some())
        );
    }

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

    #[test]
    fn origin_change_rebases_translations_without_rebuilding_nodes() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material);
        let first = (left.pos, 0);
        let second = (right.pos, 0);
        let meshes = BTreeMap::from([(first, left.clone()), (second, right.clone())]);
        let mut registry = ChunkScene::default();
        let built = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused.len(), 1);
        let section_id = registry.nodes[&first].0;
        let group_id = registry.fused[&first].0;
        let group_mesh = built.node(group_id).unwrap().mesh.clone().unwrap();

        let rebased = registry
            .update(Some(&built), &meshes, Some(&BTreeSet::new()), [16, 0, 0])
            .unwrap();
        assert_eq!(registry.nodes[&first].0, section_id, "ids survive a rebase");
        assert_eq!(registry.fused[&first].0, group_id);
        let section_node = rebased.node(section_id).unwrap();
        assert_eq!(section_node.transform.translation.x, -16.0);
        let group_node = rebased.node(group_id).unwrap();
        assert_eq!(group_node.transform.translation.x, -16.0);
        assert!(
            Arc::ptr_eq(
                rebased.node(group_id).unwrap().mesh.as_ref().unwrap(),
                &group_mesh
            ),
            "an origin rebase must not regenerate the fused mesh"
        );

        // Moving back through the same pass must keep the fused group alive.
        let restored = registry
            .update(Some(&rebased), &meshes, Some(&BTreeSet::new()), [0; 3])
            .unwrap();
        assert_eq!(
            restored.node(group_id).unwrap().transform.translation.x,
            0.0
        );
    }

    /// Streaming loads the four members of a 2x2 fusion tile across several
    /// frames. A group that flips between "fused" and "independent" while its
    /// members arrive must never expose both the fused node and the member
    /// nodes with visible geometry in the same publication (screen flash).
    #[test]
    fn streaming_member_arrival_never_double_draws_fused_group() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material.clone());
        let third = generated_chunk(0, material.clone());
        let fourth = generated_chunk(1, material.clone());
        let keys: [(ChunkPos, i32); 4] = [
            (left.pos, 0),
            (right.pos, 0),
            (
                ChunkPos { x: 0, z: 1, dimension: Dimension::Overworld },
                0,
            ),
            (
                ChunkPos { x: 1, z: 1, dimension: Dimension::Overworld },
                0,
            ),
        ];
        // Place the later arrivals on the +z row of the same 2x2 tile.
        let mut third = Arc::unwrap_or_clone(third);
        third.pos = keys[2].0;
        let third = Arc::new(third);
        let mut fourth = Arc::unwrap_or_clone(fourth);
        fourth.pos = keys[3].0;
        let fourth = Arc::new(fourth);
        let mut registry = ChunkScene::default();
        let mut scene = registry
            .update(None, &BTreeMap::new(), None, [0; 3])
            .unwrap();
        let mut meshes = BTreeMap::new();
        let sequence = [
            vec![(keys[0], left.clone())],
            vec![(keys[0], left.clone()), (keys[1], right.clone())],
            vec![
                (keys[0], left.clone()), (keys[1], right.clone()),
                (keys[2], third.clone()),
            ],
            vec![
                (keys[0], left.clone()), (keys[1], right.clone()),
                (keys[2], third.clone()), (keys[3], fourth.clone()),
            ],
            vec![
                (keys[0], left.clone()), (keys[1], right.clone()),
                (keys[3], fourth.clone()),
            ],
            vec![(keys[0], left.clone()), (keys[1], right.clone())],
        ];
        for step in sequence {
            // Real unloads are reported in `changed` even though the key is
            // absent from `meshes` (retain_chunk_sections inserts it).
            let mut changed: BTreeSet<_> = step.iter().map(|(key, _)| *key).collect();
            for key in meshes.keys() {
                if !step.iter().any(|(k, _)| k == key) {
                    changed.insert(*key);
                }
            }
            for (key, _) in &step {
                meshes.insert(*key, step.iter().find(|(k, _)| k == key).unwrap().1.clone());
            }
            // Remove keys not present in this publication (a member unloaded).
            meshes.retain(|key, _| step.iter().any(|(k, _)| k == key));
            scene = registry
                .update(Some(&scene), &meshes, Some(&changed), [0; 3])
                .unwrap();
            // Count nodes that carry a visible mesh. Exactly one fused node
            // OR each independent member, never both for the same geometry.
            let visible: Vec<_> = registry
                .nodes
                .values()
                .filter_map(|(id, _)| scene.node(*id))
                .filter(|node| node.mesh.is_some())
                .collect();
            let fused_visible = registry
                .fused
                .values()
                .filter_map(|(id, _)| scene.node(*id))
                .filter(|node| node.mesh.is_some())
                .count();
            let group_fused = !registry.fused.is_empty();
            if group_fused {
                assert_eq!(
                    visible.len(), 0,
                    "members must be hidden while a fused node is visible"
                );
                assert_eq!(fused_visible, 1, "exactly one fused node visible");
            } else {
                assert_eq!(fused_visible, 0);
                assert_eq!(visible.len(), meshes.len(), "each member renders independently");
            }
        }
    }

    /// A previously fused 2x2 group can become ineligible for merging (for
    /// example when a member's materials no longer share GPU state) while
    /// `sources.len() >= 2` still holds. The failed fusion must dissolve the
    /// old fused node; otherwise the old merged geometry and the restored
    /// independent members are both visible and the region flashes.
    #[test]
    fn failed_fusion_dissolves_old_group_instead_of_double_drawing() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material.clone());
        let keys = [(left.pos, 0), (right.pos, 0)];
        let mut meshes = BTreeMap::from([
            (keys[0], left.clone()),
            (keys[1], right.clone()),
        ]);
        let mut registry = ChunkScene::default();
        let scene = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused.len(), 1, "two opaque members must fuse");
        let fused_id = registry.fused[&keys[0]].0;
        assert!(scene.node(fused_id).unwrap().mesh.is_some());
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| scene.node(*id).unwrap().mesh.is_none())
        );

        // Give one member a distinct texture binding so the cluster refuses
        // to merge, while both sections remain opaque and eligible.
        let mut changed_material = gpui_3d::Material::new();
        changed_material.shading_model = gpui_3d::ShadingModel::Unlit;
        changed_material.albedo_texture = Some(gpui_3d::TextureAssetId(77));
        let replacement = remeshed(
            &right,
            interior_cells(),
            right.mesh.generation() + 1,
            Arc::new(changed_material),
        );
        meshes.insert(keys[1], replacement);
        let after = registry
            .update(Some(&scene), &meshes, Some(&BTreeSet::from([keys[1]])), [0; 3])
            .unwrap();

        assert!(
            registry.fused.is_empty(),
            "an unmergable group must not keep a fused node"
        );
        assert!(
            after.node(fused_id).is_none(),
            "the old fused node must be removed, not left visible"
        );
        assert_eq!(
            registry
                .nodes
                .values()
                .filter(|(id, _)| after.node(*id).and_then(|n| n.mesh.as_ref()).is_some())
                .count(),
            2,
            "both members must render independently after the dissolve"
        );
    }

    #[test]
    fn fused_cache_reuses_merged_mesh_after_section_recreation() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material.clone());
        let first = (left.pos, 0);
        let second = (right.pos, 0);
        let meshes = BTreeMap::from([(first, left.clone()), (second, right.clone())]);
        let mut registry = ChunkScene::default();
        let built = registry.update(None, &meshes, None, [0; 3]).unwrap();
        let group_id = registry.fused[&first].0;
        let fused_mesh = built.node(group_id).unwrap().mesh.clone().unwrap();

        // Recreate the Section sources with equal content the way a worker
        // re-mesh does: same deterministic MeshId, next generation, fresh mesh
        // object. Identical content must still reuse the cached fusion.
        let rebuilt_left = remeshed(
            &left,
            interior_cells(),
            left.mesh.generation() + 1,
            material.clone(),
        );
        let rebuilt_right = remeshed(
            &right,
            interior_cells(),
            right.mesh.generation() + 1,
            material.clone(),
        );
        assert_eq!(rebuilt_left.mesh.id(), left.mesh.id());
        let recreated = BTreeMap::from([(first, rebuilt_left), (second, rebuilt_right)]);
        let next = registry
            .update(
                Some(&built),
                &recreated,
                Some(&BTreeSet::from([first, second])),
                [0; 3],
            )
            .unwrap();
        assert_eq!(registry.fused[&first].0, group_id);
        assert!(
            Arc::ptr_eq(
                next.node(group_id).unwrap().mesh.as_ref().unwrap(),
                &fused_mesh
            ),
            "equal Section content must reuse the cached fused mesh"
        );
        assert!(
            registry
                .nodes
                .values()
                .all(|(id, _)| next.node(*id).unwrap().mesh.is_none())
        );
    }

    #[test]
    fn fused_cache_refuses_after_content_change() {
        let material = Arc::new(gpui_3d::Material::new());
        let left = generated_chunk(0, material.clone());
        let right = generated_chunk(1, material.clone());
        let first = (left.pos, 0);
        let second = (right.pos, 0);
        let meshes = BTreeMap::from([(first, left.clone()), (second, right.clone())]);
        let mut registry = ChunkScene::default();
        let built = registry.update(None, &meshes, None, [0; 3]).unwrap();
        let group_id = registry.fused[&first].0;
        let fused_mesh = built.node(group_id).unwrap().mesh.clone().unwrap();

        // Rebuild the Section with different occupied cells, mirroring a
        // re-mesh after a world edit.
        let edited_meshes = BTreeMap::from([(first, edited_chunk(left)), (second, right)]);
        let next = registry
            .update(
                Some(&built),
                &edited_meshes,
                Some(&BTreeSet::from([first])),
                [0; 3],
            )
            .unwrap();
        assert_eq!(registry.fused[&first].0, group_id);
        assert!(
            !Arc::ptr_eq(
                next.node(group_id).unwrap().mesh.as_ref().unwrap(),
                &fused_mesh
            ),
            "changed Section content must re-fuse instead of reusing the cache"
        );
    }

    #[test]
    fn fingerprint_memo_distinguishes_remeshes_of_one_section() {
        let mut registry = ChunkScene::default();
        let original = generated_chunk(0, tinted_material(400));
        let before = registry.member_fingerprint(&original);

        // A biome-tint change re-meshes the same Section with the same
        // deterministic MeshId but freshly derived texture asset ids.
        let retinted = remeshed(
            &original,
            interior_cells(),
            original.mesh.generation() + 1,
            tinted_material(500),
        );
        assert_eq!(retinted.mesh.id(), original.mesh.id());
        assert_ne!(
            registry.member_fingerprint(&retinted),
            before,
            "the fingerprint memo must not freeze a Section at its first mesh"
        );
    }

    #[test]
    fn retinted_remesh_never_reuses_a_fusion_with_dead_texture_ids() {
        let left = generated_chunk(0, tinted_material(400));
        let right = generated_chunk(1, tinted_material(400));
        let first = (left.pos, 0);
        let second = (right.pos, 0);
        let meshes = BTreeMap::from([(first, left.clone()), (second, right.clone())]);
        let mut registry = ChunkScene::default();
        let built = registry.update(None, &meshes, None, [0; 3]).unwrap();
        assert_eq!(registry.fused.len(), 1, "identical materials must fuse");
        let group_id = registry.fused[&first].0;
        let fused_mesh = built.node(group_id).unwrap().mesh.clone().unwrap();

        // Every member re-meshes with a new derived albedo (new texture asset
        // ids) after the previous ids stopped being referenced by resident
        // Sections. Geometry is unchanged, so only the material differs.
        let retinted = BTreeMap::from([
            (
                first,
                remeshed(
                    &left,
                    interior_cells(),
                    left.mesh.generation() + 1,
                    tinted_material(500),
                ),
            ),
            (
                second,
                remeshed(
                    &right,
                    interior_cells(),
                    right.mesh.generation() + 1,
                    tinted_material(500),
                ),
            ),
        ]);
        let next = registry
            .update(
                Some(&built),
                &retinted,
                Some(&BTreeSet::from([first, second])),
                [0; 3],
            )
            .unwrap();
        let fused = next.node(group_id).unwrap().mesh.clone().unwrap();
        assert!(
            !Arc::ptr_eq(&fused, &fused_mesh),
            "a re-mesh with new texture asset ids must re-fuse; reusing the cached \
             mesh would render materials whose textures are no longer published"
        );
        for material in next.node(group_id).unwrap().materials.iter() {
            assert_eq!(
                material.albedo_texture,
                Some(gpui_3d::TextureAssetId(500)),
                "the fused node must reference the live texture asset ids"
            );
        }
    }
}

impl ChunkScene {
    pub(in super::super) fn update(
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
            self.origin = origin;
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
            self.rebase_scene(&mut scene, origin)?;
            self.report_fusion(&scene);
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
        let all_groups = meshes
            .keys()
            .copied()
            .map(fusion_key)
            .chain(self.fused.keys().copied())
            .collect::<BTreeSet<_>>();
        for key in all_groups {
            self.sync_fusion(&mut scene, meshes, key, origin)?;
        }
        self.rebase_scene(&mut scene, origin)?;
        self.report_fusion(&scene);
        Ok(scene)
    }

    /// Shifts unchanged node translations after a render-origin change.
    /// `update_mesh`/`sync_fusion` already write the new origin into rebuilt
    /// nodes; this pass covers nodes whose Section meshes were unchanged, so a
    /// camera crossing the origin grid no longer forces a full fusion rebuild.
    fn rebase_scene(&mut self, scene: &mut Scene, origin: [i32; 3]) -> Result<(), SceneError> {
        if self.origin == origin {
            return Ok(());
        }
        for (id, mesh) in self.nodes.values() {
            let node = scene.node_mut(*id).ok_or(SceneError::StaleNode)?;
            node.transform.translation = Vec3::new(
                (mesh.origin[0] - origin[0]) as f32,
                mesh.origin[1] as f32,
                (mesh.origin[2] - origin[2]) as f32,
            );
        }
        for (id, sources) in self.fused.values() {
            let Some(first) = sources.first() else {
                continue;
            };
            let node = scene.node_mut(*id).ok_or(SceneError::StaleNode)?;
            node.transform.translation = Vec3::new(
                (first.origin[0] - origin[0]) as f32,
                first.origin[1] as f32,
                (first.origin[2] - origin[2]) as f32,
            );
        }
        self.origin = origin;
        Ok(())
    }

    fn report_fusion(&mut self, scene: &Scene) {
        if std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_none() {
            return;
        }
        let now = Instant::now();
        if self
            .last_fusion_trace
            .is_some_and(|previous| now.duration_since(previous) < Duration::from_secs(1))
        {
            return;
        }
        self.last_fusion_trace = Some(now);
        let mut source_sections = 0usize;
        let mut source_draws = 0usize;
        let mut fused_draws = 0usize;
        for (id, sources) in self.fused.values() {
            source_sections += sources.len();
            source_draws += sources
                .iter()
                .map(|mesh| mesh.mesh.parts().len())
                .sum::<usize>();
            fused_draws += scene
                .node(*id)
                .and_then(|node| node.mesh.as_ref())
                .map_or(0, |mesh| mesh.parts().len());
        }
        let saved = source_draws.saturating_sub(fused_draws);
        eprintln!(
            "world3d fusion_stats groups={} source_sections={} original_draws={} fused_draws={} eliminated_draws={} remaining_section_nodes={}",
            self.fused.len(),
            source_sections,
            source_draws,
            fused_draws,
            saved,
            self.nodes.len(),
        );
    }

    fn sync_fusion(
        &mut self,
        scene: &mut Scene,
        meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
        key: (ChunkPos, i32),
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        // A 2x2 tile bounds rebuild work to at most four Sections even when
        // the map contains thousands of resident immutable Scene nodes.
        let members = fusion_members(key);
        // A single water/glass Section must not disable merging of its
        // three opaque neighbours. Ineligible members stay independent and
        // retain their own sorted blended draws.
        let sources = members
            .into_iter()
            .filter_map(|member| meshes.get(&member).cloned())
            .filter(|source| {
                source.mesh.gpu_voxels().is_some()
                    && source
                        .materials
                        .iter()
                        .all(|material| material.alpha_mode != gpui_3d::AlphaMode::Blend)
            })
            .collect::<Vec<_>>();
        let compatible = sources.len() >= 2;
        if compatible {
            if self.fused.get(&key).is_some_and(|(_, prior)| {
                sources.len() == prior.len()
                    && sources.iter().zip(prior).all(|(a, b)| Arc::ptr_eq(a, b))
            }) {
                return Ok(());
            }
            // Fingerprints are only paid after the pointer-identity fast path
            // misses; a matching cache entry skips the full cluster re-fusion.
            let fingerprint = self.group_fingerprint(&sources);
            let fused: Option<Arc<World3dChunkMesh>> = match self.fused_cache.get(&key) {
                Some(entry) if entry.fingerprint == fingerprint => Some(entry.mesh.clone()),
                _ => fused_section_cluster(&sources).map(Arc::new),
            };
            if fused.is_none() {
                // A cluster can become ineligible after a member changes
                // (e.g. a material mix that no longer merges). Dissolve the
                // group instead of leaving the previous fused node visible
                // next to newly independent member geometry.
                if let Some((id, _)) = self.fused.remove(&key) {
                    scene.remove(id)?;
                    for member in members {
                        if let Some((id, mesh)) = self.nodes.get(&member) {
                            *scene.node_mut(*id).ok_or(SceneError::StaleNode)? =
                                chunk_node(mesh, origin);
                        }
                    }
                }
                return Ok(());
            }
            let fused = fused.expect("checked above");
            {
                self.remember_fused(key, fingerprint, Arc::clone(&fused));
                let node = fused_render_node(&fused, origin);
                let id = if let Some((id, _)) = self.fused.get(&key) {
                    *scene.node_mut(*id).ok_or(SceneError::StaleNode)? = node;
                    *id
                } else {
                    scene.insert(None, node)?
                };
                for member in members {
                    if let Some((id, original)) = self.nodes.get(&member) {
                        let node = scene.node_mut(*id).ok_or(SceneError::StaleNode)?;
                        if sources.iter().any(|source| Arc::ptr_eq(source, original)) {
                            // Eligible sources render from the shared Generated
                            // Mesh; their stable original nodes still own
                            // occluders and editing identity.
                            node.mesh = None;
                        } else {
                            // A Section can leave the group on replacement
                            // (e.g. water added). Restore its independent
                            // mesh even when the remainder still fuses.
                            *node = chunk_node(original, origin);
                        }
                    }
                }
                self.fused.insert(key, (id, sources));
                return Ok(());
            }
        }
        if let Some((id, _)) = self.fused.remove(&key) {
            scene.remove(id)?;
            // Recover independent Section geometry on transparent material
            // changes, removals, and unsupported group combinations.
            for member in members {
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

    /// Apply conservative Section visibility without recreating GPU meshes or
    /// changing stable NodeIds. The authored source nodes retain their mesh,
    /// materials and per-Section occluders; a hidden source cannot occlude
    /// another visible Section. Fused 2x2 meshes are only hidden when ALL
    /// contributing Sections are hidden, so one visible cave entrance never
    /// causes a different member's exterior to disappear.
    pub(in super::super) fn apply_visibility(
        &self,
        scene: &mut Scene,
        hidden: &BTreeSet<(ChunkPos, i32)>,
    ) -> Result<(), SceneError> {
        for (key, (id, _)) in &self.nodes {
            let visible = !hidden.contains(key);
            if scene.node(*id).ok_or(SceneError::StaleNode)?.visible != visible {
                scene.node_mut(*id).ok_or(SceneError::StaleNode)?.visible = visible;
            }
        }
        for (id, members) in self.fused.values() {
            let visible = members
                .iter()
                .any(|member| !hidden.contains(&(member.pos, member.section_y)));
            if scene.node(*id).ok_or(SceneError::StaleNode)?.visible != visible {
                scene.node_mut(*id).ok_or(SceneError::StaleNode)?.visible = visible;
            }
        }
        Ok(())
    }

    pub(in super::super) fn update_lights(
        &mut self,
        scene: &mut Scene,
        lights: &BTreeMap<ChunkPos, Arc<[super::super::types::World3dLocalLight]>>,
        changed: Option<&BTreeSet<ChunkPos>>,
        origin: [i32; 3],
    ) -> Result<(), SceneError> {
        if self.lights_origin != origin && !self.lights.is_empty() {
            // An origin change only shifts light translations; unchanged light
            // nodes keep their ids and are rewritten in place.
            for ((pos, _), (id, _)) in &self.lights {
                let node = scene.node_mut(*id).ok_or(SceneError::StaleNode)?;
                node.transform.translation = Vec3::new(
                    (pos.x * 16 - origin[0]) as f32,
                    -origin[1] as f32,
                    (pos.z * 16 - origin[2]) as f32,
                );
            }
        }
        self.lights_origin = origin;
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

    /// Content fingerprint of one fused group: member count and order, each
    /// member's placement, then each member's memoized mesh content fingerprint.
    /// Only Section sources with identical fingerprints can reuse a cached
    /// fused mesh.
    fn group_fingerprint(&mut self, sources: &[Arc<World3dChunkMesh>]) -> u64 {
        let mut combined = sources.len() as u64;
        for source in sources {
            // Mix the placement: a group that lost one member and gained
            // another must never match the cached subset it replaced. The
            // group key already partitions by dimension.
            for value in [
                source.pos.x as u64,
                source.pos.z as u64,
                source.section_y as u64,
            ] {
                combined = (combined ^ value).wrapping_mul(0x0000_0100_0000_01b3);
            }
            combined =
                combined.wrapping_mul(0x0000_0100_0000_01b3) ^ self.member_fingerprint(source);
        }
        combined
    }

    /// Memoized content fingerprint of one Section source. Section meshes are
    /// immutable for one `(MeshId, generation)` pair, so each mesh object is
    /// hashed at most once per registry lifetime (the memo is wholesale-cleared
    /// at capacity). The deterministic per-Section MeshId is intentionally
    /// reused by later re-meshes of the same Section, so keying on the MeshId
    /// alone would freeze the fingerprint of a Section after its first mesh and
    /// let a stale fused mesh (old geometry, old material texture IDs) be
    /// resurrected.
    fn member_fingerprint(&mut self, section: &World3dChunkMesh) -> u64 {
        let mesh = section.mesh.as_ref();
        let key = (mesh.id(), mesh.generation());
        if let Some(cached) = self.mesh_fingerprints.get(&key) {
            return *cached;
        }
        let mut combined = mesh
            .gpu_voxels()
            .map_or(0, |source| source.content_fingerprint());
        combined = combined
            .wrapping_mul(0x0000_0100_0000_01b3)
            .wrapping_add(section.materials.len() as u64);
        for material in section.materials.iter() {
            combined =
                (combined ^ material.content_fingerprint()).wrapping_mul(0x0000_0100_0000_01b3);
        }
        combined =
            (combined ^ section.missing_texture_faces as u64).wrapping_mul(0x0000_0100_0000_01b3);
        if self.mesh_fingerprints.len() >= MESH_FINGERPRINT_MEMO_CAPACITY {
            self.mesh_fingerprints.clear();
        }
        self.mesh_fingerprints.insert(key, combined);
        combined
    }

    /// Retains a fused mesh under its group key with an LRU bound, so view
    /// returns and registry rebuilds reuse the merged result.
    fn remember_fused(
        &mut self,
        key: (ChunkPos, i32),
        fingerprint: u64,
        mesh: Arc<World3dChunkMesh>,
    ) {
        if let Some(entry) = self.fused_cache.get_mut(&key) {
            if entry.fingerprint == fingerprint {
                entry.mesh = mesh;
                return;
            }
        } else {
            self.fused_cache_order.push_back(key);
        }
        self.fused_cache
            .insert(key, FusedCacheEntry { fingerprint, mesh });
        while self.fused_cache.len() > FUSED_CACHE_CAPACITY {
            let Some(oldest) = self.fused_cache_order.pop_front() else {
                break;
            };
            self.fused_cache.remove(&oldest);
        }
    }
}

// A stable 2x2 key bounds updates to the owning tile and preserves correct
// grouping for negative chunk coordinates.
fn fusion_key((pos, section_y): (ChunkPos, i32)) -> (ChunkPos, i32) {
    (
        ChunkPos {
            x: pos.x.div_euclid(2) * 2,
            z: pos.z.div_euclid(2) * 2,
            ..pos
        },
        section_y,
    )
}

fn fusion_members((pos, section_y): (ChunkPos, i32)) -> [(ChunkPos, i32); 4] {
    [
        (pos, section_y),
        (
            ChunkPos {
                x: pos.x + 1,
                ..pos
            },
            section_y,
        ),
        (
            ChunkPos {
                z: pos.z + 1,
                ..pos
            },
            section_y,
        ),
        (
            ChunkPos {
                x: pos.x + 1,
                z: pos.z + 1,
                ..pos
            },
            section_y,
        ),
    ]
}

fn fused_section_cluster(sources: &[Arc<World3dChunkMesh>]) -> Option<World3dChunkMesh> {
    let first = sources.first()?;
    let mut materials = Vec::<Arc<gpui_3d::Material>>::new();
    // Each streamed Section may create new MaterialIds for physically
    // identical shader state. Merge equal immutable GPU material snapshots
    // even across different IDs, but never merge unlike colors, blend modes
    // or texture bindings. Partition by the four texture resources first to
    // avoid a quadratic scan across all materials in large clusters.
    let mut index = HashMap::<
        (
            Option<gpui_3d::TextureAssetId>,
            Option<gpui_3d::TextureAssetId>,
            Option<gpui_3d::TextureAssetId>,
            Option<gpui_3d::TextureAssetId>,
        ),
        Vec<u32>,
    >::new();
    let mut mappings = vec![Vec::new(); sources.len()];
    for (section, mapping) in sources.iter().zip(mappings.iter_mut()) {
        for material in section.materials.iter() {
            let key = (
                material.albedo_texture,
                material.normal_texture,
                material.occlusion_texture,
                material.metallic_roughness_texture,
            );
            let previous = index.get(&key).and_then(|slots| {
                slots
                    .iter()
                    .copied()
                    .find(|&slot| materials[slot as usize].render_content_eq(material))
            });
            let slot = if let Some(slot) = previous {
                slot
            } else {
                let slot = u32::try_from(materials.len()).ok()?;
                index.entry(key).or_default().push(slot);
                materials.push(material.clone());
                slot
            };
            mapping.push(slot);
        }
    }
    // Don't substitute a coarser 2x2 culling bound and incur a larger
    // generated mesh when material-part coalescing cannot eliminate any
    // native draw. Count only used material slots, not unused source assets.
    let original_parts = sources.iter().map(|s| s.mesh.parts().len()).sum::<usize>();
    let mut shared_parts = HashSet::<u32>::new();
    for (section, mapping) in sources.iter().zip(&mappings) {
        for part in section.mesh.parts() {
            shared_parts.insert(*mapping.get(part.material_slot() as usize)?);
        }
    }
    if shared_parts.len() >= original_parts {
        return None;
    }
    let placed = sources
        .iter()
        .map(|section| {
            let source = section.mesh.gpu_voxels()?;
            let delta = [
                section.origin[0].checked_sub(first.origin[0])?,
                section.origin[1].checked_sub(first.origin[1])?,
                section.origin[2].checked_sub(first.origin[2])?,
            ];
            let grid =
                source.origin() + Vec3::new(delta[0] as f32, delta[1] as f32, delta[2] as f32);
            Some((delta, grid))
        })
        .collect::<Option<Vec<_>>>()?;
    let grid_origin = placed
        .iter()
        .fold(placed[0].1, |minimum, (_, point)| minimum.min(*point));
    let offsets = placed
        .iter()
        .map(|(_, grid)| {
            let relative = *grid - grid_origin;
            let mut result = [0u32; 3];
            for (axis, value) in [relative.x, relative.y, relative.z].into_iter().enumerate() {
                if !value.is_finite() || value < 0.0 || (value - value.round()).abs() > 0.0001 {
                    return None;
                }
                result[axis] = value as u32;
            }
            Some(result)
        })
        .collect::<Option<Vec<_>>>()?;
    let inputs = sources
        .iter()
        .zip(placed.iter())
        .zip(offsets.iter())
        .zip(mappings.iter())
        .map(|(((section, (translation, _)), &offset), mapping)| {
            Some(gpui_3d::VoxelFusionInput {
                source: section.mesh.gpu_voxels()?.as_ref(),
                offset,
                translation: *translation,
                material_slots: mapping,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let fused_source = gpui_3d::VoxelMeshSource::fuse_sections(&inputs).ok()?;
    let mut textures = BTreeMap::<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>::new();
    for section in sources {
        for texture in section.textures.iter() {
            textures
                .entry(texture.id())
                .or_insert_with(|| texture.clone());
        }
    }
    let mesh = Arc::new(gpui_3d::Mesh::from_gpu_voxels(fused_source));
    let faces = (mesh.index_count() / 6) as usize;
    Some(World3dChunkMesh {
        occluders: Arc::from([]),
        pos: first.pos,
        section_y: first.section_y,
        origin: first.origin,
        mesh,
        materials: Arc::from(materials),
        textures: Arc::from(textures.into_values().collect::<Vec<_>>()),
        faces,
        triangles: faces * 2,
        missing_texture_faces: sources
            .iter()
            .map(|section| section.missing_texture_faces)
            .sum(),
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
