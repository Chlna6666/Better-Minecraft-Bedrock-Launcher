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
        (self.gpu_mesh.indices.len() / 6).min(self.face_metadata.len())
    }

    fn obj_face_material(&self, face_index: usize) -> Option<&str> {
        self.face_metadata
            .get(face_index)
            .map(|metadata| metadata.material.as_ref())
    }

    fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]> {
        let vertex_index = *self.face_indices(face_index)?.first()?;
        self.gpu_mesh
            .vertices
            .get(usize::try_from(vertex_index).ok()?)
            .map(|vertex| vertex.color)
    }

    fn obj_face_triangle_positions(&self, face_index: usize) -> Option<[[f32; 3]; 6]> {
        let indices = self.face_indices(face_index)?;
        Some([
            self.index_position(indices[0])?,
            self.index_position(indices[1])?,
            self.index_position(indices[2])?,
            self.index_position(indices[3])?,
            self.index_position(indices[4])?,
            self.index_position(indices[5])?,
        ])
    }

    fn obj_face_uv(&self, face_index: usize) -> Option<[[f32; 2]; 4]> {
        self.face_metadata.get(face_index)?.uv
    }
}

impl Preview3dChunkMesh {
    fn face_indices(&self, face_index: usize) -> Option<&[u32]> {
        let start = face_index.checked_mul(6)?;
        let end = start.checked_add(6)?;
        self.gpu_mesh.indices.get(start..end)
    }

    fn index_position(&self, index: u32) -> Option<[f32; 3]> {
        self.gpu_mesh
            .vertices
            .get(usize::try_from(index).ok()?)
            .map(|vertex| vertex.position)
    }
}
