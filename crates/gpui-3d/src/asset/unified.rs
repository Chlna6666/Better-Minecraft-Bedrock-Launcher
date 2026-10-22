//! One CPU-side 3D scene representation for import, preview, and downstream tools.
//!
//! glTF/GLB reuse GPUI-3D's validated native loader (no second glTF parse).
//! OBJ/STL use the optional three-d-asset importer. Imported geometries become
//! immutable GPUI-3D Mesh/Material/TextureAsset handles exactly once.
//! Consumers should reuse this snapshot, not decode the source again.

use std::{collections::HashMap, path::Path, sync::Arc};

use super::{GltfAnimation, GltfCamera, GltfLoadOptions, load_path};
use crate::{
    AlphaMode, Camera, Material, Mat4, Mesh, Node, NodeId, Scene, SceneView,
    SceneViewTextureError, TextureAsset, TextureAssetId, Vec2, Vec3, Vertex,
};
use three_d_asset::{Geometry, Interpolation, Model, Texture2D, TextureData};

/// Import bounds shared between all formats.
#[derive(Clone, Debug)]
pub struct ImportLimits {
    /// Maximum bytes in the primary file.
    pub max_file_bytes: u64,
    /// Maximum vertices across all meshes after import.
    pub max_vertices: usize,
    /// Maximum source triangles.
    pub max_triangles: usize,
    /// Maximum RGBA8 bytes for all imported albedo textures.
    pub max_texture_bytes: usize,
    /// Existing hardened glTF limits and external-resource policy.
    pub gltf: GltfLoadOptions,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024 * 1024,
            max_vertices: 3_000_000,
            max_triangles: 1_000_000,
            max_texture_bytes: 256 * 1024 * 1024,
            gltf: GltfLoadOptions::default(),
        }
    }
}

/// A single immutable GPUI-3D asset snapshot. Textures retain their CPU RGBA
/// bytes so a CPU voxelizer can sample them without reading files again.
#[derive(Debug)]
pub struct ImportedScene {
    /// Node hierarchy and immutable mesh/material assets.
    pub scene: Arc<Scene>,
    /// CPU/GPU-shared textures referenced by scene materials.
    pub textures: HashMap<TextureAssetId, Arc<TextureAsset>>,
    /// Imported glTF animation tracks, if any.
    pub animations: Vec<GltfAnimation>,
    /// Cameras authored in glTF, if any.
    pub cameras: Vec<GltfCamera>,
    /// Total triangle count, before render LOD selection.
    pub triangles: usize,
    /// Non-fatal import diagnostics.
    pub warnings: Vec<String>,
}

/// A borrowed triangle from the same immutable mesh data GPUI-3D renders.
/// World transforms are already applied. No secondary triangle buffer is built.
pub struct ImportedTriangle<'a> {
    /// Three world positions.
    pub positions: [Vec3; 3],
    /// UVs corresponding to these vertices.
    pub uvs: [Vec2; 3],
    /// Linear per-vertex colors.
    pub colors: [[f32; 4]; 3],
    /// Shared scene material if assigned.
    pub material: Option<&'a Material>,
    /// Shared decoded RGBA texture, for CPU-side material sampling.
    pub albedo: Option<&'a TextureAsset>,
}

impl ImportedScene {
    /// Builds a renderable view without cloning mesh buffers or texture pixels.
    ///
    /// # Errors
    /// Returns an error for a duplicate texture ID or invalid texture table.
    pub fn view(&self, camera: Camera) -> Result<SceneView, SceneViewTextureError> {
        SceneView::new(Arc::clone(&self.scene), camera)
            .with_textures(self.textures.values().cloned())
    }

    /// Borrow the exact CPU geometry rendered by GPUI-3D, applying node
    /// transforms without copying mesh buffers or decoding images again.
    /// Call on a CPU worker for voxel rasterization or geometry inspection.
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
        if !node.visible {
            return;
        }
        let world = parent * node.transform.matrix();
        if let Some(mesh) = &node.mesh {
            let vertices = mesh.vertices();
            let indices = mesh.indices();
            for part in mesh.parts() {
                let material = node.materials.get(part.material_slot() as usize);
                let albedo = material.and_then(|mat| mat.albedo_texture)
                    .and_then(|id| self.textures.get(&id))
                    .map(Arc::as_ref);
                let start = part.first_index() as usize;
                let end = start + part.index_count() as usize;
                for face in indices[start..end].chunks_exact(3) {
                    let v = [
                        &vertices[face[0] as usize],
                        &vertices[face[1] as usize],
                        &vertices[face[2] as usize],
                    ];
                    visitor(ImportedTriangle {
                        positions: v.map(|vertex| world.transform_point(vertex.position)),
                        uvs: v.map(|vertex| vertex.uv),
                        colors: v.map(|vertex| vertex.color),
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

/// Import errors leave the existing file and model cache unchanged.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Format is not enabled or recognized.
    #[error("unsupported model extension: {0}")]
    Unsupported(String),
    /// Import limits were exceeded.
    #[error("model import limit exceeded: {0}")]
    Limit(String),
    /// I/O, parser or geometry conversion failed.
    #[error("model import failed: {0}")]
    Invalid(String),
}

/// Loads a local model into GPUI-3D's canonical geometry/material representation.
///
/// OBJ/STL are parsed by three-d-asset 0.10; GLB/glTF are delegated to the
/// existing GPUI-3D glTF parser. External files are only for trusted, local
/// model directories until the generic importer gains path-confinement guards.
///
/// Invoke on a CPU/IO task, never the UI frame callback.
///
/// # Errors
/// Returns an error on unsupported formats, invalid geometry, or size limits.
pub fn import_scene_path(
    path: impl AsRef<Path>,
    limits: &ImportLimits,
) -> Result<ImportedScene, ImportError> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path).map_err(|e| ImportError::Invalid(e.to_string()))?;
    if metadata.len() > limits.max_file_bytes {
        return Err(ImportError::Limit("source file size".into()));
    }
    let extension = path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if matches!(extension.as_str(), "glb" | "gltf") {
        // Reusing this implementation is essential: it already handles scene
        // hierarchy, sparse accessors, materials, animations, and import limits.
        let mut options = limits.gltf.clone();
        options.max_file_bytes = options.max_file_bytes.min(limits.max_file_bytes as usize);
        options.max_vertices = options.max_vertices.min(limits.max_vertices);
        let imported = load_path(path, &options)
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
        let triangles = usize::try_from(imported.statistics.triangles)
            .map_err(|_| ImportError::Limit("glTF triangle count".into()))?;
        if triangles > limits.max_triangles {
            return Err(ImportError::Limit("glTF triangle count".into()));
        }
        let texture_bytes = imported.textures.values()
            .try_fold(0_usize, |size, image| {
                size.checked_add(image.pixels().len())
                    .ok_or_else(|| ImportError::Limit("glTF texture allocation".into()))
            })?;
        if texture_bytes > limits.max_texture_bytes {
            return Err(ImportError::Limit("glTF texture allocation".into()));
        }
        return Ok(ImportedScene {
            scene: Arc::new(imported.scene),
            textures: imported.textures,
            animations: imported.animations,
            cameras: imported.cameras,
            triangles,
            warnings: imported.warnings,
        });
    }

    // 0.10.0 has OBJ/STL/3MF format adapters, but 3MF requires optional
    // lib3mf bindings. Avoid importing that native dependency by default.
    if !matches!(extension.as_str(), "obj" | "stl") {
        return Err(ImportError::Unsupported(extension));
    }
    let model: Model = three_d_asset::io::load_and_deserialize(path)
        .map_err(|e| ImportError::Invalid(e.to_string()))?;
    from_three_d_model(model, &extension, limits)
}

fn from_three_d_model(
    model: Model,
    format: &str,
    limits: &ImportLimits,
) -> Result<ImportedScene, ImportError> {
    let mut materials = Vec::with_capacity(model.materials.len());
    let mut textures = HashMap::new();
    let mut texture_bytes = 0_usize;

    for material in model.materials {
        let color = material.albedo.to_linear_srgb();
        let mut converted = Material::new();
        converted.double_sided = true;
        converted.base_color = [color.x, color.y, color.z, color.w];
        converted.metallic = material.metallic.clamp(0.0, 1.0);
        converted.roughness = material.roughness.clamp(0.0, 1.0);
        let emissive = material.emissive.to_linear_srgb();
        converted.emissive = [emissive.x, emissive.y, emissive.z, 1.0];
        if let Some(cutoff) = material.alpha_cutout {
            converted.alpha_mode = AlphaMode::Mask;
            converted.alpha_cutoff = cutoff.clamp(0.0, 1.0);
        }
        if color.w < 1.0 {
            converted.alpha_mode = AlphaMode::Blend;
        }
        if let Some(texture) = material.albedo_texture {
            let has_alpha = matches!(&texture.data, TextureData::RgbaU8(pixels) if pixels.iter().any(|pixel| pixel[3] != 255));
            let asset = decode_texture(texture, &mut texture_bytes, limits.max_texture_bytes)?;
            if has_alpha {
                converted.alpha_mode = AlphaMode::Blend;
            }
            // GPUI-3D's texture identity is shared by the renderer and CPU consumers.
            converted.albedo_texture = Some(asset.id());
            textures.insert(asset.id(), Arc::new(asset));
        }
        materials.push(Arc::new(converted));
    }
    let fallback = Arc::new(Material::new());
    let mut scene = Scene::new();
    let mut triangles = 0_usize;
    let mut vertices_total = 0_usize;

    for primitive in model.geometries {
        let Geometry::Triangles(mut mesh) = primitive.geometry else {
            return Err(ImportError::Unsupported("point cloud geometry".into()));
        };
        // STL can supply per-face normals while GPUI-3D expects per-vertex.
        // Recompute normals for this format (and any malformed normal stream).
        if mesh.normals.as_ref().is_some_and(|normals| normals.len() != mesh.positions.len()) {
            mesh.normals = None;
        }
        if mesh.tangents.as_ref().is_some_and(|tangents| tangents.len() != mesh.positions.len()) {
            mesh.tangents = None;
        }
        mesh.transform(primitive.transformation)
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
        let positions = mesh.positions.into_f32();
        if vertices_total.checked_add(positions.len()).is_none_or(|n| n > limits.max_vertices) {
            return Err(ImportError::Limit("vertex count".into()));
        }
        let indices: Vec<u32> = mesh.indices.to_u32()
            .unwrap_or_else(|| (0..positions.len()).map(|i| i as u32).collect());
        let triangle_count = indices.len() / 3;
        triangles = triangles.checked_add(triangle_count)
            .ok_or_else(|| ImportError::Limit("triangle count".into()))?;
        vertices_total = vertices_total.checked_add(positions.len())
            .ok_or_else(|| ImportError::Limit("vertex count".into()))?;
        if triangles > limits.max_triangles || vertices_total > limits.max_vertices {
            return Err(ImportError::Limit("model geometry".into()));
        }
        if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= positions.len()) {
            return Err(ImportError::Invalid("invalid triangle indices".into()));
        }
        let mut normals = mesh.normals.filter(|n| n.len() == positions.len())
            .map(|n| n.into_iter()
                .map(|v| Vec3::new(v.x, v.y, v.z))
                .collect::<Vec<_>>())
            .unwrap_or_else(|| vec![Vec3::ZERO; positions.len()]);
        if normals.iter().all(|n| n.length_squared() == 0.0) {
            for tri in indices.chunks_exact(3) {
                let a = &positions[tri[0] as usize];
                let b = &positions[tri[1] as usize];
                let c = &positions[tri[2] as usize];
                let p = Vec3::new(a.x, a.y, a.z);
                let q = Vec3::new(b.x, b.y, b.z);
                let r = Vec3::new(c.x, c.y, c.z);
                let normal = (q - p).cross(r - p);
                for &index in tri {
                    normals[index as usize] = normals[index as usize] + normal;
                }
            }
        }
        let vertices = positions.iter().enumerate().map(|(i,p)| {
            let normal = normals[i].normalized().unwrap_or(Vec3::Y);
            let uv = mesh.uvs.as_ref().and_then(|uvs| uvs.get(i))
                .map_or(Vec2::ZERO, |uv| {
                    Vec2::new(uv.x, if format == "obj" { 1.0 - uv.y } else { uv.y })
                });
            let color = mesh.colors.as_ref().and_then(|colors| colors.get(i))
                .map(|color| {
                    let c = color.to_linear_srgb();
                    [c.x, c.y, c.z, c.w]
                }).unwrap_or([1.0; 4]);
            Vertex {
                position: Vec3::new(p.x, p.y, p.z),
                normal,
                uv,
                color,
            }
        }).collect::<Vec<_>>();
        let geometry = Mesh::new(vertices, indices)
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
        let material = primitive.material_index
            .and_then(|idx| materials.get(idx))
            .cloned()
            .unwrap_or_else(|| Arc::clone(&fallback));
        scene.insert(None, Node::new()
            .with_mesh(Arc::new(geometry))
            .with_materials([material]))
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
    }
    if triangles == 0 {
        return Err(ImportError::Invalid("model contains no triangles".into()));
    }
    Ok(ImportedScene {
        scene: Arc::new(scene),
        textures,
        animations: Vec::new(),
        cameras: Vec::new(),
        triangles,
        warnings: vec![],
    })
}

fn decode_texture(
    texture: Texture2D,
    used: &mut usize,
    budget: usize,
) -> Result<TextureAsset, ImportError> {
    let width = texture.width;
    let height = texture.height;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(ImportError::Limit("texture dimensions".into()));
    }
    let size = usize::try_from(width).ok()
        .and_then(|w| w.checked_mul(height as usize))
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ImportError::Limit("texture dimensions".into()))?;
    *used = used.checked_add(size)
        .ok_or_else(|| ImportError::Limit("texture allocation".into()))?;
    if *used > budget {
        return Err(ImportError::Limit("texture allocation".into()));
    }
    let mut rgba = Vec::with_capacity(size);
    match texture.data {
        TextureData::RgbaU8(pixels) => {
            for p in pixels { rgba.extend_from_slice(&p); }
        }
        TextureData::RgbU8(pixels) => {
            for p in pixels { rgba.extend_from_slice(&[p[0], p[1], p[2], 255]); }
        }
        TextureData::RU8(pixels) => {
            for value in pixels { rgba.extend_from_slice(&[value, value, value, 255]); }
        }
        _ => return Err(ImportError::Unsupported("non-8-bit color texture".into())),
    }
    if rgba.len() != size {
        return Err(ImportError::Invalid("texture dimension/data mismatch".into()));
    }
    let point_sampling = texture.min_filter == Interpolation::Nearest;
    TextureAsset::rgba8(width, height, Arc::<[u8]>::from(rgba))
        .map(|asset| if point_sampling { asset.with_point_sampling() } else { asset })
        .map_err(|e| ImportError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_a_triangle_from_stl_via_three_d_asset() {
        let path = std::env::temp_dir().join(format!("gpui-3d-import-{}.stl", std::process::id()));
        std::fs::write(&path, b"solid one\nfacet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 0 0\n vertex 0 1 0\n endloop\nendfacet\nendsolid one\n")
            .unwrap();
        let scene = import_scene_path(&path, &ImportLimits::default()).unwrap();
        assert_eq!(scene.triangles, 1);
        assert_eq!(scene.scene.len(), 1);
        let mut faces = Vec::new();
        scene.for_each_triangle(|triangle| faces.push(triangle.positions));
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0][0], Vec3::new(0.0, 0.0, 0.0));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn import_limit_rejects_oversized_model_before_parse() {
        let path = std::env::temp_dir().join(format!("gpui-3d-limit-{}.obj", std::process::id()));
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let mut limits = ImportLimits::default();
        limits.max_file_bytes = 2;
        assert!(matches!(import_scene_path(&path, &limits), Err(ImportError::Limit(_))));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn obj_source_is_imported_into_the_same_scene_type_as_stl() {
        let path = std::env::temp_dir().join(format!("gpui-3d-import-{}.obj", std::process::id()));
        std::fs::write(&path, "o triangle\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
            .unwrap();
        let scene = import_scene_path(&path, &ImportLimits::default()).unwrap();
        assert_eq!(scene.triangles, 1);
        let mut visited = 0;
        scene.for_each_triangle(|_| visited += 1);
        assert_eq!(visited, 1);
        std::fs::remove_file(path).unwrap();
    }
}
