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
    clippy::too_many_lines,
    reason = "Test scene spacing and pixel centers use small integer counts converted to f32; exact rounding is irrelevant to the cache and picking assertions."
)]

use gpui_3d::{
    AntialiasMode, BloomOptions, Camera, CapturedImage, Keyframe, Material, Mesh, Node, NodeId,
    OffscreenRenderer, PostEffects, RenderOptions, RenderQuality, Scene, ShadowOptions,
    TextureAsset, TextureAssetId, Transform, TransformTrack, Vec3, Vec3Track,
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
        assert_eq!(statistics.resource_set_count, 3);
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

fn large_texture(seed: u8) -> Arc<TextureAsset> {
    let mut pixels = Vec::with_capacity(512 * 512 * 4);
    for y in 0..512_u32 {
        for x in 0..512_u32 {
            let value = if ((x / 16) ^ (y / 16)) % 2 == u32::from(seed % 2) {
                200_u8
            } else {
                40_u8
            };
            pixels.extend_from_slice(&[value, seed, 160, 255]);
        }
    }
    Arc::new(TextureAsset::rgba8(512, 512, pixels).expect("large texture"))
}

fn textured_cube() -> (Scene, HashMap<TextureAssetId, Arc<TextureAsset>>) {
    let texture = large_texture(3);
    let texture_id = texture.id();
    let mut textures = HashMap::new();
    textures.insert(texture_id, texture);
    let mut material = Material::new();
    material.albedo_texture = Some(texture_id);
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(material.validated().expect("material"))])
                .with_transform(Transform {
                    scale: Vec3::new(1.5, 1.5, 1.5),
                    ..Transform::default()
                }),
        )
        .expect("cube node");
    (scene, textures)
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn long_session_scene_cycling_stays_bounded() {
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
    let overhead = baseline.target_bytes + baseline.resident_buffer_bytes;
    renderer.set_memory_budget(Some(overhead + 2 * 1024 * 1024));

    let scenes = [textured_cube(), textured_cube(), textured_cube()];
    let mut peak_resident = 0;
    let mut peak_resource_sets = 0;
    for frame in 0..60 {
        let (scene, textures) = &scenes[frame % scenes.len()];
        let statistics = renderer
            .render(&mut device, scene, textures, camera(), &options())
            .expect("cycling frame");
        assert_eq!(statistics.draw_calls, 1);
        if frame % 5 == 0 {
            renderer.capture(&mut device).expect("cycling capture");
        }
        let stats = renderer.statistics();
        peak_resident =
            peak_resident.max(stats.resident_buffer_bytes + stats.resident_texture_bytes);
        peak_resource_sets = peak_resource_sets.max(stats.resource_set_count);
        assert!(
            stats.resident_buffer_bytes + stats.resident_texture_bytes
                <= overhead + 4 * 1024 * 1024,
            "frame {frame} resident {} exceeded the soft budget envelope",
            stats.resident_buffer_bytes + stats.resident_texture_bytes
        );
        // The budget evicts the largest idle LRU asset first, so small meshes can outlive a
        // texture eviction; the resident envelope is the actual contract.
        assert!(stats.live_meshes <= 8, "frame {frame} kept too many meshes");
    }
    let steady = renderer.statistics();
    assert!(steady.evictions >= 10, "budget never evicted assets");
    assert!(
        peak_resource_sets <= 4,
        "resource sets leaked: {peak_resource_sets}"
    );
    assert!(peak_resident <= overhead + 4 * 1024 * 1024);
    assert!(steady.upload_bytes_total > 0);
    renderer
        .trim(&mut device, gfx_core::MemoryTrimLevel::Aggressive)
        .expect("aggressive trim");
    let trimmed = renderer.statistics();
    assert_eq!(trimmed.live_meshes, 0);
    assert_eq!(trimmed.live_textures, 0);
    assert_eq!(trimmed.resource_set_count, 0);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn multiple_renderers_share_one_device_without_cross_talk() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut first = OffscreenRenderer::new(&mut device, 96, 64).expect("first renderer");
    let mut second = OffscreenRenderer::new(&mut device, 160, 120).expect("second renderer");
    let textures = HashMap::new();
    let (scene, node) = cube_scene(None);
    let first_frame = first
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("first frame");
    let first_image = first.capture(&mut device).expect("first capture");
    let second_frame = second
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("second frame");
    let second_image = second.capture(&mut device).expect("second capture");
    assert_eq!(first_frame.draw_calls, 1);
    assert_eq!(second_frame.draw_calls, 1);
    assert_eq!((first_image.width(), first_image.height()), (96, 64));
    assert_eq!((second_image.width(), second_image.height()), (160, 120));
    let first_stats = first.statistics();
    let second_stats = second.statistics();
    assert_eq!(first_stats.target_width, 96);
    assert_eq!(second_stats.target_width, 160);
    assert_ne!(first_stats.target_bytes, second_stats.target_bytes);

    first.destroy(&mut device).expect("destroy first renderer");
    // The surviving renderer keeps its own targets and caches after the other renderer dies.
    let reused = second
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("surviving renderer frame");
    assert!(reused.frame_reused);
    let cpu_hit = second
        .pick(&scene, camera(), (80.0, 60.0))
        .expect("cpu pick")
        .expect("center hit");
    assert_eq!(cpu_hit.node, node);
    second
        .destroy(&mut device)
        .expect("destroy second renderer");
}

fn differing_pixels(first: &CapturedImage, second: &CapturedImage) -> usize {
    let left = first.rgba();
    let right = second.rgba();
    let mut changed = 0;
    for index in (0..left.len().min(right.len())).step_by(4) {
        if left[index..index + 4] != right[index..index + 4] {
            changed += 1;
        }
    }
    changed
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn hdr_post_effects_and_quality_presets_change_the_resolved_frame() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 192, 144).expect("renderer");
    let textures = HashMap::new();
    let (scene, node) = cube_scene(None);
    let mut plain = RenderOptions {
        shadows: ShadowOptions {
            enabled: false,
            ..ShadowOptions::default()
        },
        bloom: BloomOptions {
            enabled: false,
            ..BloomOptions::default()
        },
        antialias: AntialiasMode::None,
        post_effects: PostEffects {
            vignette: 0.0,
            grain: 0.0,
            dither: 0.0,
        },
        ..RenderOptions::default()
    };
    renderer
        .render(&mut device, &scene, &textures, camera(), &plain)
        .expect("plain frame");
    let plain_image = renderer.capture(&mut device).expect("plain capture");
    let hdr_stats = renderer.statistics();
    assert!(
        hdr_stats.target_bytes >= 192 * 144 * 16,
        "HDR scene target plus bloom must cost 16 bytes/pixel, got {}",
        hdr_stats.target_bytes
    );
    assert!(hdr_stats.buffer_allocation_count >= 4);
    assert!(hdr_stats.cpu_staging_bytes > 0);

    plain.bloom.enabled = true;
    plain.bloom.strength = 1.0;
    plain.bloom.threshold = 0.25;
    plain.antialias = AntialiasMode::Fxaa;
    plain.post_effects = PostEffects {
        vignette: 0.6,
        grain: 0.05,
        dither: 1.0,
    };
    renderer
        .render(&mut device, &scene, &textures, camera(), &plain)
        .expect("post effect frame");
    let effects_image = renderer.capture(&mut device).expect("effects capture");
    let changed = differing_pixels(&plain_image, &effects_image);
    assert!(changed > 50, "post effects only changed {changed} pixels");

    let reused = renderer
        .render(&mut device, &scene, &textures, camera(), &plain)
        .expect("reused effects frame");
    assert!(reused.frame_reused);

    let ultra = RenderQuality::Ultra.render_options();
    assert_eq!(ultra.shadows.resolution, 4096);
    assert!(ultra.bloom.enabled);
    assert_eq!(ultra.antialias, AntialiasMode::Fxaa);
    assert!(ultra.post_effects.dither > 0.0);
    let changed_quality = renderer
        .render(&mut device, &scene, &textures, camera(), &ultra)
        .expect("quality frame");
    assert!(
        !changed_quality.frame_reused,
        "changing quality options must invalidate the retained frame"
    );
    renderer
        .trim(&mut device, gfx_core::MemoryTrimLevel::Aggressive)
        .expect("trim");
    let trimmed = renderer.statistics();
    assert!(trimmed.buffer_slack_bytes < 64 * 1024);

    let object = renderer
        .pick_object(&mut device, &scene, &textures, camera(), &plain, (96, 72))
        .expect("object pick with HDR post")
        .expect("center instance");
    assert_eq!(object.node, node);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn mesh_buffer_pool_reuses_allocations_and_releases_on_aggressive_trim() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 128, 96).expect("renderer");
    let empty_textures = HashMap::new();
    let mut options = options();
    options.post_process = false;
    options.shadows.enabled = false;
    renderer
        .render(
            &mut device,
            &empty_scene(),
            &empty_textures,
            camera(),
            &options,
        )
        .expect("baseline frame");
    let baseline = renderer.statistics();
    let overhead = baseline.target_bytes + baseline.resident_buffer_bytes;
    renderer.set_memory_budget(Some(overhead + 3 * 1024));
    let mut reuses = 0;
    for _ in 0..16 {
        let (scene, _) = cube_scene(None);
        renderer
            .render(&mut device, &scene, &empty_textures, camera(), &options)
            .expect("cycling cube frame");
        reuses = renderer.statistics().buffer_reuses;
    }
    assert!(reuses > 0, "mesh buffers were never served from the pool");
    let pooled = renderer.statistics();
    assert!(pooled.buffer_allocation_count >= 4);
    renderer
        .trim(&mut device, gfx_core::MemoryTrimLevel::Aggressive)
        .expect("aggressive trim");
    let trimmed = renderer.statistics();
    assert_eq!(trimmed.live_meshes, 0);
    assert_eq!(trimmed.pooled_buffer_bytes, 0);
    assert_eq!(trimmed.pooled_buffer_count, 0);
    assert_eq!(trimmed.buffer_allocation_count, 4);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn blended_animation_and_visibility_change_the_submitted_frame() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let (mut scene, node) = cube_scene(None);
    let one = Duration::from_secs(1);
    let left = TransformTrack::new(node).with_translation(
        Vec3Track::new([
            Keyframe::new(Duration::ZERO, Vec3::new(-1.0, 0.0, 0.0)),
            Keyframe::new(one, Vec3::new(-1.0, 0.0, 0.0)),
        ])
        .expect("left track"),
    );
    let right = TransformTrack::new(node).with_translation(
        Vec3Track::new([
            Keyframe::new(Duration::ZERO, Vec3::new(1.0, 0.0, 0.0)),
            Keyframe::new(one, Vec3::new(1.0, 0.0, 0.0)),
        ])
        .expect("right track"),
    );
    let left_tracks = [left];
    let right_tracks = [right];

    let first = renderer
        .render_blended(
            &mut device,
            &scene,
            &left_tracks,
            one,
            &right_tracks,
            one,
            0.0,
            &textures,
            camera(),
            &options(),
        )
        .expect("primary pose frame");
    let first_image = renderer.capture(&mut device).expect("primary capture");
    let second = renderer
        .render_blended(
            &mut device,
            &scene,
            &left_tracks,
            one,
            &right_tracks,
            one,
            1.0,
            &textures,
            camera(),
            &options(),
        )
        .expect("secondary pose frame");
    let second_image = renderer.capture(&mut device).expect("secondary capture");
    let middle = renderer
        .render_blended(
            &mut device,
            &scene,
            &left_tracks,
            one,
            &right_tracks,
            one,
            0.5,
            &textures,
            camera(),
            &options(),
        )
        .expect("blended pose frame");
    let middle_image = renderer.capture(&mut device).expect("blended capture");
    assert_eq!(first.draw_calls, 1);
    assert_eq!(second.draw_calls, 1);
    assert_eq!(middle.draw_calls, 1);
    assert!(differing_pixels(&first_image, &second_image) > 20);
    assert!(differing_pixels(&first_image, &middle_image) > 5);
    assert!(differing_pixels(&second_image, &middle_image) > 5);
    assert!(
        renderer
            .render_blended(
                &mut device,
                &scene,
                &left_tracks,
                one,
                &right_tracks,
                one,
                f32::NAN,
                &textures,
                camera(),
                &options(),
            )
            .is_err()
    );

    scene.set_visible(node, false);
    let hidden = renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("hidden frame");
    let hidden_image = renderer.capture(&mut device).expect("hidden capture");
    assert_eq!(hidden.draw_calls, 0);
    assert_eq!(hidden.instances, 0);
    assert!(differing_pixels(&first_image, &hidden_image) > 100);
    scene.set_visible(node, true);
    let visible = renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("visible frame");
    assert_eq!(visible.draw_calls, 1);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn render_scale_samples_the_scene_without_changing_the_output_size() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let (scene, node) = cube_scene(None);

    renderer
        .set_render_scale(&mut device, 0.5)
        .expect("half scale");
    let low = renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("half-scale frame");
    let low_stats = renderer.statistics();
    assert_eq!((low.width(), low.height()), (160, 120));
    assert_eq!(
        (
            low_stats.render_target_width,
            low_stats.render_target_height
        ),
        (80, 60)
    );

    renderer
        .set_render_scale(&mut device, 1.0)
        .expect("native scale");
    let normal = renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("native-scale frame");
    let normal_stats = renderer.statistics();
    assert_eq!(
        (
            normal_stats.render_target_width,
            normal_stats.render_target_height
        ),
        (160, 120)
    );
    assert!(normal_stats.target_bytes > low_stats.target_bytes);

    renderer
        .set_render_scale(&mut device, 2.0)
        .expect("double scale");
    let high = renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("supersampled frame");
    let high_stats = renderer.statistics();
    assert_eq!(
        (
            high_stats.render_target_width,
            high_stats.render_target_height
        ),
        (320, 240)
    );
    assert!(high_stats.target_bytes > normal_stats.target_bytes);
    assert_eq!((high.width(), high.height()), (160, 120));
    assert!(differing_pixels(&low, &normal) > 0);
    assert!(differing_pixels(&normal, &high) > 0);

    assert!(renderer.set_render_scale(&mut device, 0.0).is_err());
    assert!(renderer.set_render_scale(&mut device, f32::NAN).is_err());
    assert!((renderer.render_scale() - 2.0).abs() < 1e-5);
    let object = renderer
        .pick_object(
            &mut device,
            &scene,
            &textures,
            camera(),
            &options(),
            (80, 60),
        )
        .expect("object pick with supersampling")
        .expect("center instance");
    assert_eq!(object.node, node);
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn environment_map_lights_the_scene_and_draws_the_sky() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let (mut scene, _) = cube_scene(None);
    let plain = renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("plain frame");
    let plain_stats = renderer.statistics();
    assert_eq!(plain_stats.live_textures, 0);

    scene.set_environment(Some(std::sync::Arc::new(
        gpui_3d::EnvironmentMap::procedural_studio(),
    )));
    let lit = renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("environment frame");
    let lit_stats = renderer.statistics();
    assert_eq!(lit_stats.live_textures, 1);
    assert!(lit_stats.cached_texture_bytes > plain_stats.cached_texture_bytes);
    assert!(differing_pixels(&plain, &lit) > 100);

    let reused = renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("reused environment frame");
    assert!(reused.frame_reused);
    scene.set_environment(Some(std::sync::Arc::new(
        gpui_3d::EnvironmentMap::procedural_studio(),
    )));
    let changed = renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("changed environment frame");
    assert!(!changed.frame_reused);

    let mut empty = Scene::new();
    empty.set_environment(Some(std::sync::Arc::new(
        gpui_3d::EnvironmentMap::procedural_studio(),
    )));
    let sky = renderer
        .render_to_image(&mut device, &empty, &textures, camera(), &options())
        .expect("sky frame");
    assert!(differing_pixels(&sky, &lit) > 100);

    renderer
        .trim(&mut device, gfx_core::MemoryTrimLevel::Aggressive)
        .expect("aggressive trim");
    let trimmed = renderer.statistics();
    assert_eq!(trimmed.live_textures, 0);
    renderer
        .render_to_image(&mut device, &scene, &textures, camera(), &options())
        .expect("environment re-upload");
    renderer.destroy(&mut device).expect("destroy renderer");
}

#[test]
#[ignore = "requires native DX12 hardware"]
fn deferred_submission_can_be_polled_before_capture() {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
    let mut renderer = OffscreenRenderer::new(&mut device, 160, 120).expect("renderer");
    let textures = HashMap::new();
    let (scene, _) = cube_scene(None);
    renderer
        .render(&mut device, &scene, &textures, camera(), &options())
        .expect("render");
    let mut saw_progress = false;
    let mut completed = None;
    for _ in 0..200000 {
        match renderer.poll_render(&mut device).expect("poll") {
            Some(true) => {
                completed = Some(true);
                break;
            }
            Some(false) => saw_progress = true,
            None => {
                completed = Some(false);
                break;
            }
        }
        std::thread::yield_now();
    }
    assert!(completed.is_some(), "poll never reported a terminal state");
    if saw_progress {
        // Waiting for a polled frame must not block or fail; this capture exercises the same path.
        let image = renderer.capture(&mut device).expect("capture after poll");
        assert_eq!((image.width(), image.height()), (160, 120));
    } else {
        let image = renderer.capture(&mut device).expect("capture");
        assert_eq!((image.width(), image.height()), (160, 120));
    }
    assert_eq!(renderer.poll_render(&mut device).expect("idle poll"), None);
    renderer.destroy(&mut device).expect("destroy renderer");
}
