use image::{DynamicImage, GenericImageView as _};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[path = "custom_geometry_cube.rs"]
mod cube;
#[path = "custom_geometry_poly.rs"]
mod poly;

use self::cube::push_cube;
use self::poly::push_poly_mesh;
use super::color::sample_image_color;
use super::custom_geometry_animation::{
    CustomGeometryBoneBinding, CustomGeometryBoneDescriptor, CustomGeometryBoneRole,
    custom_geometry_bone_bindings,
};
use super::custom_geometry_json::{array3, first_number};
use super::custom_geometry_math::{
    GeometryBounds, ModelSpace, clamp_image_index, normalize, rotate_point_around, rotate_vector,
};
use super::geometry::SkinVertex;

const CUSTOM_PREVIEW_MAX_TEXTURE_SCALE: u32 = 2;
const CUSTOM_GEOMETRY_MAX_VERTICES: usize = 1_000_000;
const CUSTOM_GEOMETRY_MAX_INDICES: usize = 1_500_000;

pub(super) struct CustomGeometryMesh {
    pub(super) parts: Vec<CustomGeometryPartMesh>,
}

pub(super) struct CustomGeometryPartMesh {
    pub(super) role: CustomGeometryBoneRole,
    pub(super) pivot: [f32; 3],
    pub(super) vertices: Vec<SkinVertex>,
    pub(super) indices: Vec<u32>,
}

#[derive(Clone)]
struct BonePose {
    parent: Option<String>,
    pivot: [f32; 3],
    rotation: [f32; 3],
}

/// Authored bone poses plus the map from authored units into the preview player skeleton.
///
/// Every authored point is mapped through [`ModelSpace`] before any bone rotation is applied, and
/// every pivot is mapped the same way, so pivots and geometry keep a shared origin.
#[derive(Clone, Copy)]
struct BonePoses<'a> {
    poses: &'a HashMap<String, BonePose>,
    space: ModelSpace,
}

#[derive(Clone, Copy)]
struct TextureSpace {
    width: f32,
    height: f32,
    preview_scale: u32,
}

struct CustomGeometryBuilder {
    parts: Vec<CustomGeometryPartBuilder>,
}

struct CustomGeometryPartBuilder {
    role: CustomGeometryBoneRole,
    pivot: [f32; 3],
    vertices: Vec<SkinVertex>,
    indices: Vec<u32>,
    polygon_count: usize,
    cube_texel_count: usize,
}

impl CustomGeometryBuilder {
    fn new() -> Self {
        Self { parts: Vec::new() }
    }

    fn part_mut(
        &mut self,
        binding: CustomGeometryBoneBinding,
    ) -> Result<&mut CustomGeometryPartBuilder, String> {
        if let Some(index) = self.parts.iter().position(|part| part.role == binding.role) {
            return Ok(&mut self.parts[index]);
        }

        self.parts.push(CustomGeometryPartBuilder {
            role: binding.role,
            pivot: binding.pivot,
            vertices: Vec::new(),
            indices: Vec::new(),
            polygon_count: 0,
            cube_texel_count: 0,
        });
        self.parts
            .last_mut()
            .ok_or_else(|| "自定义皮肤 geometry.json 分组失败".to_string())
    }

    fn into_mesh(self) -> Option<CustomGeometryMesh> {
        let parts = self
            .parts
            .into_iter()
            .filter(|part| !part.indices.is_empty())
            .map(|part| CustomGeometryPartMesh {
                role: part.role,
                pivot: part.pivot,
                vertices: part.vertices,
                indices: part.indices,
            })
            .collect::<Vec<_>>();

        (!parts.is_empty()).then_some(CustomGeometryMesh { parts })
    }
}

pub(super) fn build_custom_geometry_mesh(
    image: &DynamicImage,
    geometry_path: &Path,
    identifier: &str,
) -> Result<Option<CustomGeometryMesh>, String> {
    let raw = fs::read_to_string(geometry_path)
        .map_err(|error| format!("读取 geometry.json 失败: {error}"))?;
    let root = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("解析 geometry.json 失败: {error}"))?;
    build_custom_geometry_from_value(image, &root, identifier)
}

fn build_custom_geometry_from_value(
    image: &DynamicImage,
    root: &Value,
    identifier: &str,
) -> Result<Option<CustomGeometryMesh>, String> {
    let Some(geometry) = geometry_definition(root, identifier) else {
        return Ok(None);
    };
    let Some(bones) = geometry.get("bones").and_then(Value::as_array) else {
        return Ok(None);
    };

    let space = ModelSpace::fit(model_bounds(bones));
    let bone_poses = bone_poses(bones);
    let bone_bindings = custom_geometry_bone_bindings(bone_animation_descriptors(&bone_poses));
    let poses = BonePoses {
        poses: &bone_poses,
        space,
    };
    let texture_space = texture_space(geometry, image);
    let mut builder = CustomGeometryBuilder::new();

    for bone in bones {
        let bone_name = bone.get("name").and_then(Value::as_str);
        let bone_binding = bone_name
            .and_then(|name| bone_bindings.get(name).copied())
            .unwrap_or_else(CustomGeometryBoneBinding::static_bone);

        if let Some(poly_mesh) = bone.get("poly_mesh") {
            push_poly_mesh(
                image,
                texture_space,
                poses,
                bone_name,
                poly_mesh,
                builder.part_mut(model_bone_binding(space, bone_binding))?,
            )?;
        }

        if let Some(cubes) = bone.get("cubes").and_then(Value::as_array) {
            for cube in cubes {
                push_cube(
                    image,
                    texture_space,
                    poses,
                    bone_name,
                    bone,
                    cube,
                    builder.part_mut(model_bone_binding(space, bone_binding))?,
                )?;
            }
        }
    }

    Ok(builder.into_mesh())
}

fn model_bone_binding(
    space: ModelSpace,
    binding: CustomGeometryBoneBinding,
) -> CustomGeometryBoneBinding {
    CustomGeometryBoneBinding {
        role: binding.role,
        pivot: space.point(binding.pivot),
    }
}

fn geometry_definition<'a>(root: &'a Value, identifier: &str) -> Option<&'a Value> {
    if let Some(legacy_geometry) = root.get(identifier) {
        return Some(legacy_geometry);
    }

    root.get("minecraft:geometry")?
        .as_array()?
        .iter()
        .find(|geometry| {
            geometry
                .get("description")
                .and_then(|description| description.get("identifier"))
                .and_then(Value::as_str)
                .is_some_and(|value| value == identifier)
        })
}

/// Unions the authored bounds of every poly-mesh vertex and every cube corner.
///
/// Bone pivots are excluded: a pivot can sit outside the visible geometry, and fitting to pivots
/// would move the model away from the preview skeleton.
fn model_bounds(bones: &[Value]) -> GeometryBounds {
    let mut bounds: Option<GeometryBounds> = None;

    for bone in bones {
        if let Some(poly_mesh) = bone.get("poly_mesh") {
            for position in super::custom_geometry_json::point3_array(poly_mesh.get("positions")) {
                include_point(&mut bounds, position);
            }
        }

        let Some(cubes) = bone.get("cubes").and_then(Value::as_array) else {
            continue;
        };
        for cube in cubes {
            let (Some(origin), Some(size)) = (
                array3(cube.get("origin")),
                array3(cube.get("size")),
            ) else {
                continue;
            };
            for corner in 0..8 {
                let point = [
                    origin[0] + if corner & 1 == 0 { 0.0 } else { size[0] },
                    origin[1] + if corner & 2 == 0 { 0.0 } else { size[1] },
                    origin[2] + if corner & 4 == 0 { 0.0 } else { size[2] },
                ];
                include_point(&mut bounds, point);
            }
        }
    }

    bounds.unwrap_or_else(|| GeometryBounds::from_point([0.0, 0.0, 0.0]))
}

fn include_point(bounds: &mut Option<GeometryBounds>, point: [f32; 3]) {
    match bounds {
        Some(bounds) => bounds.include(point),
        None => *bounds = Some(GeometryBounds::from_point(point)),
    }
}

fn bone_poses(bones: &[Value]) -> HashMap<String, BonePose> {
    bones
        .iter()
        .filter_map(|bone| {
            let name = bone.get("name")?.as_str()?.to_string();
            let parent = bone
                .get("parent")
                .and_then(Value::as_str)
                .filter(|parent| !parent.trim().is_empty())
                .map(ToString::to_string);
            let pivot = array3(bone.get("pivot")).unwrap_or([0.0, 0.0, 0.0]);
            let rotation = array3(bone.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);

            Some((
                name,
                BonePose {
                    parent,
                    pivot,
                    rotation,
                },
            ))
        })
        .collect()
}

fn bone_animation_descriptors(
    bone_poses: &HashMap<String, BonePose>,
) -> Vec<CustomGeometryBoneDescriptor> {
    bone_poses
        .iter()
        .map(|(name, pose)| CustomGeometryBoneDescriptor {
            name: name.clone(),
            parent: pose.parent.clone(),
            pivot: pose.pivot,
        })
        .collect()
}

fn texture_space(geometry: &Value, image: &DynamicImage) -> TextureSpace {
    let (image_width, image_height) = image.dimensions();
    let description = geometry.get("description");
    let width = first_number(
        &[
            description.and_then(|value| value.get("texture_width")),
            geometry.get("texture_width"),
            geometry.get("texturewidth"),
        ],
        image_width as f32,
    )
    .max(1.0);
    let height = first_number(
        &[
            description.and_then(|value| value.get("texture_height")),
            geometry.get("texture_height"),
            geometry.get("textureheight"),
        ],
        image_height as f32,
    )
    .max(1.0);
    let preview_scale = ((image_width as f32 / width).round() as u32)
        .max(1)
        .min(CUSTOM_PREVIEW_MAX_TEXTURE_SCALE);

    TextureSpace {
        width,
        height,
        preview_scale,
    }
}

fn sample_uv_color(
    image: &DynamicImage,
    texture_space: TextureSpace,
    uv: [f32; 2],
    normalized: bool,
) -> [f32; 4] {
    let (image_width, image_height) = image.dimensions();
    let image_x = if normalized {
        uv[0] * image_width as f32
    } else {
        uv[0] / texture_space.width * image_width as f32
    };
    let image_y = if normalized {
        (1.0 - uv[1]) * image_height as f32
    } else {
        uv[1] / texture_space.height * image_height as f32
    };

    sample_image_color(
        image,
        clamp_image_index(image_x, image_width),
        clamp_image_index(image_y, image_height),
    )
}

fn ensure_capacity(
    vertices: &[SkinVertex],
    indices: &[u32],
    add_vertices: usize,
    add_indices: usize,
) -> Result<(), String> {
    if vertices.len().saturating_add(add_vertices) > CUSTOM_GEOMETRY_MAX_VERTICES
        || indices.len().saturating_add(add_indices) > CUSTOM_GEOMETRY_MAX_INDICES
    {
        return Err("自定义皮肤 geometry.json 网格过大".to_string());
    }
    Ok(())
}

fn transform_point_for_bone(
    point: [f32; 3],
    bone_name: Option<&str>,
    poses: BonePoses<'_>,
) -> [f32; 3] {
    transform_point_for_bone_unmapped(poses.space.point(point), bone_name, poses)
}

/// Applies bone rotations to a point that is already in preview model space.
///
/// Cube faces build their corners in preview space from an already-mapped center and pivot, then
/// call this to share the authored bone chain behavior with poly meshes.
pub(super) fn transform_point_for_bone_unmapped(
    point: [f32; 3],
    bone_name: Option<&str>,
    poses: BonePoses<'_>,
) -> [f32; 3] {
    let Some(bone_name) = bone_name else {
        return point;
    };
    let mut point = point;
    let mut current = Some(bone_name);
    let mut visited = HashSet::new();

    while let Some(name) = current {
        if !visited.insert(name.to_string()) {
            break;
        }
        let Some(pose) = poses.poses.get(name) else {
            break;
        };
        point = rotate_point_around(point, poses.space.point(pose.pivot), pose.rotation);
        current = pose.parent.as_deref();
    }

    point
}

fn transform_normal_for_bone(
    normal: [f32; 3],
    bone_name: Option<&str>,
    poses: BonePoses<'_>,
) -> [f32; 3] {
    let Some(bone_name) = bone_name else {
        return normalize(normal);
    };
    let mut normal = normal;
    let mut current = Some(bone_name);
    let mut visited = HashSet::new();

    while let Some(name) = current {
        if !visited.insert(name.to_string()) {
            break;
        }
        let Some(pose) = poses.poses.get(name) else {
            break;
        };
        normal = rotate_vector(normal, pose.rotation);
        current = pose.parent.as_deref();
    }

    normalize(normal)
}

#[cfg(test)]
#[path = "custom_geometry_tests.rs"]
mod tests;
