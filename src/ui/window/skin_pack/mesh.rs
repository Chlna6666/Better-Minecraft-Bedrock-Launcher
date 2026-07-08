use gpui::{
    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dShader,
};
use image::{DynamicImage, GenericImageView as _};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::custom_geometry::{
    CustomGeometryMesh, CustomGeometryPartMesh, build_custom_geometry_mesh,
};
use super::custom_geometry_animation::CustomGeometryBoneRole;
use super::geometry::{
    CuboidSize, SKIN_MIN_SIZE, SkinTextureScale, cuboid_uv_pixel_count, face_region, push_face,
    skin_preview_faces,
};
use super::layer::push_skin_layer;
use super::math::{
    mat4_identity, mat4_mul, mat4_rotation_x, mat4_rotation_y, mat4_scale, mat4_translation,
};
use super::shader::skin_preview_shader;
use super::uv::{CuboidUv, arm_uv, body_uv, head_uv, leg_uv};
use crate::core::minecraft::skin_pack_preview::open_skin_texture;

const SKIN_OVERLAY_INFLATE: f32 = 0.24;
const LEG_WIDTH: f32 = 4.0;
const LIMB_DEPTH: f32 = 4.0;
const SKIN_PREVIEW_ANTIALIAS_OPACITY: f32 = 0.16;
const SKIN_PREVIEW_ANTIALIAS_DEPTH_BIAS: f32 = 0.002;
const SKIN_PREVIEW_ANTIALIAS_PASSES: [SkinPreviewAntialiasPass; 4] = [
    SkinPreviewAntialiasPass {
        pixel_offset: [-0.45, -0.45],
    },
    SkinPreviewAntialiasPass {
        pixel_offset: [0.45, -0.45],
    },
    SkinPreviewAntialiasPass {
        pixel_offset: [-0.45, 0.45],
    },
    SkinPreviewAntialiasPass {
        pixel_offset: [0.45, 0.45],
    },
];

#[derive(Clone, Copy)]
struct SkinPreviewAntialiasPass {
    pixel_offset: [f32; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SkinLayerMode {
    Flat,
    Extruded,
}

impl SkinLayerMode {
    pub(super) const fn is_extruded(self) -> bool {
        matches!(self, Self::Extruded)
    }

    const fn cache_label(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Extruded => "extruded",
        }
    }
}

#[derive(Clone)]
pub(super) struct SkinPreviewGeometrySource {
    pub(super) path: String,
    pub(super) identifier: String,
}

#[derive(Clone)]
pub(super) struct SkinPreviewMeshes {
    parts: Arc<[SkinPreviewPartMesh]>,
}

#[derive(Clone)]
struct SkinPreviewPartMesh {
    part: SkinPreviewPart,
    mesh: Arc<GpuMesh3d>,
}

pub(super) struct SkinPreviewPaintMesh {
    pub(super) mesh: Arc<GpuMesh3d>,
    pub(super) parameters: GpuMesh3dDrawParameters,
}

#[derive(Clone, Copy)]
pub(super) enum SkinPreviewPart {
    Head,
    Body,
    RightArm {
        width: f32,
    },
    LeftArm {
        width: f32,
    },
    RightLeg,
    LeftLeg,
    CustomGeometryBone {
        role: CustomGeometryBoneRole,
        pivot: [f32; 3],
    },
}

pub(super) fn skin_player_mesh(
    texture_path: &Path,
    slim_arms: bool,
    layer_mode: SkinLayerMode,
    geometry_source: Option<SkinPreviewGeometrySource>,
) -> Result<Arc<SkinPreviewMeshes>, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<SkinPreviewMeshes>>>> = OnceLock::new();

    let geometry_cache_key = geometry_source.as_ref().map_or_else(
        || "geometry=none".to_string(),
        |geometry| format!("geometry={}|id={}", geometry.path, geometry.identifier),
    );
    let cache_key = format!(
        "{}|slim={slim_arms}|layer={}|{}",
        texture_path.to_string_lossy(),
        layer_mode.cache_label(),
        geometry_cache_key
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(meshes) = cache.get(&cache_key)
    {
        return Ok(meshes.clone());
    }

    let image = open_skin_texture(texture_path).map_err(|error| format!("{error:#}"))?;
    let meshes = if let Some(geometry_source) = geometry_source.as_ref()
        && let Some(custom_mesh) = build_custom_geometry_mesh(
            &image,
            Path::new(&geometry_source.path),
            &geometry_source.identifier,
        )? {
        Arc::new(build_custom_geometry_meshes(custom_mesh)?)
    } else {
        Arc::new(build_skin_player_meshes(&image, slim_arms, layer_mode)?)
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(cache_key, meshes.clone());
    }
    Ok(meshes)
}

pub(super) fn skin_preview_paint_meshes(
    meshes: &SkinPreviewMeshes,
    aspect: f32,
    view_yaw: f32,
    view_pitch: f32,
    view_zoom: f32,
    walk_phase: f32,
    walking: bool,
) -> Vec<SkinPreviewPaintMesh> {
    if meshes.parts.is_empty() {
        return Vec::new();
    }

    let swing = skin_walk_swing(walk_phase, walking);
    let view_rotation = skin_preview_view_rotation(view_yaw, view_pitch);
    let mut paint_meshes = Vec::with_capacity(
        meshes
            .parts
            .len()
            .saturating_mul(1 + SKIN_PREVIEW_ANTIALIAS_PASSES.len()),
    );

    for part in meshes.parts.iter() {
        let view_part_transform = skin_preview_view_part_transform(part.part, view_rotation, swing);
        paint_meshes.push(SkinPreviewPaintMesh {
            mesh: part.mesh.clone(),
            parameters: skin_preview_draw_parameters_for_transform(
                aspect,
                view_part_transform,
                view_zoom,
                [0.0, 0.0],
                1.0,
                0.0,
            ),
        });
    }
    for pass in SKIN_PREVIEW_ANTIALIAS_PASSES {
        for part in meshes.parts.iter() {
            let view_part_transform =
                skin_preview_view_part_transform(part.part, view_rotation, swing);
            paint_meshes.push(SkinPreviewPaintMesh {
                mesh: part.mesh.clone(),
                parameters: skin_preview_draw_parameters_for_transform(
                    aspect,
                    view_part_transform,
                    view_zoom,
                    pass.pixel_offset,
                    SKIN_PREVIEW_ANTIALIAS_OPACITY,
                    SKIN_PREVIEW_ANTIALIAS_DEPTH_BIAS,
                ),
            });
        }
    }
    paint_meshes
}

fn skin_preview_draw_parameters_for_transform(
    _aspect: f32,
    view_part_transform: [[f32; 4]; 4],
    view_zoom: f32,
    pixel_offset: [f32; 2],
    opacity: f32,
    depth_bias: f32,
) -> GpuMesh3dDrawParameters {
    let scale = 0.057 * view_zoom.max(0.01);
    let model_scale = mat4_scale([scale, scale, scale]);
    let mut view_projection_model = mat4_mul(model_scale, view_part_transform);
    encode_skin_preview_draw_metadata(
        &mut view_projection_model,
        opacity,
        pixel_offset,
        depth_bias,
    );

    GpuMesh3dDrawParameters {
        view_projection_model,
    }
}

fn encode_skin_preview_draw_metadata(
    view_projection_model: &mut [[f32; 4]; 4],
    opacity: f32,
    pixel_offset: [f32; 2],
    depth_bias: f32,
) {
    view_projection_model[0][3] = opacity.clamp(0.0, 1.0);
    view_projection_model[1][3] = pixel_offset[0];
    view_projection_model[2][3] = pixel_offset[1];
    view_projection_model[3][3] = 1.0 + depth_bias.max(0.0);
}

fn skin_walk_swing(walk_phase: f32, walking: bool) -> f32 {
    if walking {
        walk_phase.sin() * 0.55
    } else {
        0.0
    }
}

fn skin_preview_view_rotation(view_yaw: f32, view_pitch: f32) -> [[f32; 4]; 4] {
    mat4_mul(mat4_rotation_y(view_yaw), mat4_rotation_x(view_pitch))
}

fn skin_preview_view_part_transform(
    part: SkinPreviewPart,
    view_rotation: [[f32; 4]; 4],
    swing: f32,
) -> [[f32; 4]; 4] {
    mat4_mul(view_rotation, skin_part_transform(part, swing))
}

fn build_custom_geometry_meshes(
    custom_mesh: CustomGeometryMesh,
) -> Result<SkinPreviewMeshes, String> {
    let shader = skin_preview_shader()?;
    let mut parts = Vec::with_capacity(custom_mesh.parts.len());

    for custom_part in custom_mesh.parts {
        let part = SkinPreviewPart::CustomGeometryBone {
            role: custom_part.role,
            pivot: custom_part.pivot,
        };
        parts.push(SkinPreviewPartMesh {
            part,
            mesh: Arc::new(build_custom_geometry_part_mesh(
                custom_part,
                shader.clone(),
            )?),
        });
    }

    Ok(SkinPreviewMeshes {
        parts: Arc::from(parts.into_boxed_slice()),
    })
}

fn build_custom_geometry_part_mesh(
    custom_part: CustomGeometryPartMesh,
    shader: Arc<GpuMesh3dShader>,
) -> Result<GpuMesh3d, String> {
    let count =
        u32::try_from(custom_part.indices.len()).map_err(|_| "3D 网格索引过多".to_string())?;
    let mesh = GpuMesh3d::new(
        Arc::from(custom_part.vertices.into_boxed_slice()),
        Arc::from(custom_part.indices.into_boxed_slice()),
        GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange { start: 0, count },
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        [0.0, 0.0, 0.0],
        1.0,
        1.0,
        shader,
    );

    Ok(mesh)
}

fn build_skin_player_meshes(
    image: &DynamicImage,
    slim_arms: bool,
    layer_mode: SkinLayerMode,
) -> Result<SkinPreviewMeshes, String> {
    let (width, height) = image.dimensions();
    if width < SKIN_MIN_SIZE || height < 32 {
        return Err(format!("皮肤贴图尺寸过小: {width}x{height}"));
    }

    let texture_scale = SkinTextureScale::from_width(width);
    let has_extended_skin = height >= 64;
    let arm_width = if slim_arms { 3.0 } else { 4.0 };
    let shader = skin_preview_shader()?;
    let mut parts = Vec::with_capacity(6);
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader.clone(),
        SkinPreviewPart::Head,
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(false),
        Some(head_uv(true)),
        layer_mode,
    )?;
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader.clone(),
        SkinPreviewPart::Body,
        CuboidSize {
            width: 8.0,
            height: 12.0,
            depth: 4.0,
        },
        body_uv(false),
        has_extended_skin.then(|| body_uv(true)),
        layer_mode,
    )?;
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader.clone(),
        SkinPreviewPart::RightArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(false, false, slim_arms),
        has_extended_skin.then(|| arm_uv(false, true, slim_arms)),
        layer_mode,
    )?;
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader.clone(),
        SkinPreviewPart::LeftArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(has_extended_skin, false, slim_arms),
        has_extended_skin.then(|| arm_uv(true, true, slim_arms)),
        layer_mode,
    )?;
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader.clone(),
        SkinPreviewPart::RightLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(false, false),
        has_extended_skin.then(|| leg_uv(false, true)),
        layer_mode,
    )?;
    push_part(
        &mut parts,
        image,
        texture_scale,
        shader,
        SkinPreviewPart::LeftLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(has_extended_skin, false),
        has_extended_skin.then(|| leg_uv(true, true)),
        layer_mode,
    )?;

    Ok(SkinPreviewMeshes {
        parts: Arc::from(parts.into_boxed_slice()),
    })
}

fn push_part(
    parts: &mut Vec<SkinPreviewPartMesh>,
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    shader: Arc<GpuMesh3dShader>,
    part: SkinPreviewPart,
    size: CuboidSize,
    base_uv: CuboidUv,
    overlay_uv: Option<CuboidUv>,
    layer_mode: SkinLayerMode,
) -> Result<(), String> {
    let overlay_quad_multiplier = if layer_mode.is_extruded() { 5 } else { 1 };
    let estimated_quads = cuboid_uv_pixel_count(base_uv, texture_scale.preview).saturating_add(
        overlay_uv.map_or(0, |uv| {
            cuboid_uv_pixel_count(uv, texture_scale.preview).saturating_mul(overlay_quad_multiplier)
        }),
    );
    let mut vertices = Vec::with_capacity(estimated_quads.saturating_mul(6));
    let mut indices = Vec::with_capacity(estimated_quads.saturating_mul(6));

    for face in skin_preview_faces() {
        push_face(
            image,
            texture_scale,
            size,
            *face,
            face_region(base_uv, *face),
            0.0,
            false,
            &mut vertices,
            &mut indices,
        );
    }

    if let Some(overlay_uv) = overlay_uv {
        if layer_mode.is_extruded() {
            push_skin_layer(
                image,
                texture_scale,
                size,
                overlay_uv,
                SKIN_OVERLAY_INFLATE,
                &mut vertices,
                &mut indices,
            );
        } else {
            for face in skin_preview_faces() {
                push_face(
                    image,
                    texture_scale,
                    size,
                    *face,
                    face_region(overlay_uv, *face),
                    SKIN_OVERLAY_INFLATE,
                    true,
                    &mut vertices,
                    &mut indices,
                );
            }
        }
    }

    let count = u32::try_from(indices.len()).map_err(|_| "3D 网格索引过多".to_string())?;
    let mesh = GpuMesh3d::new(
        Arc::from(vertices.into_boxed_slice()),
        Arc::from(indices.into_boxed_slice()),
        GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange { start: 0, count },
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        [0.0, 0.0, 0.0],
        1.0,
        1.0,
        shader,
    );
    parts.push(SkinPreviewPartMesh {
        part,
        mesh: Arc::new(mesh),
    });
    Ok(())
}

fn skin_part_transform(part: SkinPreviewPart, swing: f32) -> [[f32; 4]; 4] {
    match part {
        SkinPreviewPart::Head => mat4_translation([0.0, 12.0, 0.0]),
        SkinPreviewPart::Body => mat4_translation([0.0, 2.0, 0.0]),
        SkinPreviewPart::RightArm { width } => {
            let center_x = -4.0 - width * 0.5;
            limb_transform([center_x, 8.0, 0.0], [center_x, 2.0, 0.0], swing)
        }
        SkinPreviewPart::LeftArm { width } => {
            let center_x = 4.0 + width * 0.5;
            limb_transform([center_x, 8.0, 0.0], [center_x, 2.0, 0.0], -swing)
        }
        SkinPreviewPart::RightLeg => limb_transform([-2.0, -4.0, 0.0], [-2.0, -10.0, 0.0], -swing),
        SkinPreviewPart::LeftLeg => limb_transform([2.0, -4.0, 0.0], [2.0, -10.0, 0.0], swing),
        SkinPreviewPart::CustomGeometryBone { role, pivot } => {
            custom_geometry_bone_transform(role, pivot, swing)
        }
    }
}

fn custom_geometry_bone_transform(
    role: CustomGeometryBoneRole,
    pivot: [f32; 3],
    swing: f32,
) -> [[f32; 4]; 4] {
    let angle = match role {
        CustomGeometryBoneRole::RightArm => swing,
        CustomGeometryBoneRole::LeftArm => -swing,
        CustomGeometryBoneRole::RightLeg => -swing,
        CustomGeometryBoneRole::LeftLeg => swing,
        CustomGeometryBoneRole::Static
        | CustomGeometryBoneRole::Head
        | CustomGeometryBoneRole::Body => 0.0,
    };
    if angle.abs() <= f32::EPSILON {
        return mat4_identity();
    }

    mat4_mul(
        mat4_translation(pivot),
        mat4_mul(
            mat4_rotation_x(angle),
            mat4_translation([-pivot[0], -pivot[1], -pivot[2]]),
        ),
    )
}

fn limb_transform(pivot: [f32; 3], center: [f32; 3], angle: f32) -> [[f32; 4]; 4] {
    mat4_mul(
        mat4_translation(pivot),
        mat4_mul(
            mat4_rotation_x(angle),
            mat4_translation([
                center[0] - pivot[0],
                center[1] - pivot[1],
                center[2] - pivot[2],
            ]),
        ),
    )
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
