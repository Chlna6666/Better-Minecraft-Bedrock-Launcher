//! Native GPU memory-lifetime, incremental-upload, resize, and picking tests.
//!
//! Run explicitly:
//!
//!   cargo test -p gpui-3d --test renderer_memory -- --ignored --test-threads=1
//!
//! These tests use a real DX12 device and assert that caches stay bounded, budget trimming
//! releases idle assets, animation frames upload only changed instance ranges, resize keeps the
//! mesh/texture caches warm, and CPU/GPU picking agree on the submitted frame.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::doc_markdown,
    reason = "Test scene spacing and pixel centers use small integer counts converted to f32; exact rounding is irrelevant to the cache and picking assertions."
)]

use gpui_3d::{
    Camera, Keyframe, Material, Mesh, Node, NodeId, OffscreenRenderer, RenderOptions, Scene,
    ShadowOptions, TextureAsset, TextureAssetId, Transform, TransformTrack, Vec3, Vec3Track,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn descriptor() -> gfx_core::DeviceDescriptor {
    gfx_core::DeviceDescriptor {
        application_name: "gpui-3d renderer memory test".to_string(),
        power_preference: gfx_core::PowerPreference::HighPerformance,
        ..gfx_core::DeviceDescriptor::default()
    }
}

fn options() -> RenderOptions {
    RenderOptions {
        shadows: ShadowOptions {
            enabled: false,
            ..ShadowOptions::default()
        },
        ..RenderOptions::default()
    }
}

fn camera() -> Camera {
    Camera::perspective(
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::ZERO,
        Vec3::Y,
        45.0_f32.to_radians(),
        0.1,
        100.0,
    )
}

fn wide_camera() -> Camera {
    Camera::perspective(
        Vec3::new(0.0, 0.0, 10.0),
        Vec3::ZERO,
        Vec3::Y,
        70.0_f32.to_radians(),
        0.1,
        100.0,
    )
}

fn empty_scene() -> Scene {
    Scene::new()
}

fn checker_texture(seed: u8) -> Arc<TextureAsset> {
    let mut pixels = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32_u8 {
        for x in 0..32_u8 {
            let value = if (x / 4 + y / 4) % 2 == seed % 2 {
                220_u8
            } else {
                60_u8
            };
            pixels.extend_from_slice(&[value, value / 2, seed, 255]);
        }
    }
    Arc::new(TextureAsset::rgba8(32, 32, pixels).expect("checker texture"))
}

fn cube_scene(texture: Option<TextureAssetId>) -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.base_color = [0.7, 0.45, 0.2, 1.0];
    material.albedo_texture = texture;
    let node = scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(material.validated().expect("material"))])
                .with_transform(Transform {
                    scale: Vec3::new(1.4, 1.4, 1.4),
                    ..Transform::default()
                }),
        )
        .expect("cube node");
    (scene, node)
}

fn pick_viewport(renderer: &OffscreenRenderer) -> (f32, f32) {
    (
        renderer.width() as f32 / 2.0,
        renderer.height() as f32 / 2.0,
    )
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn memory_budget_evicts_idle_assets_and_trim_releases_caches() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 128, 96).expect("renderer");
    let empty_textures = HashMap::new();
    renderer
        .render(
            &mut device,
            &empty_scene(),
            &empty_textures,
            camera(),
            &options(),
        )
        .expect("baseline frame");
    let baseline = renderer.statistics();
    let overhead = baseline.target_bytes
        + baseline.resident_buffer_bytes
        + baseline.cached_texture_bytes
        + baseline.cached_mesh_bytes;
    renderer.set_memory_budget(Some(overhead + 1));
    assert_eq!(renderer.memory_budget(), Some(overhead + 1));

    let texture = checker_texture(1);
    let texture_id = texture.id();
    let mut textures = HashMap::new();
    textures.insert(texture_id, texture.clone());
    let (scene, _) = cube_scene(Some(texture_id));
    renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("load frame");
    let loaded = renderer.statistics();
    assert!(loaded.cached_mesh_bytes > 0);
    assert!(loaded.cached_texture_bytes > baseline.cached_texture_bytes);
    assert_eq!(loaded.mesh_cache_misses, 1);
    assert_eq!(loaded.live_meshes, 1);
    assert_eq!(loaded.live_textures, 1);

    // A frame that references no assets makes them idle; the soft budget trims them.
    renderer
        .render(&mut device, &empty_scene(), &textures, camera(), &options())
        .expect("idle frame");
    let trimmed = renderer.statistics();
    assert!(trimmed.evictions >= 2, "{trimmed:?}");
    assert_eq!(trimmed.live_meshes, 0);
    assert_eq!(trimmed.live_textures, 0);
    let resident = trimmed.resident_buffer_bytes + trimmed.resident_texture_bytes;
    assert!(
        resident <= overhead + 16,
        "resident {resident} above budget {}",
        overhead + 1
    );
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn asset_cycling_stays_bounded_and_aggressive_trim_reclaims_everything() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 128, 96).expect("renderer");
    let textures = HashMap::new();
    let mut peak = 0;
    for cycle in 0..8_u32 {
        let mut material = Material::new();
        material.base_color = [0.2 + cycle as f32 * 0.05, 0.5, 0.4, 1.0];
        let mesh = Arc::new(Mesh::uv_sphere(0.6, [16, 24]).expect("sphere"));
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh)
                    .with_materials([Arc::new(material.validated().expect("material"))]),
            )
            .expect("cycle node");
        renderer
            .render(&mut device, &scene, &textures, camera(), &options())
            .expect("cycle frame");
        let statistics = renderer.statistics();
        peak = peak.max(statistics.resident_buffer_bytes + statistics.resident_texture_bytes);
        assert_eq!(statistics.live_meshes, (cycle + 1) as usize);
    }
    let loaded = renderer.statistics();
    assert_eq!(loaded.live_meshes, 8);
    assert!(loaded.peak_resident_bytes >= peak);
    renderer
        .render(&mut device, &empty_scene(), &textures, camera(), &options())
        .expect("empty frame");
    renderer
        .trim(&mut device, gfx_core::MemoryTrimLevel::Aggressive)
        .expect("aggressive trim");
    let trimmed = renderer.statistics();
    assert_eq!(trimmed.live_meshes, 0);
    assert_eq!(trimmed.live_textures, 0);
    assert_eq!(trimmed.resource_set_count, 0);
    assert!(trimmed.evictions >= 8);
    assert!(trimmed.peak_resident_bytes >= trimmed.resident_buffer_bytes);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn animation_frame_uploads_only_changed_instance_ranges() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let mesh = Arc::new(Mesh::cube());
    let material = Arc::new(Material::new().validated().expect("material"));
    let mut scene = Scene::new();
    let mut nodes = Vec::new();
    for row in 0..8 {
        for column in 0..8 {
            let node = scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_materials([material.clone()])
                        .with_transform(Transform {
                            translation: Vec3::new(
                                column as f32 * 0.5 - 1.75,
                                row as f32 * 0.5 - 1.75,
                                0.0,
                            ),
                            scale: Vec3::new(0.2, 0.2, 0.2),
                            ..Transform::default()
                        }),
                )
                .expect("instance node");
            nodes.push(node);
        }
    }
    let start = Vec3::new(-1.75, -1.75, 0.0);
    let moved = Vec3::new(-1.25, -1.75, 0.0);
    let track = TransformTrack::new(nodes[0]).with_translation(
        Vec3Track::new([
            Keyframe::new(Duration::ZERO, start),
            Keyframe::new(Duration::from_secs(1), moved),
        ])
        .expect("translation track"),
    );
    let first = renderer
        .render_animated(
            &mut device,
            &scene,
            std::slice::from_ref(&track),
            Duration::ZERO,
            &textures,
            wide_camera(),
            &options(),
        )
        .expect("first animated frame");
    assert_eq!(first.instances, 64);
    assert_eq!(first.draw_calls, 1, "shared mesh/material must batch");
    let second = renderer
        .render_animated(
            &mut device,
            &scene,
            &[track],
            Duration::from_millis(500),
            &textures,
            wide_camera(),
            &options(),
        )
        .expect("second animated frame");
    assert_eq!(second.mesh_uploads, 0);
    assert_eq!(second.texture_uploads, 0);
    assert_eq!(second.draw_calls, 1);
    assert_eq!(second.instances, 64);
    // One 144-byte instance changed; the full instance table is 64 * 144 = 9216 bytes.
    assert!(
        second.upload_bytes >= 128 && second.upload_bytes <= 512,
        "uploaded {} bytes for one changed instance",
        second.upload_bytes
    );
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn resize_stress_keeps_caches_warm_and_targets_correct() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 64, 64).expect("renderer");
    let textures = HashMap::new();
    let (scene, _) = cube_scene(None);
    let mut first_frame = true;
    for (width, height) in [(64_u32, 64_u32), (160, 90), (96, 96), (320, 180), (128, 72)] {
        renderer.resize(&mut device, width, height).expect("resize");
        let frame = renderer
            .render(&mut device, &scene, &textures, camera(), &options())
            .expect("resized frame");
        let image = renderer.capture(&mut device).expect("resized capture");
        assert_eq!((image.width(), image.height()), (width, height));
        if first_frame {
            assert!(frame.mesh_uploads <= 1);
            first_frame = false;
        } else {
            assert_eq!(frame.mesh_uploads, 0, "mesh cache lost across resize");
        }
        let statistics = renderer.statistics();
        assert_eq!(statistics.target_width, width);
        assert_eq!(statistics.target_height, height);
        assert_eq!(statistics.resource_set_count, 2);
    }
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn cpu_and_gpu_object_picking_agree_on_the_submitted_frame() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 128, 96).expect("renderer");
    let textures = HashMap::new();
    let (scene, node) = cube_scene(None);
    renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("frame");
    let (center_x, center_y) = pick_viewport(&renderer);
    let cpu_hit = renderer
        .pick(&scene, camera(), (center_x, center_y))
        .expect("cpu pick")
        .expect("cpu center hit");
    assert_eq!(cpu_hit.node, node);
    let object_hit = renderer
        .pick_object(
            &mut device,
            &scene,
            &textures,
            camera(),
            &options(),
            (center_x as u32, center_y as u32),
        )
        .expect("object pick")
        .expect("gpu center hit");
    assert_eq!(object_hit.node, node);
    assert!(
        renderer
            .pick(&scene, camera(), (1.0, 1.0))
            .expect("cpu corner pick")
            .is_none()
    );
    assert!(
        renderer
            .pick_object(&mut device, &scene, &textures, camera(), &options(), (1, 1))
            .expect("gpu corner pick")
            .is_none()
    );
    renderer.destroy(&mut device).expect("destroy renderer");
}

fn instance_field_scene(count: u32) -> (Scene, Camera, NodeId) {
    let mut scene = Scene::new();
    let mesh = Arc::new(Mesh::cube());
    let material = Arc::new(Material::new().validated().expect("material"));
    let side = (f64::from(count).sqrt().ceil() as u32).max(1);
    let spacing = 0.35_f32;
    let offset = (side as f32 - 1.0) * 0.5;
    let mut first_node = None;
    for index in 0..count {
        let column = (index % side) as f32;
        let row = (index / side) as f32;
        let node = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([material.clone()])
                    .with_transform(Transform {
                        translation: Vec3::new(
                            (column - offset) * spacing,
                            0.0,
                            (row - offset) * spacing,
                        ),
                        scale: Vec3::new(0.2, 0.2, 0.2),
                        ..Transform::default()
                    }),
            )
            .expect("field node");
        first_node.get_or_insert(node);
    }
    scene
        .insert(
            None,
            Node::new().with_light(gpui_3d::Light::Directional {
                direction: Vec3::new(-0.4, -0.8, -0.3),
                color: [1.0, 0.95, 0.9],
                intensity: 3.5,
            }),
        )
        .expect("field light");
    let radius = ((side as f32 * spacing).powi(2) * 2.0).sqrt() * 0.5 + 0.5;
    let camera = Camera::perspective(
        Vec3::new(0.0, radius * 1.6, radius * 4.2),
        Vec3::new(0.0, radius * 0.15, 0.0),
        Vec3::Y,
        55.0_f32.to_radians(),
        0.05,
        radius * 12.0 + 50.0,
    );
    (scene, camera, first_node.expect("at least one field node"))
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn static_frames_reuse_and_instanced_batches_keep_memory_linear() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let (mut scene, camera, first_node) = instance_field_scene(10_000);
    let first = renderer
        .render(&mut device, &scene, &textures, camera, &options())
        .expect("first field frame");
    renderer.capture(&mut device).expect("sync field frame");
    assert_eq!(
        first.draw_calls, 1,
        "instanced field must be one indexed draw"
    );
    assert_eq!(first.instances, 10_000);
    assert_eq!(first.mesh_uploads, 1);
    assert!(first.upload_bytes >= 10_000 * 144);
    let loaded = renderer.statistics();
    assert_eq!(
        loaded.draw_capacity, 1,
        "draw slots must not scale with instances"
    );
    assert!(loaded.instance_capacity >= 10_000);
    assert!(
        loaded.resident_buffer_bytes < 2 * 10_000 * 144,
        "buffer bytes {} should stay near the instance table",
        loaded.resident_buffer_bytes
    );

    let second = renderer
        .render(&mut device, &scene, &textures, camera, &options())
        .expect("reused field frame");
    assert!(second.frame_reused, "unchanged frame must not resubmit");
    assert_eq!(second.upload_bytes, 0);
    assert_eq!(second.mesh_uploads, 0);
    assert_eq!(second.draw_calls, 1);

    let moved_camera = Camera::perspective(
        Vec3::new(2.0, 9.0, 21.0),
        Vec3::ZERO,
        Vec3::Y,
        55.0_f32.to_radians(),
        0.05,
        200.0,
    );
    let third = renderer
        .render(&mut device, &scene, &textures, moved_camera, &options())
        .expect("camera-only frame");
    assert!(
        third.camera_only_update,
        "camera move must skip the scene walk"
    );
    assert!(!third.frame_reused);
    assert_eq!(third.mesh_uploads, 0);
    assert_eq!(third.draw_calls, 1);
    assert!(
        third.upload_bytes <= 512,
        "camera-only frame uploaded {} bytes",
        third.upload_bytes
    );

    scene.node_mut(first_node).unwrap().transform.translation.y += 0.3;
    let fourth = renderer
        .render(&mut device, &scene, &textures, camera, &options())
        .expect("one-instance mutation frame");
    assert!(!fourth.camera_only_update);
    assert_eq!(fourth.draw_calls, 1);
    assert_eq!(fourth.instances, 10_000);
    assert!(
        fourth.upload_bytes >= 128 && fourth.upload_bytes <= 1024,
        "one changed instance uploaded {} bytes",
        fourth.upload_bytes
    );
    renderer.destroy(&mut device).expect("destroy renderer");
}
