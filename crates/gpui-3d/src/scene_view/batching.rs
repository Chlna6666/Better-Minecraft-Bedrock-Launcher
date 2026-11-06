//! Coalesces resident opaque quad ranges without changing blended draw order.

use super::{
    gpu::MeshResources,
    scene::{DrawBatchRange, mesh_key},
};
use crate::{AlphaMode, MeshId, PreparedDraw};
use std::collections::HashMap;

// Quad batches use the exact GPU binding + raster identity, not MaterialId;
// their material constants are fetched from independent DrawParams slots.
// Generated/regular meshes cannot share a single native binding and remain
// singleton draws. Keep their key compact and in the original sort order:
// computing four texture IDs for every generated Section would be a regression.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum BatchKey {
    Quad {
        textures: [Option<u64>; 4],
        page: u64,
        size_class: u32,
        raster: u8,
    },
    Singleton {
        material: u64,
        mesh: u64,
        size_class: u32,
        mirrored: bool,
    },
}

/// Persistent scratch for streaming batch updates. Group by one computed key
/// per draw, retain previously sorted distinct keys, and sort only newly
/// encountered keys before merging them into the ordered key set.
///
/// The index lists are cleared (not deallocated) between publications. They
/// preserve the original input order for equal keys, matching stable sorting.
#[derive(Default)]
pub(super) struct BatchWorkspace {
    buckets: HashMap<BatchKey, Vec<usize>>,
    sorted_keys: Vec<BatchKey>,
    new_keys: Vec<BatchKey>,
    spare_keys: Vec<BatchKey>,
    order: Vec<usize>,
    positions: Vec<usize>,
}

#[cfg(test)]
pub(super) fn group_voxel_batches(
    draws: &mut [PreparedDraw],
    meshes: &HashMap<(MeshId, bool), MeshResources>,
) -> Vec<DrawBatchRange> {
    let mut workspace = BatchWorkspace::default();
    let mut batches = Vec::new();
    group_voxel_batches_into(draws, meshes, &mut workspace, &mut batches);
    batches
}

/// Rebuild only the distinct-key ordering on scene deltas instead of sorting
/// every opaque draw. Group compatible quad resources by their actual GPU
/// texture bindings and raster state, not by their logical MaterialId.
/// Opaque and masked draws keep distinct per-instance material parameters.
pub(super) fn group_voxel_batches_into(
    draws: &mut [PreparedDraw],
    meshes: &HashMap<(MeshId, bool), MeshResources>,
    workspace: &mut BatchWorkspace,
    batches: &mut Vec<DrawBatchRange>,
) {
    batches.clear();
    let opaque = draws.partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);

    // Streaming GPU Generated Meshes are singleton draws: their unique template
    // and generated-vertex buffers cannot be shared by instancing. Building a
    // HashMap bucket, sorting tens of thousands of keys, and permuting every
    // resident PreparedDraw adds O(N) CPU work but merges exactly zero draws.
    // Preserve the scene's existing opaque order; opaque depth semantics do
    // not require material sorting, and the transparent tail remains untouched.
    // One scan over resident mesh resources is much cheaper than the old
    // per-draw hashing, distinct-key sorting and inverse permutation.
    if !meshes.values().any(|mesh| mesh.procedural_quads && mesh.generated.is_none()) {
        workspace.buckets.clear();
        workspace.sorted_keys.clear();
        workspace.new_keys.clear();
        workspace.spare_keys.clear();
        workspace.order.clear();
        batches.reserve(draws.len());
        batches.extend((0..draws.len()).map(|index| DrawBatchRange {
            first_instance: index,
            instance_count: 1,
        }));
        return;
    }

    // Tiny scenes are cheaper to handle directly than through hash buckets.
    // Reset the historical key set when switching back to this path.
    if opaque < 128 {
        workspace.buckets.clear();
        workspace.sorted_keys.clear();
        workspace.new_keys.clear();
        draws[..opaque].sort_by_cached_key(|draw| batch_key(draw, meshes));
        let mut last_key = None;
        for (index, draw) in draws.iter().enumerate() {
            let key = batch_key(draw, meshes);
            if let Some(batch) = batches.last_mut() {
                if draw.material.alpha_mode != AlphaMode::Blend
                    && last_key == Some(key)
                    && matches!(key, BatchKey::Quad { .. })
                {
                    batch.instance_count += 1;
                    continue;
                }
            }
            batches.push(DrawBatchRange {
                first_instance: index,
                instance_count: 1,
            });
            last_key = Some(key);
        }
        return;
    }

    for entries in workspace.buckets.values_mut() {
        entries.clear();
    }
    workspace.new_keys.clear();
    workspace.order.clear();
    workspace.order.reserve(opaque);

    // O(N) distribution. Each section draw computes its resident mesh/page
    // grouping key once rather than once during sort and again during batch
    // coalescing. Retain each equal-key input order unchanged.
    for (index, draw) in draws[..opaque].iter().enumerate() {
        let key = batch_key(draw, meshes);
        match workspace.buckets.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().push(index);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                workspace.new_keys.push(key);
                entry.insert(vec![index]);
            }
        }
    }

    // Old keys were already sorted on the preceding publication. Only sort
    // genuinely new keys, merge them with retained keys, and drop retired
    // keys so stale material/page identities cannot grow the cache forever.
    workspace.new_keys.sort_unstable();
    workspace.buckets.retain(|_, entries| !entries.is_empty());
    let old_keys = std::mem::take(&mut workspace.sorted_keys);
    let mut merged = std::mem::take(&mut workspace.spare_keys);
    merged.clear();
    merged.reserve(old_keys.len().saturating_add(workspace.new_keys.len()));
    let mut fresh = workspace.new_keys.iter().copied().peekable();
    for key in old_keys.iter().copied() {
        if !workspace.buckets.contains_key(&key) {
            continue;
        }
        while fresh.peek().is_some_and(|new| *new < key) {
            merged.push(fresh.next().expect("peeked key is present"));
        }
        merged.push(key);
    }
    merged.extend(fresh);
    workspace.spare_keys = old_keys;
    workspace.sorted_keys = merged;

    let mut last_key = None;
    for &key in &workspace.sorted_keys {
        let indices = workspace
            .buckets
            .get(&key)
            .expect("active batch key must have an index bucket");
        for &source_index in indices {
            let draw = &draws[source_index];
            if let Some(batch) = batches.last_mut() {
                if last_key == Some(key)
                    && matches!(key, BatchKey::Quad { .. })
                {
                    batch.instance_count += 1;
                    workspace.order.push(source_index);
                    continue;
                }
            }
            batches.push(DrawBatchRange {
                first_instance: workspace.order.len(),
                instance_count: 1,
            });
            workspace.order.push(source_index);
            last_key = Some(key);
        }
    }
    debug_assert_eq!(workspace.order.len(), opaque);
    // Material and voxel-page grouping often produces the same ordered draw
    // sequence on consecutive Section publications. Avoid building a second
    // N-entry inverse permutation table and walking N swaps in that case.
    // The comparison is allocation-free and preserves stable sort semantics.
    if workspace.order.iter().enumerate().any(|(index, &source)| index != source) {
        crate::prepare::apply_order(
            &mut draws[..opaque],
            &workspace.order,
            &mut workspace.positions,
        );
    }

    // Transparent draws are already depth sorted. Never group or permute
    // them, even when their material and geometry keys happen to match.
    for index in opaque..draws.len() {
        batches.push(DrawBatchRange {
            first_instance: index,
            instance_count: 1,
        });
    }
}

fn batch_key(
    draw: &PreparedDraw,
    meshes: &HashMap<(MeshId, bool), MeshResources>,
) -> BatchKey {
    let resource = meshes.get(&mesh_key(draw, true));
    let quad = resource.is_some_and(|mesh| mesh.procedural_quads && mesh.generated.is_none());
    // Round up to a power-of-two vertex-count class: padding costs less than 2x vertex
    // invocations per instance instead of a large part forcing every tiny part to its size.
    let size_class = u32::BITS - draw.part.index_count().saturating_sub(1).leading_zeros();
    if !quad {
        return BatchKey::Singleton {
            material: draw.material.id().0,
            mesh: draw.mesh.id().0,
            size_class,
            mirrored: !draw.material.double_sided && draw.is_mirrored(),
        };
    }
    // One canonical raster state: double-sided draws ignore the stored front
    // face, while single-sided draws account for mirrored transforms.
    let raster = match draw.raster_front_face() {
        None => 0u8,
        Some(gfx_core::FrontFace::Ccw) => 1,
        Some(gfx_core::FrontFace::Cw) => 2,
    };
    BatchKey::Quad {
        textures: [
            draw.material.albedo_texture.map(|id| id.0),
            draw.material.normal_texture.map(|id| id.0),
            draw.material.occlusion_texture.map(|id| id.0),
            draw.material.metallic_roughness_texture.map(|id| id.0),
        ],
        page: resource.expect("quad resource was found").vertices.raw(),
        size_class,
        raster,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, Material, Mesh, Node, PreparedScene, Scene, Transform, Vec3};
    use std::sync::Arc;

    /// Differential oracle: full stable sort with the same resource grouping
    /// rule, independent of the optimized incremental bucket order.
    fn original(
        draws: &mut [PreparedDraw],
        meshes: &HashMap<(MeshId, bool), MeshResources>,
    ) -> Vec<DrawBatchRange> {
        let opaque = draws.partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);
        if meshes.values().any(|mesh| mesh.procedural_quads && mesh.generated.is_none()) {
            draws[..opaque].sort_by_cached_key(|draw| batch_key(draw, meshes));
        }
        let mut batches: Vec<DrawBatchRange> = Vec::new();
        let mut last_key = None;
        for (index, draw) in draws.iter().enumerate() {
            let key = batch_key(draw, meshes);
            if let Some(batch) = batches.last_mut() {
                if draw.material.alpha_mode != AlphaMode::Blend
                    && last_key == Some(key)
                    && matches!(key, BatchKey::Quad { .. })
                {
                    batch.instance_count += 1;
                    continue;
                }
            }
            batches.push(DrawBatchRange { first_instance: index, instance_count: 1 });
            last_key = Some(key);
        }
        batches
    }

    fn fixture() -> (Scene, Vec<Arc<Material>>) {
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let materials = (0..12).map(|i| {
            let mut material = Material::new();
            if i == 11 {
                material.alpha_mode = AlphaMode::Blend;
            }
            material.base_color = [i as f32 / 12.0, 0.7, 0.4, 0.8];
            Arc::new(material)
        }).collect::<Vec<_>>();
        for i in 0..350 {
            scene.insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([materials[(i * 7 + 3) % materials.len()].clone()])
                    .with_transform(Transform {
                        translation: Vec3::new((i % 23) as f32, 0.0, (i / 23) as f32),
                        ..Transform::IDENTITY
                    }),
            ).unwrap();
        }
        (scene, materials)
    }

    fn prepare(scene: &Scene) -> Vec<PreparedDraw> {
        let camera = Camera::perspective(
            Vec3::new(4.0, 12.0, 45.0), Vec3::ZERO, Vec3::Y, 1.0, 0.1, 150.0,
        );
        let mut prepared = PreparedScene::default();
        prepared.set_gpu_culling(true);
        prepared.update(scene, camera, 1.0).unwrap();
        prepared.draws
    }

    fn compare(
        draws: Vec<PreparedDraw>,
        workspace: &mut BatchWorkspace,
        meshes: &HashMap<(MeshId, bool), MeshResources>,
    ) {
        let mut expected = draws.clone();
        let baseline = original(&mut expected, meshes);
        let mut actual = draws;
        let mut batches = Vec::new();
        group_voxel_batches_into(&mut actual, meshes, workspace, &mut batches);
        let draw_id = |draws: &[PreparedDraw]| {
            draws.iter().map(|d| (d.node, d.mesh.id(), d.part, d.material.id()))
                .collect::<Vec<_>>()
        };
        assert_eq!(draw_id(&actual), draw_id(&expected));
        assert_eq!(
            batches.iter().map(|b| (b.first_instance, b.instance_count)).collect::<Vec<_>>(),
            baseline.iter().map(|b| (b.first_instance, b.instance_count)).collect::<Vec<_>>(),
        );
    }

    fn mock_quad_resources(mesh: &std::sync::Arc<Mesh>) -> MeshResources {
        let buffer = gfx_core::BufferId::from_parts(5, 1);
        MeshResources {
            generated: None,
            generation: mesh.generation(),
            vertices: buffer,
            vertex_binding_bytes: 4096,
            vertex_stride: 24,
            vertex_base: 0,
            tangents: None,
            tangent_bytes: 0,
            indices: buffer,
            index_format: gfx_core::IndexFormat::Uint16,
            index_offset: 0,
            procedural_quads: true,
            allocation: None,
        }
    }

    #[test]
    fn identical_gpu_bindings_batch_distinct_material_constants() {
        // Different logical materials still read their own DrawParams via
        // the per-instance physical slot; they do not require separate draws.
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        let mut ids = Vec::new();
        for i in 0..192 {
            let mut material = Material::new();
            material.shading_model = crate::ShadingModel::Unlit;
            material.base_color = [i as f32 / 192.0, 0.4, 0.7, 1.0];
            ids.push(material.id());
            scene.insert(None, Node::new()
                .with_mesh(mesh.clone())
                .with_materials([Arc::new(material)]))
                .unwrap();
        }
        let mut draws = prepare(&scene);
        assert_eq!(draws.len(), 192);
        assert_ne!(ids[0], ids[1]);
        let mut meshes = HashMap::new();
        meshes.insert(mesh_key(&draws[0], true), mock_quad_resources(&mesh));
        let batches = group_voxel_batches(&mut draws, &meshes);
        assert_eq!(batches.len(), 1, "per-instance material data must not split a quad draw");
        assert_eq!(batches[0].instance_count, 192);
        assert_eq!(draws.iter().map(|d| d.material.id()).collect::<std::collections::HashSet<_>>().len(), 192);
    }

    #[test]
    fn small_and_large_scenes_preserve_transparent_order() {
        for count in [16usize, 192] {
            let mesh = Arc::new(Mesh::cube());
            let mut scene = Scene::new();
            for i in 0..count {
                let mut material = Material::new();
                material.shading_model = crate::ShadingModel::Unlit;
                material.alpha_mode = if i % 4 == 0 {
                    AlphaMode::Blend
                } else {
                    AlphaMode::Opaque
                };
                scene.insert(None, Node::new().with_mesh(mesh.clone())
                    .with_materials([Arc::new(material)])).unwrap();
            }
            let mut draws = prepare(&scene);
            let blend_count = draws.iter().filter(|draw| draw.material.alpha_mode == AlphaMode::Blend).count();
            let mut meshes = HashMap::new();
            meshes.insert(mesh_key(&draws[0], true), mock_quad_resources(&mesh));
            let batches = group_voxel_batches(&mut draws, &meshes);
            assert_eq!(batches.len(), blend_count + 1);
            assert_eq!(batches[0].instance_count, count - blend_count);
            assert!(batches.iter().skip(1).all(|batch| batch.instance_count == 1));
            assert!(draws[batches[1].first_instance..].iter()
                .all(|draw| draw.material.alpha_mode == AlphaMode::Blend));
        }
    }

    #[test]
    fn distinct_texture_bindings_and_raster_states_never_merge() {
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        let mut plain = Material::new();
        plain.shading_model = crate::ShadingModel::Unlit;
        let mut textured = plain.clone();
        textured.albedo_texture = Some(crate::TextureAssetId(42));
        let mut double_sided = plain.clone();
        double_sided.double_sided = true;
        let mut flipped = plain.clone();
        flipped.front_face = gfx_core::FrontFace::Cw;
        for material in [plain, textured, double_sided, flipped] {
            scene.insert(None, Node::new()
                .with_mesh(mesh.clone())
                .with_materials([Arc::new(material)]))
                .unwrap();
        }
        let mut draws = prepare(&scene);
        let mut meshes = HashMap::new();
        meshes.insert(mesh_key(&draws[0], true), mock_quad_resources(&mesh));
        let batches = group_voxel_batches(&mut draws, &meshes);
        assert_eq!(batches.len(), 4, "texture or culling state changes need independent bindings");
        assert!(batches.iter().all(|batch| batch.instance_count == 1));
    }

    #[test]
    fn incremental_distinct_key_merge_matches_original_sort_across_edits() {
        let (mut scene, materials) = fixture();
        let mut workspace = BatchWorkspace::default();
        let meshes = HashMap::new();
        compare(prepare(&scene), &mut workspace, &meshes);
        for round in 0..5 {
            let mut next = scene.fork();
            for id in next.roots().iter().copied().take(3).collect::<Vec<_>>() {
                next.remove(id).unwrap();
            }
            for index in 0..(round + 8) {
                let material = materials[(index + round * 3) % materials.len()].clone();
                next.insert(None, Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([material])).unwrap();
            }
            compare(prepare(&next), &mut workspace, &meshes);
            scene = next;
        }
    }

    #[test]
    fn small_scene_and_transparent_order_keep_previous_behavior() {
        let (scene, _) = fixture();
        let mut workspace = BatchWorkspace::default();
        let meshes = HashMap::new();
        let mut draws = prepare(&scene);
        draws.truncate(40);
        compare(draws, &mut workspace, &meshes);
        compare(Vec::new(), &mut workspace, &meshes);
        compare(prepare(&scene), &mut workspace, &meshes);
    }
    #[test]
    fn generated_singleton_scene_does_not_build_sort_buckets_or_reorder_draws() {
        let (scene, _) = fixture();
        let meshes = HashMap::new();
        let mut draws = prepare(&scene);
        let before = draws.iter().map(|draw| draw.node).collect::<Vec<_>>();
        let mut workspace = BatchWorkspace::default();
        let mut batches = Vec::new();
        group_voxel_batches_into(&mut draws, &meshes, &mut workspace, &mut batches);
        assert_eq!(draws.iter().map(|draw| draw.node).collect::<Vec<_>>(), before);
        assert_eq!(batches.len(), draws.len());
        assert!(batches.iter().enumerate().all(|(index, batch)| {
            batch.first_instance == index && batch.instance_count == 1
        }));
        assert!(workspace.sorted_keys.is_empty());
        assert!(workspace.buckets.is_empty());
        assert!(workspace.positions.is_empty());

        // After a streaming mutation the same direct path must continue to
        // leave untouched resident draw identities in their existing order.
        let mut changed = scene.fork();
        let removed = changed.roots()[0];
        changed.remove(removed).unwrap();
        let draws = prepare(&changed);
        compare(draws, &mut workspace, &meshes);
        assert!(workspace.buckets.is_empty());
    }

    #[test]
    fn identity_grouped_draw_order_does_not_allocate_inverse_permutation() {
        let (scene, _) = fixture();
        let meshes = HashMap::new();
        let mut draws = prepare(&scene);
        let mut workspace = BatchWorkspace::default();
        let mut batches = Vec::new();
        // First pass establishes stable material/mesh ordering.
        group_voxel_batches_into(&mut draws, &meshes, &mut workspace, &mut batches);
        workspace.positions.clear();
        workspace.positions.shrink_to_fit();
        // Identical input must produce an identity permutation and leave the
        // inverse-position scratch empty instead of allocating one entry/draw.
        let before = draws.iter().map(|d| d.node).collect::<Vec<_>>();
        group_voxel_batches_into(&mut draws, &meshes, &mut workspace, &mut batches);
        assert_eq!(draws.iter().map(|d| d.node).collect::<Vec<_>>(), before);
        assert!(workspace.positions.is_empty(),
            "no inverse permutation is needed for an identity order");
    }

}
