#[path = "tests/materials.rs"]
mod materials;
use super::super::targets::Target;
use super::*;
use crate::{AlphaMode, Light, Material, Mesh, Node, VoxelIllumination, VoxelShading};
use crate::{Camera, Scene, Vec3};
use materials::{verify_shadow_retry, verify_shadows, verify_sky_transmission};

fn verify<D: Device + TextureTransferDevice>(mut device: D) {
    let extent = Extent2d::new(64, 64).unwrap();
    let target = Target::create(&mut device, extent, Format::Rgba8Unorm, false).unwrap();
    let depth = Target::create(&mut device, extent, Format::Depth32Float, true).unwrap();
    let pass_id = PipelineDevice::create_render_pass(
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
    assert!(owned.len() >= 4);
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        texture_view: target.view,
        render_pass: pass_id,
        commands: &steps,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
        }),
        clear_region: None,
        viewport: None,
    });
    PresentationDevice::render_to_textures(&mut device, &passes).unwrap();
    renderer.render_passes_submitted();
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
        owned.len() >= 4,
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
    verify_shadows(&mut device, &mut renderer, &context, &target, &depth);
    verify_shadow_retry(&mut device, &mut renderer, &context, &view);
    verify_sky_transmission(&mut device, &mut renderer, &context, &view, &target, &depth);
    renderer.destroy(&mut device).unwrap();
    target.destroy(&mut device).unwrap();
    depth.destroy(&mut device).unwrap();
    PipelineDevice::destroy_render_pass(&mut device, pass_id).unwrap();
}

fn draw<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
    target: &Target,
    depth: &Target,
) -> Vec<u8> {
    let mut steps = Vec::new();
    renderer
        .render(view, device, context.clone(), &mut steps)
        .unwrap();
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        texture_view: target.view,
        render_pass: context.render_pass(),
        commands: &steps,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
        }),
        clear_region: None,
        viewport: None,
    });
    PresentationDevice::render_to_textures(device, &passes).unwrap();
    renderer.render_passes_submitted();
    TextureTransferDevice::read_texture(device, target.texture)
        .unwrap()
        .bytes
}

#[test]
#[ignore = "requires D3D11; production screen pass color/depth and colored local lighting"]
fn effects_dx11() {
    verify(gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires D3D12; production screen pass color/depth and colored local lighting"]
fn effects_dx12() {
    verify(gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires Vulkan; production screen pass color/depth and colored local lighting"]
fn effects_vulkan() {
    verify(gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap());
}
