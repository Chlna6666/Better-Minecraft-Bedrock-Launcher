//! Shared "large world" scene for both rendering paths.
//!
//! Both the direct offscreen benchmark and the GPUI sidecar window build their world through this
//! module so geometry, materials, lights and camera framing are identical. The two paths differ
//! only in how they attach a sky: the direct renderer uses a real environment map through
//! [`Scene::set_environment`], while the sidecar optionally uses analytic sky/screen effects (see
//! the sidecar module). Keeping one scene source makes the performance comparison meaningful.
//!
//! The primary model is a downloaded large glTF/GLB scene (e.g. Sponza). When that asset is not
//! present, the loader falls back to the repository's DamagedHelmet fixture, then to a purely
//! procedural instanced city, so the example always runs offline.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    reason = "Procedural world authoring reads as one ordered composition; small integer counters are deliberately narrowed for display-space colors and grid layout."
)]

use crate::trace;
use gpui_3d::{
    AlphaMode, Camera, EnvironmentMap, GltfLoadOptions, Light, Material, Mesh, Node, Scene,
    ShadingModel, SpotCone, SpotLight, TextureAsset, TextureAssetId, Transform, Vec3,
};
use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;

/// Which world source to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldSource {
    /// Prefer the downloaded glTF, fall back to the helmet fixture, then procedural.
    Model,
    /// Skip glTF entirely and build the procedural instanced world.
    Studio,
}

/// The assembled world plus everything needed to render it on either path.
pub(crate) struct WorldScene {
    pub scene: Scene,
    pub textures: HashMap<TextureAssetId, Arc<TextureAsset>>,
    /// Radius of the world bounds, used to frame the starting camera.
    pub radius: f32,
    /// World-space center, used as the camera look-at target.
    pub center: Vec3,
    /// A short description of what was actually loaded (printed in the report).
    pub description: String,
}

/// Options controlling world construction.
pub(crate) struct WorldOptions {
    pub source: WorldSource,
    /// Path to the primary downloaded model (default `assets/Sponza.glb`).
    pub model_path: String,
    /// Fallback model shipped in the repository test assets.
    pub fallback_model_path: String,
    /// Number of instanced buildings in the procedural world.
    pub instances: u32,
    /// Sun azimuth / elevation in degrees.
    pub sun: (f32, f32),
}

impl Default for WorldOptions {
    fn default() -> Self {
        Self {
            source: WorldSource::Model,
            model_path: "crates/gpui-3d/examples/world_bench/assets/Sponza.glb".to_string(),
            fallback_model_path: "crates/gpui-3d/test_assets/cache/DamagedHelmet.glb".to_string(),
            instances: 20_000,
            sun: (135.0, 52.0),
        }
    }
}

/// Builds the shared world for the requested source.
pub(crate) fn build(options: &WorldOptions) -> Result<WorldScene, Box<dyn Error>> {
    trace::event("scene.build", "begin", None);

    // Try the downloaded model, then the in-repo fixture.
    let mut imported = if options.source == WorldSource::Model {
        let primary = Path::new(&options.model_path);
        if primary.exists() {
            match load_model(primary) {
                Ok(gltf) => Some(gltf),
                Err(error) => {
                    eprintln!("world_bench: primary model failed: {error}");
                    try_fallback(options)
                }
            }
        } else {
            println!(
                "world_bench: primary model not found at {} (run fetch_model.ps1); using fallback",
                primary.display()
            );
            try_fallback(options)
        }
    } else {
        None
    };

    // The procedural world supplies the ground, instanced buildings and the four lights. A
    // downloaded model is grafted on top so both a real complex model and a large scene exist.
    let mut proc = build_procedural(options)?;
    let description;
    if let Some(gltf) = imported.take() {
        description = format!(
            "glTF model ({} nodes, {} triangles, {} textures) + procedural instanced world",
            gltf.scene.len(),
            gltf.statistics.triangles,
            gltf.statistics.textures
        );
        graft(&mut proc.scene, &gltf.scene)?;
        for (id, texture) in gltf.textures {
            proc.textures.insert(id, texture);
        }
    } else {
        description = "procedural instanced world (model unavailable)".to_string();
    }

    let (center, radius) = scene_bounds(&proc.scene);
    trace::event("scene.build", "done", Some(&description));
    Ok(WorldScene {
        scene: proc.scene,
        textures: proc.textures,
        radius,
        center,
        description,
    })
}

/// Frames a starting camera around the world center.
pub(crate) fn starting_camera(world: &WorldScene, yaw: f32, pitch: f32) -> Camera {
    let radius = world.radius.max(0.05);
    let distance = radius * 1.8;
    let horizontal = distance * pitch.cos();
    let target = Vec3::new(world.center.x, world.center.y - radius * 0.05, world.center.z);
    let eye = Vec3::new(
        target.x + horizontal * yaw.sin(),
        target.y + distance * pitch.sin(),
        target.z + horizontal * yaw.cos(),
    );
    Camera::perspective(
        eye,
        target,
        Vec3::Y,
        50.0_f32.to_radians(),
        (radius * 0.005).max(0.01),
        distance * 12.0 + radius * 6.0,
    )
}

/// A deterministic studio environment map for the direct renderer's sky and IBL.
pub(crate) fn environment() -> Arc<EnvironmentMap> {
    Arc::new(EnvironmentMap::procedural_studio())
}

fn load_model(path: &Path) -> Result<gpui_3d::GltfScene, Box<dyn Error>> {
    trace::event("scene.model.load", &path.to_string_lossy(), None);
    let options = GltfLoadOptions::default();
    let gltf = gpui_3d::load_path(path, &options)
        .map_err(|error| format!("import {}: {error}", path.display()))?;
    println!(
        "world_bench: loaded {} (nodes={} triangles={} textures={} warnings={})",
        path.display(),
        gltf.scene.len(),
        gltf.statistics.triangles,
        gltf.statistics.textures,
        gltf.warnings.len()
    );
    for warning in &gltf.warnings {
        println!("world_bench: import warning: {warning}");
    }
    Ok(gltf)
}

fn try_fallback(options: &WorldOptions) -> Option<gpui_3d::GltfScene> {
    let path = Path::new(&options.fallback_model_path);
    if path.exists() {
        match load_model(path) {
            Ok(gltf) => return Some(gltf),
            Err(error) => eprintln!("world_bench: fallback model failed: {error}"),
        }
    }
    None
}

/// Copies a glTF scene graph into `target`, preserving hierarchy, transforms and lights.
fn graft(target: &mut Scene, source: &Scene) -> Result<(), Box<dyn Error>> {
    fn copy_node(
        source: &Scene,
        target: &mut Scene,
        id: gpui_3d::NodeId,
        parent: Option<gpui_3d::NodeId>,
    ) -> Result<(), Box<dyn Error>> {
        let node = source
            .node(id)
            .ok_or_else(|| format!("missing source node {id:?}"))?;
        let mut copy = Node::new()
            .with_transform(node.transform)
            .with_visible(node.visible)
            .with_materials(node.materials.iter().cloned())
            .with_lod(node.lod.iter().cloned());
        if let Some(mesh) = &node.mesh {
            copy = copy.with_mesh(mesh.clone());
        }
        if let Some(light) = node.light {
            copy = copy.with_light(light);
        }
        let copied_id = target.insert(parent, copy)?;
        for &child in source.children(id).unwrap_or_default() {
            copy_node(source, target, child, Some(copied_id))?;
        }
        Ok(())
    }
    for &root in source.roots() {
        copy_node(source, target, root, None)?;
    }
    Ok(())
}

/// Builds the procedural large world: ground, an instanced building field, and four lights.
fn build_procedural(options: &WorldOptions) -> Result<WorldScene, Box<dyn Error>> {
    let mut scene = Scene::new();
    let mut textures = HashMap::new();

    // Procedural albedo / normal / occlusion maps shared by the instanced buildings.
    let albedo = TextureAsset::rgba8(256, 256, checker_pixels(256, 16))?;
    let albedo_id = albedo.id();
    textures.insert(albedo_id, Arc::new(albedo));
    let normal = TextureAsset::linear_rgba8(256, 256, normal_pixels(256))?;
    let normal_id = normal.id();
    textures.insert(normal_id, Arc::new(normal));
    let occlusion = TextureAsset::linear_rgba8(256, 256, occlusion_pixels(256))?;
    let occlusion_id = occlusion.id();
    textures.insert(occlusion_id, Arc::new(occlusion));

    let mut building = Material::new();
    building.base_color = [0.62, 0.66, 0.76, 1.0];
    building.metallic = 0.18;
    building.roughness = 0.42;
    building.albedo_texture = Some(albedo_id);
    building.normal_texture = Some(normal_id);
    building.occlusion_texture = Some(occlusion_id);
    building.occlusion_strength = 0.7;
    let building = Arc::new(building.validated().ok_or("building material invalid")?);

    let mut ground = Material::new();
    ground.base_color = [0.22, 0.26, 0.23, 1.0];
    ground.roughness = 0.92;
    let ground = Arc::new(ground.validated().ok_or("ground material invalid")?);

    let mut accent = Material::new();
    accent.base_color = [0.95, 0.42, 0.12, 1.0];
    accent.emissive = [1.0, 0.32, 0.05, 0.7];
    accent.shading_model = ShadingModel::Unlit;
    accent.alpha_mode = AlphaMode::Opaque;
    let accent = Arc::new(accent.validated().ok_or("accent material invalid")?);

    // A tangent-bearing cube is required for materials that sample a normal map. Without
    // tangents the offscreen renderer tolerates it, but the sidecar scene-view preparation
    // cannot build the normal-map attributes and silently drops the scene.
    let cube = Arc::new(
        Mesh::cube()
            .generate_tangents()?
            .into_parts()
            .0,
    );
    let (center, radius) = build_building_field(&mut scene, cube.clone(), building, options.instances);
    let extent = (radius * 2.0).max(20.0);

    // Ground plane beneath the city.
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::plane(extent * 1.4, extent * 1.4)?))
            .with_materials([ground])
            .with_transform(Transform {
                translation: Vec3::new(center.x, center.y - radius * 0.6, center.z),
                ..Transform::IDENTITY
            }),
    )?;

    // Emissive landmarks scattered at the center and the four quadrant corners.
    let landmark_scale = (radius * 0.03).max(0.5);
    let landmarks = [
        Vec3::new(center.x, center.y, center.z),
        Vec3::new(center.x - radius * 0.6, center.y, center.z - radius * 0.6),
        Vec3::new(center.x + radius * 0.6, center.y, center.z - radius * 0.6),
        Vec3::new(center.x - radius * 0.6, center.y, center.z + radius * 0.6),
        Vec3::new(center.x + radius * 0.6, center.y, center.z + radius * 0.6),
    ];
    for position in landmarks {
        scene.insert(
            None,
            Node::new()
                .with_mesh(cube.clone())
                .with_materials([accent.clone()])
                .with_transform(Transform {
                    translation: position,
                    scale: Vec3::new(landmark_scale, landmark_scale * 3.0, landmark_scale),
                    ..Transform::IDENTITY
                }),
        )?;
    }

    insert_lights(&mut scene, options, center, radius)?;
    Ok(WorldScene {
        scene,
        textures,
        radius,
        center,
        description: "procedural instanced world".to_string(),
    })
}

/// Lays out `count` instanced buildings in a jittered grid and returns (center, radius).
fn build_building_field(
    scene: &mut Scene,
    cube: Arc<Mesh>,
    material: Arc<Material>,
    count: u32,
) -> (Vec3, f32) {
    let count = count.max(1);
    let side = (f64::from(count).sqrt().ceil() as u32).max(1);
    let spacing = 2.4_f32;
    let offset = (side as f32 - 1.0) * 0.5;
    for index in 0..count {
        let column = (index % side) as f32;
        let row = (index / side) as f32;
        // A deterministic hash keeps the layout stable without a random dependency.
        let hash = (index.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_215.0;
        let jitter = (hash - 0.5) * spacing * 0.35;
        let height = 1.0 + (index % 9) as f32 * 0.9;
        let translation = Vec3::new(
            (column - offset) * spacing + jitter,
            height * 0.5,
            (row - offset) * spacing - jitter,
        );
        if let Err(error) = scene.insert(
            None,
            Node::new()
                .with_mesh(cube.clone())
                .with_materials([material.clone()])
                .with_transform(Transform {
                    translation,
                    scale: Vec3::new(0.9, height, 0.9),
                    ..Transform::IDENTITY
                }),
        ) {
            eprintln!("world_bench: skipped building {index}: {error}");
        }
    }
    let half = (side as f32 * spacing) * 0.5;
    (Vec3::ZERO, half.max(1.0))
}

fn insert_lights(
    scene: &mut Scene,
    options: &WorldOptions,
    center: Vec3,
    radius: f32,
) -> Result<(), Box<dyn Error>> {
    let azimuth = options.sun.0.to_radians();
    let elevation = options.sun.1.to_radians();
    let sun = Vec3::new(
        -(elevation.cos() * azimuth.sin()),
        -elevation.sin(),
        -(elevation.cos() * azimuth.cos()),
    );
    scene.insert(
        None,
        Node::new().with_light(Light::Directional {
            direction: sun,
            color: [1.0, 0.94, 0.84],
            intensity: 4.0,
        }),
    )?;
    scene.insert(
        None,
        Node::new().with_light(Light::Ambient {
            color: [0.42, 0.52, 0.72],
            intensity: 0.24,
        }),
    )?;
    scene.insert(
        None,
        Node::new().with_light(Light::Point {
            position: center + Vec3::new(radius * 0.15, radius * 0.35, radius * 0.15),
            color: [1.0, 0.66, 0.34],
            intensity: 900.0,
            range: radius * 2.0,
        }),
    )?;
    let spot = SpotLight::new(
        center + Vec3::new(-radius * 0.2, radius * 0.5, radius * 0.2),
        Vec3::new(0.3, -1.0, -0.3),
        radius * 3.0,
        SpotCone::new(0.22, 0.55)?,
    )?
    .with_color([0.55, 0.7, 1.0])?
    .with_intensity(1500.0)?;
    scene.insert(None, Node::new().with_light(Light::Spot(spot)))?;
    Ok(())
}

fn scene_bounds(scene: &Scene) -> (Vec3, f32) {
    let Some(bounds) = scene.bounds().ok().flatten() else {
        return (Vec3::ZERO, 1.0);
    };
    let center = Vec3::new(
        (bounds.min.x + bounds.max.x) * 0.5,
        (bounds.min.y + bounds.max.y) * 0.5,
        (bounds.min.z + bounds.max.z) * 0.5,
    );
    let extent = Vec3::new(
        bounds.max.x - bounds.min.x,
        bounds.max.y - bounds.min.y,
        bounds.max.z - bounds.min.z,
    );
    let radius = (extent.x * extent.x + extent.y * extent.y + extent.z * extent.z).sqrt() * 0.5;
    (center, radius.max(0.5))
}

fn checker_pixels(size: u32, cells: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            let cell = ((x * cells / size) + (y * cells / size)) % 2;
            let (r, g, b) = if cell == 0 {
                (214, 220, 232)
            } else {
                (150, 160, 178)
            };
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    pixels
}

fn normal_pixels(size: u32) -> Vec<u8> {
    use std::f32::consts::TAU;
    let sample = |x: i32, y: i32| -> f32 {
        let x = x.rem_euclid(size as i32) as f32 / size as f32;
        let y = y.rem_euclid(size as i32) as f32 / size as f32;
        (x * TAU * 4.0).sin() * (y * TAU * 4.0).sin() * 0.5
    };
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let dx = sample(x + 1, y) - sample(x - 1, y);
            let dy = sample(x, y + 1) - sample(x, y - 1);
            let normal = Vec3::new(-dx * 12.0, -dy * 12.0, 1.0)
                .normalized()
                .unwrap_or(Vec3::Z);
            pixels.extend_from_slice(&[
                ((normal.x * 0.5 + 0.5) * 255.0) as u8,
                ((normal.y * 0.5 + 0.5) * 255.0) as u8,
                ((normal.z * 0.5 + 0.5) * 255.0) as u8,
                255,
            ]);
        }
    }
    pixels
}

fn occlusion_pixels(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let value = (0.55
                + 0.45 * (u * std::f32::consts::PI).sin() * (v * std::f32::consts::PI).sin())
            .clamp(0.0, 1.0);
            let byte = (value * 255.0) as u8;
            pixels.extend_from_slice(&[byte, byte, byte, 255]);
        }
    }
    pixels
}
