//! Bounded source-geometry preview for OBJ converters.
//! This is the actual triangulated OBJ geometry (not Minecraft voxels). MTL diffuse
//! colors and alpha are retained; texture sampling will be supported separately.

use std::sync::Arc;

use bedrock_voxel::ObjModel;
use gpui_3d::{
    Camera, Material, Mesh, Node, ProjectionRegion, Scene, SceneView,
    ShadingModel, Vec2, Vec3, Vertex,
};

/// Limit CPU/GPU retention while preserving a deterministic overview of huge models.
const MAX_PREVIEW_TRIANGLES: usize = 80_000;

#[derive(Clone, Debug)]
pub struct ObjSourcePreview {
    pub scene_view: Arc<SceneView>,
    pub total_triangles: usize,
    pub displayed_triangles: usize,
}

/// Builds an immutable, centered source mesh, on the TaskManager CPU worker.
/// This intentionally does not open a world or create GPUI-owned futures.
pub fn build_obj_source_preview(model: &ObjModel) -> Result<Arc<ObjSourcePreview>, String> {
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
    let mut vertices = Vec::<Vertex>::with_capacity(count * 3);
    let mut indices = Vec::<u32>::with_capacity(count * 3);

    for triangle in model.triangles.iter().step_by(stride) {
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
        let color = triangle.material.and_then(|index| model.materials.get(index))
            .map_or([0.79, 0.81, 0.86, 1.0], |material| {
                let rgb = material.diffuse.unwrap_or([0.79, 0.81, 0.86]);
                [rgb[0], rgb[1], rgb[2], material.opacity]
            });
        for source in triangle.vertices {
            let position = source.position;
            let base = Vec3::new(
                (position[0] - center[0]) / span * 2.0,
                (position[1] - center[1]) / span * 2.0,
                (position[2] - center[2]) / span * 2.0,
            );
            let uv = source.uv.unwrap_or([0.0, 0.0]);
            indices.push(vertices.len() as u32);
            vertices.push(Vertex {
                position: base,
                normal,
                uv: Vec2::new(uv[0], uv[1]),
                color,
            });
        }
    }
    let displayed_triangles = indices.len() / 3;
    let mesh = Arc::new(Mesh::new(vertices, indices).map_err(|e| e.to_string())?);
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.shading_model = ShadingModel::Unlit;
    material.double_sided = true;
    scene.insert(None, Node::new().with_mesh(mesh).with_materials([Arc::new(material)]))
        .map_err(|e| e.to_string())?;
    let view = SceneView::new(
        Arc::new(scene),
        Camera::perspective(
            Vec3::new(2.5, 1.5, 2.6),
            Vec3::ZERO, Vec3::Y,
            50.0_f32.to_radians(), 0.05, 64.0,
        ),
    )
    .with_projection_region(ProjectionRegion::VisibleContent)
    .with_projection_inset(4.0, 0.06)
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(ObjSourcePreview {
        scene_view: Arc::new(view),
        total_triangles,
        displayed_triangles,
    }))
}

#[cfg(test)]
mod tests {
    use super::build_obj_source_preview;
    use bedrock_voxel::{ObjModel, ObjTriangle, ObjVertex};

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
