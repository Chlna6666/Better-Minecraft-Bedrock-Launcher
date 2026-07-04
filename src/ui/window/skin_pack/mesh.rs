use gpui::{
    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dVertex,
};
use image::{DynamicImage, GenericImageView as _};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::color::{Face, sample_skin_color, shade_face_color};
use super::math::{mat4_mul, mat4_rotation_x, mat4_rotation_y, mat4_scale, mat4_translation};
use super::shader::skin_preview_shader;
use super::uv::{CuboidUv, TextureRegion, arm_uv, body_uv, head_uv, leg_uv};

const SKIN_MIN_SIZE: u32 = 64;
const LEG_WIDTH: f32 = 4.0;
const LIMB_DEPTH: f32 = 4.0;

#[derive(Clone)]
pub(super) struct SkinPreviewMeshes {
    pub(super) parts: Arc<[SkinPreviewPartMesh]>,
}

#[derive(Clone)]
pub(super) struct SkinPreviewPartMesh {
    pub(super) part: SkinPreviewPart,
    pub(super) mesh: Arc<GpuMesh3d>,
}

#[derive(Clone, Copy)]
pub(super) enum SkinPreviewPart {
    Head,
    Body,
    RightArm { width: f32 },
    LeftArm { width: f32 },
    RightLeg,
    LeftLeg,
}

#[derive(Clone, Copy)]
struct CuboidSize {
    width: f32,
    height: f32,
    depth: f32,
}

pub(super) fn skin_player_mesh(
    texture_path: &Path,
    slim_arms: bool,
) -> Result<Arc<SkinPreviewMeshes>, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<SkinPreviewMeshes>>>> = OnceLock::new();

    let cache_key = format!("{}|slim={slim_arms}", texture_path.to_string_lossy());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(meshes) = cache.get(&cache_key)
    {
        return Ok(meshes.clone());
    }

    let image = image::open(texture_path).map_err(|error| format!("读取皮肤贴图失败: {error}"))?;
    let meshes = Arc::new(build_skin_player_meshes(&image, slim_arms)?);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(cache_key, meshes.clone());
    }
    Ok(meshes)
}

pub(super) fn skin_preview_draw_parameters(
    aspect: f32,
    part: SkinPreviewPart,
    view_yaw: f32,
    view_pitch: f32,
    walk_phase: f32,
    walking: bool,
) -> GpuMesh3dDrawParameters {
    let aspect_x = if aspect > 1.0 { 1.0 / aspect } else { 1.0 };
    let aspect_y = if aspect < 1.0 { aspect } else { 1.0 };
    let swing = if walking {
        walk_phase.sin() * 0.55
    } else {
        0.0
    };
    let view_rotation = mat4_mul(mat4_rotation_y(view_yaw), mat4_rotation_x(view_pitch));
    let model_scale = mat4_scale([0.057 * aspect_x, 0.057 * aspect_y, 0.057]);
    let part_transform = skin_part_transform(part, swing);

    GpuMesh3dDrawParameters {
        view_projection_model: mat4_mul(model_scale, mat4_mul(view_rotation, part_transform)),
    }
}

fn build_skin_player_meshes(
    image: &DynamicImage,
    slim_arms: bool,
) -> Result<SkinPreviewMeshes, String> {
    let (width, height) = image.dimensions();
    if width < SKIN_MIN_SIZE || height < 32 {
        return Err(format!("皮肤贴图尺寸过小: {width}x{height}"));
    }

    let unit = (width / SKIN_MIN_SIZE).max(1);
    let has_extended_skin = height >= 64;
    let arm_width = if slim_arms { 3.0 } else { 4.0 };
    let mut parts = Vec::with_capacity(6);
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::Head,
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(false),
        Some(head_uv(true)),
    )?;
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::Body,
        CuboidSize {
            width: 8.0,
            height: 12.0,
            depth: 4.0,
        },
        body_uv(false),
        has_extended_skin.then(|| body_uv(true)),
    )?;
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::RightArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(false, false, slim_arms),
        has_extended_skin.then(|| arm_uv(false, true, slim_arms)),
    )?;
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::LeftArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(has_extended_skin, false, slim_arms),
        has_extended_skin.then(|| arm_uv(true, true, slim_arms)),
    )?;
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::RightLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(false, false),
        has_extended_skin.then(|| leg_uv(false, true)),
    )?;
    push_part(
        &mut parts,
        image,
        unit,
        SkinPreviewPart::LeftLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(has_extended_skin, false),
        has_extended_skin.then(|| leg_uv(true, true)),
    )?;

    Ok(SkinPreviewMeshes {
        parts: Arc::from(parts.into_boxed_slice()),
    })
}

fn push_part(
    parts: &mut Vec<SkinPreviewPartMesh>,
    image: &DynamicImage,
    unit: u32,
    part: SkinPreviewPart,
    size: CuboidSize,
    base_uv: CuboidUv,
    overlay_uv: Option<CuboidUv>,
) -> Result<(), String> {
    let mut vertices = Vec::with_capacity(256);
    let mut indices = Vec::with_capacity(384);
    push_cuboid(
        image,
        unit,
        size,
        base_uv,
        0.0,
        false,
        &mut vertices,
        &mut indices,
    );
    if let Some(overlay_uv) = overlay_uv {
        push_cuboid(
            image,
            unit,
            size,
            overlay_uv,
            0.08,
            true,
            &mut vertices,
            &mut indices,
        );
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
        skin_preview_shader()?,
    );
    parts.push(SkinPreviewPartMesh {
        part,
        mesh: Arc::new(mesh),
    });
    Ok(())
}

fn push_cuboid(
    image: &DynamicImage,
    unit: u32,
    size: CuboidSize,
    uv: CuboidUv,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    push_face(
        image,
        unit,
        size,
        Face::Top,
        uv.top,
        inflate,
        transparent,
        vertices,
        indices,
    );
    push_face(
        image,
        unit,
        size,
        Face::Bottom,
        uv.bottom,
        inflate,
        transparent,
        vertices,
        indices,
    );
    push_face(
        image,
        unit,
        size,
        Face::Right,
        uv.right,
        inflate,
        transparent,
        vertices,
        indices,
    );
    push_face(
        image,
        unit,
        size,
        Face::Front,
        uv.front,
        inflate,
        transparent,
        vertices,
        indices,
    );
    push_face(
        image,
        unit,
        size,
        Face::Left,
        uv.left,
        inflate,
        transparent,
        vertices,
        indices,
    );
    push_face(
        image,
        unit,
        size,
        Face::Back,
        uv.back,
        inflate,
        transparent,
        vertices,
        indices,
    );
}

fn push_face(
    image: &DynamicImage,
    unit: u32,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    for py in 0..region.height {
        for px in 0..region.width {
            let mut color = sample_skin_color(image, unit, region.x + px, region.y + py);
            if color[3] <= 0.04 && transparent {
                continue;
            }
            if !transparent {
                color[3] = color[3].max(1.0);
            }
            let corners = face_pixel_corners(size, face, region, px, py, inflate);
            push_quad(vertices, indices, corners, shade_face_color(color, face));
        }
    }
}

fn face_pixel_corners(
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    px: u32,
    py: u32,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let half_width = size.width * 0.5 + inflate;
    let half_height = size.height * 0.5 + inflate;
    let half_depth = size.depth * 0.5 + inflate;
    let u0 = px as f32 / region.width as f32;
    let u1 = (px + 1) as f32 / region.width as f32;
    let v0 = py as f32 / region.height as f32;
    let v1 = (py + 1) as f32 / region.height as f32;

    match face {
        Face::Front => front_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Back => front_face(
            half_width,
            half_height,
            -half_depth,
            1.0 - u0,
            1.0 - u1,
            v0,
            v1,
        ),
        Face::Right => side_face(-half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Left => side_face(
            half_width,
            half_height,
            half_depth,
            1.0 - u0,
            1.0 - u1,
            v0,
            v1,
        ),
        Face::Top => cap_face(half_width, half_depth, half_height, u0, u1, v0, v1),
        Face::Bottom => cap_face(
            half_width,
            half_depth,
            -half_height,
            u0,
            u1,
            1.0 - v0,
            1.0 - v1,
        ),
    }
}

fn front_face(w: f32, h: f32, z: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]]
}

fn side_face(x: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let z0 = d - u0 * d * 2.0;
    let z1 = d - u1 * d * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[x, y0, z0], [x, y0, z1], [x, y1, z1], [x, y1, z0]]
}

fn cap_face(w: f32, d: f32, y: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let z0 = d - v0 * d * 2.0;
    let z1 = d - v1 * d * 2.0;
    [[x0, y, z1], [x1, y, z1], [x1, y, z0], [x0, y, z0]]
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
    }
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

fn push_quad(
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    color: [f32; 4],
) {
    let Ok(base) = u32::try_from(vertices.len()) else {
        return;
    };
    vertices.extend(corners.map(|position| GpuMesh3dVertex { position, color }));
    indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pose_places_limbs_on_body_sides() {
        for (part, expected) in [
            (SkinPreviewPart::RightArm { width: 4.0 }, [-6.0, 2.0, 0.0]),
            (SkinPreviewPart::LeftArm { width: 4.0 }, [6.0, 2.0, 0.0]),
            (SkinPreviewPart::RightArm { width: 3.0 }, [-5.5, 2.0, 0.0]),
            (SkinPreviewPart::RightLeg, [-2.0, -10.0, 0.0]),
            (SkinPreviewPart::LeftLeg, [2.0, -10.0, 0.0]),
        ] {
            assert_translation(skin_part_transform(part, 0.0), expected);
        }
    }

    fn assert_translation(matrix: [[f32; 4]; 4], expected: [f32; 3]) {
        for (actual, expected) in [matrix[3][0], matrix[3][1], matrix[3][2]]
            .into_iter()
            .zip(expected)
        {
            assert!(
                (actual - expected).abs() < 0.001,
                "expected translation {expected}, got {actual}",
            );
        }
    }
}
