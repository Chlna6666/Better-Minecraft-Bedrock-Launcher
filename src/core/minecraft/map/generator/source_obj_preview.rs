//! Bounded source-geometry preview for OBJ converters.
//! This is the actual triangulated OBJ geometry (not Minecraft voxels). MTL diffuse
//! colors and alpha are retained; texture sampling will be supported separately.

use std::{collections::HashMap, sync::Arc};

use bedrock_voxel::{ObjModel, SourceTexture, load_obj_textures};
use gpui_3d::{
    AlphaMode, Camera, ImportedScene, Mat4, Material, Mesh, MeshPart, Node, ProjectionRegion, Scene, SceneView,
    ShadingModel, TextureAsset, Vec2, Vec3, Vertex,
};

/// Limit CPU/GPU retention while preserving a deterministic overview of huge models.
const MAX_PREVIEW_TRIANGLES: usize = 80_000;
#[derive(Clone, Debug)]
pub struct ObjSourcePreview {
    pub scene_view: Arc<SceneView>,
    /// The exact GPUI-3D source assets backing the rendered preview.
    /// Retained by Arc; no second mesh or texture decode is necessary.
    pub imported_scene: Arc<ImportedScene>,
    /// Source coordinates → the normalized [-1, 1] preview space.
    pub source_normalization: Mat4,
    pub total_triangles: usize,
    pub displayed_triangles: usize,
    /// Materials that actually use a decoded MTL map_Kd image.
    pub textured_materials: usize,
}

/// Builds an immutable, centered source mesh, on the TaskManager CPU worker.
/// This intentionally does not open a world or create GPUI-owned futures.
pub fn build_obj_source_preview(model: &ObjModel) -> Result<Arc<ObjSourcePreview>, String> {
    let textures = load_obj_textures(model).map_err(|error| error.to_string())?;
    build_obj_source_preview_with_textures(model, &textures)
}

/// Uses exactly the same decoded MTL pixels as the voxelizer. The texture data
/// are shared by Arc, so this only creates lightweight GPU asset handles.
pub fn build_obj_source_preview_with_textures(
    model: &ObjModel,
    decoded_textures: &[Option<Arc<SourceTexture>>],
) -> Result<Arc<ObjSourcePreview>, String> {
    if decoded_textures.len() != model.materials.len() {
        return Err("OBJ texture table has incorrect material count".to_owned());
    }
    let total_triangles = model.triangles.len();
    if total_triangles == 0 {
        return Err("OBJ 没有可显示的三角形".to_owned());
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for triangle in &model.triangles {
        for vertex in &triangle.vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex.position[axis]);
                max[axis] = max[axis].max(vertex.position[axis]);
            }
        }
    }
    if !min.iter().chain(max.iter()).all(|coordinate| coordinate.is_finite()) {
        return Err("OBJ 模型包含无效坐标".to_owned());
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let span = (0..3).map(|axis| max[axis] - min[axis])
        .fold(0.0_f32, f32::max).max(1.0e-5);
    let stride = total_triangles.div_ceil(MAX_PREVIEW_TRIANGLES);
    let count = total_triangles.div_ceil(stride);
    let fallback_slot = model.materials.len();
    let mut groups = vec![Vec::new(); fallback_slot + 1];
    for triangle in model.triangles.iter().step_by(stride) {
        groups[triangle.material.filter(|index| *index < fallback_slot).unwrap_or(fallback_slot)]
            .push(triangle);
    }
    let mut vertices = Vec::<Vertex>::with_capacity(count * 3);
    let mut indices = Vec::<u32>::with_capacity(count * 3);
    let mut parts = Vec::<MeshPart>::new();

    // Each OBJ material owns a real gpui-3d material slot. A textured OBJ
    // cannot be rendered correctly through one default white material even if
    // its triangles preserve diffuse colors as vertex attributes.
    let mut textures = Vec::<Arc<TextureAsset>>::new();
    let mut textured_materials = 0_usize;
    let mut texture_by_source = HashMap::<usize, Arc<TextureAsset>>::new();
    let mut materials = Vec::<Arc<Material>>::with_capacity(groups.len());
    for (slot, triangles) in groups.into_iter().enumerate() {
        let source_material = model.materials.get(slot);
        let mut material = Material::new();
        material.shading_model = ShadingModel::Unlit;
        material.double_sided = true;
        let mut rgb = [0.79, 0.81, 0.86];
        let mut opacity = 1.0;
        if let Some(source) = source_material {
            rgb = source.diffuse.unwrap_or(if source.diffuse_texture.is_some() {
                [1.0; 3]
            } else {
                [0.79, 0.81, 0.86]
            });
            opacity = source.opacity;
            if let Some(decoded) = decoded_textures.get(slot).and_then(Option::as_ref) {
                let key = Arc::as_ptr(decoded) as usize;
                let texture = if let Some(asset) = texture_by_source.get(&key) {
                    asset.clone()
                } else {
                    let asset = Arc::new(TextureAsset::rgba8(
                        decoded.width,
                        decoded.height,
                        decoded.pixels.clone(),
                    ).map_err(|error| error.to_string())?);
                    textures.push(asset.clone());
                    texture_by_source.insert(key, asset.clone());
                    asset
                };
                material.albedo_texture = Some(texture.id());
                textured_materials += 1;
                if decoded.has_alpha {
                    material.alpha_mode = AlphaMode::Blend;
                }
            }
        }
        if opacity < 1.0 {
            material.alpha_mode = AlphaMode::Blend;
        }
        // Material Kd multiplies sampled map_Kd in linear space on the GPU.
        // Vertex color remains neutral to avoid double multiplication.
        material.base_color = [rgb[0], rgb[1], rgb[2], opacity];
        materials.push(Arc::new(material));

        let part_start = u32::try_from(indices.len())
            .map_err(|_| "原始 OBJ 预览索引超出 u32 上限".to_owned())?;
        for triangle in triangles {
            let a = triangle.vertices[0].position;
            let b = triangle.vertices[1].position;
            let c = triangle.vertices[2].position;
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let length = (cross[0] * cross[0] + cross[1] * cross[1]
                + cross[2] * cross[2]).sqrt();
            if !length.is_finite() || length < 1.0e-10 {
                continue;
            }
            let normal = Vec3::new(cross[0] / length, cross[1] / length, cross[2] / length);
            for source in triangle.vertices {
                let position = source.position;
                let base = Vec3::new(
                    (position[0] - center[0]) / span * 2.0,
                    (position[1] - center[1]) / span * 2.0,
                    (position[2] - center[2]) / span * 2.0,
                );
                // OBJ's V is bottom-up, decoded PNG/JPEG textures are top-down.
                let uv = source.uv.unwrap_or([0.0, 0.0]);
                indices.push(vertices.len() as u32);
                vertices.push(Vertex {
                    position: base,
                    normal,
                    uv: Vec2::new(uv[0], 1.0 - uv[1]),
                    color: [1.0; 4],
                });
            }
        }
        let count = u32::try_from(indices.len())
            .map_err(|_| "原始 OBJ 预览索引超出 u32 上限".to_owned())?
            - part_start;
        if count > 0 {
            parts.push(MeshPart::new(part_start, count, slot as u32));
        }
    }
    let displayed_triangles = indices.len() / 3;
    let mesh = Arc::new(Mesh::with_parts(vertices, indices, parts)
        .map_err(|error| error.to_string())?);
    let mut scene = Scene::new();
    scene.insert(None, Node::new().with_mesh(mesh).with_materials(materials))
        .map_err(|error| error.to_string())?;
    let imported_scene = Arc::new(ImportedScene::from_parts(
        scene, textures, displayed_triangles,
    ));
    let view = imported_scene.view(
        Camera::perspective(
            Vec3::new(2.5, 1.5, 2.6),
            Vec3::ZERO, Vec3::Y,
            50.0_f32.to_radians(), 0.05, 64.0,
        ),
    )
    .map_err(|error| error.to_string())?
    .with_projection_region(ProjectionRegion::VisibleContent)
    .with_projection_inset(4.0, 0.06)
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(ObjSourcePreview {
        scene_view: Arc::new(view),
        imported_scene,
        source_normalization: Mat4::IDENTITY,
        total_triangles,
        displayed_triangles,
        textured_materials,
    }))
}


/// Reuses a glTF/GLB GPUI-3D scene directly. Unlike rebuilding a preview mesh
/// from voxel triangles, this retains the source's node hierarchy, materials
/// and texture handles, with no redundant model decoding.
pub fn source_preview_from_imported(
    scene: Arc<ImportedScene>,
    model: &ObjModel,
) -> Result<Arc<ObjSourcePreview>, String> {
    if model.triangles.is_empty() {
        return Err("模型没有可显示的三角形".to_owned());
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for tri in &model.triangles {
        for v in tri.vertices {
            for i in 0..3 {
                min[i] = min[i].min(v.position[i]);
                max[i] = max[i].max(v.position[i]);
            }
        }
    }
    let center = [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5);
    let span = (0..3).map(|i| max[i] - min[i])
        .fold(0.0_f32, f32::max).max(1.0e-5);
    let scale = 2.0 / span;
    let normalization = Mat4::from_columns([
        [scale, 0.0, 0.0, 0.0],
        [0.0, scale, 0.0, 0.0],
        [0.0, 0.0, scale, 0.0],
        [-center[0] * scale, -center[1] * scale, -center[2] * scale, 1.0],
    ]);
    let view = scene.view(Camera::perspective(
        Vec3::new(2.5, 1.5, 2.6),
        Vec3::ZERO, Vec3::Y,
        50.0_f32.to_radians(), 0.05, 64.0,
    )).map_err(|error| error.to_string())?
        .with_projection_region(ProjectionRegion::VisibleContent)
        .with_projection_inset(4.0, 0.06)
        .map_err(|error| error.to_string())?;
    let textured_materials = scene.textures().len();
    Ok(Arc::new(ObjSourcePreview {
        scene_view: Arc::new(view),
        imported_scene: scene,
        source_normalization: normalization,
        total_triangles: model.triangles.len(),
        displayed_triangles: model.triangles.len(),
        textured_materials,
    }))
}

#[cfg(test)]
mod tests {
    use super::build_obj_source_preview;
    use bedrock_voxel::{ObjModel, ObjTriangle, ObjVertex};

    #[test]
    fn loads_original_mtl_texture_into_gpu_scene() {
        use bedrock_voxel::ObjMaterial;
        let path = std::env::temp_dir().join(format!(
            "bmcbl-obj-source-preview-{}.png", std::process::id()
        ));
        let image = image::RgbaImage::from_raw(
            2, 2,
            vec![255, 0, 0, 255, 0, 255, 0, 255,
                 0, 0, 255, 255, 255, 255, 0, 255],
        ).expect("valid RGBA");
        image.save(&path).expect("write texture fixture");
        let model = ObjModel {
            triangles: vec![ObjTriangle {
                vertices: [
                    ObjVertex { position: [0.0, 0.0, 0.0], uv: Some([0.0, 0.0]) },
                    ObjVertex { position: [1.0, 0.0, 0.0], uv: Some([1.0, 0.0]) },
                    ObjVertex { position: [0.0, 1.0, 0.0], uv: Some([0.0, 1.0]) },
                ],
                material: Some(0),
            }],
            materials: vec![ObjMaterial {
                diffuse: Some([1.0; 3]),
                opacity: 1.0,
                diffuse_texture: Some(path.clone()),
            }],
        };
        let preview = build_obj_source_preview(&model).expect("textured preview");
        assert_eq!(preview.textured_materials, 1);
        assert_eq!(preview.displayed_triangles, 1);
        assert_eq!(preview.imported_scene.triangle_count, 1);
        std::fs::remove_file(&path).expect("clean fixture");
    }

    #[test]
    fn creates_original_triangle_not_a_voxel_mesh() {
        let model = ObjModel {
            triangles: vec![ObjTriangle {
                vertices: [
                    ObjVertex { position: [0.0, 0.0, 0.0], uv: None },
                    ObjVertex { position: [1.0, 0.0, 0.0], uv: None },
                    ObjVertex { position: [0.0, 1.0, 0.0], uv: None },
                ],
                material: None,
            }],
            materials: vec![],
        };
        let preview = build_obj_source_preview(&model).expect("triangle");
        assert_eq!(preview.total_triangles, 1);
        assert_eq!(preview.displayed_triangles, 1);
    }
}
