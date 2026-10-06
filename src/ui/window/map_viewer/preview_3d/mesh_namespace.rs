use super::*;
use gpui_3d::MeshId;

pub(in crate::ui::window::map_viewer) fn namespace_preview_3d_mesh(
    mesh: &Preview3dMesh,
    namespace: u64,
) -> Vec<Preview3dChunkMesh> {
    mesh.chunk_meshes
        .iter()
        .cloned()
        .map(|mut chunk| {
            chunk.mesh = namespace_mesh(&chunk.mesh, namespace, 0);
            if let Some(lod1) = &chunk.lod1_mesh {
                chunk.lod1_mesh = Some(namespace_mesh(lod1, namespace, 1));
            }
            if let Some(lod2) = &chunk.lod2_mesh {
                chunk.lod2_mesh = Some(namespace_mesh(lod2, namespace, 2));
            }
            chunk
        })
        .collect()
}

fn namespace_mesh(mesh: &Arc<Mesh>, namespace: u64, lod: u64) -> Arc<Mesh> {
    Arc::new(mesh.as_ref().clone().with_identity(
        namespaced_mesh_id(mesh.id(), namespace, lod),
        mesh.generation(),
    ))
}

fn namespaced_mesh_id(id: MeshId, namespace: u64, lod: u64) -> MeshId {
    // Exact selections are decomposed into independent read jobs. Two jobs can still
    // contain the same spatial region, so keep their renderer cache keys separate.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [id.0, namespace, lod, 0x4558_4143_545f_3344] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    MeshId(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_parts_never_share_mesh_cache_identity() {
        let source = MeshId(0x1234);
        assert_ne!(
            namespaced_mesh_id(source, 1, 0),
            namespaced_mesh_id(source, 2, 0)
        );
        assert_ne!(
            namespaced_mesh_id(source, 1, 0),
            namespaced_mesh_id(source, 1, 1)
        );
        assert_eq!(
            namespaced_mesh_id(source, 7, 2),
            namespaced_mesh_id(source, 7, 2)
        );
    }
}
