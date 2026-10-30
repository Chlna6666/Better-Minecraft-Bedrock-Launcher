//! Coalesces resident opaque quad ranges without changing blended draw order.

use super::{
    gpu::MeshResources,
    scene::{DrawBatchRange, mesh_key},
};
use crate::{AlphaMode, MeshId, PreparedDraw};
use std::collections::HashMap;

pub(super) fn group_voxel_batches(
    draws: &mut [PreparedDraw],
    meshes: &HashMap<(MeshId, bool), MeshResources>,
) -> Vec<DrawBatchRange> {
    let opaque = draws.partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);
    // Looking up a resident mesh for every comparison becomes expensive for large
    // scenes. Compute each sort key once while preserving the stable ordering.
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
        batches.push(DrawBatchRange {
            first_instance: index,
            instance_count: 1,
        });
        last_key = Some(key);
    }
    batches
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
