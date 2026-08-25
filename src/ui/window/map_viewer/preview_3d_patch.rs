// Extra helpers kept in the same `preview_3d` module via `include!` so they can
// preserve the renderer-private LOD meshes while exact-selection parts are merged.

pub(super) fn namespace_preview_3d_mesh(
    mesh: &Preview3dMesh,
    namespace: u64,
) -> Preview3dMesh {
    let mut namespaced = mesh.clone();
    for chunk in &mut namespaced.chunk_meshes {
        chunk.gpu_mesh = Arc::new(namespace_gpu_mesh(chunk.gpu_mesh.as_ref(), namespace, 0));
        if let Some(lod1) = chunk.lod1_mesh.as_ref() {
            chunk.lod1_mesh = Some(Arc::new(namespace_gpu_mesh(lod1.as_ref(), namespace, 1)));
        }
        if let Some(lod2) = chunk.lod2_mesh.as_ref() {
            chunk.lod2_mesh = Some(Arc::new(namespace_gpu_mesh(lod2.as_ref(), namespace, 2)));
        }
    }
    namespaced
}

fn namespace_gpu_mesh(mesh: &GpuMesh3d, namespace: u64, lod: u64) -> GpuMesh3d {
    let mut namespaced = mesh.clone();
    namespaced.id = namespaced_gpu_mesh_id(mesh.id, namespace, lod);
    namespaced
}

fn namespaced_gpu_mesh_id(id: GpuMesh3dId, namespace: u64, lod: u64) -> GpuMesh3dId {
    // Exact selections are decomposed into independent rectangular read jobs. Two
    // jobs can still land in the same 8x8 spatial mesh region; without a namespace
    // they receive the same renderer cache id even though their vertex/index data
    // differ. Keep ids stable for a given exact-part index while separating parts.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [
        id.0 as u64,
        namespace,
        lod,
        0x4558_4143_545f_3344, // "EXACT_3D"
    ] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    GpuMesh3dId(hash as usize)
}

#[cfg(test)]
mod exact_mesh_namespace_tests {
    use super::*;

    #[test]
    fn exact_parts_never_share_gpu_cache_identity() {
        let source = GpuMesh3dId(0x1234);
        assert_ne!(
            namespaced_gpu_mesh_id(source, 1, 0),
            namespaced_gpu_mesh_id(source, 2, 0)
        );
        assert_ne!(
            namespaced_gpu_mesh_id(source, 1, 0),
            namespaced_gpu_mesh_id(source, 1, 1)
        );
        assert_eq!(
            namespaced_gpu_mesh_id(source, 7, 2),
            namespaced_gpu_mesh_id(source, 7, 2)
        );
    }
}
