//! Native count/argument validation; only pixels are read back by these tests.
use super::*;

/// Renders the generated pipeline and asserts both probe pixels exactly.
pub(super) fn render_generated<D: Device + TextureTransferDevice>(
    device: &mut D,
    mesh: &Mesh,
    resource: &MeshResources,
    arguments: BufferId,
    left: [u8; 4],
    fog: Option<crate::DistanceFog>,
    style: Option<(crate::VoxelShading, [u8; 4])>,
) {
    probe_generated(device, mesh, resource, arguments, left, fog, style, true);
}

/// Renders the generated pipeline and returns the two probe pixels.
///
/// Smooth per-corner lighting interpolates vertex colors across each quad, so tests
/// whose light field varies per cell assert on ranges instead of exact pixels.
pub(super) fn probe_generated<D: Device + TextureTransferDevice>(
    device: &mut D,
    mesh: &Mesh,
    resource: &MeshResources,
    arguments: BufferId,
    left: [u8; 4],
    fog: Option<crate::DistanceFog>,
    style: Option<(crate::VoxelShading, [u8; 4])>,
    assert_pixels: bool,
) -> [[u8; 4]; 2] {
    let mut probes = [[0u8; 4]; 2];
    use super::super::{culling::tests as culling_tests, gpu};
    use crate::{
        Camera, Mat4, Material, Node, PreparedScene, Scene, ShadingModel, Transform, Vec3,
    };
    use std::sync::Arc;
    let frame = |matrix| {
        let mut bytes = culling_tests::frame(matrix);
        if let Some(fog) = fog {
            for (destination, value) in bytes[112..].chunks_exact_mut(4).zip(fog.encoded()) {
                destination.copy_from_slice(&value.to_le_bytes());
            }
        }
        if let Some((style, _)) = style {
            for (dst, value) in bytes[144..192].chunks_exact_mut(4).zip(style.encoded()) {
                dst.copy_from_slice(&value.to_le_bytes());
            }
            if style.shadow_strength > 0. {
                for (dst, value) in bytes[192..256]
                    .chunks_exact_mut(4)
                    .zip(Mat4::IDENTITY.columns().into_iter().flatten())
                {
                    dst.copy_from_slice(&value.to_le_bytes());
                }
                for (dst, value) in
                    bytes[256..]
                        .chunks_exact_mut(4)
                        .zip([style.shadow_strength, 0., 512., 0.])
                {
                    dst.copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        bytes
    };
    let right = fog.map_or(style.map_or([255; 4], |(_, right)| right), |fog| {
        [
            (fog.color[0] * 255.0).round() as u8,
            (fog.color[1] * 255.0).round() as u8,
            (fog.color[2] * 255.0).round() as u8,
            255,
        ]
    });
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.shading_model = ShadingModel::Unlit;
    material.double_sided = true;
    if style.is_some_and(|(style, _)| style.illumination == crate::VoxelIllumination::Sunlight) {
        material.normal_texture = Some(crate::TextureAssetId(101));
        material.metallic_roughness_texture = Some(crate::TextureAssetId(102));
        material.pbr_texture_layout = crate::PbrTextureLayout::MetallicEmissiveRoughness;
    }
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(mesh.clone()))
                .with_materials([Arc::new(material)])
                .with_transform(Transform {
                    translation: Vec3::new(0.0, -0.4, 0.2),
                    scale: Vec3::new(0.6, 0.8, 0.2),
                    ..Transform::IDENTITY
                }),
        )
        .unwrap();
    let mut prepared = PreparedScene::default();
    prepared
        .update(
            &scene,
            Camera::perspective(
                Vec3::new(0.0, 0.0, 2.0),
                Vec3::ZERO,
                Vec3::Y,
                1.0,
                0.1,
                100.0,
            ),
            1.0,
        )
        .unwrap();
    assert_eq!(prepared.draws.len(), 1);
    let mut buffers = Vec::new();
    for (bytes, usage) in [
        (gpu::encode_draws(&prepared).unwrap(), BufferUsage::STORAGE),
        (
            {
                let mut instances = gpu::encode_instances(&prepared).unwrap();
                // The standalone source begins at descriptor zero. A pooled
                // generated mesh instead addresses a different suballocation
                // in a shared face buffer; mirror the production instance
                // encoder's 140-byte vertex-base field for native rendering.
                if resource.vertex_base > 0 {
                    instances[140..144]
                        .copy_from_slice(&(resource.vertex_base | 0x8000_0000).to_le_bytes());
                }
                // In the page-based generated vertex path, the normal matrix
                // is reserved metadata. Pass the header record offset exactly
                // as the production Instance encoder does.
                let base = resource.generated.as_ref().unwrap().template_base;
                instances[68..72].copy_from_slice(&((base & 0xffff) as f32).to_le_bytes());
                instances[72..76].copy_from_slice(&((base >> 16) as f32).to_le_bytes());
                instances
            },
            BufferUsage::STORAGE,
        ),
        (frame(Mat4::IDENTITY), BufferUsage::UNIFORM),
    ] {
        let buffer = ResourceDevice::create_buffer(
            device,
            &BufferDescriptor {
                label: None,
                size: bytes.len() as u64,
                usage: usage | BufferUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly,
            },
        )
        .unwrap();
        ResourceDevice::write_buffer(device, buffer, 0, &bytes).unwrap();
        buffers.push((buffer, bytes.len() as u64));
    }
    let generated = resource.generated.as_ref().unwrap();
    let shadow = style
        .filter(|(style, _)| style.shadow_strength > 0.)
        .map(|_| culling_tests::upload(device, &vec![0; 512 * 512 * 4], BufferUsage::STORAGE));
    let (group, layout) = culling_tests::graphics_group(
        device,
        &[
            (0, resource.vertices, resource.vertex_binding_bytes as u64),
            (1, buffers[0].0, buffers[0].1),
            (8, buffers[1].0, buffers[1].1),
            (9, buffers[2].0, buffers[2].1),
            (12, generated.templates, generated.template_page_bytes / 112 * 112),
            (13, resource.vertices, resource.vertex_binding_bytes as u64),
            (14, generated.templates, generated.template_page_bytes),
            (
                15,
                shadow.unwrap_or(buffers[0].0),
                if shadow.is_some() {
                    512 * 512 * 4
                } else {
                    buffers[0].1
                },
            ),
        ],
    );
    let extent = Extent2d::new(16, 16).unwrap();
    let texture = ResourceDevice::create_texture(
        device,
        &TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
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
    let pass = PipelineDevice::create_render_pass(
        device,
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
    let vs = shader(device, ShaderStage::Vertex, "vs_generated_voxel");
    let fs = shader(device, ShaderStage::Fragment, "fs_voxel");
    let pipeline = PipelineDevice::create_render_pipeline(
        device,
        &RenderPipelineDescriptor {
            label: None,
            vertex_shader: vs,
            vertex_entry_point: "vs_generated_voxel".into(),
            fragment_shader: fs,
            fragment_entry_point: "fs_voxel".into(),
            vertex_buffers: vec![],
            render_pass: pass,
            pipeline_layout: Some(layout),
            color_targets: vec![ColorTargetState::new(Format::Rgba8Unorm, BlendMode::Replace)],
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
    let mut culling = super::super::culling::CullResources::create(
        device,
        &prepared.draws,
        &[super::super::scene::DrawBatchRange {
            first_instance: 0,
            instance_count: 1,
        }],
        D::BACKEND_KIND,
        None,
    )
    .unwrap();
    ResourceDevice::write_buffer(device, culling.batches, 0, &1u32.to_le_bytes()).unwrap();
    let cs = create_shader(device, D::BACKEND_KIND, "cs_cull").unwrap();
    for visible in [true, false, true] {
        let matrix = if visible {
            Mat4::IDENTITY
        } else {
            Transform {
                translation: Vec3::new(0.0, 200.0, 0.0),
                ..Transform::IDENTITY
            }
            .matrix()
        };
        ResourceDevice::write_buffer(device, buffers[2].0, 0, &frame(matrix)).unwrap();
        culling
            .dispatch(device, cs, buffers[2].0, None, Some((arguments, 3)))
            .unwrap();
        ResourceDevice::write_buffer(device, buffers[2].0, 0, &frame(Mat4::IDENTITY)).unwrap();
        PresentationDevice::render_to_textures(
            device,
            &[RenderPassCommands {
                color_attachments: std::slice::from_ref(&view),
                render_pass: pass,
                commands: &[RenderCommand::DrawIndirect(DrawIndirectCommand {
                    pipeline,
                    bind_groups: bind_group_list([group]),
                    arguments: culling.arguments,
                    offset: 0,
                    scissor: None,
                })],
                color_load_op: LoadOp::Clear(ClearColor::default()),
                depth_attachment: None,
                clear_region: None,
                viewport: None,
            resolve_target: None,

        }],
        )
        .unwrap();
        let pixels = TextureTransferDevice::read_texture(device, texture)
            .unwrap()
            .bytes;
        probes = [
            pixels[(8 * 16 + 5) * 4..][..4].try_into().unwrap(),
            pixels[(8 * 16 + 10) * 4..][..4].try_into().unwrap(),
        ];
        if assert_pixels {
            assert_eq!(
                &pixels[(8 * 16 + 5) * 4..][..4],
                if visible { &left } else { &[0, 0, 0, 255] }
            );
            assert_eq!(
                &pixels[(8 * 16 + 10) * 4..][..4],
                if visible { &right } else { &[0, 0, 0, 255] }
            );
            assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
        }
    }
    culling.destroy(device).unwrap();
    PipelineDevice::destroy_shader_module(device, cs).unwrap();
    probes
}

fn shader<D: Device>(device: &mut D, stage: ShaderStage, entry: &str) -> ShaderModuleId {
    let artifact = match D::BACKEND_KIND {
        BackendKind::Dx11 => artifacts::scene_view_dx11_shader(entry),
        BackendKind::Dx12 => artifacts::scene_view_dx12_shader(entry),
        BackendKind::Vulkan => artifacts::scene_view_vulkan_shader(entry),
        BackendKind::OpenGl => artifacts::scene_view_opengl_shader(entry),
        _ => None,
    }
    .unwrap();
    PipelineDevice::create_shader_module(
        device,
        &ShaderModuleDescriptor {
            label: None,
            binary: artifact.to_binary(stage, entry).unwrap(),
        },
    )
    .unwrap()
}
pub(in crate::scene_view) fn counts<D: Device + TextureTransferDevice>(
    device: &mut D,
    buffer: BufferId,
    size: u64,
) -> [u8; 4] {
    probe(device, buffer, size, "fs_mesh_check")
}

pub(in crate::scene_view) fn probe<D: Device + TextureTransferDevice>(
    device: &mut D,
    buffer: BufferId,
    size: u64,
    fragment: &str,
) -> [u8; 4] {
    let extent = Extent2d::new(4, 4).unwrap();
    let texture = ResourceDevice::create_texture(
        device,
        &TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
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
    let pass = PipelineDevice::create_render_pass(
        device,
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
    let layout = ResourceDevice::create_bind_group_layout(
        device,
        &BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::FRAGMENT,
            }],
        },
    )
    .unwrap();
    let group = ResourceDevice::create_bind_group(
        device,
        &BindGroupDescriptor {
            label: None,
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer,
                    offset: 0,
                    size,
                    stride: Some(4),
                }),
            }],
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
    let vs = shader(device, ShaderStage::Vertex, "vs_mesh_check");
    let fs = shader(device, ShaderStage::Fragment, fragment);
    let pipeline = PipelineDevice::create_render_pipeline(
        device,
        &RenderPipelineDescriptor {
            label: None,
            vertex_shader: vs,
            vertex_entry_point: "vs_mesh_check".into(),
            fragment_shader: fs,
            fragment_entry_point: fragment.into(),
            vertex_buffers: vec![],
            render_pass: pass,
            pipeline_layout: Some(pipeline_layout),
            color_targets: vec![ColorTargetState::new(Format::Rgba8Unorm, BlendMode::Replace)],
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
    PresentationDevice::render_to_textures(
        device,
        &[RenderPassCommands {
            color_attachments: std::slice::from_ref(&view),
            render_pass: pass,
            commands: &[RenderCommand::Draw(DrawCommand {
                pipeline,
                bind_groups: bind_group_list([group]),
                vertex_count: 3,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            })],
            color_load_op: LoadOp::Clear(ClearColor::default()),
            depth_attachment: None,
            clear_region: None,
            viewport: None,
            resolve_target: None,

        }],
    )
    .unwrap();
    TextureTransferDevice::read_texture(device, texture)
        .unwrap()
        .bytes[..4]
        .try_into()
        .unwrap()
}
fn verify<D: Device + TextureTransferDevice>(mut device: D) {
    let cs = create_shaders(&mut device, D::BACKEND_KIND).unwrap();
    let source = crate::voxel_mesh::tests::source();
    let wide_source = crate::voxel_mesh::tests::source();
    let wide_templates = wide_source.templates.to_vec();
    let wide_classes = wide_source
        .cells
        .iter()
        .map(|v| u32::from(v != 0) << 16)
        .collect();
    let wide = wide_source
        .with_instances(
            wide_classes,
            wide_templates,
            vec![crate::VoxelMeshInstance {
                position: [1, 1, 1],
                faces: 0..6,
                height: 0.4,
                occlusion_mask: 1 << 16,
            }],
        )
        .unwrap();
    let extra = source.templates[0].clone();
    let extra_source = crate::voxel_mesh::tests::source();
    let extra_templates = extra_source.templates.to_vec();
    let classes = extra_source
        .cells
        .iter()
        .map(|v| u32::from(v != 0))
        .collect();
    let instances = extra_source
        .with_instances(
            classes,
            extra_templates,
            vec![crate::VoxelMeshInstance {
                position: [1, 1, 1],
                faces: 0..6,
                height: 0.4,
                occlusion_mask: 1,
            }],
        )
        .unwrap();
    let ordered_source = crate::voxel_mesh::tests::source();
    let mut red = ordered_source
        .templates
        .iter()
        .find(|face| face.neighbour == [0, 0, 1])
        .unwrap()
        .clone();
    red.vertices
        .iter_mut()
        .for_each(|v| v.color = [1.0, 0.0, 0.0, 1.0]);
    let mut blue = red.clone();
    blue.vertices
        .iter_mut()
        .for_each(|v| v.color = [0.0, 0.0, 1.0, 1.0]);
    let ordered = ordered_source
        .with_instances(
            vec![0; 36],
            vec![red, blue],
            (0..129)
                .map(|i| crate::VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: if i % 2 == 0 { 0..1 } else { 1..2 },
                    height: 1.0,
                    occlusion_mask: 0,
                })
                .collect(),
        )
        .unwrap();
    let base = crate::voxel_mesh::tests::source();
    let classes = base
        .cells
        .iter()
        .map(|value| u32::from(value != 0))
        .collect();
    let instances_only = crate::VoxelMeshSource::new(base.size, base.origin, vec![0; 36], vec![])
        .unwrap()
        .with_instances(
            classes,
            base.templates.to_vec(),
            vec![
                crate::VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 1.0,
                    occlusion_mask: 1,
                },
                crate::VoxelMeshInstance {
                    position: [2, 1, 1],
                    faces: 0..6,
                    height: 1.0,
                    occlusion_mask: 1,
                },
            ],
        )
        .unwrap();
    for (source, expected, static_count) in [
        // Two touching canonical cubes have 10 exposed faces. The opt-in
        // horizontal GPU run fuses their shared top, bottom, front and back
        // plane pairs into six quads while CPU fallback remains unchanged.
        (crate::voxel_mesh::tests::source().with_greedy_visible_faces(true), 36u16, 0),
        (crate::voxel_mesh::tests::source(), 60u16, 0),
        (
            crate::voxel_mesh::tests::source()
                .with_y_range(0.0, 0.5)
                .unwrap(),
            12,
            0,
        ),
        (source.with_static_faces(vec![extra]).unwrap(), 66, 1),
        (instances, 90, 0),
        (wide, 90, 0),
        (ordered, 834, 0),
        (instances_only, 60, 0),
    ] {
        let mesh = Mesh::from_gpu_voxels(source);
        let generated = create(&mut device, &mesh, &cs).unwrap();
        let args = generated.generated.as_ref().unwrap().arguments;
        assert_eq!(
            counts(&mut device, args, 16),
            [expected as u8, (expected >> 8) as u8, 1, static_count]
        );
        let mapping_bytes = [0u32, 0, 11, 0, 0, 2, 7, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let mapping = ResourceDevice::create_buffer(
            &mut device,
            &BufferDescriptor {
                label: None,
                size: mapping_bytes.len() as u64,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly,
            },
        )
        .unwrap();
        ResourceDevice::write_buffer(&mut device, mapping, 0, &mapping_bytes).unwrap();
        let output = ResourceDevice::create_buffer(
            &mut device,
            &BufferDescriptor {
                label: None,
                size: 48,
                usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
                memory_location: MemoryLocation::GpuOnly,
            },
        )
        .unwrap();
        let bindings = [
            (args, 16, ComputeBufferAccess::ReadOnly),
            (mapping, 32, ComputeBufferAccess::ReadOnly),
            (output, 48, ComputeBufferAccess::ReadWrite),
        ]
        .into_iter()
        .enumerate()
        .map(|(slot, (buffer, size, access))| ComputeBufferBinding {
            binding: slot as u32,
            buffer: BufferBinding {
                buffer,
                offset: 0,
                size,
                stride: None,
            },
            access,
        })
        .collect::<Vec<_>>();
        let copy = create_shader(&mut device, D::BACKEND_KIND, "cs_mesh_draws").unwrap();
        ResourceDevice::dispatch_compute(
            &mut device,
            &ComputeDispatch {
                shader: copy,
                bindings: &bindings,
                workgroups: [1, 1, 1],
            },
        )
        .unwrap();
        assert_eq!(
            counts(&mut device, output, 48),
            [expected as u8, (expected >> 8) as u8, 1, 11]
        );
        if expected == 36 || expected == 60 || expected == 834 {
            let before = mesh.geometry_storage_bytes();
            ResourceDevice::write_buffer(&mut device, mapping, 8, &0u32.to_le_bytes()).unwrap();
            ResourceDevice::dispatch_compute(
                &mut device,
                &ComputeDispatch {
                    shader: copy,
                    bindings: &bindings,
                    workgroups: [1, 1, 1],
                },
            )
            .unwrap();
            render_generated(
                &mut device,
                &mesh,
                &generated,
                output,
                if expected == 834 {
                    [255, 0, 0, 255]
                } else {
                    [255; 4]
                },
                None,
                None,
            );
            assert_eq!(
                before,
                mesh.geometry_storage_bytes(),
                "GPU render must not materialize the CPU fallback"
            );
        }
    }
    SubmissionDevice::wait_queue_idle(&mut device).unwrap();
}
/// Optional native regression: two independent Generated Sections must
/// share one face-buffer allocation without aliasing descriptors or counts.
#[test]
#[ignore = "requires D3D11 device; shared GPU face pool and deferred retirement"]
fn generated_faces_share_a_page_with_independent_section_offsets() {
    let mut device = gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap();
    let shaders = create_shaders(&mut device, BackendKind::Dx11).unwrap();
    let meshes = [
        Mesh::from_gpu_voxels(crate::voxel_mesh::tests::source()),
        Mesh::from_gpu_voxels(crate::voxel_mesh::tests::source()),
    ];
    let limits = gpui::ExtensionDevice::buffer_binding_limits(&device);
    let mut arena = MeshArena::generated_output(limits);
    let mut templates = MeshArena::generated_templates(limits);
    let mut arguments = MeshArena::generated_arguments(limits);
    let mut resources = create_many_pooled(
        &mut device, &[&meshes[0], &meshes[1]], &shaders,
        &mut arena, &mut templates, &mut arguments, BackendKind::Dx11,
    ).unwrap();
    assert_eq!(resources[0].generated.as_ref().unwrap().templates,
               resources[1].generated.as_ref().unwrap().templates);
    assert_ne!(resources[0].generated.as_ref().unwrap().template_base,
               resources[1].generated.as_ref().unwrap().template_base);
    assert_eq!(resources[0].generated.as_ref().unwrap().arguments,
               resources[1].generated.as_ref().unwrap().arguments);
    assert_ne!(resources[0].generated.as_ref().unwrap().argument_base,
               resources[1].generated.as_ref().unwrap().argument_base);
    assert_ne!(resources[0].generated.as_ref().unwrap().templates, resources[0].vertices);
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].vertices, resources[1].vertices);
    assert_ne!(resources[0].vertex_base, resources[1].vertex_base);
    assert!(resources.iter().all(|resource| resource.allocation.is_some()));
    assert_eq!(arena.stats().0, 1);
    for (mesh, resource) in meshes.iter().zip(&resources) {
        assert_eq!(counts(
            &mut device,
            resource.generated.as_ref().unwrap().arguments,
            16,
        )[0], 60);
        render_generated(
            &mut device,
            mesh,
            resource,
            resource.generated.as_ref().unwrap().arguments,
            [255; 4],
            None,
            None,
        );
    }
    SubmissionDevice::wait_queue_idle(&mut device).unwrap();
    let mut ordinary = MeshArena::default();
    for resource in resources.drain(..) {
        super::super::gpu::destroy_mesh(
            &mut device, resource, &mut ordinary, &mut arena, &mut templates, &mut arguments,
        ).unwrap();
    }
    assert_eq!(arena.stats().0, 0);
    assert_eq!(templates.stats().0, 0);
    assert_eq!(arguments.stats().0, 0);
}

#[test]
#[ignore = "requires D3D11; offscreen GPU-generated mesh counts"]
fn gpu_mesher_dx11() {
    verify(gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires D3D12; offscreen GPU-generated mesh counts"]
fn gpu_mesher_dx12() {
    verify(gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires Vulkan; offscreen GPU-generated mesh counts"]
fn gpu_mesher_vulkan() {
    verify(gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap());
}
#[test]
#[ignore = "requires OpenGL; hidden bootstrap only, no presentation"]
fn gpu_mesher_opengl() {
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
