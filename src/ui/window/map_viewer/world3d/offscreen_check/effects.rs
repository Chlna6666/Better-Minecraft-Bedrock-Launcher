//! Exercise the production SceneView multipass renderer against a read-only real world.
use super::*;
use gfx_core::*;
use gpui::{RendererExtension, RendererExtensionContext};
use gpui_3d::{SceneView, ScreenEffects, VoxelIllumination, VoxelShading};

mod frames;
use frames::{Attachment, frame, inspect_passes};

fn capture<D: Device + TextureTransferDevice>(
    mut device: D,
    world: SnapshotWorld,
    spawn: [f64; 3],
) {
    let (mut scene, textures) = scene_from_world(&world);
    let light_count = world
        .lights
        .values()
        .map(|lights| lights.len())
        .sum::<usize>();
    for (&(x, z), lights) in &world.lights {
        for light in lights.iter() {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_light(light.light)
                        .with_transform(Transform {
                            translation: Vec3::new((x * 16) as f32, 0., (z * 16) as f32),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
    }
    let camera = snapshot_camera_for(&world, spawn);
    let size = Extent2d::new(1280, 800).unwrap();
    let color = Attachment::new(&mut device, size, Format::Rgba8Unorm);
    let depth = Attachment::new(&mut device, size, Format::Depth32Float);
    let pass = PipelineDevice::create_render_pass(
        &mut device,
        &RenderPassDescriptor {
            label: None,
            color_attachment: ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            },
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
            }),
        },
    )
    .unwrap();
    let fixed =
        RendererExtensionContext::for_target(D::BACKEND_KIND, pass, Format::Rgba8Unorm, size);
    let context = || fixed.clone();
    let vibrant = std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref() == Ok("vibrant");
    let settings = ScreenEffects {
        cloud_texture: world.cloud.as_ref().map(|cloud| cloud.id()),
        water_texture: world.water[0].as_ref().map(|v| gpui_3d::FlipbookTexture {
            texture: v.asset.id(),
            seconds_per_frame: v.seconds_per_frame,
        }),
        water_flow_texture: world.water[1].as_ref().map(|v| gpui_3d::FlipbookTexture {
            texture: v.asset.id(),
            seconds_per_frame: v.seconds_per_frame,
        }),
        ..ScreenEffects::default()
    };
    let view = SceneView::new(Arc::new(scene), camera)
        .with_textures(textures.into_values())
        .unwrap()
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: if vibrant {
                VoxelIllumination::Sunlight
            } else {
                VoxelIllumination::Unlit
            },
            sun_direction: Vec3::new(0.4, 0.8, 0.3).normalized().unwrap(),
            sun_intensity: 1.,
            shadow_strength: if vibrant { 0.9 } else { 0. },
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(vibrant.then_some(settings))
        .unwrap();
    let mut renderer = view.create_renderer(&mut device, context()).unwrap();
    frame(
        &mut device,
        renderer.as_mut(),
        &view,
        context(),
        &color,
        &depth,
    );
    let image = TextureTransferDevice::read_texture(&mut device, color.texture).unwrap();
    assert!(image.bytes.chunks_exact(4).all(|pixel| pixel[3] == 255));
    let output = env_path("BMCBL_WORLD3D_TEST_SNAPSHOT")
        .unwrap_or_else(|| PathBuf::from("target/world3d-effects.png"));
    image::RgbaImage::from_raw(size.width(), size.height(), image.bytes)
        .unwrap()
        .save(&output)
        .unwrap();
    eprintln!(
        "world3d effects adapter={} size={}x{} scale={} face_capacity={} source_count={} occluder_proxies={}",
        device.adapter_name(),
        size.width(),
        size.height(),
        if vibrant {
            settings.resolution_scale
        } else {
            1.
        },
        world.meshes.values().map(|mesh| mesh.faces).sum::<usize>(),
        light_count,
        world
            .meshes
            .values()
            .map(|mesh| mesh.occluders.len())
            .sum::<usize>()
    );
    let profiles = if vibrant {
        vec![
            ("full", Some(settings)),
            (
                "without_atmosphere",
                Some(ScreenEffects {
                    clouds: false,
                    volumetric_fog: false,
                    ..settings
                }),
            ),
        ]
    } else {
        vec![("night_vision", None)]
    };
    for (profile, effects) in profiles {
        let view = view.with_screen_effects(effects).unwrap();
        frame(
            &mut device,
            renderer.as_mut(),
            &view,
            context(),
            &color,
            &depth,
        );
        let pixels = TextureTransferDevice::read_texture(&mut device, color.texture).unwrap();
        if profile == "without_atmosphere" {
            image::RgbaImage::from_raw(size.width(), size.height(), pixels.bytes)
                .unwrap()
                .save(output.with_file_name("world3d-effects-without-atmosphere.png"))
                .unwrap();
        }
        for moving in [false, true] {
            let mut cpu = Vec::new();
            let mut submit = Vec::new();
            let mut records = Vec::new();
            let mut waits = Vec::new();
            for index in 0..40 {
                let offset = if moving {
                    Vec3::new(
                        (index as f32 * 0.17).sin() * 4.,
                        0.,
                        (index as f32 * 0.17).cos() * 4.,
                    )
                } else {
                    Vec3::ZERO
                };
                let sample = view.with_camera(Camera {
                    eye: camera.eye + offset,
                    ..camera
                });
                let (encode, recorded, waited) = frame(
                    &mut device,
                    renderer.as_mut(),
                    &sample,
                    context(),
                    &color,
                    &depth,
                );
                cpu.push(encode);
                submit.push(recorded + waited);
                records.push(recorded);
                waits.push(waited);
            }
            cpu.sort_by(f64::total_cmp);
            submit.sort_by(f64::total_cmp);
            records.sort_by(f64::total_cmp);
            waits.sort_by(f64::total_cmp);
            eprintln!(
                "world3d effects {:?}: profile={profile} moving={moving} cpu_encode_p50_ms={:.3} cpu_encode_p95_ms={:.3} submit_wait_p50_ms={:.3} submit_wait_p95_ms={:.3} sections={} face_capacity={} lights={} clouds={} eye={:?} target={:?} -> {}",
                D::BACKEND_KIND,
                cpu[19],
                cpu[37],
                submit[19],
                submit[37],
                world.meshes.len(),
                world.meshes.values().map(|mesh| mesh.faces).sum::<usize>(),
                light_count,
                world.cloud.is_some(),
                camera.eye,
                camera.target,
                output.display()
            );
            eprintln!(
                "world3d native profile={profile} moving={moving} record_p50_ms={:.3} record_p95_ms={:.3} fence_wait_p50_ms={:.3} fence_wait_p95_ms={:.3}",
                records[19], records[37], waits[19], waits[37]
            );
        }
    }
    inspect_passes(
        &mut device,
        renderer.as_mut(),
        &view.with_camera(Camera {
            eye: camera.eye + Vec3::new(17., 0., 17.),
            ..camera
        }),
        context(),
        &color,
        &depth,
    );
    renderer.destroy(&mut device).unwrap();
    color.destroy(&mut device);
    depth.destroy(&mut device);
    PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD, BMCBL_WORLD3D_TEST_LIGHTING=vibrant, resource pack and GPU"]
fn render_real_world_effects() {
    assert_eq!(
        std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref(),
        Ok("vibrant")
    );
    render_world();
}

#[test]
#[ignore = "requires a read-only Bedrock world, resource pack and native GPU"]
fn render_real_world_default() {
    assert!(std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref() != Ok("vibrant"));
    render_world();
}

fn render_world() {
    let world = env_path("BMCBL_WORLD3D_TEST_WORLD").expect("read-only world root");
    let roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE").into_iter().collect();
    let spawn = read_world_spawn(&world);
    let radius = env_i32("BMCBL_WORLD3D_TEST_RADIUS")
        .unwrap_or(3)
        .clamp(1, 16);
    let (world, mut service) = collect_world(
        &world,
        roots,
        spawn,
        radius,
        ((2 * radius + 1).pow(2) / 2).max(9) as usize,
    );
    service.shutdown();
    assert!(!world.meshes.is_empty());
    let descriptor = DeviceDescriptor {
        power_preference: PowerPreference::HighPerformance,
        ..DeviceDescriptor::default()
    };
    match std::env::var("BMCBL_WORLD3D_TEST_BACKEND").as_deref() {
        Ok("dx11") => capture(
            gfx_dx11::Dx11Device::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
        Ok("vulkan") => capture(
            gfx_vulkan::VulkanDevice::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
        _ => capture(
            gfx_dx12::Dx12Device::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
    }
}
