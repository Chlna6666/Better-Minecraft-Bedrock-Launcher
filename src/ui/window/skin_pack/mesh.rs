use gpui_3d::{
    Aabb, AlphaMode, Camera, Keyframe, Material, Mesh, Node, OrbitCamera, Projection,
    ProjectionRegion, Quat, RotationTrack, Scene, SceneView, ShadingModel, TextureAsset,
    TextureSampling, Transform, TransformTrack, TriangleEdgeMask, Vec2, Vec3, Vertex,
};
use image::{DynamicImage, GenericImageView as _};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::color::{Face, shade_cuboid_face};
use super::custom_geometry::{
    CustomGeometryMesh, CustomGeometryPartMesh, build_custom_geometry_mesh,
};
use super::custom_geometry_animation::CustomGeometryBoneRole;
use super::geometry::{
    ColorRun, CuboidSize, FaceGrid, SKIN_MIN_SIZE, SkinTextureScale, SkinVertex,
    face_rect_corners, face_region, skin_preview_faces,
};
use super::uv::{CuboidUv, TextureRegion, arm_uv, body_uv, head_uv, leg_uv};
use crate::core::minecraft::skin_pack_preview::open_skin_texture;
use std::time::Duration;

const SKIN_OVERLAY_INFLATE: f32 = 0.24;
const LEG_WIDTH: f32 = 4.0;
const LIMB_DEPTH: f32 = 4.0;
const WALK_KEYFRAMES: u32 = 128;
const SKIN_PREVIEW_SCALE: f32 = 0.057;
const PREVIEW_CAMERA_DISTANCE: f32 = 3.0;
const WALK_PERIOD: Duration = Duration::from_nanos(1_208_304_867);
/// Atlas size assumed by the vanilla skin UV layout.
const SKIN_ATLAS_UNITS: f32 = 64.0;
const SKIN_LAYER_ALPHA_CUTOFF: f32 = 0.04;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SkinLayerMode {
    Flat,
    Extruded,
}

impl SkinLayerMode {
    pub(super) const fn is_extruded(self) -> bool {
        matches!(self, Self::Extruded)
    }
}

#[derive(Clone)]
pub(super) struct SkinPreviewGeometrySource {
    pub(super) path: String,
    pub(super) identifier: String,
}

/// Immutable preview resources: a retained scene view plus the skin atlas it samples.
#[derive(Clone)]
pub(super) struct SkinPreviewMeshes {
    scene_view: Arc<SceneView>,
    texture: Arc<TextureAsset>,
    walk_period: Duration,
    bounds: Option<Aabb>,
}

#[derive(Clone)]
struct SkinPreviewPartMesh {
    part: SkinPreviewPart,
    base_mesh: Arc<Mesh>,
    layer_mesh: Option<Arc<Mesh>>,
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
        "{}|slim={slim_arms}|layer={layer_mode:?}|{geometry_cache_key}",
        texture_path.to_string_lossy(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(meshes) = cache.get(&cache_key)
    {
        return Ok(meshes.clone());
    }

    let image = open_skin_texture(texture_path).map_err(|error| format!("{error:#}"))?;
    let texture = skin_texture_asset(&image)?;
    let meshes = if let Some(geometry_source) = geometry_source.as_ref()
        && let Some(custom_mesh) = build_custom_geometry_mesh(
            &image,
            Path::new(&geometry_source.path),
            &geometry_source.identifier,
        )? {
        Arc::new(build_custom_geometry_meshes(image, texture, custom_mesh)?)
    } else {
        Arc::new(build_skin_player_meshes(texture, slim_arms, layer_mode)?)
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(cache_key, meshes.clone());
    }
    Ok(meshes)
}

/// Samples the preview scene at one camera pose.
pub(super) fn skin_preview_scene_view(
    meshes: &SkinPreviewMeshes,
    view_yaw: f32,
    view_pitch: f32,
    view_zoom: f32,
    walk_time: Duration,
) -> Result<Arc<SceneView>, String> {
    let scale = SKIN_PREVIEW_SCALE;
    let scene_transform = Transform {
        scale: Vec3::new(scale, scale, scale),
        ..Transform::IDENTITY
    }
    .matrix();
    let zoom = view_zoom.max(0.01);
    let camera = match meshes.bounds {
        Some(bounds) => OrbitCamera::new(
            Vec3::ZERO,
            -view_yaw,
            view_pitch,
            PREVIEW_CAMERA_DISTANCE,
            Projection::Orthographic {
                height: 1.0,
                near: 0.1,
                far: 100.0,
            },
        )
        .and_then(|camera| camera.fit_bounds(bounds.transformed(scene_transform), 1.0, 1.1))
        .and_then(|camera| camera.zoom(zoom.recip()))
        .map_err(|error| error.to_string())?
        .camera(),
        None => Camera::orthographic(
            Vec3::new(0.0, 0.0, PREVIEW_CAMERA_DISTANCE),
            Vec3::ZERO,
            Vec3::Y,
            2.0 / zoom,
            0.1,
            100.0,
        ),
    };
    let scene_view = meshes
        .scene_view
        .with_camera(camera)
        .with_scene_transform(scene_transform)
        .map_err(|error| error.to_string())?
        .with_animation_time(walk_time.min(meshes.walk_period));
    Ok(Arc::new(scene_view))
}

/// Uploads the skin atlas as an sRGB texture.
///
/// The albedo format performs sRGB decoding in the sampler, which matches the authored per-face
/// shade factors the preview multiplies in.
fn skin_texture_asset(image: &DynamicImage) -> Result<Arc<TextureAsset>, String> {
    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8();
    TextureAsset::rgba8(width, height, Arc::<[u8]>::from(rgba.into_raw()))
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

fn build_custom_geometry_meshes(
    image: DynamicImage,
    texture: Arc<TextureAsset>,
    custom_mesh: CustomGeometryMesh,
) -> Result<SkinPreviewMeshes, String> {
    let mut parts = Vec::with_capacity(custom_mesh.parts.len());
    let size = image.dimensions();

    for custom_part in custom_mesh.parts {
        if custom_part.indices.is_empty() {
            continue;
        }
        parts.push(CustomGeometryPartMesh {
            role: custom_part.role,
            pivot: custom_part.pivot,
            vertices: custom_part.vertices,
            indices: custom_part.indices,
        });
    }
    if parts.is_empty() {
        return Err(format!(
            "自定义皮肤 geometry.json 没有可预览的网格: {}x{}",
            size.0, size.1
        ));
    }

    let mut scene = Scene::new();
    let material = baked_skin_material(AlphaMode::Opaque);

    for part in parts {
        let (pivot, mesh_offset, _) = skin_part_layout(SkinPreviewPart::CustomGeometryBone {
            role: part.role,
            pivot: part.pivot,
        });
        let mesh = Arc::new(build_skin_mesh(part.vertices, part.indices)?);
        insert_part_node(&mut scene, pivot, mesh_offset, mesh, &material)?;
    }

    finish_skin_preview(scene, texture, Vec::new())
}

fn build_skin_player_meshes(
    texture: Arc<TextureAsset>,
    slim_arms: bool,
    layer_mode: SkinLayerMode,
) -> Result<SkinPreviewMeshes, String> {
    let (width, height) = (texture.width(), texture.height());
    if width < SKIN_MIN_SIZE || height < 32 {
        return Err(format!("皮肤贴图尺寸过小: {width}x{height}"));
    }

    let has_extended_skin = height >= 64;
    let arm_width = if slim_arms { 3.0 } else { 4.0 };
    let extruded = layer_mode.is_extruded();
    let mut parts = Vec::with_capacity(6);
    push_part(
        &mut parts,
        SkinPreviewPart::Head,
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(false),
        has_extended_skin.then(|| head_uv(true)),
        extruded,
    )?;
    push_part(
        &mut parts,
        SkinPreviewPart::Body,
        CuboidSize {
            width: 8.0,
            height: 12.0,
            depth: 4.0,
        },
        body_uv(false),
        has_extended_skin.then(|| body_uv(true)),
        extruded,
    )?;
    push_part(
        &mut parts,
        SkinPreviewPart::RightArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(false, false, slim_arms),
        has_extended_skin.then(|| arm_uv(false, true, slim_arms)),
        extruded,
    )?;
    push_part(
        &mut parts,
        SkinPreviewPart::LeftArm { width: arm_width },
        CuboidSize {
            width: arm_width,
            height: 12.0,
            depth: 4.0,
        },
        arm_uv(has_extended_skin, false, slim_arms),
        has_extended_skin.then(|| arm_uv(true, true, slim_arms)),
        extruded,
    )?;
    push_part(
        &mut parts,
        SkinPreviewPart::RightLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(false, false),
        has_extended_skin.then(|| leg_uv(false, true)),
        extruded,
    )?;
    push_part(
        &mut parts,
        SkinPreviewPart::LeftLeg,
        CuboidSize {
            width: LEG_WIDTH,
            height: 12.0,
            depth: LIMB_DEPTH,
        },
        leg_uv(has_extended_skin, false),
        has_extended_skin.then(|| leg_uv(true, true)),
        extruded,
    )?;

    let mut scene = Scene::new();
    let base_material = textured_skin_material(&texture, AlphaMode::Opaque);
    let layer_material = textured_skin_material(&texture, AlphaMode::Mask);
    let mut tracks = Vec::new();

    for part in parts {
        let (pivot, mesh_offset, swing_sign) = skin_part_layout(part.part);
        let node = scene
            .insert(
                None,
                Node::new().with_transform(Transform {
                    translation: vec3(pivot),
                    ..Transform::IDENTITY
                }),
            )
            .map_err(|error| error.to_string())?;
        insert_mesh_node(&mut scene, node, mesh_offset, part.base_mesh, &base_material)?;
        if let Some(layer_mesh) = part.layer_mesh {
            // The overlay shares its part's pivot so both meshes swing together.
            insert_mesh_node(&mut scene, node, mesh_offset, layer_mesh, &layer_material)?;
        }
        if swing_sign != 0.0 {
            tracks.push(skin_walk_track(node, swing_sign)?);
        }
    }

    finish_skin_preview(scene, texture, tracks)
}

/// Inserts one animated part: a pivot node plus one mesh node offset into place.
fn insert_part_node(
    scene: &mut Scene,
    pivot: [f32; 3],
    mesh_offset: [f32; 3],
    mesh: Arc<Mesh>,
    material: &Arc<Material>,
) -> Result<gpui_3d::NodeId, String> {
    let pivot_node = scene
        .insert(
            None,
            Node::new().with_transform(Transform {
                translation: vec3(pivot),
                ..Transform::IDENTITY
            }),
        )
        .map_err(|error| error.to_string())?;
    insert_mesh_node(scene, pivot_node, mesh_offset, mesh, material)?;
    Ok(pivot_node)
}

/// Attaches one mesh node under an existing pivot node.
fn insert_mesh_node(
    scene: &mut Scene,
    pivot_node: gpui_3d::NodeId,
    mesh_offset: [f32; 3],
    mesh: Arc<Mesh>,
    material: &Arc<Material>,
) -> Result<(), String> {
    let node = Node::new()
        .with_mesh(mesh)
        .with_materials([material.clone()])
        .with_transform(Transform {
            translation: vec3(mesh_offset),
            ..Transform::IDENTITY
        });
    scene
        .insert(Some(pivot_node), node)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn finish_skin_preview(
    scene: Scene,
    texture: Arc<TextureAsset>,
    tracks: Vec<TransformTrack>,
) -> Result<SkinPreviewMeshes, String> {
    let bounds = scene.bounds().map_err(|error| error.to_string())?;
    let camera = Camera::orthographic(
        Vec3::new(0.0, 0.0, PREVIEW_CAMERA_DISTANCE),
        Vec3::ZERO,
        Vec3::Y,
        2.0,
        0.1,
        100.0,
    );
    let scene_view = SceneView::new(Arc::new(scene), camera)
        .with_textures([texture.clone()])
        .map_err(|error| error.to_string())?
        .with_animation(tracks, Duration::ZERO)
        .map_err(|error| error.to_string())?
        .with_projection_region(ProjectionRegion::VisibleSquare)
        .with_projection_inset(6.0, 0.08)
        .map_err(|error| error.to_string())?
        .with_blend_edge_feather(1.0)
        .map_err(|error| error.to_string())?
        .with_sampling(TextureSampling::Nearest);

    Ok(SkinPreviewMeshes {
        scene_view: Arc::new(scene_view),
        texture,
        walk_period: WALK_PERIOD,
        bounds,
    })
}

/// Material for parts that sample the skin atlas with their mesh UVs.
fn textured_skin_material(texture: &Arc<TextureAsset>, alpha_mode: AlphaMode) -> Arc<Material> {
    let mut material = Material::new();
    material.alpha_mode = alpha_mode;
    if alpha_mode == AlphaMode::Mask {
        material.alpha_cutoff = SKIN_LAYER_ALPHA_CUTOFF;
    }
    material.shading_model = ShadingModel::Unlit;
    material.double_sided = true;
    material.albedo_texture = Some(texture.id());
    Arc::new(material)
}

/// Material for parts whose colors were baked into the vertex stream.
fn baked_skin_material(alpha_mode: AlphaMode) -> Arc<Material> {
    let mut material = Material::new();
    material.alpha_mode = alpha_mode;
    material.shading_model = ShadingModel::Unlit;
    material.double_sided = true;
    Arc::new(material)
}

fn skin_walk_track(node: gpui_3d::NodeId, swing_sign: f32) -> Result<TransformTrack, String> {
    let mut keys = Vec::with_capacity(usize::try_from(WALK_KEYFRAMES).unwrap_or(0) + 1);
    let frame_count = u16::try_from(WALK_KEYFRAMES).map_err(|error| error.to_string())?;
    for step in 0..=WALK_KEYFRAMES {
        let step = u16::try_from(step).map_err(|error| error.to_string())?;
        let time = Duration::from_nanos(
            u64::try_from(WALK_PERIOD.as_nanos() * u128::from(step) / u128::from(WALK_KEYFRAMES))
                .map_err(|error| error.to_string())?,
        );
        let phase = std::f32::consts::TAU * f32::from(step) / f32::from(frame_count);
        let angle = phase.sin() * 0.55 * swing_sign;
        let rotation = Quat::from_axis_angle(Vec3::X, angle)
            .ok_or_else(|| "皮肤预览动画旋转无效".to_string())?;
        keys.push(Keyframe::new(time, rotation));
    }
    let rotation = RotationTrack::new(keys).map_err(|error| error.to_string())?;
    Ok(TransformTrack::new(node).with_rotation(rotation))
}

fn push_part(
    parts: &mut Vec<SkinPreviewPartMesh>,
    part: SkinPreviewPart,
    size: CuboidSize,
    base_uv: CuboidUv,
    overlay_uv: Option<CuboidUv>,
    extruded: bool,
) -> Result<(), String> {
    let base_mesh = Arc::new(build_cuboid_mesh(size, base_uv, 0.0)?);
    let layer_mesh = match overlay_uv.filter(|_| extruded) {
        Some(overlay_uv) => Some(Arc::new(build_cuboid_mesh(
            size,
            overlay_uv,
            SKIN_OVERLAY_INFLATE,
        )?)),
        None => None,
    };
    parts.push(SkinPreviewPartMesh {
        part,
        base_mesh,
        layer_mesh,
    });
    Ok(())
}

/// Builds one textured cuboid: six quads whose UVs address the skin atlas.
///
/// One cuboid replaces the previous per-texel tessellation, which emitted a quad per preview texel
/// and another side quad per overlay texel. Neighbouring texel quads also overlapped along the
/// texture's vertical axis, which produced the see-through seams in the preview.
fn build_cuboid_mesh(size: CuboidSize, uv: CuboidUv, inflate: f32) -> Result<Mesh, String> {
    let grid = FaceGrid {
        width: 1,
        height: 1,
    };
    let run = ColorRun {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        color_index: 0,
    };
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    for face in skin_preview_faces() {
        let region = face_region(uv, *face);
        let corners = face_rect_corners(size, *face, grid, run, inflate);
        let texture = face_texture_rect(region);
        let shade = shade_cuboid_face(*face);
        let base = u32::try_from(vertices.len()).map_err(|error| error.to_string())?;
        for corner in 0..4 {
            vertices.push(Vertex {
                position: vec3(corners[corner]),
                normal: cuboid_face_normal(*face),
                uv: Vec2::new(texture[corner][0], texture[corner][1]),
                color: [shade, shade, shade, 1.0],
            });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    Mesh::new(vertices, indices)
        .map(|mesh| mesh.with_uv_regions(true))
        .map_err(|error| error.to_string())
}

/// Texture rectangle for one atlas face in the corner order used by [`face_rect_corners`].
fn face_texture_rect(region: TextureRegion) -> [[f32; 2]; 4] {
    let left = region.x as f32 / SKIN_ATLAS_UNITS;
    let right = (region.x + region.width) as f32 / SKIN_ATLAS_UNITS;
    let top = region.y as f32 / SKIN_ATLAS_UNITS;
    let bottom = (region.y + region.height) as f32 / SKIN_ATLAS_UNITS;
    [[left, bottom], [right, bottom], [right, top], [left, top]]
}

fn cuboid_face_normal(face: Face) -> Vec3 {
    match face {
        Face::Top => Vec3::Y,
        Face::Bottom => -Vec3::Y,
        Face::Right => -Vec3::X,
        Face::Left => Vec3::X,
        Face::Front => Vec3::Z,
        Face::Back => -Vec3::Z,
    }
}

/// Builds a mesh whose colors were baked into the vertex stream.
fn build_skin_mesh(vertices: Vec<SkinVertex>, indices: Vec<u32>) -> Result<Mesh, String> {
    let mut normals = vec![Vec3::ZERO; vertices.len()];
    let mut edge_masks = Vec::with_capacity(indices.len() / 3);
    for triangle in indices.chunks_exact(3) {
        let a_index = usize::try_from(triangle[0]).map_err(|error| error.to_string())?;
        let b_index = usize::try_from(triangle[1]).map_err(|error| error.to_string())?;
        let c_index = usize::try_from(triangle[2]).map_err(|error| error.to_string())?;
        let Some(a) = vertices.get(a_index) else {
            return Err("皮肤预览索引超出网格范围".to_string());
        };
        let Some(b) = vertices.get(b_index) else {
            return Err("皮肤预览索引超出网格范围".to_string());
        };
        let Some(c) = vertices.get(c_index) else {
            return Err("皮肤预览索引超出网格范围".to_string());
        };
        let edge1 = vec3(b.position) - vec3(a.position);
        let edge2 = vec3(c.position) - vec3(a.position);
        let normal = edge1.cross(edge2).normalized().unwrap_or(Vec3::Y);
        for index in [a_index, b_index, c_index] {
            normals[index] = normals[index] + normal;
        }
        let mask = a.edge_mask;
        edge_masks.push(TriangleEdgeMask::new([
            mask & 1 != 0,
            mask & 2 != 0,
            mask & 4 != 0,
        ]));
    }

    let vertices = vertices
        .into_iter()
        .zip(normals)
        .map(|(vertex, normal)| Vertex {
            position: vec3(vertex.position),
            normal: normal.normalized().unwrap_or(Vec3::Y),
            uv: Vec2::ZERO,
            color: skin_vertex_color_to_linear(vertex.color),
        })
        .collect::<Vec<_>>();
    let mesh = Mesh::new(vertices, indices).map_err(|error| error.to_string())?;
    if edge_masks
        .iter()
        .all(|mask| *mask == TriangleEdgeMask::NONE)
    {
        Ok(mesh)
    } else {
        mesh.with_edge_masks(edge_masks)
            .map_err(|error| error.to_string())
    }
}

/// Converts CPU-baked skin texels from the PNG's sRGB encoding into the linear vertex-color
/// space required by gpui-3d. Alpha is coverage, not a color channel, so it stays unmodified.
fn skin_vertex_color_to_linear(color: [f32; 4]) -> [f32; 4] {
    [
        srgb_channel_to_linear(color[0]),
        srgb_channel_to_linear(color[1]),
        srgb_channel_to_linear(color[2]),
        color[3],
    ]
}

fn srgb_channel_to_linear(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn vec3(value: [f32; 3]) -> Vec3 {
    Vec3::new(value[0], value[1], value[2])
}

fn skin_part_layout(part: SkinPreviewPart) -> ([f32; 3], [f32; 3], f32) {
    match part {
        SkinPreviewPart::Head => ([0.0; 3], [0.0, 12.0, 0.0], 0.0),
        SkinPreviewPart::Body => ([0.0; 3], [0.0, 2.0, 0.0], 0.0),
        SkinPreviewPart::RightArm { width } => {
            let center_x = -4.0 - width * 0.5;
            ([center_x, 8.0, 0.0], [0.0, -6.0, 0.0], 1.0)
        }
        SkinPreviewPart::LeftArm { width } => {
            let center_x = 4.0 + width * 0.5;
            ([center_x, 8.0, 0.0], [0.0, -6.0, 0.0], -1.0)
        }
        SkinPreviewPart::RightLeg => ([-2.0, -4.0, 0.0], [0.0, -6.0, 0.0], -1.0),
        SkinPreviewPart::LeftLeg => ([2.0, -4.0, 0.0], [0.0, -6.0, 0.0], 1.0),
        SkinPreviewPart::CustomGeometryBone { role, pivot } => (
            pivot,
            [-pivot[0], -pivot[1], -pivot[2]],
            match role {
                CustomGeometryBoneRole::RightArm | CustomGeometryBoneRole::LeftLeg => 1.0,
                CustomGeometryBoneRole::LeftArm | CustomGeometryBoneRole::RightLeg => -1.0,
                CustomGeometryBoneRole::Static
                | CustomGeometryBoneRole::Head
                | CustomGeometryBoneRole::Body => 0.0,
            },
        ),
    }
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
