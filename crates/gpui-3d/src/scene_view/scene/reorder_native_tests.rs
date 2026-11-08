//! Offscreen differential check of retained ordering against a full GPU table rebuild.
use super::*;
use crate::scene_view::{artifacts, batching::group_voxel_batches};
use crate::{Material, Mesh, Node, Transform, Vec2, Vec3, Vertex};
use gfx_core::*;
use gfx_dx11::Dx11Device;

fn shader(device: &mut Dx11Device, stage: ShaderStage, entry: &str) -> ShaderModuleId {
    PipelineDevice::create_shader_module(
        device,
        &ShaderModuleDescriptor {
            label: None,
            binary: artifacts::scene_view_dx11_shader(entry)
                .unwrap()
                .to_binary(stage, entry)
                .unwrap(),
        },
    )
    .unwrap()
}

fn color_target(device: &mut Dx11Device, extent: Extent2d) -> (TextureId, TextureViewId) {
    let texture = ResourceDevice::create_texture(
        device,
        &TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
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
            format: Format::Rgba8Unorm,
        },
    )
    .unwrap();
    (texture, view)
}

fn pipelines(
    device: &mut Dx11Device,
    pass: RenderPassId,
    extent: Extent2d,
) -> (
    RendererResources,
    BindGroupLayoutId,
    SamplerId,
    ShaderModuleId,
) {
    let layout = ResourceDevice::create_bind_group_layout(
        device,
        &BindGroupLayoutDescriptor {
            label: None,
            entries: (0..17)
                .map(|binding| BindGroupLayoutEntry {
                    binding,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    binding_type: match binding {
                        3 | 5 | 6 | 10 => BindingType::SampledTexture,
                        4 => BindingType::Sampler,
                        9 => BindingType::UniformBuffer,
                        _ => BindingType::StorageBuffer,
                    },
                })
                .collect(),
        },
    )
    .unwrap();
    let pipeline_layout = PipelineDevice::create_pipeline_layout(
        device,
        &PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: vec![layout],
        },
    )
    .unwrap();
    let vertex_shader = shader(device, ShaderStage::Vertex, "vs_voxel_gpu");
    let fragment_shader = shader(device, ShaderStage::Fragment, "fs_voxel");
    let cs = shader(device, ShaderStage::Compute, "cs_cull");
    let pipeline = PipelineDevice::create_render_pipeline(
        device,
        &RenderPipelineDescriptor {
            label: None,
            vertex_shader,
            vertex_entry_point: "vs_voxel_gpu".into(),
            fragment_shader,
            fragment_entry_point: "fs_voxel".into(),
            vertex_buffers: vec![],
            render_pass: pass,
            pipeline_layout: Some(pipeline_layout),
            color_targets: vec![ColorTargetState::new(
                Format::Rgba8Unorm,
                BlendMode::PremultipliedAlpha,
            )],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
            },
            depth_state: None,
        },
        extent,
    )
    .unwrap();
    let (_, fallback) = color_target(device, Extent2d::new(1, 1).unwrap());
    let sampler = ResourceDevice::create_sampler(device, &SamplerDescriptor::default()).unwrap();
    (
        RendererResources::test_pipeline(layout, pipeline, fallback),
        layout,
        sampler,
        cs,
    )
}

fn fixture() -> (Scene, Camera, Camera) {
    let corners = [(-0.3, -0.3), (0.3, -0.3), (0.3, 0.3), (-0.3, 0.3)].map(|(x, y)| Vertex {
        position: Vec3::new(x, y, 0.3),
        normal: Vec3::Z,
        uv: Vec2::ZERO,
        color: [1.0; 4],
    });
    let quad = Arc::new(
        Mesh::from_voxel_quads_without_raycast([corners], vec![MeshPart::new(0, 6, 0)]).unwrap(),
    );
    let triangle = Arc::new(Mesh::new(corners[..3].to_vec(), vec![0, 1, 2]).unwrap());
    let mut opaque = Material::new();
    opaque.shading_model = ShadingModel::Unlit;
    opaque.double_sided = true;
    let opaque = Arc::new(opaque);
    let mut scene = Scene::new();
    for index in 0..3 {
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(quad.clone())
                    .with_materials([opaque.clone()])
                    .with_transform(Transform {
                        translation: Vec3::new(-0.65, index as f32 * 0.2, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
    }
    for (index, mesh) in [quad, triangle].into_iter().enumerate() {
        let mut material = Material::new();
        material.shading_model = ShadingModel::Unlit;
        material.double_sided = true;
        material.alpha_mode = AlphaMode::Blend;
        material.base_color = if index == 0 {
            [1.0, 0.0, 0.0, 0.5]
        } else {
            [0.0, 0.0, 1.0, 0.5]
        };
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh)
                    .with_materials([Arc::new(material)])
                    .with_transform(Transform {
                        translation: Vec3::new(0.0, 0.0, index as f32 * 0.2),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
    }
    let front = Camera::perspective(
        Vec3::new(0.0, 0.0, 2.0),
        Vec3::ZERO,
        Vec3::Y,
        1.0,
        0.1,
        100.0,
    );
    let back = Camera::perspective(
        Vec3::new(0.0, 0.0, -2.0),
        Vec3::ZERO,
        Vec3::Y,
        1.0,
        0.1,
        100.0,
    );
    (scene, front, back)
}

fn render(
    device: &mut Dx11Device,
    state: &mut SceneResources,
    renderer: &RendererResources,
    cs: ShaderModuleId,
    pass: RenderPassId,
    target: (TextureId, TextureViewId),
) -> Vec<u8> {
    state
        .culling
        .as_mut()
        .unwrap()
        .dispatch(
            device,
            cs,
            state.frame_buffer,
            None,
            state.generated_arguments,
        )
        .unwrap();
    let mut commands = Vec::new();
    state.append_steps(renderer, true, &mut commands).unwrap();
    PresentationDevice::render_to_textures(
        device,
        &[RenderPassCommands {
            color_attachments: std::slice::from_ref(&target.1),
            render_pass: pass,
            commands: &commands,
            color_load_op: LoadOp::Clear(ClearColor::default()),
            depth_attachment: None,
            clear_region: None,
            viewport: None,
            resolve_target: None,
        }],
    )
    .unwrap();
    TextureTransferDevice::read_texture(device, target.0)
        .unwrap()
        .bytes
}

#[test]
#[ignore = "requires D3D11 hardware; offscreen retained-order/reference pixel comparison"]
fn retained_transparent_order_matches_full_rebuild_pixels() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).unwrap();
    let extent = Extent2d::new(32, 32).unwrap();
    let target = color_target(&mut device, extent);
    let pass = PipelineDevice::create_render_pass(
        &mut device,
        &RenderPassDescriptor {
            label: None,
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: None,

            sample_count: 1,
            resolve_target: None,
        },
    )
    .unwrap();
    let (renderer, layout, sampler, cs) = pipelines(&mut device, pass, extent);
    let (scene, front, back) = fixture();
    let mut state = SceneResources::new(&mut device, BackendKind::Dx11).unwrap();
    state.voxel_shader = true;
    state.prepared.set_gpu_culling(true);
    state.prepared.update(&scene, front, 1.0).unwrap();
    state
        .ensure_meshes(&mut device, BackendKind::Dx11, true)
        .unwrap();
    state.batches = group_voxel_batches(&mut state.prepared.draws, &state.meshes);
    assert_eq!(state.batches[0].instance_count, 3);
    state.culling = Some(
        CullResources::create(
            &mut device,
            &state.prepared.draws,
            &state.batches,
            BackendKind::Dx11,
            Some(&state.meshes),
        )
        .unwrap(),
    );
    state
        .sync_buffers(&mut device, &renderer, layout, sampler, true, true, false)
        .unwrap();
    let frame = Mat4::IDENTITY
        .columns()
        .into_iter()
        .flatten()
        .chain([
            0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 32.0, 32.0, 32.0, 32.0, 0.0, 0.0,
        ])
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    ResourceDevice::write_buffer(&mut device, state.frame_buffer, 0, &frame).unwrap();
    let mut outputs = Vec::new();
    for (index, camera) in [back, front, back].into_iter().enumerate() {
        state.prepared.update_camera(camera, 1.0).unwrap();
        let start = state
            .prepared
            .draws
            .partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);
        state.sync_reordered_blends(start).unwrap();
        if index == 1 {
            state
                .sync_draw_resources(
                    &mut device,
                    &renderer,
                    layout,
                    sampler,
                    true,
                    true,
                    false,
                    true,
                )
                .unwrap();
        }
        let retained = render(&mut device, &mut state, &renderer, cs, pass, target);
        state
            .culling
            .as_mut()
            .unwrap()
            .update(
                &mut device,
                &state.prepared.draws,
                &state.batches,
                Some(&state.meshes),
            )
            .unwrap();
        state
            .sync_buffers(&mut device, &renderer, layout, sampler, true, true, false)
            .unwrap();
        let reference = render(&mut device, &mut state, &renderer, cs, pass, target);
        assert_eq!(
            retained, reference,
            "camera {index}: retained slot/argument mapping differs"
        );
        outputs.push(retained);
    }
    assert_ne!(
        outputs[0], outputs[1],
        "overlapping blended colors must change with depth order"
    );
    assert_eq!(outputs[0], outputs[2]);
    SubmissionDevice::wait_queue_idle(&mut device).unwrap();
}
