//! Exercise the production SceneView multipass renderer against a read-only real world.
use super::*;
use gfx_core::*;
use gpui::{
    RendererExtension, RendererExtensionContext, RendererExtensionPass, RendererExtensionRenderer,
};
use gpui_3d::{SceneView, ScreenEffects, VoxelIllumination, VoxelShading};

struct Attachment {
    texture: TextureId,
    view: TextureViewId,
}
impl Attachment {
    fn new<D: Device>(device: &mut D, size: Extent2d, format: Format) -> Self {
        let usage = if format == Format::Depth32Float {
            TextureUsage::DEPTH_ATTACHMENT
        } else {
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC
        };
        let texture = ResourceDevice::create_texture(
            device,
            &TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                format,
                usage,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
            },
        )
        .unwrap();
        let view = ResourceDevice::create_texture_view(
            device,
            &TextureViewDescriptor {
                label: None,
                texture,
                base_mip_level: 0,
                mip_level_count: 1,
                format,
            },
        )
        .unwrap();
        Self { texture, view }
    }
    fn destroy<D: Device>(self, device: &mut D) {
        ResourceDevice::destroy_texture_view(device, self.view).unwrap();
        ResourceDevice::destroy_texture(device, self.texture).unwrap();
    }
}

fn frame<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    view: &SceneView,
    context: RendererExtensionContext,
    color: &Attachment,
    depth: &Attachment,
) -> (f64, f64) {
    let start = Instant::now();
    let mut commands = Vec::new();
    renderer
        .render(view, device, context.clone(), &mut commands)
        .unwrap();
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    let encode = start.elapsed().as_secs_f64() * 1000.;
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        texture_view: color.view,
        render_pass: context.render_pass(),
        commands: &commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
        }),
        clear_region: None,
        viewport: None,
    });
    let submitted = Instant::now();
    if let Some(submission) = PresentationDevice::render_to_textures(device, &passes).unwrap() {
        SubmissionDevice::wait_submission(device, submission).unwrap();
    }
    renderer.render_passes_submitted();
    // Vulkan's deferred legacy return has no token; wait for its readback only outside timings.
    (encode, submitted.elapsed().as_secs_f64() * 1000.)
}

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
    let context =
        || RendererExtensionContext::for_target(D::BACKEND_KIND, pass, Format::Rgba8Unorm, size);
    let settings = ScreenEffects {
        cloud_texture: world.cloud.as_ref().map(|cloud| cloud.id()),
        ..ScreenEffects::default()
    };
    let view = SceneView::new(Arc::new(scene), camera)
        .with_textures(textures.into_values())
        .unwrap()
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sun_direction: Vec3::new(0.4, 0.8, 0.3).normalized().unwrap(),
            sun_intensity: 1.,
            shadow_strength: 0.9,
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(Some(settings))
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
        "world3d effects adapter={} size={}x{} scale={} face_count={} source_count={}",
        device.adapter_name(),
        size.width(),
        size.height(),
        settings.resolution_scale,
        world.meshes.values().map(|mesh| mesh.faces).sum::<usize>(),
        light_count
    );
    for (profile, effects) in [
        ("full", settings),
        (
            "without_atmosphere",
            ScreenEffects {
                clouds: false,
                volumetric_fog: false,
                ..settings
            },
        ),
    ] {
        let view = view.with_screen_effects(Some(effects)).unwrap();
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
                let (encode, submitted) = frame(
                    &mut device,
                    renderer.as_mut(),
                    &sample,
                    context(),
                    &color,
                    &depth,
                );
                // Force completion separately for APIs whose legacy multipass return is untracked.
                if D::BACKEND_KIND == BackendKind::Vulkan {
                    let _ =
                        TextureTransferDevice::read_texture(&mut device, color.texture).unwrap();
                }
                cpu.push(encode);
                submit.push(submitted);
            }
            cpu.sort_by(f64::total_cmp);
            submit.sort_by(f64::total_cmp);
            eprintln!(
                "world3d effects {:?}: profile={profile} moving={moving} cpu_encode_p50_ms={:.3} cpu_encode_p95_ms={:.3} submit_wait_p50_ms={:.3} submit_wait_p95_ms={:.3} sections={} faces={} lights={} clouds={} eye={:?} target={:?} -> {}",
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
        }
    }
    renderer.destroy(&mut device).unwrap();
    color.destroy(&mut device);
    depth.destroy(&mut device);
    PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD, BMCBL_WORLD3D_TEST_LIGHTING=vibrant, resource pack and GPU"]
fn render_real_world_effects() {
    let world = env_path("BMCBL_WORLD3D_TEST_WORLD").expect("read-only world root");
    assert_eq!(
        std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref(),
        Ok("vibrant")
    );
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
