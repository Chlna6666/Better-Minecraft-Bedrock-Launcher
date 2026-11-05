//! Coalesces resident opaque quad ranges without changing blended draw order.

use super::{
    gpu::MeshResources,
    scene::{DrawBatchRange, mesh_key},
};
use crate::{AlphaMode, MeshId, PreparedDraw};
use std::collections::HashMap;

type BatchKey = (bool, u64, u64, u32, bool);

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
/// every opaque draw. The GPU-visible draw/instance order stays equivalent to
/// stable sort_by_cached_key, including material and mirror-sensitive batching.
pub(super) fn group_voxel_batches_into(
    draws: &mut [PreparedDraw],
    meshes: &HashMap<(MeshId, bool), MeshResources>,
    workspace: &mut BatchWorkspace,
    batches: &mut Vec<DrawBatchRange>,
) {
    batches.clear();
    let opaque = draws.partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);

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
                let first = &draws[batch.first_instance];
                if draw.material.alpha_mode != AlphaMode::Blend
                    && first.material.alpha_mode != AlphaMode::Blend
                    && last_key == Some(key)
                    && first.material.as_ref() == draw.material.as_ref()
                    && !key.0
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
                let first = &draws[workspace.order[batch.first_instance]];
                if last_key == Some(key)
                    && first.material.as_ref() == draw.material.as_ref()
                    && !key.0
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
    crate::prepare::apply_order(
        &mut draws[..opaque],
        &workspace.order,
        &mut workspace.positions,
    );

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
) -> (bool, u64, u64, u32, bool) {
    let resource = meshes.get(&mesh_key(draw, true));
    let quad = resource.is_some_and(|mesh| mesh.procedural_quads && mesh.generated.is_none());
    let page = if quad {
        resource.map_or(0, |mesh| mesh.vertices.raw())
    } else {
        draw.mesh.id().0
    };
    // Round up to a power-of-two vertex-count class: padding costs less than 2x vertex
    // invocations per instance instead of a large part forcing every tiny part to its size.
    let size_class = u32::BITS - draw.part.index_count().saturating_sub(1).leading_zeros();
    (
        !quad,
        draw.material.id().0,
        page,
        size_class,
        !draw.material.double_sided && draw.is_mirrored(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, Material, Mesh, Node, PreparedScene, Scene, Transform, Vec3};
    use std::sync::Arc;

    /// Differential oracle: retain the previous stable full-draw sort and
    /// exact material/quad grouping semantics.
    fn original(
        draws: &mut [PreparedDraw],
        meshes: &HashMap<(MeshId, bool), MeshResources>,
    ) -> Vec<DrawBatchRange> {
        let opaque = draws.partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);
        draws[..opaque].sort_by_cached_key(|draw| batch_key(draw, meshes));
        let mut batches: Vec<DrawBatchRange> = Vec::new();
        let mut last_key = None;
        for (index, draw) in draws.iter().enumerate() {
            let key = batch_key(draw, meshes);
            if let Some(batch) = batches.last_mut() {
                let first = &draws[batch.first_instance];
                if draw.material.alpha_mode != AlphaMode::Blend
                    && first.material.alpha_mode != AlphaMode::Blend
                    && last_key == Some(key)
                    && first.material.as_ref() == draw.material.as_ref()
                    && !key.0
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
}
