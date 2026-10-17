//! Deterministic procedural studio scene for the offscreen scene lab.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "Procedural texture and scene authoring reads as one ordered composition; pixel math intentionally uses f32 display values from small integer counters."
)]
//!
//! The scene exercises the renderer features that have GPU-visible results in one capture:
//! metallic/roughness PBR, albedo + tangent-space normal + occlusion maps, instanced draws,
//! all four light kinds, alpha blending, an unlit emissive material, and directional shadows.

use gpui_3d::{
    AlphaMode, Camera, Light, Material, Mesh, Node, Quat, Scene, ShadingModel, SpotCone, SpotLight,
    TextureAsset, TextureAssetId, Transform, Vec3,
};
use std::collections::HashMap;
use std::error::Error;
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

pub(crate) struct StudioScene {
    pub scene: Scene,
    pub textures: HashMap<TextureAssetId, Arc<TextureAsset>>,
}

pub(crate) struct StudioOptions {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub sun_azimuth_degrees: f32,
    pub sun_elevation_degrees: f32,
}

impl Default for StudioOptions {
    fn default() -> Self {
        Self {
            yaw: 0.62,
            pitch: 0.30,
            distance: 11.5,
            sun_azimuth_degrees: 135.0,
            sun_elevation_degrees: 52.0,
        }
    }
}

pub(crate) fn build(options: &StudioOptions) -> Result<StudioScene, Box<dyn Error>> {
    let mut scene = Scene::new();
    let mut textures = HashMap::new();

    let checker = TextureAsset::rgba8(256, 256, checker_pixels(256, 8))?;
    let checker_id = checker.id();
    textures.insert(checker_id, Arc::new(checker));
    let normal_map = TextureAsset::linear_rgba8(256, 256, normal_pixels(256))?;
    let normal_id = normal_map.id();
    textures.insert(normal_id, Arc::new(normal_map));
    let occlusion = TextureAsset::linear_rgba8(256, 256, occlusion_pixels(256, 8))?;
    let occlusion_id = occlusion.id();
    textures.insert(occlusion_id, Arc::new(occlusion));

    let floor = arc_material({
        let mut material = Material::new();
        material.base_color = [0.52, 0.54, 0.58, 1.0];
        material.roughness = 0.86;
        material.albedo_texture = Some(checker_id);
        material.occlusion_texture = Some(occlusion_id);
        material.occlusion_strength = 0.55;
        material
    })?;
    let copper = arc_material({
        let mut material = Material::new();
        material.base_color = [0.95, 0.64, 0.54, 1.0];
        material.metallic = 1.0;
        material.roughness = 0.24;
        material
    })?;
    let gold = arc_material({
        let mut material = Material::new();
        material.base_color = [1.0, 0.78, 0.34, 1.0];
        material.metallic = 1.0;
        material.roughness = 0.18;
        material
    })?;
    let ceramic = arc_material({
        let mut material = Material::new();
        material.base_color = [0.11, 0.44, 0.42, 1.0];
        material.metallic = 0.0;
        material.roughness = 0.4;
        material.albedo_texture = Some(checker_id);
        material.normal_texture = Some(normal_id);
        material.occlusion_texture = Some(occlusion_id);
        material.occlusion_strength = 0.8;
        material.double_sided = true;
        material
    })?;
    let painted = arc_material({
        let mut material = Material::new();
        material.base_color = [0.52, 0.6, 0.78, 1.0];
        material.metallic = 0.0;
        material.roughness = 0.46;
        material.albedo_texture = Some(checker_id);
        material
    })?;
    let glass = arc_material({
        let mut material = Material::new();
        material.base_color = [0.36, 0.62, 0.72, 0.34];
        material.metallic = 0.0;
        material.roughness = 0.08;
        material.alpha_mode = AlphaMode::Blend;
        material
    })?;
    let glow = arc_material({
        let mut material = Material::new();
        material.base_color = [0.85, 0.2, 0.04, 1.0];
        material.emissive = [1.0, 0.35, 0.05, 0.8];
        material.shading_model = ShadingModel::Unlit;
        material
    })?;

    let floor_mesh = Arc::new(Mesh::plane(26.0, 26.0)?);
    let cube_mesh = Arc::new(Mesh::cube());
    let sphere_mesh = Arc::new(Mesh::uv_sphere(1.0, [48, 64])?);
    let tangent_sphere = Arc::new(
        Mesh::uv_sphere(1.0, [48, 64])?
            .generate_tangents()?
            .into_parts()
            .0,
    );
    let cylinder_mesh = Arc::new(Mesh::cylinder(0.85, 2.4, 48)?);
    let cone_mesh = Arc::new(Mesh::cone(0.9, 1.8, 48)?);

    let mut insert = |node: Node| -> Result<(), Box<dyn Error>> {
        scene.insert(None, node)?;
        Ok(())
    };

    insert(
        Node::new()
            .with_mesh(floor_mesh)
            .with_materials([floor.clone()])
            .with_transform(Transform {
                translation: Vec3::new(0.0, -1.2, 0.0),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(sphere_mesh.clone())
            .with_materials([copper.clone()])
            .with_transform(Transform {
                translation: Vec3::new(-3.1, -0.1, 0.5),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(tangent_sphere.clone())
            .with_materials([ceramic.clone()])
            .with_transform(Transform {
                translation: Vec3::new(3.3, -0.1, 0.2),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(cylinder_mesh)
            .with_materials([gold.clone()])
            .with_transform(Transform {
                translation: Vec3::new(0.0, -0.25, -0.3),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(cone_mesh)
            .with_materials([copper.clone()])
            .with_transform(Transform {
                translation: Vec3::new(-1.3, -0.35, 2.5),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(sphere_mesh.clone())
            .with_materials([glass.clone()])
            .with_transform(Transform {
                translation: Vec3::new(2.7, -0.35, 3.0),
                scale: Vec3::new(0.62, 0.62, 0.62),
                ..Transform::default()
            }),
    )?;
    insert(
        Node::new()
            .with_mesh(cube_mesh.clone())
            .with_materials([glow.clone()])
            .with_transform(Transform {
                translation: Vec3::new(-5.2, 2.0, -1.0),
                scale: Vec3::new(0.34, 0.34, 0.34),
                ..Transform::default()
            }),
    )?;
    // Six small cubes share one mesh and one material snapshot, so the renderer must collapse
    // them into one indexed instanced draw instead of re-uploading geometry per instance.
    for (index, x) in [-4.6_f32, -3.0, -1.4, 0.2, 1.8, 3.4]
        .into_iter()
        .enumerate()
    {
        insert(
            Node::new()
                .with_mesh(cube_mesh.clone())
                .with_materials([painted.clone()])
                .with_transform(Transform {
                    translation: Vec3::new(x, -0.9, -3.4),
                    rotation: gpui_3d::Quat::from_axis_angle(Vec3::Y, index as f32 * 0.38 + 0.2)
                        .ok_or("invalid instance rotation")?,
                    scale: Vec3::new(0.42, 0.42, 0.42),
                }),
        )?;
    }

    let sun = sun_direction(
        options.sun_azimuth_degrees.to_radians(),
        options.sun_elevation_degrees.to_radians(),
    );
    insert(Node::new().with_light(Light::Directional {
        direction: sun,
        color: [1.0, 0.93, 0.82],
        intensity: 3.4,
    }))?;
    insert(Node::new().with_light(Light::Ambient {
        color: [0.42, 0.52, 0.72],
        intensity: 0.16,
    }))?;
    insert(Node::new().with_light(Light::Point {
        position: Vec3::new(3.4, 3.2, 2.8),
        color: [1.0, 0.6, 0.28],
        intensity: 70.0,
        range: 16.0,
    }))?;
    insert(
        Node::new().with_light(Light::Spot(
            SpotLight::new(
                Vec3::new(-3.8, 5.2, 2.4),
                Vec3::new(0.42, -0.62, -0.28),
                20.0,
                SpotCone::new(0.2, 0.5)?,
            )?
            .with_color([0.5, 0.68, 1.0])?
            .with_intensity(160.0)?,
        )),
    )?;

    Ok(StudioScene { scene, textures })
}

/// Builds a dense grid of cubes that share one mesh and one material snapshot.
///
/// The offscreen renderer must collapse these nodes into a single indexed instanced draw and keep
/// per-frame uploads proportional to changed instances, not to the node count. Returns the scene
/// and its world bounds radius so the caller can frame the camera.
pub(crate) fn build_instance_field(count: u32) -> Result<(Scene, f32), Box<dyn Error>> {
    let mut scene = Scene::new();
    let cube = Arc::new(Mesh::cube());
    let mut material = Material::new();
    material.base_color = [0.62, 0.68, 0.82, 1.0];
    material.metallic = 0.15;
    material.roughness = 0.5;
    let material = Arc::new(
        material
            .validated()
            .ok_or("instance material failed validation")?,
    );
    let side = (f64::from(count).sqrt().ceil() as u32).max(1);
    let spacing = 0.55_f32;
    let offset = (side as f32 - 1.0) * 0.5;
    for index in 0..count {
        let column = (index % side) as f32;
        let row = (index / side) as f32;
        let translation = Vec3::new(
            (column - offset) * spacing,
            ((index % 7) as f32 - 3.0) * 0.012,
            (row - offset) * spacing,
        );
        scene.insert(
            None,
            Node::new()
                .with_mesh(cube.clone())
                .with_materials([material.clone()])
                .with_transform(Transform {
                    translation,
                    rotation: Quat::from_axis_angle(Vec3::Y, (index % 17) as f32 * 0.2)
                        .ok_or("invalid field rotation")?,
                    scale: Vec3::new(0.28, 0.28, 0.28),
                }),
        )?;
    }
    scene.insert(
        None,
        Node::new().with_light(Light::Directional {
            direction: Vec3::new(-0.45, -0.8, -0.35),
            color: [1.0, 0.93, 0.82],
            intensity: 4.2,
        }),
    )?;
    scene.insert(
        None,
        Node::new().with_light(Light::Ambient {
            color: [0.44, 0.53, 0.72],
            intensity: 0.26,
        }),
    )?;
    let radius = scene
        .bounds()?
        .map_or(1.0, |bounds| {
            let extent = Vec3::new(
                bounds.max.x - bounds.min.x,
                bounds.max.y - bounds.min.y,
                bounds.max.z - bounds.min.z,
            );
            (extent.x * extent.x + extent.y * extent.y + extent.z * extent.z).sqrt() * 0.5
        })
        .max(0.5);
    Ok((scene, radius))
}

pub(crate) fn studio_camera(options: &StudioOptions) -> Camera {
    let target = Vec3::new(0.0, 0.15, 0.3);
    let horizontal = options.distance * options.pitch.cos();
    let eye = Vec3::new(
        target.x + horizontal * options.yaw.sin(),
        target.y + options.distance * options.pitch.sin(),
        target.z + horizontal * options.yaw.cos(),
    );
    Camera::perspective(eye, target, Vec3::Y, 42.0_f32.to_radians(), 0.1, 120.0)
}

fn arc_material(material: Material) -> Result<Arc<Material>, Box<dyn Error>> {
    let validated = material
        .validated()
        .ok_or("studio material failed validation")?;
    Ok(Arc::new(validated))
}

fn sun_direction(azimuth: f32, elevation: f32) -> Vec3 {
    let to_sun = Vec3::new(
        elevation.cos() * azimuth.sin(),
        elevation.sin(),
        elevation.cos() * azimuth.cos(),
    );
    Vec3::new(-to_sun.x, -to_sun.y, -to_sun.z)
}

fn checker_pixels(size: u32, cells: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            let cell = ((x * cells / size) + (y * cells / size)) % 2;
            let (r, g, b) = if cell == 0 {
                (226, 226, 232)
            } else {
                (158, 166, 178)
            };
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    pixels
}

fn normal_pixels(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    let sample = |x: i32, y: i32| -> f32 {
        let x = x.rem_euclid(size as i32) as f32 / size as f32;
        let y = y.rem_euclid(size as i32) as f32 / size as f32;
        (x * TAU * 6.0).sin() * (y * TAU * 6.0).sin() * 0.5
    };
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let dx = sample(x + 1, y) - sample(x - 1, y);
            let dy = sample(x, y + 1) - sample(x, y - 1);
            let strength = 14.0;
            let normal = Vec3::new(-dx * strength, -dy * strength, 1.0)
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

fn occlusion_pixels(size: u32, cells: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            let cell_x = (x * cells / size) as f32 / cells as f32;
            let cell_y = (y * cells / size) as f32 / cells as f32;
            let edge = (cell_x.min(cell_y) * 30.0).clamp(0.0, 1.0);
            let value = (0.45 + 0.55 * edge.min(1.0) + 0.1 * (1.0 - edge) * PI.sin()).min(1.0);
            let byte = (value * 255.0) as u8;
            pixels.extend_from_slice(&[byte, byte, byte, 255]);
        }
    }
    pixels
}
