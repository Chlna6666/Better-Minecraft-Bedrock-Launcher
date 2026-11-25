mod indirect;
mod light_tiles;
mod materials;
mod reflections;
mod shadow_planes;
mod water_waves;
use super::super::targets::Target;
use super::*;
use crate::{AlphaMode, Light, Material, Mesh, Node, VoxelIllumination, VoxelShading};
use crate::{Camera, Scene, Vec3};
use materials::{verify_shadow_retry, verify_shadows, verify_sky_transmission};

#[test]
fn capture_reuse_respects_visibility_and_fog() {
    let mut scene = Scene::new();
    let node = scene.insert(None, Node::new()).unwrap();
    let camera = Camera::perspective(Vec3::Z, Vec3::ZERO, Vec3::Y, 1., 0.1, 10.);
    let view = SceneView::new(Arc::new(scene), camera);
    let hidden = view.with_hidden_nodes(Arc::new([node].into_iter().collect()));
    let same_hidden = view.with_hidden_nodes(Arc::new([node].into_iter().collect()));
    assert!(!same_capture(&view, &hidden));
    assert!(same_capture(&hidden, &same_hidden));
    let fogged = view.with_fog(crate::DistanceFog::new([1.; 3], 1., 5.));
    assert!(!same_capture(&view, &fogged));
    let uncull = view.with_culling_disabled();
    assert!(!same_capture(&view, &uncull));
    assert!(same_capture(&uncull, &uncull.clone()));
}

fn test_render_pass<D: Device>(device: &mut D) -> RenderPassId {
    PipelineDevice::create_render_pass(
        device,
        &RenderPassDescriptor {
            label: None,
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,

                stencil_format: None,
            }),

            sample_count: 1,
            resolve_target: None,
        },
    )
    .unwrap()
}

fn verify<D: Device + TextureTransferDevice>(mut device: D) {
    let extent = Extent2d::new(64, 64).unwrap();
    let target = Target::create(&mut device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(&mut device, extent, Format::Depth32Float, true, 1).unwrap();
    let pass_id = test_render_pass(&mut device);
    let context =
        RendererExtensionContext::for_target(D::BACKEND_KIND, pass_id, Format::Rgba8Unorm, extent);
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(Material::new())]),
        )
        .unwrap();
    let light_id = scene
        .insert(
            None,
            Node::new().with_light(Light::Point {
                position: Vec3::new(0., 0., 2.),
                color: [1., 0., 0.],
                intensity: 16.,
                range: 8.,
            }),
        )
        .unwrap();
    let camera = Camera::perspective(Vec3::new(0., 0., 3.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
    let settings = ScreenEffects {
        resolution_scale: 1.,
        reflections: false,
        water: false,
        volumetric_fog: false,
        clouds: false,
        ..ScreenEffects::default()
    };
    let view = SceneView::new(Arc::new(scene), camera)
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sun_intensity: 0.,
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(Some(settings))
        .unwrap();
    let mut renderer = EffectsRenderer::new(&mut device, context.clone(), settings).unwrap();
    let mut steps = Vec::new();
    renderer
        .render(&view, &mut device, context.clone(), &mut steps)
        .unwrap();
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    assert!(owned.len() >= 3);
    assert!(owned.iter().any(|pass| pass.color_attachments.len() == 2));
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        color_attachments: std::slice::from_ref(&target.view),
        render_pass: pass_id,
        commands: &steps,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        }),
        clear_region: None,
        viewport: None,
        resolve_target: None,
    });
    PresentationDevice::render_to_textures(&mut device, &passes).unwrap();
    renderer.render_passes_submitted();
    verify_attribute_mrt_matches_split(&mut device, &renderer, &context, &view);
    let pixels = TextureTransferDevice::read_texture(&mut device, target.texture).unwrap();
    let center = &pixels.bytes[(32 * 64 + 32) * 4..][..4];
    assert!(
        center[0] > center[1] + 40,
        "point light must contribute red PBR illumination: {center:?}"
    );
    assert_eq!(center[3], 255);
    drop(passes);
    steps.clear();
    renderer
        .render(&view, &mut device, context.clone(), &mut steps)
        .unwrap();
    owned.clear();
    renderer.drain_render_passes(&mut owned);
    assert_eq!(
        owned.len(),
        1,
        "a static capture must submit only the screen resolve"
    );
    for diffuse_only in [false, true] {
        let mut scene = view.scene.fork();
        scene.node_mut(light_id).unwrap().light = Some(Light::Spherical {
            position: Vec3::new(0.5, 0., 1.5),
            color: [1., 0., 0.],
            intensity: 16.,
            range: 8.,
            radius: 0.27,
            diffuse_only,
        });
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(Material::new())])
                    .with_transform(crate::Transform {
                        translation: Vec3::new(0.5, 0., 1.5),
                        scale: Vec3::new(0.3, 0.3, 0.3),
                        ..crate::Transform::IDENTITY
                    }),
            )
            .unwrap();
        let pixels = draw(
            &mut device,
            &mut renderer,
            &context,
            &view.with_scene(Arc::new(scene)),
            &target,
            &depth,
        );
        let color = &pixels[(32 * 64 + 32) * 4..][..4];
        assert!(
            color[0] > color[1] + 40,
            "finite emitters must illuminate their neighbours without self-occluding: {color:?}, diffuse={diffuse_only}"
        );
    }
    let mut scaled = settings;
    scaled.resolution_scale = 0.5;
    let scaled_view = view.with_screen_effects(Some(scaled)).unwrap();
    steps.clear();
    renderer
        .render(&scaled_view, &mut device, context.clone(), &mut steps)
        .unwrap();
    owned.clear();
    renderer.drain_render_passes(&mut owned);
    assert!(
        owned.len() >= 3,
        "resizing must invalidate every geometry attachment"
    );
    assert_eq!(renderer.scenes[&view.id].retired.len(), 1);
    assert_eq!(renderer.scenes[&view.id].submitted_retirements, 0);
    let passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    PresentationDevice::render_to_textures(&mut device, &passes).unwrap();
    renderer.render_passes_submitted();
    assert_eq!(renderer.scenes[&view.id].submitted_retirements, 1);
    let _ = TextureTransferDevice::read_texture(&mut device, target.texture).unwrap();
    draw(
        &mut device,
        &mut renderer,
        &context,
        &scaled_view,
        &target,
        &depth,
    );
    assert!(
        renderer.scenes[&view.id].retired.is_empty(),
        "submitted attachments retire without an idle notification"
    );
    // Fixed camera, same draw count, replacement of the last light. A
    // Scene publication must update the frame light count even without
    // camera or shading changes.
    let mut without_light = view.scene.fork();
    without_light.remove(light_id).unwrap();
    let no_light = draw(&mut device, &mut renderer, &context,
        &view.with_scene(Arc::new(without_light)), &target, &depth);
    let dark_center = &no_light[(32 * 64 + 32) * 4..][..4];
    assert!(dark_center[0].abs_diff(dark_center[1]) < 10,
        "removed red emitter must not leak through retained frame: {dark_center:?}");
    shadow_planes::verify_receiver_planes(&mut device, &mut renderer, &context, &target, &depth);
    shadow_planes::verify_shadow_coverage(&mut device, &mut renderer, &context);
    verify_shadows(&mut device, &mut renderer, &context, &target, &depth);
    // The intervening fixtures use their own SceneView IDs. The renderer
    // legitimately retires inactive scenes, so establish a live capture of
    // this exact view before exercising its failed-update retry path.
    draw(&mut device, &mut renderer, &context, &view, &target, &depth);
    verify_shadow_retry(&mut device, &mut renderer, &context, &view);
    verify_sky_transmission(&mut device, &mut renderer, &context, &view, &target, &depth);
    reflections::verify_reflection_controls(
        &mut device,
        &mut renderer,
        &context,
        &view,
        &target,
        &depth,
    );
    reflections::verify_material_mips(&mut device, &mut renderer, &context, &view, &target, &depth);
    materials::verify_hidden_lines(&mut device, &context, &view, &target, &depth);
    materials::verify_water_animation(&mut device, &mut renderer, &context, &view, &target, &depth);
    water_waves::verify_water_waves(&mut device, &mut renderer, &context, &target, &depth);
    indirect::verify_indirect_updates(&mut device, &mut renderer, &context, &target, &depth);
    materials::verify_material_reflections(
        &mut device,
        &mut renderer,
        &context,
        &view,
        &target,
        &depth,
    );
    renderer.destroy(&mut device).unwrap();
    target.destroy(&mut device).unwrap();
    depth.destroy(&mut device).unwrap();
    PipelineDevice::destroy_render_pass(&mut device, pass_id).unwrap();
}

fn verify_partial_light_tiles<D: Device + TextureTransferDevice>(device: &mut D) {
    let extent = Extent2d::new(65, 65).unwrap();
    let target = Target::create(device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(device, extent, Format::Depth32Float, true, 1).unwrap();
    let pass = test_render_pass(device);
    let context =
        RendererExtensionContext::for_target(D::BACKEND_KIND, pass, Format::Rgba8Unorm, extent);
    let settings = ScreenEffects {
        resolution_scale: 1.,
        reflections: false,
        water: false,
        volumetric_fog: false,
        ..ScreenEffects::default()
    };
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::plane(65., 65.).unwrap()))
                .with_materials([Arc::new(Material::new())]),
        )
        .unwrap();
    let light = scene
        .insert(
            None,
            Node::new().with_light(Light::Point {
                position: Vec3::new(-18., 0.25, 0.),
                color: [1., 0., 0.],
                intensity: 16.,
                range: 0.75,
            }),
        )
        .unwrap();
    let camera = Camera::orthographic(Vec3::Y * 5., Vec3::ZERO, -Vec3::Z, 65., 0.1, 100.);
    let view = SceneView::new(Arc::new(scene), camera)
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sky_brightness: 0.,
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(Some(settings))
        .unwrap();
    let view = light_tiles::animate_light(view, light);
    let mut renderer = EffectsRenderer::new(device, context.clone(), settings).unwrap();
    assert!(renderer.msaa_sample_count > 1, "test requires MSAA targets");
    let pixels = draw(device, &mut renderer, &context, &view, &target, &depth);
    // The light projects to x=14.5 with a 0.75 pixel radius. Normalized
    // binning by ceil(65/16) puts it in tile 1, but pixel 14 reads tile 0.
    let lit = &pixels[(32 * 65 + 14) * 4..][..4];
    assert!(
        i16::from(lit[0]) - i16::from(lit[1]) > 40,
        "local light missing in pixel 14: {lit:?}"
    );
    light_tiles::verify_animated_light(device, &mut renderer, &context, &view, &target, &depth);
    renderer.destroy(device).unwrap();
    target.destroy(device).unwrap();
    depth.destroy(device).unwrap();
    PipelineDevice::destroy_render_pass(device, pass).unwrap();
}

/// Per-backend render equivalence against the original two independent
/// attribute passes. Both sides use the same native scene commands, single-
/// sample format and depth state; only the fragment MRT output differs.
fn verify_attribute_mrt_matches_split<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &EffectsRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
) {
    let extent = renderer.scenes[&view.id].targets.extent;
    let depth = Target::create(device, extent, Format::Depth32Float, true, 1).unwrap();
    let targets = (0..4)
        .map(|_| Target::create(device, extent, Format::Rgba16Float, false, 1).unwrap())
        .collect::<Vec<_>>();
    let single_pass = PipelineDevice::create_render_pass(
        device,
        &RenderPassDescriptor {
            label: Some("MRT reference surface/albedo".into()),
            color_attachments: vec![ColorAttachmentDescriptor { format: Format::Rgba16Float }],
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
                stencil_format: None,
            }),
            sample_count: 1,
            resolve_target: None,
        },
    ).unwrap();
    let mrt_pass = PipelineDevice::create_render_pass(
        device,
        &RenderPassDescriptor {
            label: Some("MRT comparison surface+albedo".into()),
            color_attachments: vec![ColorAttachmentDescriptor { format: Format::Rgba16Float }; 2],
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
                stencil_format: None,
            }),
            sample_count: 1,
            resolve_target: None,
        },
    ).unwrap();
    let local = context.offscreen(single_pass, Format::Rgba16Float, extent);
    let mrt_local = context.offscreen(mrt_pass, Format::Rgba16Float, extent);
    let mut reference_surface = RendererResources::new_pass(
        device, &local, Some(&renderer.hdr), PassKind::Surface,
    ).unwrap();
    let mut reference_albedo = RendererResources::new_pass(
        device, &local, Some(&renderer.hdr), PassKind::Albedo,
    ).unwrap();
    let mut combined = RendererResources::new_pass(
        device, &mrt_local, Some(&renderer.hdr), PassKind::AttributesMrt,
    ).unwrap();
    // Never mutate the production SceneResources' persistent binding cache
    // when building temporary MRT-reference renderers. The old test used the
    // production object and invalidated its bind groups before the next draw,
    // causing native Vulkan's "stale or invalid bind group handle" failure.
    let mut scene = super::super::super::scene::SceneResources::new(
        device, D::BACKEND_KIND,
    ).unwrap();
    let mut shading = view.voxel_shading;
    shading.shadow_strength = 0.0;
    let reference_view = view.with_voxel_shader().with_voxel_shading(shading).unwrap();
    scene.prepare(device, &mut reference_surface, &reference_view, &local).unwrap();
    for (renderer, target) in [
        (&reference_surface, &targets[0]),
        (&reference_albedo, &targets[1]),
    ] {
        let mut steps = Vec::new();
        scene.append_steps(renderer, true, &mut steps).unwrap();
        let capture = super::pass(
            single_pass, target.view, depth.view, None, steps, true,
        );
        PresentationDevice::render_to_textures(device, &[capture.commands()]).unwrap();
    }
    let mut commands = Vec::new();
    scene.append_steps(&combined, true, &mut commands).unwrap();
    let mut capture = super::pass(
        mrt_pass, targets[2].view, depth.view, None, commands, true,
    );
    capture.color_attachments = Arc::from([targets[2].view, targets[3].view]);
    assert_eq!(capture.commands().color_attachments.len(), 2);
    PresentationDevice::render_to_textures(device, &[capture.commands()]).unwrap();
    // A complete byte comparison detects incorrect backend attachment mapping,
    // channel swizzles, missing second target, and Shader output divergence.
    for index in 0..2 {
        let reference = TextureTransferDevice::read_texture(device, targets[index].texture)
            .unwrap().bytes;
        let actual = TextureTransferDevice::read_texture(device, targets[index+2].texture)
            .unwrap().bytes;
        assert_eq!(
            reference, actual,
            "MRT attribute {} differs from split pass on {:?}",
            index, D::BACKEND_KIND
        );
        assert!(
            actual.iter().any(|&byte| byte != 0),
            "MRT attribute {index} was never populated"
        );
    }
    scene.destroy(device).unwrap();
    combined.destroy(device).unwrap();
    reference_albedo.destroy(device).unwrap();
    reference_surface.destroy(device).unwrap();
    PipelineDevice::destroy_render_pass(device, mrt_pass).unwrap();
    PipelineDevice::destroy_render_pass(device, single_pass).unwrap();
    for target in targets {
        target.destroy(device).unwrap();
    }
    depth.destroy(device).unwrap();
}

fn draw<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
    target: &Target,
    depth: &Target,
) -> Vec<u8> {
    if let Some(submission) = queue_draw(device, renderer, context, view, target, depth) {
        SubmissionDevice::wait_submission(device, submission).unwrap();
        renderer.gpu_submission_completed(submission);
    }
    TextureTransferDevice::read_texture(device, target.texture)
        .unwrap()
        .bytes
}

fn queue_draw<D: Device>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
    target: &Target,
    depth: &Target,
) -> Option<SubmissionId> {
    let mut steps = Vec::new();
    renderer
        .render(view, device, context.clone(), &mut steps)
        .unwrap();
    submit_draw(device, renderer, context, target, depth, &steps)
}

fn submit_draw<D: Device>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    context: &RendererExtensionContext,
    target: &Target,
    depth: &Target,
    steps: &[RenderCommand],
) -> Option<SubmissionId> {
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        color_attachments: std::slice::from_ref(&target.view),
        render_pass: context.render_pass(),
        commands: steps,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        }),
        clear_region: None,
        viewport: None,
        resolve_target: None,
    });
    let submission = PresentationDevice::render_to_textures(device, &passes).unwrap();
    renderer.render_passes_submitted();
    if let Some(submission) = submission {
        renderer.gpu_submission_accepted(submission);
    }
    submission
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires D3D11; production screen pass color/depth and colored local lighting"]
fn effects_dx11() {
    verify(gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap());
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires D3D11 and native MSAA effect targets"]
fn partial_light_tiles_dx11() {
    verify_partial_light_tiles(
        &mut gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap(),
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires D3D12 and native MSAA effect targets"]
fn partial_light_tiles_dx12() {
    verify_partial_light_tiles(
        &mut gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap(),
    );
}
#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires D3D12; production screen pass color/depth and colored local lighting"]
fn effects_dx12() {
    verify(gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires Vulkan descriptor indexing and GPUI_3D_BINDLESS=1"]
fn bindless_production_color_vulkan() {
    use crate::scene_view::scene::SceneResources;
    let mut device = gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap();
    let extent = Extent2d::new(64, 64).unwrap();
    let target = Target::create(&mut device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(&mut device, extent, Format::Depth32Float, true, 1).unwrap();
    let pass_id = test_render_pass(&mut device);
    let context = RendererExtensionContext::for_target(
        gfx_core::BackendKind::Vulkan, pass_id, Format::Rgba8Unorm, extent,
    );
    let mut legacy = RendererResources::new_legacy_for_test(&mut device, &context).unwrap();
    let mut sampled = RendererResources::new_sampled_for_test(&mut device, &context, 64).unwrap();
    assert_eq!(sampled.sampled_table_capacity(), Some(64), "production sampled PSO must be available");
    assert!(legacy.sampled_table_layout().is_none());
    let camera = Camera::perspective(Vec3::new(0., 0., 3.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
    let mesh = Arc::new(Mesh::cube());
    let fixture = |colors: &[[u8; 4]], voxel: bool| {
        let mut scene = Scene::new();
        let mut assets = Vec::new();
        for (i, color) in colors.iter().enumerate() {
            let texture = Arc::new(crate::TextureAsset::rgba8(1, 1, color.to_vec()).unwrap());
            let mut material = Material::new();
            material.shading_model = crate::ShadingModel::Unlit;
            material.albedo_texture = Some(texture.id());
            scene.insert(None, Node::new().with_mesh(mesh.clone())
                .with_materials([Arc::new(material)])
                .with_transform(crate::Transform {
                    translation: Vec3::new(if i % 2 == 0 { -0.6 } else { 0.6 }, 0., 0.),
                    ..crate::Transform::IDENTITY
                })).unwrap();
            assets.push(texture);
        }
        let view = SceneView::new(Arc::new(scene), camera).with_textures(assets).unwrap();
        if voxel {
            view.with_voxel_shader().with_voxel_shading(VoxelShading {
                illumination: VoxelIllumination::Unlit, shadow_strength: 0.,
                ..VoxelShading::default()
            }).unwrap()
        } else { view }
    };
    let render = |device: &mut gfx_vulkan::VulkanDevice, scene: &mut SceneResources,
                  renderer: &mut RendererResources, view: &SceneView, set_count: Option<usize>, phase: &str| {
        scene.prepare(device, renderer, view, &context).unwrap();
        let mut commands = Vec::new();
        scene.append_steps(renderer, view.voxel_shader, &mut commands).unwrap();
        assert!(!commands.is_empty());
        let actual_sets = commands.iter().map(|command| command.bind_groups().len()).collect::<Vec<_>>();
        assert!(actual_sets.iter().all(|&count| count == actual_sets[0]),
            "{phase}: mixed descriptor layouts in one frame: {actual_sets:?}");
        if let Some(set_count) = set_count {
            assert_eq!(actual_sets[0], set_count,
                "{phase}: production descriptor sets, voxel={}", view.voxel_shader);
        }
        if actual_sets[0] == 2 {
            let geometry = commands[0].bind_groups()[0];
            assert!(commands.iter().all(|command| command.bind_groups()[0] == geometry),
                "identical resident geometry must share set zero across materials");
        }
        PresentationDevice::render_to_textures(device, &[RenderPassCommands {
            color_attachments: &[target.view], render_pass: pass_id, commands: &commands,
            color_load_op: LoadOp::Clear(ClearColor::default()),
            depth_attachment: Some(DepthAttachment {
                target: depth.view, depth_load_op: LoadOp::Clear(1.), stencil_load_op: None,
            }),
            clear_region: None, viewport: None, resolve_target: None,
        }]).unwrap();
        let explicit_pixels = TextureTransferDevice::read_texture(device, target.texture).unwrap().bytes;
        if actual_sets[0] == 2 {
            let snapshot = scene.sampled_snapshot(renderer).expect("sampled table");
            let mut shared_commands = Vec::new();
            scene.append_offscreen_steps(renderer, view.voxel_shader, &mut shared_commands).unwrap();
            assert_eq!(shared_commands.len(), commands.len());
            for (local, explicit) in shared_commands.iter().zip(&commands) {
                assert_eq!(local.bind_groups(), &explicit.bind_groups()[..1]);
                assert_eq!(local.pipeline(), explicit.pipeline());
            }
            let pass = RenderPassCommands {
                color_attachments: &[target.view], render_pass: pass_id, commands: &shared_commands,
                color_load_op: LoadOp::Clear(ClearColor::default()),
                depth_attachment: Some(DepthAttachment {
                    target: depth.view, depth_load_op: LoadOp::Clear(1.), stencil_load_op: None,
                }),
                clear_region: None, viewport: None, resolve_target: None,
            };
            // A shared table cannot silently override a draw-local second set.
            let invalid = RenderPassCommands { commands: &commands, ..pass };
            assert!(PresentationDevice::render_to_textures_sampled_profiled(
                device, &[invalid], &[Some(snapshot)], &[], &[None],
            ).is_err());
            assert!(PresentationDevice::render_to_textures_sampled_profiled(
                device, &[pass], &[Some(shared_commands[0].bind_groups()[0])], &[], &[None],
            ).is_err());
            let submitted = PresentationDevice::render_to_textures_sampled_profiled(
                device, &[pass], &[Some(snapshot)], &[], &[None],
            ).unwrap();
            if let Some(id) = submitted {
                SubmissionDevice::wait_submission(device, id).unwrap();
            }
            let shared_pixels = TextureTransferDevice::read_texture(device, target.texture).unwrap().bytes;
            assert_eq!(shared_pixels, explicit_pixels, "pass-level sampled binding differs from per-draw binding");
        }
        explicit_pixels
    };
    for voxel in [false, true] {
        let mut legacy_scene = SceneResources::new(&mut device, gfx_core::BackendKind::Vulkan).unwrap();
        let mut sampled_scene = SceneResources::new(&mut device, gfx_core::BackendKind::Vulkan).unwrap();
        for colors in [
            [[255, 0, 0, 255], [0, 0, 255, 255]],
            [[0, 255, 0, 255], [255, 0, 255, 255]],
        ] {
            let view = fixture(&colors, voxel);
            let expected = render(&mut device, &mut legacy_scene, &mut legacy, &view, Some(1), "legacy");
            let actual = render(&mut device, &mut sampled_scene, &mut sampled, &view, Some(2), "sampled pixels");
            assert_eq!(actual, expected, "production bindless color mismatch, voxel={voxel}");
            assert!(actual.chunks_exact(4).any(|pixel| pixel[0] > 200 || pixel[1] > 200));
            assert!(actual.chunks_exact(4).any(|pixel| pixel[2] > 200));
        }
        // The fallback entry occupies slot zero. 64 different live images
        // cannot all fit; no out-of-range or fallback-colored draw is allowed.
        let overflow = (0..64u8).map(|i| [i, 255, 255 - i, 255]).collect::<Vec<_>>();
        let view = fixture(&overflow, voxel);
        render(&mut device, &mut sampled_scene, &mut sampled, &view, Some(1), "overflow");
        // Shrinking the working set retires old slots. A subsequent unchanged
        // frame must be able to recover and use the sampled PSO again.
        let recovered = fixture(&[[255, 0, 0, 255], [0, 0, 255, 255]], voxel);
        render(&mut device, &mut sampled_scene, &mut sampled, &recovered, None, "shrink working set");
        // Removed descriptors are processed before the next batch plan.
        let actual = render(&mut device, &mut sampled_scene, &mut sampled, &recovered, Some(2), "recovered");
        let expected = render(&mut device, &mut legacy_scene, &mut legacy, &recovered, Some(1), "recovered legacy reference");
        assert_eq!(actual, expected);
        sampled_scene.destroy(&mut device).unwrap();
        legacy_scene.destroy(&mut device).unwrap();
    }
    sampled.destroy(&mut device).unwrap();
    legacy.destroy(&mut device).unwrap();
    target.destroy(&mut device).unwrap();
    depth.destroy(&mut device).unwrap();
    PipelineDevice::destroy_render_pass(&mut device, pass_id).unwrap();
    eprintln!("GPUI3D_VULKAN_PRODUCTION_BINDLESS_COLOR=pass");
    eprintln!("GPUI3D_VULKAN_PASS_SAMPLED_TABLE=pass");
}

#[test]
#[ignore = "requires Vulkan descriptor indexing and GPUI_3D_BINDLESS=1"]
fn bindless_production_effects_vulkan() {
    let mut device = gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap();
    let extent = Extent2d::new(64, 64).unwrap();
    let target = Target::create(&mut device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(&mut device, extent, Format::Depth32Float, true, 1).unwrap();
    let pass_id = test_render_pass(&mut device);
    let context = RendererExtensionContext::for_target(
        gfx_core::BackendKind::Vulkan, pass_id, Format::Rgba8Unorm, extent,
    );
    let settings = ScreenEffects {
        resolution_scale: 1., reflections: false, water: false,
        volumetric_fog: false, clouds: false, ..ScreenEffects::default()
    };
    let mut legacy = EffectsRenderer::new_with_sampled_limit(&mut device, context.clone(), settings, 0).unwrap();
    let mut sampled = EffectsRenderer::new_with_sampled_limit(&mut device, context.clone(), settings, 64).unwrap();
    let layout = sampled.hdr.sampled_table_layout().expect("sampled HDR PSO required");
    assert_eq!(sampled.attributes.sampled_table_layout(), Some(layout));
    assert_eq!(sampled.shadow_resources.sampled_table_layout(), Some(layout));
    let mesh = Arc::new(Mesh::cube());
    let mut scene = Scene::new();
    let mut textures = Vec::new();
    for (i, color) in [[255, 20, 20, 255], [20, 20, 255, 255]].into_iter().enumerate() {
        let texture = Arc::new(crate::TextureAsset::rgba8(1, 1, color.to_vec()).unwrap());
        let mut material = Material::new();
        material.shading_model = crate::ShadingModel::Unlit;
        material.albedo_texture = Some(texture.id());
        scene.insert(None, Node::new().with_mesh(mesh.clone()).with_materials([Arc::new(material)])
            .with_transform(crate::Transform {
                translation: Vec3::new(if i == 0 { -0.6 } else { 0.6 }, 0., 0.),
                ..crate::Transform::IDENTITY
            })).unwrap();
        textures.push(texture);
    }
    let view = SceneView::new(Arc::new(scene),
        Camera::perspective(Vec3::new(0., 0., 3.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.))
        .with_textures(textures).unwrap().with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sun_direction: Vec3::new(1., 2., 1.).normalized().unwrap(),
            shadow_strength: 1., ..VoxelShading::default()
        }).unwrap().with_screen_effects(Some(settings)).unwrap();
    let expected = draw(&mut device, &mut legacy, &context, &view, &target, &depth);
    let mut steps = Vec::new();
    sampled.render(&view, &mut device, context.clone(), &mut steps).unwrap();
    let mut owned = Vec::new();
    sampled.drain_render_passes(&mut owned);
    for pass in [sampled.pass, sampled.attributes_pass, sampled.shadow_pass] {
        let commands: Vec<_> = owned.iter().filter(|plan| plan.commands().render_pass == pass)
            .flat_map(|plan| plan.commands().commands.iter()).collect();
        assert!(!commands.is_empty(), "missing sampled production geometry/shadow commands");
        assert!(commands.iter().all(|command| command.bind_groups().len() == 2));
    }
    sampled.passes = owned;
    if let Some(submission) = submit_draw(&mut device, &mut sampled, &context, &target, &depth, &steps) {
        SubmissionDevice::wait_submission(&mut device, submission).unwrap();
        sampled.gpu_submission_completed(submission);
    }
    let actual = TextureTransferDevice::read_texture(&mut device, target.texture).unwrap().bytes;
    assert_eq!(actual, expected, "sampled effects composite differs from Legacy");
    assert!(actual.chunks_exact(4).any(|pixel| i16::from(pixel[0]) > i16::from(pixel[2]) + 10));
    let a = &sampled.scenes[&view.id];
    let b = &legacy.scenes[&view.id];
    for (actual, expected) in [(a.targets.color.texture, b.targets.color.texture),
        (a.targets.surface.texture, b.targets.surface.texture)] {
        assert_eq!(TextureTransferDevice::read_texture(&mut device, actual).unwrap().bytes,
            TextureTransferDevice::read_texture(&mut device, expected).unwrap().bytes);
    }
    for (actual, expected) in a.shadow.iter().zip(&b.shadow) {
        assert_eq!(TextureTransferDevice::read_texture(&mut device, actual.texture).unwrap().bytes,
            TextureTransferDevice::read_texture(&mut device, expected.texture).unwrap().bytes);
    }
    sampled.destroy(&mut device).unwrap();
    legacy.destroy(&mut device).unwrap();
    target.destroy(&mut device).unwrap();
    depth.destroy(&mut device).unwrap();
    PipelineDevice::destroy_render_pass(&mut device, pass_id).unwrap();
    eprintln!("GPUI3D_VULKAN_PRODUCTION_BINDLESS_EFFECTS=pass");
}

#[test]
#[ignore = "requires Vulkan; production screen pass color/depth and colored local lighting"]
fn effects_vulkan() {
    verify(gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap());
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires OpenGL 4.5; hidden bootstrap only, no presentation"]
fn effects_opengl() {
    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    // SAFETY: the OS handle outlives the private hidden WGL bootstrap; no desktop interaction.
    let device = unsafe {
        let hwnd = windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow();
        gfx_opengl::OpenGlDevice::new(
            &DeviceDescriptor::default(),
            RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
            RawWindowHandle::Win32(Win32WindowHandle::new(
                std::num::NonZeroIsize::new(hwnd.0 as isize).unwrap(),
            )),
        )
    }
    .unwrap();
    verify(device);
}
