use gpui::{
    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dShader,
    GpuMesh3dVertex,
};
use image::{DynamicImage, GenericImageView as _};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::color::{Face, sample_image_color, shade_face_color};
use super::math::{mat4_mul, mat4_rotation_x, mat4_rotation_y, mat4_scale, mat4_translation};
use super::shader::skin_preview_shader;
use super::uv::{CuboidUv, TextureRegion, arm_uv, body_uv, head_uv, leg_uv};

const SKIN_MIN_SIZE: u32 = 64;
const SKIN_OVERLAY_INFLATE: f32 = 0.24;
const SKIN_PREVIEW_MAX_TEXTURE_SCALE: u32 = 2;
const LEG_WIDTH: f32 = 4.0;
const LIMB_DEPTH: f32 = 4.0;

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

#[derive(Clone, Copy)]
struct SkinTextureScale {
    source: u32,
    preview: u32,
}

#[derive(Clone, Copy)]
struct FaceGrid {
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct SkinPreviewPartTransforms {
    head: [[f32; 4]; 4],
    body: [[f32; 4]; 4],
    right_arm_wide: [[f32; 4]; 4],
    left_arm_wide: [[f32; 4]; 4],
    right_arm_slim: [[f32; 4]; 4],
    left_arm_slim: [[f32; 4]; 4],
    right_leg: [[f32; 4]; 4],
    left_leg: [[f32; 4]; 4],
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

pub(super) fn skin_preview_paint_meshes(
    meshes: &SkinPreviewMeshes,
    aspect: f32,
    view_yaw: f32,
    view_pitch: f32,
    walk_phase: f32,
    walking: bool,
) -> Vec<SkinPreviewPaintMesh> {
    if meshes.parts.is_empty() {
        return Vec::new();
    }

    let swing = skin_walk_swing(walk_phase, walking);
    let view_rotation = skin_preview_view_rotation(view_yaw, view_pitch);
    let part_transforms = SkinPreviewPartTransforms::new(view_rotation, swing);
    let mut paint_meshes = Vec::with_capacity(meshes.parts.len());
    for part in meshes.parts.iter() {
        let view_part_transform = part_transforms.for_part(part.part);
        paint_meshes.push(SkinPreviewPaintMesh {
            mesh: part.mesh.clone(),
            parameters: skin_preview_draw_parameters_for_transform(aspect, view_part_transform),
        });
    }
    paint_meshes
}

fn skin_preview_draw_parameters_for_transform(
    _aspect: f32,
    view_part_transform: [[f32; 4]; 4],
) -> GpuMesh3dDrawParameters {
    let model_scale = mat4_scale([0.057, 0.057, 0.057]);

    GpuMesh3dDrawParameters {
        view_projection_model: mat4_mul(model_scale, view_part_transform),
    }
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

impl SkinPreviewPartTransforms {
    fn new(view_rotation: [[f32; 4]; 4], swing: f32) -> Self {
        Self {
            head: skin_preview_view_part_transform(SkinPreviewPart::Head, view_rotation, swing),
            body: skin_preview_view_part_transform(SkinPreviewPart::Body, view_rotation, swing),
            right_arm_wide: skin_preview_view_part_transform(
                SkinPreviewPart::RightArm { width: 4.0 },
                view_rotation,
                swing,
            ),
            left_arm_wide: skin_preview_view_part_transform(
                SkinPreviewPart::LeftArm { width: 4.0 },
                view_rotation,
                swing,
            ),
            right_arm_slim: skin_preview_view_part_transform(
                SkinPreviewPart::RightArm { width: 3.0 },
                view_rotation,
                swing,
            ),
            left_arm_slim: skin_preview_view_part_transform(
                SkinPreviewPart::LeftArm { width: 3.0 },
                view_rotation,
                swing,
            ),
            right_leg: skin_preview_view_part_transform(
                SkinPreviewPart::RightLeg,
                view_rotation,
                swing,
            ),
            left_leg: skin_preview_view_part_transform(
                SkinPreviewPart::LeftLeg,
                view_rotation,
                swing,
            ),
        }
    }

    fn for_part(&self, part: SkinPreviewPart) -> [[f32; 4]; 4] {
        match part {
            SkinPreviewPart::Head => self.head,
            SkinPreviewPart::Body => self.body,
            SkinPreviewPart::RightArm { width } if width < 3.5 => self.right_arm_slim,
            SkinPreviewPart::RightArm { .. } => self.right_arm_wide,
            SkinPreviewPart::LeftArm { width } if width < 3.5 => self.left_arm_slim,
            SkinPreviewPart::LeftArm { .. } => self.left_arm_wide,
            SkinPreviewPart::RightLeg => self.right_leg,
            SkinPreviewPart::LeftLeg => self.left_leg,
        }
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
) -> Result<(), String> {
    let estimated_quads = cuboid_uv_pixel_count(base_uv, texture_scale.preview).saturating_add(
        overlay_uv.map_or(0, |uv| cuboid_uv_pixel_count(uv, texture_scale.preview)),
    );
    let mut vertices = Vec::with_capacity(estimated_quads.saturating_mul(4));
    let mut indices = Vec::with_capacity(estimated_quads.saturating_mul(6));

    for face in skin_preview_faces() {
        push_part_face(
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
        for face in skin_preview_faces() {
            push_part_face(
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

fn push_part_face(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    push_face(
        image,
        texture_scale,
        size,
        face,
        region,
        inflate,
        transparent,
        vertices,
        indices,
    );
}

fn skin_preview_faces() -> &'static [Face; 6] {
    &[
        Face::Top,
        Face::Bottom,
        Face::Right,
        Face::Front,
        Face::Left,
        Face::Back,
    ]
}

fn face_region(uv: CuboidUv, face: Face) -> TextureRegion {
    match face {
        Face::Top => uv.top,
        Face::Bottom => uv.bottom,
        Face::Right => uv.right,
        Face::Front => uv.front,
        Face::Left => uv.left,
        Face::Back => uv.back,
    }
}

fn cuboid_uv_pixel_count(uv: CuboidUv, preview_scale: u32) -> usize {
    skin_preview_faces()
        .iter()
        .map(|face| {
            let region = face_region(uv, *face);
            let grid = face_grid(region, preview_scale);
            (grid.width as usize).saturating_mul(grid.height as usize)
        })
        .sum()
}

fn push_face(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    let grid = face_grid(region, texture_scale.preview);
    let image_origin_x = region.x.saturating_mul(texture_scale.source);
    let image_origin_y = region.y.saturating_mul(texture_scale.source);
    for py in 0..grid.height {
        for px in 0..grid.width {
            let image_x = image_origin_x.saturating_add(source_pixel_offset(px, texture_scale));
            let image_y = image_origin_y.saturating_add(source_pixel_offset(py, texture_scale));
            let mut color = sample_image_color(image, image_x, image_y);
            if color[3] <= 0.04 && transparent {
                continue;
            }
            if !transparent {
                color[3] = color[3].max(1.0);
            }
            let corners = face_pixel_corners(size, face, grid, px, py, inflate);
            push_quad(vertices, indices, corners, shade_face_color(color, face));
        }
    }
}

impl SkinTextureScale {
    fn from_width(width: u32) -> Self {
        let source = (width / SKIN_MIN_SIZE).max(1);
        Self {
            source,
            preview: source.min(SKIN_PREVIEW_MAX_TEXTURE_SCALE).max(1),
        }
    }
}

fn face_grid(region: TextureRegion, preview_scale: u32) -> FaceGrid {
    FaceGrid {
        width: region.width.saturating_mul(preview_scale).max(1),
        height: region.height.saturating_mul(preview_scale).max(1),
    }
}

fn source_pixel_offset(preview_pixel: u32, texture_scale: SkinTextureScale) -> u32 {
    let numerator = preview_pixel
        .saturating_mul(texture_scale.source)
        .saturating_mul(2)
        .saturating_add(texture_scale.source);
    let denominator = texture_scale.preview.saturating_mul(2).max(1);
    numerator / denominator
}

#[cfg(test)]
fn quad_center(corners: [[f32; 3]; 4]) -> [f32; 3] {
    [
        (corners[0][0] + corners[1][0] + corners[2][0] + corners[3][0]) * 0.25,
        (corners[0][1] + corners[1][1] + corners[2][1] + corners[3][1]) * 0.25,
        (corners[0][2] + corners[1][2] + corners[2][2] + corners[3][2]) * 0.25,
    ]
}

fn face_pixel_corners(
    size: CuboidSize,
    face: Face,
    grid: FaceGrid,
    px: u32,
    py: u32,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let half_width = size.width * 0.5 + inflate;
    let half_height = size.height * 0.5 + inflate;
    let half_depth = size.depth * 0.5 + inflate;
    let u0 = px as f32 / grid.width as f32;
    let u1 = (px + 1) as f32 / grid.width as f32;
    let v0 = py as f32 / grid.height as f32;
    let v1 = (py + 1) as f32 / grid.height as f32;

    match face {
        Face::Front => front_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Back => back_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Right => right_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Left => left_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Top => top_face(half_width, half_depth, half_height, u0, u1, v0, v1),
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

fn back_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = w - u0 * w * 2.0;
    let x1 = w - u1 * w * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[x0, y0, -d], [x1, y0, -d], [x1, y1, -d], [x0, y1, -d]]
}

fn right_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let z0 = -d + u0 * d * 2.0;
    let z1 = -d + u1 * d * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[-w, y0, z0], [-w, y0, z1], [-w, y1, z1], [-w, y1, z0]]
}

fn left_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let z0 = d - u0 * d * 2.0;
    let z1 = d - u1 * d * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[w, y0, z0], [w, y0, z1], [w, y1, z1], [w, y1, z0]]
}

fn top_face(w: f32, d: f32, y: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let z0 = -d + v0 * d * 2.0;
    let z1 = -d + v1 * d * 2.0;
    [[x0, y, z1], [x1, y, z1], [x1, y, z0], [x0, y, z0]]
}

fn cap_face(w: f32, d: f32, y: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let z0 = d - v0 * d * 2.0;
    let z1 = d - v1 * d * 2.0;
    [[x1, y, z1], [x0, y, z1], [x0, y, z0], [x1, y, z0]]
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
    indices.extend([
        base,
        base.saturating_add(1),
        base.saturating_add(2),
        base,
        base.saturating_add(2),
        base.saturating_add(3),
    ]);
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
