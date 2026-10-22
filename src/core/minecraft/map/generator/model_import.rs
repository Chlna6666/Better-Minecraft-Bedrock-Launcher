//! Map model import: Wavefront via tobj and glTF/GLB via GPUI-3D / gltf-rs.
//!
//! Parsing belongs on TaskManager workers. Immutable GPUI-3D assets are the
//! source of truth; voxelization receives a bounded compatibility mesh without
//! reparsing glTF or decoding the source textures a second time.

use std::{collections::HashMap, path::Path, sync::Arc};

use bedrock_voxel::{
    ObjMaterial, ObjModel, ObjTriangle, ObjVertex, SourceTexture, load_obj_model, load_obj_textures,
};
use gpui_3d::{GltfLoadOptions, ImportedScene, MaterialId, TextureAssetId};

use super::source_obj_preview::{
    ObjSourcePreview, build_obj_source_preview_with_textures, source_preview_from_imported,
};

pub(super) struct PreparedMapModel {
    pub model: ObjModel,
    pub decoded: Vec<Option<Arc<SourceTexture>>>,
    pub preview: Option<Arc<ObjSourcePreview>>,
    /// Kept resident while downstream consumers share this source.
    pub scene: Option<Arc<ImportedScene>>,
}

pub(super) fn recognized_model(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(
            extension.to_ascii_lowercase().as_str(), "obj" | "gltf" | "glb"
        ))
}

pub(super) fn prepare_map_model(path: &Path) -> Result<PreparedMapModel, String> {
    prepare_model(path, true)
}

/// Export tasks only need source triangles + decoded materials, not an
/// additional render-preview mesh or SceneView.
pub(super) fn prepare_model_for_export(path: &Path) -> Result<PreparedMapModel, String> {
    prepare_model(path, false)
}

fn prepare_model(path: &Path, build_preview: bool) -> Result<PreparedMapModel, String> {
    let extension = path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "obj" => {
            let model = load_obj_model(path).map_err(|error| error.to_string())?;
            let decoded = load_obj_textures(&model).map_err(|error| error.to_string())?;
            let preview = if build_preview {
                build_obj_source_preview_with_textures(&model, &decoded).ok()
            } else { None };
            let scene = preview.as_ref().map(|p| p.imported_scene.clone());
            Ok(PreparedMapModel { model, decoded, preview, scene })
        }
        "gltf" | "glb" => {
            let options = GltfLoadOptions::default();
            let imported = Arc::new(gpui_3d::load_imported_path(path, &options)
                .map_err(|error| format!("读取 glTF/GLB 失败：{error}"))?);
            let (model, decoded) = raster_input_from_scene(&imported)?;
            let preview = if build_preview {
                source_preview_from_imported(imported.clone(), &model).ok()
            } else { None };
            Ok(PreparedMapModel { model, decoded, preview, scene: Some(imported) })
        }
        _ => Err("请选择 OBJ、glTF 或 GLB 模型文件（MTL 随 OBJ 自动解析）".to_string()),
    }
}

pub(super) fn raster_input_from_scene(
    scene: &ImportedScene,
) -> Result<(ObjModel, Vec<Option<Arc<SourceTexture>>>), String> {
    const MAX_TRIANGLES: usize = 2_000_000;
    const MAX_TEXTURE_BYTES: usize = 256 * 1024 * 1024;
    if scene.triangle_count > MAX_TRIANGLES {
        return Err("模型三角形超过 200 万上限".to_owned());
    }

    let mut model = ObjModel { triangles: Vec::new(), materials: Vec::new() };
    model.triangles.reserve(scene.triangle_count.min(MAX_TRIANGLES));
    let mut decoded = Vec::<Option<Arc<SourceTexture>>>::new();
    let mut material_slots = HashMap::<MaterialId, usize>::new();
    let mut texture_cache = HashMap::<TextureAssetId, Arc<SourceTexture>>::new();
    let mut texture_bytes = 0_usize;
    let mut failure = None;
    scene.for_each_triangle(|triangle| {
        if failure.is_some() { return; }
        if model.triangles.len() >= MAX_TRIANGLES {
            failure = Some("模型三角形超过 200 万上限".to_owned());
            return;
        }
        if triangle.positions.iter().any(|p| !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite()) {
            failure = Some("模型包含无效顶点位置".to_owned());
            return;
        }
        let material = triangle.material;
        let slot = material.map(|m| m.id()).and_then(|id| material_slots.get(&id).copied());
        let slot = if let Some(slot) = slot {
            Some(slot)
        } else if let Some(material) = material {
            let rgba = material.base_color;
            let rgb = [rgba[0], rgba[1], rgba[2]].map(|v| v.clamp(0.0, 1.0));
            let opacity = rgba[3].clamp(0.0, 1.0);
            let source_texture = if let Some(albedo) = triangle.albedo {
                if let Some(cached) = texture_cache.get(&albedo.id()) {
                    Some(cached.clone())
                } else {
                    let (width, height) = albedo.dimensions();
                    let pixels = albedo.shared_pixels();
                    texture_bytes = texture_bytes.saturating_add(pixels.len());
                    if texture_bytes > MAX_TEXTURE_BYTES {
                        failure = Some("模型解码纹理超过 256 MiB".to_owned());
                        return;
                    }
                    let item = Arc::new(SourceTexture {
                        width, height,
                        has_alpha: pixels.chunks_exact(4).any(|p| p[3] != 255),
                        pixels,
                    });
                    texture_cache.insert(albedo.id(), item.clone());
                    Some(item)
                }
            } else { None };
            let slot = model.materials.len();
            // OBJ compatibility material. Actual texture ownership is already
            // in decoded; paths never need to be reconstructed or reopened.
            model.materials.push(ObjMaterial {
                diffuse: Some(rgb),
                opacity,
                diffuse_texture: None,
            });
            decoded.push(source_texture);
            material_slots.insert(material.id(), slot);
            Some(slot)
        } else { None };
        let vertices = std::array::from_fn(|i| {
            let p = triangle.positions[i];
            let uv = triangle.uvs[i];
            ObjVertex {
                position: [p.x, p.y, p.z],
                // Raster sampling of ObjModel expects bottom-up Wavefront UVs.
                uv: Some([uv.x, 1.0 - uv.y]),
            }
        });
        model.triangles.push(ObjTriangle { vertices, material: slot });
    });
    if let Some(error) = failure { return Err(error); }
    if model.triangles.is_empty() { return Err("模型没有可体素化的三角形".to_owned()); }
    Ok((model, decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_extensions_are_explicit() {
        for path in ["source.obj", "source.gltf", "source.GLB"] {
            assert!(recognized_model(Path::new(path)));
        }
        for path in ["source.mtl", "source.mcstructure", "source.fbx", "source.png"] {
            assert!(!recognized_model(Path::new(path)));
        }
    }
}
