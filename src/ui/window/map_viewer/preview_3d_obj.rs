use std::path::PathBuf;

use bedrock_block_model::{
    ObjDocumentHeader, ObjExport, ObjMeshFaceSource, ObjTextureResolver, export_face_sources,
};

use super::preview_3d::{Preview3dChunkMesh, Preview3dMesh};

pub(super) type Preview3dObjExport = ObjExport;

/// Configuration for one preview-3D OBJ export. Grouped so the entry point stays
/// a short action with a typed parameter object instead of a positional chain.
pub(super) struct Preview3dObjOptions<'a> {
    pub mesh: &'a Preview3dMesh,
    pub material_library_name: &'a str,
    pub texture_directory_name: &'a str,
    pub game_package_paths: &'a [PathBuf],
}

pub(super) fn export_preview_3d_obj(
    options: Preview3dObjOptions<'_>,
    progress: impl FnMut(usize, usize),
) -> Preview3dObjExport {
    let header = ObjDocumentHeader {
        comment: "BMCBL preview_3d OBJ export",
        material_library_name: options.material_library_name,
        object_name: "bmcb_preview_selection",
    };
    let resolver = ObjTextureResolver::with_package_roots(
        options.game_package_paths,
        options.texture_directory_name,
    );
    export_face_sources(
        &header,
        options.mesh.chunk_meshes.iter(),
        &resolver,
        progress,
    )
}

impl ObjMeshFaceSource for Preview3dChunkMesh {
    fn obj_face_count(&self) -> usize {
        (self.mesh.indices().len() / 6).min(self.face_metadata.len())
    }

    fn obj_face_material(&self, face_index: usize) -> Option<&str> {
        self.face_material(face_index)
    }

    fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]> {
        let vertex_index = *self.face_indices(face_index)?.first()?;
        self.mesh
            .vertices()
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
        self.mesh.indices().get(start..end)
    }

    fn index_position(&self, index: u32) -> Option<[f32; 3]> {
        self.mesh
            .vertices()
            .get(usize::try_from(index).ok()?)
            .map(|vertex| {
                [
                    vertex.position.x + self.world_origin[0] as f32,
                    vertex.position.y + self.world_origin[1] as f32,
                    vertex.position.z + self.world_origin[2] as f32,
                ]
            })
    }
}
