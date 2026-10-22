//! Immutable, renderer-owned imported scene shared by preview and CPU consumers.
//!
//! Model files are parsed only on a worker. The resulting geometry and textures
//! stay in GPUI-3D assets; voxel tools borrow triangles instead of reparsing files.

use std::{path::Path, sync::Arc};

use super::{GltfAnimation, GltfCamera, GltfError, GltfLoadOptions, GltfScene, load_bytes, load_path};
use crate::{
    Camera, Mat4, Material, NodeId, Scene, SceneView, SceneViewTextureError,
    TextureAsset, Vec2, Vec3,
};

/// Canonical owned asset snapshot, independent of rendering and file handles.
#[derive(Clone, Debug)]
pub struct ImportedScene {
    scene: Arc<Scene>,
    textures: Arc<[Arc<TextureAsset>]>,
    /// Imported, absolute-time animation clips.
    pub animations: Vec<GltfAnimation>,
    /// Cameras declared by the source format.
    pub cameras: Vec<GltfCamera>,
    /// Non-fatal import diagnostics.
    pub warnings: Vec<String>,
    /// Number of source triangles, not a render-LOD estimate.
    pub triangle_count: usize,
}

/// Borrowed CPU triangle in world coordinates; no additional vertex buffers.
pub struct ImportedTriangle<'a> {
    pub positions: [Vec3; 3],
    pub uvs: [Vec2; 3],
    pub colors: [[f32; 4]; 3],
    pub material: Option<&'a Material>,
    pub albedo: Option<&'a TextureAsset>,
}

impl ImportedScene {
    /// Constructs a canonical scene from pre-parsed geometry, such as the
    /// application's existing hardened `tobj` / MTL loader.
    #[must_use]
    pub fn from_parts(
        scene: Scene,
        textures: impl IntoIterator<Item = Arc<TextureAsset>>,
        triangle_count: usize,
    ) -> Self {
        Self {
            scene: Arc::new(scene),
            textures: textures.into_iter().collect(),
            animations: Vec::new(),
            cameras: Vec::new(),
            warnings: Vec::new(),
            triangle_count,
        }
    }

    /// Consumes a glTF import without copying decoded meshes or image bytes.
    #[must_use]
    pub fn from_gltf(source: GltfScene) -> Self {
        let triangle_count = usize::try_from(source.statistics.triangles).unwrap_or(usize::MAX);
        Self {
            scene: Arc::new(source.scene),
            textures: source.textures.into_values().collect(),
            animations: source.animations,
            cameras: source.cameras,
            warnings: source.warnings,
            triangle_count,
        }
    }

    /// Scene used directly by GPUI-3D's retained renderer.
    #[must_use]
    pub fn scene(&self) -> &Arc<Scene> {
        &self.scene
    }

    /// Decoded texture assets reused by GPU upload and CPU material sampling.
    #[must_use]
    pub fn textures(&self) -> &[Arc<TextureAsset>] {
        &self.textures
    }

    /// Creates a lightweight scene view without duplicating model buffers.
    pub fn view(&self, camera: Camera) -> Result<SceneView, SceneViewTextureError> {
        SceneView::new(self.scene.clone(), camera)
            .with_textures(self.textures.iter().cloned())
    }

    /// Iterates shared mesh data and resolves hierarchical transforms. The
    /// callback can rasterize each triangle directly on a TaskManager worker.
    ///
    /// Does not decode textures, access files, or allocate a triangle list.
    pub fn for_each_triangle(&self, mut visitor: impl FnMut(ImportedTriangle<'_>)) {
        for &id in self.scene.roots() {
            self.visit_node(id, Mat4::IDENTITY, &mut visitor);
        }
    }

    fn visit_node(
        &self,
        id: NodeId,
        parent: Mat4,
        visitor: &mut impl FnMut(ImportedTriangle<'_>),
    ) {
        let Some(node) = self.scene.node(id) else { return };
        if !node.visible { return; }
        let world = parent * node.transform.matrix();
        if let Some(mesh) = &node.mesh {
            for part in mesh.parts() {
                let material = node.materials.get(part.material_slot() as usize);
                let albedo = material
                    .and_then(|mat| mat.albedo_texture)
                    .and_then(|id| self.textures.iter().find(|asset| asset.id() == id))
                    .map(Arc::as_ref);
                let begin = part.first_index() as usize;
                let Some(end) = begin.checked_add(part.index_count() as usize) else { continue; };
                let Some(indices) = mesh.indices().get(begin..end) else { continue; };
                for face in indices.chunks_exact(3) {
                    let Some((a, b, c)) = mesh.vertices().get(face[0] as usize)
                        .zip(mesh.vertices().get(face[1] as usize))
                        .zip(mesh.vertices().get(face[2] as usize))
                        .map(|((a, b), c)| (a, b, c))
                    else { continue };
                    let vertices = [a, b, c];
                    visitor(ImportedTriangle {
                        positions: vertices.map(|v| world.transform_point(v.position)),
                        uvs: vertices.map(|v| v.uv),
                        colors: vertices.map(|v| v.color),
                        material: material.map(Arc::as_ref),
                        albedo,
                    });
                }
            }
        }
        if let Some(children) = self.scene.children(id) {
            for &child in children {
                self.visit_node(child, world, visitor);
            }
        }
    }
}

/// Converts an existing glTF/GLB import to the same asset type used for OBJ.
///
/// Always invoke on a worker; this is synchronous file I/O and CPU decoding.
pub fn load_imported_path(
    path: impl AsRef<Path>,
    options: &GltfLoadOptions,
) -> Result<ImportedScene, GltfError> {
    load_path(path, options).map(ImportedScene::from_gltf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Material, Mesh, Node, Vertex, Vec2, Vec3};

    #[test]
    fn imported_mesh_is_shared_by_view_and_cpu_rasterization() {
        let vertices = vec![
            Vertex { position: Vec3::new(0.0,0.0,0.0), normal: Vec3::Z, uv: Vec2::ZERO, color: [1.0; 4] },
            Vertex { position: Vec3::new(1.0,0.0,0.0), normal: Vec3::Z, uv: Vec2::ZERO, color: [1.0; 4] },
            Vertex { position: Vec3::new(0.0,1.0,0.0), normal: Vec3::Z, uv: Vec2::ZERO, color: [1.0; 4] },
        ];
        let mesh = Arc::new(Mesh::new(vertices, vec![0,1,2]).unwrap());
        let mut graph = Scene::new();
        graph.insert(None, Node::new().with_mesh(mesh).with_materials([Arc::new(Material::new())])).unwrap();
        let imported = ImportedScene::from_parts(graph, [], 1);
        let mut seen = 0;
        imported.for_each_triangle(|triangle| {
            assert_eq!(triangle.positions[1], Vec3::new(1.0,0.0,0.0));
            seen += 1;
        });
        assert_eq!(seen, 1);
        assert_eq!(imported.triangle_count, 1);
    }
}

/// Converts glTF/GLB bytes already read by an async I/O worker. Parse and
/// scene assembly should run on the bounded TaskManager CPU pool.
///
/// No additional source file read occurs here. External glTF resources use
/// the hardened base-directory policy from `GltfLoadOptions`.
pub fn load_imported_bytes(
    bytes: &[u8],
    options: &GltfLoadOptions,
    base_dir: Option<&Path>,
) -> Result<ImportedScene, GltfError> {
    load_bytes(bytes, options, base_dir).map(ImportedScene::from_gltf)
}
