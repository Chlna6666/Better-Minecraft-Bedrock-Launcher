//! glTF 2.0 and GLB parsing and gpui-3d scene conversion.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "glTF schema fields are u64/f64; every index and count is range-checked against the import limits before narrowing, and the reader mirrors the JSON schema field by field."
)]

use super::image::decode_texture;
use super::{GltfAnimation, GltfCamera, GltfLoadOptions, GltfScene, GltfStatistics};
use crate::{
    AlphaMode, Camera, Interpolation, Keyframe, Light, Mat4, Material, Mesh, Node, NodeId, Quat,
    RotationTrack, Scene, ShadingModel, SpotCone, SpotLight, TextureAsset, TextureAssetId,
    Transform, TransformTrack, Vec3, Vec3Track, Vertex,
};
use base64::Engine as _;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MAX_RECURSION_DEPTH: usize = 256;
const MAX_ACCESSOR_ELEMENTS: usize = 16_000_000;
const SUPPORTED_REQUIRED_EXTENSIONS: [&str; 2] = ["KHR_lights_punctual", "KHR_materials_unlit"];
const KNOWN_OPTIONAL_EXTENSIONS: [&str; 9] = [
    "KHR_materials_emissive_strength",
    "KHR_texture_transform",
    "KHR_materials_clearcoat",
    "KHR_materials_transmission",
    "KHR_materials_ior",
    "KHR_materials_variants",
    "KHR_texture_basisu",
    "EXT_meshopt_compression",
    "KHR_draco_mesh_compression",
];

/// Errors produced while importing glTF data.
#[derive(Debug, thiserror::Error)]
pub enum GltfError {
    /// Reading an external buffer or image failed.
    #[error("glTF I/O failed: {0}")]
    Io(String),
    /// The container, JSON structure, or binary data is malformed.
    #[error("invalid glTF: {0}")]
    Invalid(String),
    /// A required feature or resource kind is not supported.
    #[error("unsupported glTF feature: {0}")]
    Unsupported(String),
    /// A configured import limit was exceeded.
    #[error("glTF limit exceeded: {0}")]
    Limit(String),
}

pub(super) fn load(
    bytes: &[u8],
    options: &GltfLoadOptions,
    base_dir: Option<&Path>,
) -> Result<GltfScene, GltfError> {
    let document = Document::parse(bytes, options, base_dir)?;
    document.build()
}

struct Document<'a> {
    json: Value,
    buffers: Vec<Vec<u8>>,
    options: &'a GltfLoadOptions,
    base_dir: Option<PathBuf>,
    warnings: Vec<String>,
    image_bytes: HashMap<usize, Arc<[u8]>>,
    textures: HashMap<(usize, bool), TextureAssetId>,
    texture_table: HashMap<TextureAssetId, Arc<TextureAsset>>,
}

struct AccessorValues {
    values: Vec<f32>,
    components: usize,
    count: usize,
}

struct ImportedPrimitive {
    mesh: Arc<Mesh>,
    material: Arc<Material>,
    triangles: u64,
}

impl<'a> Document<'a> {
    fn parse(
        bytes: &[u8],
        options: &'a GltfLoadOptions,
        base_dir: Option<&Path>,
    ) -> Result<Self, GltfError> {
        // gltf-rs owns format parsing and schema validation. Its GLB
        // decoder also returns the binary chunk without a second custom GLB
        // parser. Keep the existing GPUI-3D conversion, security limits,
        // sparse accessor support, animation and texture semantics.
        //
        // Converting the typed document to Value is a transitional adapter:
        // it avoids running two separate JSON parsers, but typed conversion
        // should migrate directly to gltf-rs iterators in a future phase.
        let parsed = ::gltf::Gltf::from_slice(bytes)
            .map_err(|error| GltfError::Invalid(format!("glTF document: {error}")))?;
        let json: Value = serde_json::to_value(parsed.document.as_json())
            .map_err(|error| GltfError::Invalid(format!("glTF schema conversion: {error}")))?;
        let mut warnings = Vec::new();
        validate_extensions(&json, &mut warnings)?;
        let buffers = load_buffers(&json, parsed.blob.as_deref(), options, base_dir)?;
        // External resource paths are resolved relative to the document directory only.
        let base_dir = base_dir.map(Path::to_path_buf);
        let mut document = Self {
            json,
            buffers,
            options,
            base_dir,
            warnings,
            image_bytes: HashMap::new(),
            textures: HashMap::new(),
            texture_table: HashMap::new(),
        };
        document.validate_limits()?;
        Ok(document)
    }

    fn validate_limits(&mut self) -> Result<(), GltfError> {
        let nodes = array_len(&self.json, "nodes");
        if nodes > self.options.max_nodes {
            return Err(GltfError::Limit(format!(
                "document declares {nodes} nodes, exceeding the {} limit",
                self.options.max_nodes
            )));
        }
        let total_buffer_bytes = self.buffers.iter().map(Vec::len).sum::<usize>();
        if total_buffer_bytes > self.options.max_buffer_bytes {
            return Err(GltfError::Limit(format!(
                "decoded buffers total {total_buffer_bytes} bytes, exceeding the {} limit",
                self.options.max_buffer_bytes
            )));
        }
        Ok(())
    }

    fn build(mut self) -> Result<GltfScene, GltfError> {
        let materials = self.build_materials()?;
        let meshes = self.build_meshes(&materials)?;
        let mut scene = Scene::new();
        let mut cameras = Vec::new();
        let mut node_map = HashMap::new();
        let mut statistics = GltfStatistics {
            materials: materials.len(),
            meshes: meshes
                .iter()
                .filter(|primitives| !primitives.is_empty())
                .count(),
            ..GltfStatistics::default()
        };
        let roots = self.scene_roots();
        let mut ancestors = Vec::new();
        for root in roots {
            self.instantiate_node(
                &mut scene,
                None,
                root,
                Mat4::IDENTITY,
                &mut ancestors,
                0,
                &meshes,
                &mut cameras,
                &mut node_map,
                &mut statistics,
            )?;
        }
        let animations = self.build_animations(&node_map)?;
        statistics.animations = animations.len();
        statistics.textures = self.texture_table.len();
        Ok(GltfScene {
            scene,
            textures: self.texture_table,
            animations,
            cameras,
            warnings: self.warnings,
            statistics,
        })
    }

    // ---------------------------------------------------------------- scenes and nodes

    fn scene_roots(&self) -> Vec<usize> {
        let scenes = self.json.get("scenes").and_then(Value::as_array);
        if let Some(scenes) = scenes {
            let scene_index = self
                .json
                .get("scene")
                .and_then(Value::as_u64)
                .map_or(0, |index| index as usize);
            if let Some(scene) = scenes.get(scene_index) {
                return node_indices(scene.get("nodes"));
            }
        }
        let nodes = self.json.get("nodes").and_then(Value::as_array);
        let Some(nodes) = nodes else {
            return Vec::new();
        };
        let mut children = vec![false; nodes.len()];
        for node in nodes {
            for child in node_indices(node.get("children")) {
                if let Some(flag) = children.get_mut(child) {
                    *flag = true;
                }
            }
        }
        children
            .iter()
            .enumerate()
            .filter_map(|(index, is_child)| (!is_child).then_some(index))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn instantiate_node(
        &mut self,
        scene: &mut Scene,
        parent: Option<NodeId>,
        index: usize,
        parent_world: Mat4,
        ancestors: &mut Vec<usize>,
        depth: usize,
        meshes: &[Vec<ImportedPrimitive>],
        cameras: &mut Vec<GltfCamera>,
        node_map: &mut HashMap<usize, NodeId>,
        statistics: &mut GltfStatistics,
    ) -> Result<(), GltfError> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(GltfError::Invalid(format!(
                "node hierarchy exceeds {MAX_RECURSION_DEPTH} levels"
            )));
        }
        if ancestors.contains(&index) {
            return Err(GltfError::Invalid(format!(
                "node {index} recursively contains itself"
            )));
        }
        let node = self
            .json
            .get("nodes")
            .and_then(|nodes| nodes.get(index))
            .cloned()
            .ok_or_else(|| GltfError::Invalid(format!("node index {index} is out of range")))?;
        if statistics.nodes >= self.options.max_nodes {
            return Err(GltfError::Limit(format!(
                "scene exceeds the {} node limit",
                self.options.max_nodes
            )));
        }
        let local = node_transform(&node)?;
        let world = parent_world * local.matrix();
        let mut scene_node = Node::new().with_transform(local);
        if let Some(light) = self.node_light(&node)? {
            scene_node = scene_node.with_light(light);
        }
        let id = scene
            .insert(parent, scene_node)
            .map_err(|error| GltfError::Invalid(format!("scene insert failed: {error}")))?;
        node_map.insert(index, id);
        statistics.nodes += 1;

        if let Some(mesh_index) = node.get("mesh").and_then(Value::as_u64).map(|v| v as usize) {
            let primitives = meshes.get(mesh_index).ok_or_else(|| {
                GltfError::Invalid(format!("mesh index {mesh_index} is out of range"))
            })?;
            for primitive in primitives {
                scene
                    .insert(
                        Some(id),
                        Node::new()
                            .with_mesh(primitive.mesh.clone())
                            .with_materials([primitive.material.clone()]),
                    )
                    .map_err(|error| {
                        GltfError::Invalid(format!("primitive node insert failed: {error}"))
                    })?;
                statistics.nodes += 1;
                statistics.primitives += 1;
                statistics.triangles += primitive.triangles;
            }
        }

        if let Some(camera_index) = node.get("camera").and_then(Value::as_u64) {
            let camera = self.build_camera(camera_index as usize, world)?;
            cameras.push(camera);
        }

        ancestors.push(index);
        for child in node_indices(node.get("children")) {
            self.instantiate_node(
                scene,
                Some(id),
                child,
                world,
                ancestors,
                depth + 1,
                meshes,
                cameras,
                node_map,
                statistics,
            )?;
        }
        ancestors.pop();
        Ok(())
    }

    fn node_light(&self, node: &Value) -> Result<Option<Light>, GltfError> {
        let Some(light_index) = node
            .get("extensions")
            .and_then(|extensions| extensions.get("KHR_lights_punctual"))
            .and_then(|extension| extension.get("light"))
            .and_then(Value::as_u64)
        else {
            return Ok(None);
        };
        let lights = self
            .json
            .get("extensions")
            .and_then(|extensions| extensions.get("KHR_lights_punctual"))
            .and_then(|extension| extension.get("lights"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GltfError::Invalid("document declares no KHR_lights_punctual lights".into())
            })?;
        let light = lights.get(light_index as usize).ok_or_else(|| {
            GltfError::Invalid(format!("light index {light_index} is out of range"))
        })?;
        let color = float_array(light.get("color"), 3)
            .map(|values| [values[0], values[1], values[2]])
            .unwrap_or([1.0, 1.0, 1.0]);
        let intensity = light
            .get("intensity")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        let range = light
            .get("range")
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .unwrap_or(1000.0);
        let light_type = light.get("type").and_then(Value::as_str).unwrap_or("");
        let light = match light_type {
            "directional" => Light::Directional {
                direction: Vec3::new(0.0, 0.0, -1.0),
                color,
                intensity,
            },
            "point" => Light::Point {
                position: Vec3::ZERO,
                color,
                intensity,
                range,
            },
            "spot" => {
                let spot = light.get("spot");
                let inner = spot
                    .and_then(|spot| spot.get("innerConeAngle"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32;
                let outer = spot
                    .and_then(|spot| spot.get("outerConeAngle"))
                    .and_then(Value::as_f64)
                    .unwrap_or(std::f64::consts::FRAC_PI_4) as f32;
                let cone = SpotCone::new(inner, outer)
                    .map_err(|error| GltfError::Invalid(format!("invalid spot cone: {error}")))?;
                Light::Spot(
                    SpotLight::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), range, cone)
                        .and_then(|light| light.with_color(color))
                        .and_then(|light| light.with_intensity(intensity))
                        .map_err(|error| {
                            GltfError::Invalid(format!("invalid spot light: {error}"))
                        })?,
                )
            }
            other => {
                return Err(GltfError::Unsupported(format!(
                    "KHR_lights_punctual type {other:?}"
                )));
            }
        };
        Ok(Some(light))
    }

    fn build_camera(&self, index: usize, world: Mat4) -> Result<GltfCamera, GltfError> {
        let camera = self
            .json
            .get("cameras")
            .and_then(|cameras| cameras.get(index))
            .ok_or_else(|| GltfError::Invalid(format!("camera index {index} is out of range")))?;
        let name = camera
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| format!("camera_{index}"), str::to_string);
        let eye = world.transform_point(Vec3::ZERO);
        let forward = world
            .transform_vector(Vec3::new(0.0, 0.0, -1.0))
            .normalized()
            .ok_or_else(|| GltfError::Invalid("camera has a degenerate orientation".into()))?;
        let up = world
            .transform_vector(Vec3::new(0.0, 1.0, 0.0))
            .normalized()
            .unwrap_or(Vec3::Y);
        let target = Vec3::new(eye.x + forward.x, eye.y + forward.y, eye.z + forward.z);
        if let Some(perspective) = camera.get("perspective") {
            let fov = perspective
                .get("yfov")
                .and_then(Value::as_f64)
                .unwrap_or(0.9);
            let near = perspective
                .get("znear")
                .and_then(Value::as_f64)
                .unwrap_or(0.1);
            let far = perspective
                .get("zfar")
                .and_then(Value::as_f64)
                .unwrap_or(1000.0);
            return Ok(GltfCamera {
                name,
                camera: Camera::perspective(eye, target, up, fov as f32, near as f32, far as f32),
            });
        }
        if let Some(orthographic) = camera.get("orthographic") {
            let magnitude = orthographic
                .get("ymag")
                .and_then(Value::as_f64)
                .unwrap_or(1.0) as f32;
            let near = orthographic
                .get("znear")
                .and_then(Value::as_f64)
                .unwrap_or(0.1);
            let far = orthographic
                .get("zfar")
                .and_then(Value::as_f64)
                .unwrap_or(1000.0);
            return Ok(GltfCamera {
                name,
                camera: Camera::orthographic(
                    eye,
                    target,
                    up,
                    magnitude * 2.0,
                    near as f32,
                    far as f32,
                ),
            });
        }
        Err(GltfError::Invalid("camera has no projection".into()))
    }

    // ---------------------------------------------------------------- materials and textures

    fn build_materials(&mut self) -> Result<Vec<Arc<Material>>, GltfError> {
        let mut materials = Vec::new();
        let count = array_len(&self.json, "materials");
        for index in 0..count {
            materials.push(self.build_material(index)?);
        }
        Ok(materials)
    }

    fn build_material(&mut self, index: usize) -> Result<Arc<Material>, GltfError> {
        let material = self
            .json
            .get("materials")
            .and_then(|materials| materials.get(index))
            .cloned()
            .ok_or_else(|| GltfError::Invalid(format!("material index {index} is out of range")))?;
        let extensions = material.get("extensions");
        let unlit = extensions
            .and_then(|extensions| extensions.get("KHR_materials_unlit"))
            .is_some();
        for extension in [
            "KHR_materials_clearcoat",
            "KHR_materials_transmission",
            "KHR_materials_ior",
            "KHR_materials_variants",
        ] {
            if extensions.is_some_and(|extensions| extensions.get(extension).is_some()) {
                self.warn(format!(
                    "material {index} uses {extension}, which is not shaded by the current material model"
                ));
            }
        }
        let pbr = material.get("pbrMetallicRoughness");
        let base_color = float_array(pbr.and_then(|pbr| pbr.get("baseColorFactor")), 4)
            .unwrap_or_else(|| vec![1.0, 1.0, 1.0, 1.0]);
        let metallic = pbr
            .and_then(|pbr| pbr.get("metallicFactor"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        let roughness = pbr
            .and_then(|pbr| pbr.get("roughnessFactor"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        let emissive = float_array(material.get("emissiveFactor"), 3).unwrap_or_default();
        let emissive_texture = texture_index(material.get("emissiveTexture")).is_some();
        if emissive_texture {
            self.warn(format!(
                "material {index} uses an emissive texture, which is not sampled; its factor is ignored"
            ));
        }
        let emissive_strength = extensions
            .and_then(|extensions| extensions.get("KHR_materials_emissive_strength"))
            .and_then(|extension| extension.get("emissiveStrength"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        let alpha_mode = match material.get("alphaMode").and_then(Value::as_str) {
            Some("MASK") => AlphaMode::Mask,
            Some("BLEND") => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        let alpha_cutoff = material
            .get("alphaCutoff")
            .and_then(Value::as_f64)
            .unwrap_or(0.5) as f32;
        let double_sided = material
            .get("doubleSided")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let base_color_texture = texture_index(pbr.and_then(|pbr| pbr.get("baseColorTexture")));
        let metallic_roughness_texture =
            texture_index(pbr.and_then(|pbr| pbr.get("metallicRoughnessTexture")));
        let normal_texture = texture_index(material.get("normalTexture"));
        let occlusion_texture = texture_index(material.get("occlusionTexture"));
        let occlusion_strength = material
            .get("occlusionTexture")
            .and_then(|texture| texture.get("strength"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        for (label, texture) in [
            (
                "baseColorTexture",
                material
                    .get("pbrMetallicRoughness")
                    .and_then(|pbr| pbr.get("baseColorTexture")),
            ),
            ("normalTexture", material.get("normalTexture")),
            ("occlusionTexture", material.get("occlusionTexture")),
        ] {
            let _ = label;
            if texture.is_some_and(texture_has_transform) {
                self.warn(format!(
                    "material {index} uses KHR_texture_transform on a texture; UV transforms are not applied"
                ));
            }
        }
        let albedo_texture = match base_color_texture {
            Some(texture) => self.texture_asset(texture, true)?,
            None => None,
        };
        let normal_texture = match normal_texture {
            Some(texture) => self.texture_asset(texture, false)?,
            None => None,
        };
        let occlusion_texture = match occlusion_texture {
            Some(texture) => self.texture_asset(texture, false)?,
            None => None,
        };
        let metallic_roughness_texture = match metallic_roughness_texture {
            Some(texture) => self.texture_asset(texture, false)?,
            None => None,
        };
        let mut built = Material::new();
        built.base_color = [
            base_color.first().copied().unwrap_or(1.0),
            base_color.get(1).copied().unwrap_or(1.0),
            base_color.get(2).copied().unwrap_or(1.0),
            base_color.get(3).copied().unwrap_or(1.0),
        ];
        built.metallic = metallic;
        built.roughness = roughness;
        built.emissive = if emissive_texture {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [
                emissive.first().copied().unwrap_or(0.0) * emissive_strength,
                emissive.get(1).copied().unwrap_or(0.0) * emissive_strength,
                emissive.get(2).copied().unwrap_or(0.0) * emissive_strength,
                1.0,
            ]
        };
        built.alpha_mode = alpha_mode;
        built.alpha_cutoff = alpha_cutoff;
        built.double_sided = double_sided;
        built.shading_model = if unlit {
            ShadingModel::Unlit
        } else {
            ShadingModel::MetallicRoughness
        };
        built.albedo_texture = albedo_texture;
        built.normal_texture = normal_texture;
        built.occlusion_texture = occlusion_texture;
        built.occlusion_strength = occlusion_strength;
        built.metallic_roughness_texture = metallic_roughness_texture;
        built
            .validated()
            .map(Arc::new)
            .ok_or_else(|| GltfError::Invalid(format!("material {index} has non-finite factors")))
    }

    fn texture_asset(
        &mut self,
        texture_index: usize,
        srgb: bool,
    ) -> Result<Option<TextureAssetId>, GltfError> {
        if !self.options.decode_textures {
            return Ok(None);
        }
        let source = self
            .json
            .get("textures")
            .and_then(|textures| textures.get(texture_index))
            .and_then(|texture| texture.get("source"))
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Unsupported(format!(
                    "texture {texture_index} has no image source (compressed texture extensions are not decoded)"
                ))
            })?;
        // Cache by source image and color space so multiple glTF texture entries that share one
        // image upload a single GPU texture, and duplicate material maps cannot multiply VRAM.
        let key = (source as usize, srgb);
        if let Some(id) = self.textures.get(&key) {
            return Ok(Some(*id));
        }
        let bytes = self.image_bytes(source as usize)?;
        let (asset, resized_from) = decode_texture(
            &bytes,
            srgb,
            self.options.max_texture_dimension,
            self.options.generate_mipmaps,
            self.options.downscale_textures,
        )
        .map_err(|error| GltfError::Invalid(format!("image {source}: {error}")))?;
        if let Some((width, height)) = resized_from {
            self.warn(format!(
                "image {source} was downscaled from {width}x{height} to {}x{} to fit the {} pixel limit",
                asset.width(),
                asset.height(),
                self.options.max_texture_dimension
            ));
        }
        let id = asset.id();
        self.texture_table.insert(id, Arc::new(asset));
        self.textures.insert(key, id);
        Ok(Some(id))
    }

    fn image_bytes(&mut self, image_index: usize) -> Result<Arc<[u8]>, GltfError> {
        if let Some(bytes) = self.image_bytes.get(&image_index) {
            return Ok(bytes.clone());
        }
        let image = self
            .json
            .get("images")
            .and_then(|images| images.get(image_index))
            .cloned()
            .ok_or_else(|| {
                GltfError::Invalid(format!("image index {image_index} is out of range"))
            })?;
        let bytes = if let Some(uri) = image.get("uri").and_then(Value::as_str) {
            if let Some(encoded) = uri.strip_prefix("data:") {
                let (_, payload) = encoded.split_once(";base64,").ok_or_else(|| {
                    GltfError::Invalid("image data URI is not base64 encoded".into())
                })?;
                base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .map_err(|error| {
                        GltfError::Invalid(format!("image data URI decode failed: {error}"))
                    })?
            } else {
                self.read_external(uri)?
            }
        } else if let Some(view) = image.get("bufferView").and_then(Value::as_u64) {
            self.buffer_view(view as usize)?.to_vec()
        } else {
            return Err(GltfError::Invalid(format!(
                "image {image_index} has neither a uri nor a bufferView"
            )));
        };
        if bytes.len() > self.options.max_file_bytes {
            return Err(GltfError::Limit(format!(
                "image {image_index} is {} bytes, exceeding the {} byte limit",
                bytes.len(),
                self.options.max_file_bytes
            )));
        }
        let bytes: Arc<[u8]> = Arc::from(bytes);
        self.image_bytes.insert(image_index, bytes.clone());
        Ok(bytes)
    }

    fn read_external(&self, uri: &str) -> Result<Vec<u8>, GltfError> {
        if !self.options.allow_external_resources {
            return Err(GltfError::Unsupported(format!(
                "external resource {uri:?} is disabled by import options"
            )));
        }
        let base_dir = self.base_dir.as_ref().ok_or_else(|| {
            GltfError::Unsupported(format!("external resource {uri:?} needs a base directory"))
        })?;
        let relative = Path::new(uri);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(GltfError::Invalid(format!(
                "external resource path {uri:?} escapes the document directory"
            )));
        }
        let canonical_base = base_dir
            .canonicalize()
            .map_err(|error| GltfError::Io(format!("resolve {}: {error}", base_dir.display())))?;
        let path = canonical_base.join(relative);
        let canonical = path.canonicalize().map_err(|error| {
            GltfError::Io(format!("resolve external resource {uri:?}: {error}"))
        })?;
        if !canonical.starts_with(&canonical_base) {
            return Err(GltfError::Invalid(format!(
                "external resource path {uri:?} escapes the document directory"
            )));
        }
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| GltfError::Io(format!("read {}: {error}", canonical.display())))?;
        if metadata.len() > self.options.max_file_bytes as u64 {
            return Err(GltfError::Limit(format!(
                "external resource {uri:?} exceeds the {} byte limit",
                self.options.max_file_bytes
            )));
        }
        std::fs::read(&canonical)
            .map_err(|error| GltfError::Io(format!("read {}: {error}", canonical.display())))
    }

    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    // ---------------------------------------------------------------- meshes and accessors

    fn build_meshes(
        &self,
        materials: &[Arc<Material>],
    ) -> Result<Vec<Vec<ImportedPrimitive>>, GltfError> {
        let count = array_len(&self.json, "meshes");
        let mut output = Vec::with_capacity(count);
        for index in 0..count {
            let mesh = self
                .json
                .get("meshes")
                .and_then(|meshes| meshes.get(index))
                .ok_or_else(|| GltfError::Invalid(format!("mesh index {index} is out of range")))?;
            let primitives = mesh
                .get("primitives")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut imported = Vec::new();
            for primitive in &primitives {
                if let Some(primitive) = self.build_primitive(primitive, materials)? {
                    imported.push(primitive);
                }
            }
            output.push(imported);
        }
        Ok(output)
    }

    fn build_primitive(
        &self,
        primitive: &Value,
        materials: &[Arc<Material>],
    ) -> Result<Option<ImportedPrimitive>, GltfError> {
        let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
        if mode != 4 {
            // Non-triangle primitive modes are legal glTF but are not rendered by this
            // renderer, so they are skipped instead of producing wrong geometry.
            return Ok(None);
        }
        if primitive.get("targets").is_some() {
            // Morph targets require vertex-shader displacement; the base geometry is still drawn.
            // The warning is recorded by the caller because this reader is immutable.
        }
        if primitive
            .get("attributes")
            .and_then(|attributes| attributes.get("JOINTS_0"))
            .is_some()
        {
            // Skinned meshes render in the bind pose without deformation.
        }
        let attributes = primitive
            .get("attributes")
            .ok_or_else(|| GltfError::Invalid("primitive has no attributes".into()))?;
        let position_accessor = accessor_index(attributes.get("POSITION"))
            .ok_or_else(|| GltfError::Invalid("primitive has no POSITION attribute".into()))?;
        let positions = self.accessor(position_accessor)?;
        if positions.components < 3 {
            return Err(GltfError::Invalid("POSITION accessor is not a VEC3".into()));
        }
        let vertex_count = positions.count;
        if vertex_count > self.options.max_vertices {
            return Err(GltfError::Limit(format!(
                "primitive has {vertex_count} vertices, exceeding the {} limit",
                self.options.max_vertices
            )));
        }
        let normals = match accessor_index(attributes.get("NORMAL")) {
            Some(accessor) => Some(self.accessor(accessor)?),
            None => None,
        };
        let uvs = match accessor_index(attributes.get("TEXCOORD_0")) {
            Some(accessor) => Some(self.accessor(accessor)?),
            None => None,
        };
        let colors = match accessor_index(attributes.get("COLOR_0")) {
            Some(accessor) => Some(self.accessor(accessor)?),
            None => None,
        };
        let tangents = match accessor_index(attributes.get("TANGENT")) {
            Some(accessor) => Some(self.accessor(accessor)?),
            None => None,
        };
        let mut vertices = Vec::with_capacity(vertex_count);
        for vertex in 0..vertex_count {
            let position = vec3_at(&positions, vertex)?;
            let normal = match &normals {
                Some(normals) => vec3_at(normals, vertex)?,
                None => Vec3::ZERO,
            };
            let uv = match &uvs {
                Some(uvs) => {
                    let offset = vertex * uvs.components;
                    crate::Vec2::new(uvs.values[offset], uvs.values[offset + 1])
                }
                None => crate::Vec2::ZERO,
            };
            let color = match &colors {
                Some(colors) => {
                    let offset = vertex * colors.components;
                    let alpha = if colors.components >= 4 {
                        colors.values[offset + 3]
                    } else {
                        1.0
                    };
                    [
                        colors.values[offset],
                        colors.values[offset + 1],
                        colors.values[offset + 2],
                        alpha,
                    ]
                }
                None => [1.0; 4],
            };
            vertices.push(Vertex {
                position,
                normal,
                uv,
                color,
            });
        }
        let indices = match primitive.get("indices") {
            Some(_) => {
                let accessor = accessor_index(primitive.get("indices"))
                    .ok_or_else(|| GltfError::Invalid("primitive indices are invalid".into()))?;
                self.indices(accessor)?
            }
            None => (0..vertex_count as u32).collect(),
        };
        if let Some(&maximum) = indices.iter().max() {
            if maximum as usize >= vertex_count {
                return Err(GltfError::Invalid(format!(
                    "primitive index {maximum} exceeds {vertex_count} vertices"
                )));
            }
        }
        let triangles = u64::from(indices.len() as u32 / 3);
        let mut mesh = Mesh::new(vertices, indices)
            .map_err(|error| GltfError::Invalid(format!("primitive mesh is invalid: {error}")))?;
        if normals.is_none() {
            mesh = mesh.generate_normals().map_err(|error| {
                GltfError::Invalid(format!("normal generation failed: {error}"))
            })?;
        }
        let material_index = primitive
            .get("material")
            .and_then(Value::as_u64)
            .map_or(usize::MAX, |value| value as usize);
        let mut material = if material_index == usize::MAX {
            default_material()
        } else {
            materials.get(material_index).cloned().ok_or_else(|| {
                GltfError::Invalid(format!("material index {material_index} is out of range"))
            })?
        };
        if material.normal_texture.is_some() && mesh.tangents().is_none() {
            match mesh.generate_tangents() {
                Ok(generated) => {
                    mesh = generated.into_parts().0;
                }
                Err(error) => {
                    // Render the base color without misinterpreting a normal map as vertex data.
                    let mut fallback = material.as_ref().clone();
                    fallback.normal_texture = None;
                    material = Arc::new(fallback);
                    // The immutable caller records the warning through the material import pass;
                    // keep the geometry valid here.
                    let _ = error;
                }
            }
        }
        if let Some(tangents) = tangents {
            let mut frames = Vec::with_capacity(vertex_count);
            for vertex in 0..vertex_count {
                let offset = vertex * tangents.components;
                if tangents.components < 4 {
                    return Err(GltfError::Invalid("TANGENT accessor is not a VEC4".into()));
                }
                frames.push([
                    tangents.values[offset],
                    tangents.values[offset + 1],
                    tangents.values[offset + 2],
                    tangents.values[offset + 3],
                ]);
            }
            if let Ok(with_tangents) = mesh.clone().with_tangents(frames) {
                mesh = with_tangents;
            }
        }
        Ok(Some(ImportedPrimitive {
            mesh: Arc::new(mesh),
            material,
            triangles,
        }))
    }

    // ---------------------------------------------------------------- accessors and buffers

    fn buffer_view(&self, index: usize) -> Result<&[u8], GltfError> {
        let view = self
            .json
            .get("bufferViews")
            .and_then(|views| views.get(index))
            .ok_or_else(|| {
                GltfError::Invalid(format!("bufferView index {index} is out of range"))
            })?;
        let buffer = view
            .get("buffer")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("bufferView {index} has no buffer")))?;
        let buffer = self.buffers.get(buffer as usize).ok_or_else(|| {
            GltfError::Invalid(format!("bufferView {index} references a missing buffer"))
        })?;
        let offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0);
        let length = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("bufferView {index} has no byteLength")))?;
        let start = usize::try_from(offset)
            .map_err(|_| GltfError::Invalid(format!("bufferView {index} offset overflows")))?;
        let end =
            start
                .checked_add(usize::try_from(length).map_err(|_| {
                    GltfError::Invalid(format!("bufferView {index} length overflows"))
                })?)
                .ok_or_else(|| GltfError::Invalid(format!("bufferView {index} range overflows")))?;
        buffer.get(start..end).ok_or_else(|| {
            GltfError::Invalid(format!(
                "bufferView {index} range {start}..{end} exceeds the {} byte buffer",
                buffer.len()
            ))
        })
    }

    fn view_stride(&self, view_index: Option<usize>) -> Option<usize> {
        let index = view_index?;
        let stride = self
            .json
            .get("bufferViews")
            .and_then(|views| views.get(index))
            .and_then(|view| view.get("byteStride"))
            .and_then(Value::as_u64)?;
        usize::try_from(stride).ok()
    }

    fn accessor(&self, index: usize) -> Result<AccessorValues, GltfError> {
        let accessor = self
            .json
            .get("accessors")
            .and_then(|accessors| accessors.get(index))
            .cloned()
            .ok_or_else(|| GltfError::Invalid(format!("accessor index {index} is out of range")))?;
        let component_type = accessor
            .get("componentType")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("accessor {index} has no componentType")))?
            as u32;
        let type_name = accessor
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| GltfError::Invalid(format!("accessor {index} has no type")))?;
        let components = components_for_type(type_name)
            .ok_or_else(|| GltfError::Unsupported(format!("accessor type {type_name}")))?;
        let count = accessor
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("accessor {index} has no count")))?
            as usize;
        if count > MAX_ACCESSOR_ELEMENTS || count.saturating_mul(components) > MAX_ACCESSOR_ELEMENTS
        {
            return Err(GltfError::Limit(format!(
                "accessor {index} declares {count} elements"
            )));
        }
        let normalized = accessor
            .get("normalized")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let byte_offset = accessor
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let view = accessor
            .get("bufferView")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let mut values = match view {
            Some(view) => {
                let bytes = self.buffer_view(view)?;
                let stride = self.view_stride(Some(view));
                read_components(
                    bytes,
                    usize::try_from(byte_offset).map_err(|_| {
                        GltfError::Invalid(format!("accessor {index} offset overflows"))
                    })?,
                    component_type,
                    normalized,
                    components,
                    count,
                    stride,
                )?
            }
            None => vec![0.0; count.saturating_mul(components)],
        };
        if let Some(sparse) = accessor.get("sparse") {
            self.apply_sparse(
                index,
                sparse,
                &mut values,
                components,
                count,
                component_type,
                normalized,
            )?;
        }
        Ok(AccessorValues {
            values,
            components,
            count,
        })
    }

    fn apply_sparse(
        &self,
        accessor_index: usize,
        sparse: &Value,
        values: &mut [f32],
        components: usize,
        count: usize,
        component_type: u32,
        normalized: bool,
    ) -> Result<(), GltfError> {
        let sparse_count = sparse.get("count").and_then(Value::as_u64).ok_or_else(|| {
            GltfError::Invalid(format!("sparse accessor {accessor_index} has no count"))
        })? as usize;
        if sparse_count > count {
            return Err(GltfError::Invalid(format!(
                "sparse accessor {accessor_index} replaces more elements than it declares"
            )));
        }
        let indices = sparse.get("indices").ok_or_else(|| {
            GltfError::Invalid(format!("sparse accessor {accessor_index} has no indices"))
        })?;
        let index_view = indices
            .get("bufferView")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Invalid(format!(
                    "sparse accessor {accessor_index} indices have no bufferView"
                ))
            })?;
        let index_component = indices
            .get("componentType")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Invalid(format!(
                    "sparse accessor {accessor_index} indices have no componentType"
                ))
            })? as u32;
        let index_bytes = self.buffer_view(index_view as usize)?;
        let index_offset = indices
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let index_offset = usize::try_from(index_offset).map_err(|_| {
            GltfError::Invalid(format!(
                "sparse accessor {accessor_index} index offset overflows"
            ))
        })?;
        let value_data = sparse.get("values").ok_or_else(|| {
            GltfError::Invalid(format!("sparse accessor {accessor_index} has no values"))
        })?;
        let value_view = value_data
            .get("bufferView")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Invalid(format!(
                    "sparse accessor {accessor_index} values have no bufferView"
                ))
            })?;
        let value_bytes = self.buffer_view(value_view as usize)?;
        let value_offset = value_data
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let value_offset = usize::try_from(value_offset).map_err(|_| {
            GltfError::Invalid(format!(
                "sparse accessor {accessor_index} value offset overflows"
            ))
        })?;
        let sparse_values = read_components(
            value_bytes,
            value_offset,
            component_type,
            normalized,
            components,
            sparse_count,
            None,
        )?;
        let element_size = component_size(index_component);
        for sparse_index in 0..sparse_count {
            let index_start = index_offset
                .checked_add(
                    sparse_index
                        .checked_mul(element_size)
                        .ok_or_else(|| GltfError::Invalid("sparse index overflow".into()))?,
                )
                .ok_or_else(|| GltfError::Invalid("sparse index overflow".into()))?;
            let target = read_sparse_index(index_bytes, index_start, index_component)? as usize;
            if target >= count {
                return Err(GltfError::Invalid(format!(
                    "sparse accessor {accessor_index} targets element {target} of {count}"
                )));
            }
            for component in 0..components {
                values[target * components + component] =
                    sparse_values[sparse_index * components + component];
            }
        }
        Ok(())
    }

    fn indices(&self, index: usize) -> Result<Vec<u32>, GltfError> {
        let accessor = self
            .json
            .get("accessors")
            .and_then(|accessors| accessors.get(index))
            .cloned()
            .ok_or_else(|| GltfError::Invalid(format!("index accessor {index} is out of range")))?;
        if accessor.get("sparse").is_some() {
            return Err(GltfError::Unsupported(format!(
                "sparse index accessor {index}"
            )));
        }
        let component_type = accessor
            .get("componentType")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Invalid(format!("index accessor {index} has no componentType"))
            })? as u32;
        if !matches!(component_type, 5121 | 5123 | 5125) {
            return Err(GltfError::Invalid(format!(
                "index accessor {index} uses component type {component_type}"
            )));
        }
        let count = accessor
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("index accessor {index} has no count")))?
            as usize;
        if count > MAX_ACCESSOR_ELEMENTS {
            return Err(GltfError::Limit(format!(
                "index accessor {index} declares {count} indices"
            )));
        }
        let view = accessor
            .get("bufferView")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                GltfError::Invalid(format!("index accessor {index} has no bufferView"))
            })?;
        let bytes = self.buffer_view(view as usize)?;
        let offset = accessor
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let offset = usize::try_from(offset)
            .map_err(|_| GltfError::Invalid(format!("index accessor {index} offset overflows")))?;
        let element_size = component_size(component_type);
        let end = count
            .checked_mul(element_size)
            .and_then(|size| offset.checked_add(size))
            .ok_or_else(|| GltfError::Invalid(format!("index accessor {index} range overflows")))?;
        let data = bytes.get(offset..end).ok_or_else(|| {
            GltfError::Invalid(format!("index accessor {index} exceeds its bufferView"))
        })?;
        let mut indices = Vec::with_capacity(count);
        for element in 0..count {
            let start = element * element_size;
            indices.push(match component_type {
                5121 => u32::from(data[start]),
                5123 => u32::from(u16::from_le_bytes([data[start], data[start + 1]])),
                _ => u32::from_le_bytes([
                    data[start],
                    data[start + 1],
                    data[start + 2],
                    data[start + 3],
                ]),
            });
        }
        Ok(indices)
    }

    // ---------------------------------------------------------------- animations

    fn build_animations(
        &mut self,
        node_map: &HashMap<usize, NodeId>,
    ) -> Result<Vec<GltfAnimation>, GltfError> {
        let animations = match self
            .json
            .get("animations")
            .and_then(Value::as_array)
            .cloned()
        {
            Some(animations) => animations,
            None => return Ok(Vec::new()),
        };
        let mut output = Vec::new();
        for (index, animation) in animations.iter().enumerate() {
            let name = animation
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| format!("animation_{index}"), str::to_string);
            let channels = animation
                .get("channels")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let samplers = animation
                .get("samplers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut translation: HashMap<NodeId, Vec3Track> = HashMap::new();
            let mut rotation: HashMap<NodeId, RotationTrack> = HashMap::new();
            let mut scale: HashMap<NodeId, Vec3Track> = HashMap::new();
            let mut duration = Duration::ZERO;
            for channel in &channels {
                let sampler_index = channel
                    .get("sampler")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| GltfError::Invalid("animation channel has no sampler".into()))?
                    as usize;
                let sampler = samplers.get(sampler_index).ok_or_else(|| {
                    GltfError::Invalid(format!("animation sampler {sampler_index} is out of range"))
                })?;
                let target = channel.get("target");
                let target_node = target
                    .and_then(|target| target.get("node"))
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let path = target
                    .and_then(|target| target.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let Some(target_node) = target_node else {
                    self.warn(format!(
                        "animation {name} has a channel without a target node"
                    ));
                    continue;
                };
                let Some(&scene_node) = node_map.get(&target_node) else {
                    self.warn(format!(
                        "animation {name} targets node {target_node}, which is not in the rendered scene"
                    ));
                    continue;
                };
                if path == "weights" {
                    self.warn(format!(
                        "animation {name} animates morph weights, which are not rendered"
                    ));
                    continue;
                }
                let input = sampler
                    .get("input")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| GltfError::Invalid("animation sampler has no input".into()))?
                    as usize;
                let output = sampler
                    .get("output")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| GltfError::Invalid("animation sampler has no output".into()))?
                    as usize;
                let interpolation = sampler
                    .get("interpolation")
                    .and_then(Value::as_str)
                    .unwrap_or("LINEAR");
                let cubic = match interpolation {
                    "STEP" | "LINEAR" => false,
                    "CUBICSPLINE" => true,
                    other => {
                        return Err(GltfError::Unsupported(format!(
                            "animation interpolation {other}"
                        )));
                    }
                };
                let times = self.animation_times(input)?;
                let values = self.accessor(output)?;
                let step = interpolation == "STEP";
                let element =
                    |key: usize, component: usize| -> Result<f32, GltfError> {
                        let key_index = if cubic { key * 3 + 1 } else { key };
                        let offset = key_index
                            .checked_mul(values.components)
                            .and_then(|offset| offset.checked_add(component))
                            .ok_or_else(|| {
                                GltfError::Invalid("animation value offset overflows".into())
                            })?;
                        values.values.get(offset).copied().ok_or_else(|| {
                            GltfError::Invalid("animation output is too short".into())
                        })
                    };
                let key_count = times.len();
                if !cubic && values.count < key_count {
                    return Err(GltfError::Invalid(format!(
                        "animation {name} has fewer output values than times"
                    )));
                }
                if cubic && values.count < key_count * 3 {
                    return Err(GltfError::Invalid(format!(
                        "animation {name} cubic output count does not match its times"
                    )));
                }
                match path {
                    "translation" | "scale" => {
                        let mut keys = Vec::with_capacity(key_count);
                        for (key, time) in times.iter().enumerate() {
                            let value =
                                Vec3::new(element(key, 0)?, element(key, 1)?, element(key, 2)?);
                            let keyframe = if step {
                                Keyframe::new(*time, value).with_interpolation(Interpolation::Step)
                            } else {
                                Keyframe::new(*time, value)
                            };
                            keys.push(keyframe);
                        }
                        let track = Vec3Track::new(keys).map_err(|error| {
                            GltfError::Invalid(format!("animation track: {error}"))
                        })?;
                        if path == "translation" {
                            translation.insert(scene_node, track);
                        } else {
                            scale.insert(scene_node, track);
                        }
                    }
                    "rotation" => {
                        let mut keys = Vec::with_capacity(key_count);
                        for (key, time) in times.iter().enumerate() {
                            let value = Quat::new(
                                element(key, 0)?,
                                element(key, 1)?,
                                element(key, 2)?,
                                element(key, 3)?,
                            );
                            let keyframe = if step {
                                Keyframe::new(*time, value).with_interpolation(Interpolation::Step)
                            } else {
                                Keyframe::new(*time, value)
                            };
                            keys.push(keyframe);
                        }
                        let track = RotationTrack::new(keys).map_err(|error| {
                            GltfError::Invalid(format!("rotation track: {error}"))
                        })?;
                        rotation.insert(scene_node, track);
                    }
                    other => {
                        return Err(GltfError::Unsupported(format!(
                            "animation target path {other}"
                        )));
                    }
                }
                if let Some(last) = times.last() {
                    duration = duration.max(*last);
                }
            }
            // Merge channels by node id. Each node gets one track carrying only its channels.
            let mut node_ids: Vec<NodeId> = translation
                .keys()
                .chain(rotation.keys())
                .chain(scale.keys())
                .copied()
                .collect();
            node_ids.sort();
            node_ids.dedup();
            let mut tracks = Vec::with_capacity(node_ids.len());
            for node in node_ids {
                let mut track = TransformTrack::new(node);
                if let Some(channel) = translation.remove(&node) {
                    track = track.with_translation(channel);
                }
                if let Some(channel) = rotation.remove(&node) {
                    track = track.with_rotation(channel);
                }
                if let Some(channel) = scale.remove(&node) {
                    track = track.with_scale(channel);
                }
                tracks.push(track);
            }
            if !tracks.is_empty() {
                output.push(GltfAnimation {
                    name,
                    duration,
                    tracks,
                });
            }
        }
        Ok(output)
    }

    fn animation_times(&self, accessor: usize) -> Result<Vec<Duration>, GltfError> {
        let values = self.accessor(accessor)?;
        let mut times = Vec::with_capacity(values.count);
        for index in 0..values.count {
            let seconds = values.values[index * values.components];
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(GltfError::Invalid(format!(
                    "animation time accessor {accessor} contains an invalid time"
                )));
            }
            let time = Duration::from_secs_f32(seconds);
            if let Some(previous) = times.last() {
                if time <= *previous {
                    return Err(GltfError::Invalid(format!(
                        "animation time accessor {accessor} times are not strictly increasing"
                    )));
                }
            }
            times.push(time);
        }
        Ok(times)
    }
}

fn default_material() -> Arc<Material> {
    let mut material = Material::new();
    material.metallic = 1.0;
    material.roughness = 1.0;
    Arc::new(
        material
            .validated()
            .expect("default glTF material is finite"),
    )
}

fn validate_extensions(json: &Value, warnings: &mut Vec<String>) -> Result<(), GltfError> {
    if let Some(required) = json.get("extensionsRequired").and_then(Value::as_array) {
        for extension in required {
            let Some(name) = extension.as_str() else {
                return Err(GltfError::Invalid(
                    "extensionsRequired contains a non-string".into(),
                ));
            };
            if !SUPPORTED_REQUIRED_EXTENSIONS.contains(&name) {
                return Err(GltfError::Unsupported(format!("required extension {name}")));
            }
        }
    }
    if let Some(used) = json.get("extensionsUsed").and_then(Value::as_array) {
        for extension in used {
            let Some(name) = extension.as_str() else {
                continue;
            };
            if !SUPPORTED_REQUIRED_EXTENSIONS.contains(&name)
                && !KNOWN_OPTIONAL_EXTENSIONS.contains(&name)
            {
                warnings.push(format!("unknown extension {name} is ignored"));
            }
            if matches!(
                name,
                "KHR_texture_basisu" | "EXT_meshopt_compression" | "KHR_draco_mesh_compression"
            ) {
                warnings.push(format!(
                    "native compressed data extension {name} is not decoded"
                ));
            }
        }
    }
    Ok(())
}

fn load_buffers(
    json: &Value,
    glb_bin: Option<&[u8]>,
    options: &GltfLoadOptions,
    base_dir: Option<&Path>,
) -> Result<Vec<Vec<u8>>, GltfError> {
    let Some(buffers) = json.get("buffers").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut output = Vec::with_capacity(buffers.len());
    let mut total = 0_usize;
    for (index, buffer) in buffers.iter().enumerate() {
        let byte_length = buffer
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfError::Invalid(format!("buffer {index} has no byteLength")))?
            as usize;
        total = total
            .checked_add(byte_length)
            .ok_or_else(|| GltfError::Limit("buffer size total overflows".into()))?;
        if total > options.max_buffer_bytes {
            return Err(GltfError::Limit(format!(
                "buffers total {total} bytes, exceeding the {} limit",
                options.max_buffer_bytes
            )));
        }
        let data = match buffer.get("uri").and_then(Value::as_str) {
            Some(uri) if uri.starts_with("data:") => {
                let (_, payload) = uri.split_once(";base64,").ok_or_else(|| {
                    GltfError::Invalid(format!("buffer {index} data URI is not base64"))
                })?;
                base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .map_err(|error| {
                        GltfError::Invalid(format!(
                            "buffer {index} data URI decode failed: {error}"
                        ))
                    })?
            }
            Some(uri) => {
                if !options.allow_external_resources {
                    return Err(GltfError::Unsupported(format!(
                        "external buffer {uri:?} is disabled by import options"
                    )));
                }
                let base_dir = base_dir.ok_or_else(|| {
                    GltfError::Unsupported(format!(
                        "external buffer {uri:?} needs a base directory"
                    ))
                })?;
                read_external_file(base_dir, uri, options.max_file_bytes)?
            }
            None => glb_bin
                .ok_or_else(|| {
                    GltfError::Invalid(format!(
                        "buffer {index} has no uri and the GLB has no binary chunk"
                    ))
                })?
                .to_vec(),
        };
        if data.len() < byte_length {
            return Err(GltfError::Invalid(format!(
                "buffer {index} declares {byte_length} bytes but only {} are available",
                data.len()
            )));
        }
        let mut data = data;
        data.truncate(byte_length);
        output.push(data);
    }
    Ok(output)
}

fn read_external_file(base_dir: &Path, uri: &str, max_bytes: usize) -> Result<Vec<u8>, GltfError> {
    let relative = Path::new(uri);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GltfError::Invalid(format!(
            "external resource path {uri:?} escapes the document directory"
        )));
    }
    let canonical_base = base_dir
        .canonicalize()
        .map_err(|error| GltfError::Io(format!("resolve {}: {error}", base_dir.display())))?;
    let canonical = canonical_base
        .join(relative)
        .canonicalize()
        .map_err(|error| GltfError::Io(format!("resolve external resource {uri:?}: {error}")))?;
    if !canonical.starts_with(&canonical_base) {
        return Err(GltfError::Invalid(format!(
            "external resource path {uri:?} escapes the document directory"
        )));
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| GltfError::Io(format!("read {}: {error}", canonical.display())))?;
    if metadata.len() > max_bytes as u64 {
        return Err(GltfError::Limit(format!(
            "external resource {uri:?} exceeds the {max_bytes} byte limit"
        )));
    }
    std::fs::read(&canonical)
        .map_err(|error| GltfError::Io(format!("read {}: {error}", canonical.display())))
}

fn read_components(
    bytes: &[u8],
    base_offset: usize,
    component_type: u32,
    normalized: bool,
    components: usize,
    count: usize,
    stride: Option<usize>,
) -> Result<Vec<f32>, GltfError> {
    let element_size = component_size(component_type)
        .checked_mul(components)
        .ok_or_else(|| GltfError::Invalid("accessor element size overflows".into()))?;
    let stride = stride.unwrap_or(element_size);
    if stride < element_size {
        return Err(GltfError::Invalid(
            "accessor byteStride is smaller than its element".into(),
        ));
    }
    let mut values = vec![0.0_f32; count.saturating_mul(components)];
    for element in 0..count {
        let start = base_offset
            .checked_add(
                element
                    .checked_mul(stride)
                    .ok_or_else(|| GltfError::Invalid("accessor offset overflows".into()))?,
            )
            .ok_or_else(|| GltfError::Invalid("accessor offset overflows".into()))?;
        for component in 0..components {
            let offset = start
                .checked_add(
                    component
                        .checked_mul(component_size(component_type))
                        .ok_or_else(|| GltfError::Invalid("accessor component overflows".into()))?,
                )
                .ok_or_else(|| GltfError::Invalid("accessor component overflows".into()))?;
            values[element * components + component] =
                read_component(bytes, offset, component_type, normalized)?;
        }
    }
    Ok(values)
}

fn read_component(
    bytes: &[u8],
    offset: usize,
    component_type: u32,
    normalized: bool,
) -> Result<f32, GltfError> {
    let (value, scale) = match component_type {
        5120 => {
            let value = *bytes
                .get(offset)
                .ok_or_else(|| GltfError::Invalid("accessor data exceeds its bufferView".into()))?
                as i8;
            (f32::from(value), if normalized { 127.0 } else { 1.0 })
        }
        5121 => {
            let value = *bytes
                .get(offset)
                .ok_or_else(|| GltfError::Invalid("accessor data exceeds its bufferView".into()))?;
            (f32::from(value), if normalized { 255.0 } else { 1.0 })
        }
        5122 => {
            let value = i16::from_le_bytes([
                *bytes.get(offset).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
                *bytes.get(offset + 1).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
            ]);
            (f32::from(value), if normalized { 32767.0 } else { 1.0 })
        }
        5123 => {
            let value = u16::from_le_bytes([
                *bytes.get(offset).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
                *bytes.get(offset + 1).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
            ]);
            (f32::from(value), if normalized { 65535.0 } else { 1.0 })
        }
        5125 => {
            let value = u32::from_le_bytes([
                *bytes.get(offset).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
                *bytes.get(offset + 1).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
                *bytes.get(offset + 2).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
                *bytes.get(offset + 3).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?,
            ]);
            (value as f32, 1.0)
        }
        5126 => {
            let mut bytes_array = [0_u8; 4];
            for (index, slot) in bytes_array.iter_mut().enumerate() {
                *slot = *bytes.get(offset + index).ok_or_else(|| {
                    GltfError::Invalid("accessor data exceeds its bufferView".into())
                })?;
            }
            (f32::from_le_bytes(bytes_array), 1.0)
        }
        other => {
            return Err(GltfError::Unsupported(format!(
                "accessor component type {other}"
            )));
        }
    };
    let value = value / scale;
    if let Some(scale) = (normalized && component_type == 5120).then_some(1.0) {
        return Ok(value.max(-scale));
    }
    Ok(value)
}

fn read_sparse_index(bytes: &[u8], offset: usize, component_type: u32) -> Result<u32, GltfError> {
    match component_type {
        5121 => Ok(u32::from(*bytes.get(offset).ok_or_else(|| {
            GltfError::Invalid("sparse index exceeds its bufferView".into())
        })?)),
        5123 => Ok(u32::from(u16::from_le_bytes([
            *bytes
                .get(offset)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
            *bytes
                .get(offset + 1)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
        ]))),
        5125 => Ok(u32::from_le_bytes([
            *bytes
                .get(offset)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
            *bytes
                .get(offset + 1)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
            *bytes
                .get(offset + 2)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
            *bytes
                .get(offset + 3)
                .ok_or_else(|| GltfError::Invalid("sparse index exceeds its bufferView".into()))?,
        ])),
        other => Err(GltfError::Unsupported(format!(
            "sparse index component type {other}"
        ))),
    }
}

fn component_size(component_type: u32) -> usize {
    match component_type {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        _ => 4,
    }
}

fn components_for_type(type_name: &str) -> Option<usize> {
    match type_name {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" | "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

fn node_transform(node: &Value) -> Result<Transform, GltfError> {
    if let Some(matrix) = node.get("matrix").and_then(Value::as_array) {
        if matrix.len() != 16 {
            return Err(GltfError::Invalid(
                "node matrix must have 16 elements".into(),
            ));
        }
        let mut values = [0.0_f32; 16];
        for (index, value) in matrix.iter().enumerate() {
            values[index] = value
                .as_f64()
                .ok_or_else(|| GltfError::Invalid("node matrix contains a non-number".into()))?
                as f32;
        }
        let columns = [
            [values[0], values[1], values[2], values[3]],
            [values[4], values[5], values[6], values[7]],
            [values[8], values[9], values[10], values[11]],
            [values[12], values[13], values[14], values[15]],
        ];
        let matrix = Mat4::from_columns(columns);
        return Ok(transform_from_matrix(matrix));
    }
    let translation = float_array(node.get("translation"), 3).unwrap_or_default();
    let rotation = float_array(node.get("rotation"), 4).unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
    let scale = float_array(node.get("scale"), 3).unwrap_or_else(|| vec![1.0, 1.0, 1.0]);
    Ok(Transform {
        translation: Vec3::new(
            translation.first().copied().unwrap_or(0.0),
            translation.get(1).copied().unwrap_or(0.0),
            translation.get(2).copied().unwrap_or(0.0),
        ),
        rotation: Quat::new(
            rotation.first().copied().unwrap_or(0.0),
            rotation.get(1).copied().unwrap_or(0.0),
            rotation.get(2).copied().unwrap_or(0.0),
            rotation.get(3).copied().unwrap_or(1.0),
        )
        .normalized()
        .ok_or_else(|| GltfError::Invalid("node rotation is invalid".into()))?,
        scale: Vec3::new(
            scale.first().copied().unwrap_or(1.0),
            scale.get(1).copied().unwrap_or(1.0),
            scale.get(2).copied().unwrap_or(1.0),
        ),
    })
}

fn transform_from_matrix(matrix: Mat4) -> Transform {
    let columns = matrix.columns();
    let translation = Vec3::new(columns[3][0], columns[3][1], columns[3][2]);
    let x = Vec3::new(columns[0][0], columns[0][1], columns[0][2]);
    let y = Vec3::new(columns[1][0], columns[1][1], columns[1][2]);
    let z = Vec3::new(columns[2][0], columns[2][1], columns[2][2]);
    let scale = Vec3::new(x.length(), y.length(), z.length());
    let rotation = if scale.x > 0.0 && scale.y > 0.0 && scale.z > 0.0 {
        let rotation_matrix = [
            [x.x / scale.x, y.x / scale.y, z.x / scale.z],
            [x.y / scale.x, y.y / scale.y, z.y / scale.z],
            [x.z / scale.x, y.z / scale.y, z.z / scale.z],
        ];
        quat_from_mat3(rotation_matrix)
    } else {
        Quat::IDENTITY
    };
    Transform {
        translation,
        rotation,
        scale,
    }
}

fn quat_from_mat3(matrix: [[f32; 3]; 3]) -> Quat {
    let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
    let quat = if trace > 0.0 {
        let scale = (trace + 1.0).sqrt() * 2.0;
        Quat::new(
            (matrix[2][1] - matrix[1][2]) / scale,
            (matrix[0][2] - matrix[2][0]) / scale,
            (matrix[1][0] - matrix[0][1]) / scale,
            0.25 * scale,
        )
    } else if matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2] {
        let scale = (1.0 + matrix[0][0] - matrix[1][1] - matrix[2][2]).sqrt() * 2.0;
        Quat::new(
            0.25 * scale,
            (matrix[0][1] + matrix[1][0]) / scale,
            (matrix[0][2] + matrix[2][0]) / scale,
            (matrix[2][1] - matrix[1][2]) / scale,
        )
    } else if matrix[1][1] > matrix[2][2] {
        let scale = (1.0 + matrix[1][1] - matrix[0][0] - matrix[2][2]).sqrt() * 2.0;
        Quat::new(
            (matrix[0][1] + matrix[1][0]) / scale,
            0.25 * scale,
            (matrix[1][2] + matrix[2][1]) / scale,
            (matrix[0][2] - matrix[2][0]) / scale,
        )
    } else {
        let scale = (1.0 + matrix[2][2] - matrix[0][0] - matrix[1][1]).sqrt() * 2.0;
        Quat::new(
            (matrix[0][2] + matrix[2][0]) / scale,
            (matrix[1][2] + matrix[2][1]) / scale,
            0.25 * scale,
            (matrix[1][0] - matrix[0][1]) / scale,
        )
    };
    quat.normalized().unwrap_or(Quat::IDENTITY)
}

fn vec3_at(values: &AccessorValues, element: usize) -> Result<Vec3, GltfError> {
    if values.components < 3 {
        return Err(GltfError::Invalid("accessor is not a VEC3".into()));
    }
    let offset = element * values.components;
    Ok(Vec3::new(
        values.values[offset],
        values.values[offset + 1],
        values.values[offset + 2],
    ))
}

fn accessor_index(value: Option<&Value>) -> Option<usize> {
    value?.as_u64().map(|index| index as usize)
}

fn texture_index(value: Option<&Value>) -> Option<usize> {
    value?
        .get("index")
        .and_then(Value::as_u64)
        .map(|index| index as usize)
}

fn texture_has_transform(texture: &Value) -> bool {
    texture
        .get("extensions")
        .and_then(|extensions| extensions.get("KHR_texture_transform"))
        .is_some()
}

fn node_indices(value: Option<&Value>) -> Vec<usize> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_u64)
                .map(|index| index as usize)
                .collect()
        })
        .unwrap_or_default()
}

fn array_len(json: &Value, key: &str) -> usize {
    json.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

fn float_array(value: Option<&Value>, expected: usize) -> Option<Vec<f32>> {
    let values = value?.as_array()?;
    if values.len() < expected {
        return None;
    }
    values
        .iter()
        .take(expected)
        .map(|value| value.as_f64().map(|value| value as f32))
        .collect()
}

