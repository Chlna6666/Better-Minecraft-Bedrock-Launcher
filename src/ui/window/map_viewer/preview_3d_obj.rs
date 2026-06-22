use std::path::PathBuf;

use bedrock_block_model::{
    ObjExport, ObjMeshFaceSource, export_obj_from_face_sources_with_package_roots,
};

use super::preview_3d::{Preview3dChunkMesh, Preview3dMesh};

pub(super) type Preview3dObjExport = ObjExport;

pub(super) fn export_preview_3d_obj_with_materials_with_progress(
    mesh: &Preview3dMesh,
    material_library_name: &str,
    texture_directory_name: &str,
    game_package_paths: &[PathBuf],
    progress: impl FnMut(usize, usize),
) -> Preview3dObjExport {
    export_obj_from_face_sources_with_package_roots(
        "BMCBL preview_3d OBJ export",
        material_library_name,
        "bmcb_preview_selection",
        mesh.chunk_meshes.iter(),
        game_package_paths,
        texture_directory_name,
        progress,
    )
}

impl ObjMeshFaceSource for Preview3dChunkMesh {
    fn obj_face_count(&self) -> usize {
        (self.gpu_mesh.vertices.len() / 6)
            .min(self.face_materials.len())
            .min(self.face_uvs.len())
    }

    fn obj_face_material(&self, face_index: usize) -> Option<&str> {
        self.face_materials.get(face_index).map(AsRef::as_ref)
    }

    fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]> {
        self.gpu_mesh
            .vertices
            .get(face_index.checked_mul(6)?)
            .map(|vertex| vertex.color)
    }

    fn obj_face_triangle_positions(&self, face_index: usize) -> Option<[[f32; 3]; 6]> {
        let vertex_start = face_index.checked_mul(6)?;
        let vertices = self.gpu_mesh.vertices.get(vertex_start..vertex_start + 6)?;
        Some([
            vertices[0].position,
            vertices[1].position,
            vertices[2].position,
            vertices[3].position,
            vertices[4].position,
            vertices[5].position,
        ])
    }

    fn obj_face_uv(&self, face_index: usize) -> Option<[[f32; 2]; 4]> {
        self.face_uvs.get(face_index).copied()
    }
}
