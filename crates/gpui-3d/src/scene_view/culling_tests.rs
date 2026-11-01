//! Headless verification of the production compute and voxel vertex shaders.

use super::*;
use crate::scene_view::{
    artifacts,
    batching::group_voxel_batches,
    gpu::{self, MeshArena},
};
use crate::{
    Aabb, Camera, Mat4, Material, Mesh, MeshPart, Node, PreparedScene, Scene, ShadingModel,
    Transform, Vec2, Vec3, Vertex,
};
use gfx_core::*;
use gfx_dx11::Dx11Device;
use std::{collections::HashMap, sync::Arc};

#[path = "hiz_tests.rs"]
mod hiz_tests;

fn quad_mesh(quads: usize, color: [f32; 4]) -> Arc<Mesh> {
    let corners = [(-0.9, -0.5), (-0.1, -0.5), (-0.1, 0.5), (-0.9, 0.5)];
    let left = corners.map(|(x, y)| Vertex {
        position: Vec3::new(x, y, 0.5),
        normal: Vec3::Z,
        uv: Vec2::new(0.0, 0.0),
        color,
    });
    let right = left.map(|mut vertex| {
        vertex.position.x += 1.0;
        vertex.color = [0.0, 0.0, 1.0, 1.0];
        vertex
    });
    Arc::new(
        Mesh::from_voxel_quads_without_raycast(
            std::iter::repeat_n(left, 2).chain(std::iter::repeat_n(right, quads)),
            vec![
                MeshPart::new(0, 12, 0),
                MeshPart::new(12, (quads * 6) as u32, 0),
            ],
        )
        .unwrap(),
    )
}

fn prepared() -> PreparedScene {
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.shading_model = ShadingModel::Unlit;
    let material = Arc::new(material);
    for index in 0..1024 {
        let x = if index == 1 { 0.0 } else { 10.0 };
        // The visible mesh is smaller than the hidden meshes in its size class.
        let count = 3 + (index + 1) % 2;
        let color = if index == 1 {
            [0.0, 1.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0, 1.0]
        };
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(quad_mesh(count, color))
                    .with_materials([material.clone()])
                    .with_culling_origin()
                    .with_transform(Transform {
                        translation: Vec3::new(x, 0.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
    }
    let triangle = [(-0.4, 0.6), (0.4, 0.6), (0.0, 0.9)].map(|(x, y)| Vertex {
        position: Vec3::new(x, y, 0.5),
        normal: Vec3::Z,
        uv: Vec2::new(0.0, 0.0),
        color: [1.0, 0.0, 1.0, 1.0],
    });
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::new(triangle, [0, 1, 2]).unwrap()))
                .with_materials([material]),
        )
        .unwrap();
    let mut prepared = PreparedScene::default();
    prepared.set_gpu_culling(true);
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
    prepared
}

fn upload(device: &mut impl ResourceDevice, bytes: &[u8], usage: BufferUsage) -> BufferId {
    let buffer = ResourceDevice::create_buffer(
        device,
        &BufferDescriptor {
            label: None,
            size: bytes.len().max(16) as u64,
            usage: usage | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
        },
    )
    .unwrap();
    ResourceDevice::write_buffer(device, buffer, 0, bytes).unwrap();
    buffer
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

pub(in crate::scene_view) fn frame(matrix: Mat4) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in matrix.columns().into_iter().flatten().chain([
        0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 16.0, 16.0, 16.0, 16.0, 0.0, 0.0,
    ]) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.resize(FRAME_PARAMS_STRIDE, 0);
    bytes
}

pub(in crate::scene_view) fn graphics_group<D: Device>(
    device: &mut D,
    buffers: &[(u32, BufferId, u64)],
) -> (BindGroupId, PipelineLayoutId) {
    let mut entries = Vec::new();
    let mut layout_entries = Vec::new();
    for &(binding, buffer, size) in buffers {
        entries.push(BindGroupEntry {
            binding,
            resource: BindingResource::Buffer(BufferBinding {
                buffer,
                offset: 0,
                size,
                stride: None,
            }),
        });
        layout_entries.push(BindGroupLayoutEntry {
            binding,
            binding_type: if binding == 9 {
                BindingType::UniformBuffer
            } else {
                BindingType::StorageBuffer
            },
            stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
        });
    }
    // Vulkan pipeline layouts must include every statically referenced shader binding,
    // even when the material has no texture and its fragment branch will not sample.
    let texture = ResourceDevice::create_texture(
        device,
        &TextureDescriptor {
            label: None,
            size: Extent2d::new(1, 1).unwrap(),
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
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
            format: Format::Rgba8Unorm,
        },
    )
    .unwrap();
    let sampler = ResourceDevice::create_sampler(device, &SamplerDescriptor::default()).unwrap();
    entries.extend([
        BindGroupEntry {
            binding: 3,
            resource: BindingResource::Texture(TextureBinding { texture_view: view }),
        },
        BindGroupEntry {
            binding: 4,
            resource: BindingResource::Sampler(SamplerBinding { sampler }),
        },
    ]);
    layout_entries.extend([
        BindGroupLayoutEntry {
            binding: 3,
            binding_type: BindingType::SampledTexture,
            stages: ShaderStages::FRAGMENT,
        },
        BindGroupLayoutEntry {
            binding: 4,
            binding_type: BindingType::Sampler,
            stages: ShaderStages::FRAGMENT,
        },
    ]);
    let layout = ResourceDevice::create_bind_group_layout(
        device,
        &BindGroupLayoutDescriptor {
            label: None,
            entries: layout_entries,
        },
    )
    .unwrap();
    let group = ResourceDevice::create_bind_group(
        device,
        &BindGroupDescriptor {
            label: None,
            layout,
            entries,
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
    (group, pipeline_layout)
}

#[test]
#[ignore = "requires Windows D3D11 hardware; no window is created"]
fn gpu_compacts_voxel_instances_and_camera_updates_reuse_buffers() {
    verify_gpu_compaction(Dx11Device::new(&DeviceDescriptor::default()).unwrap(), 1);
}

#[test]
#[ignore = "requires Vulkan hardware; no window is created"]
fn vulkan_compacts_voxel_instances_and_camera_updates_reuse_buffers() {
    verify_gpu_compaction(
        gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap(),
        1,
    );
}

#[test]
#[ignore = "requires Windows D3D12 hardware; no window is created"]
fn dx12_compacts_voxel_instances_and_camera_updates_reuse_buffers() {
    verify_gpu_compaction(
        gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap(),
        1,
    );
}

#[test]
#[ignore = "requires OpenGL4.5 and ARB_shader_draw_parameters; renders only offscreen"]
fn opengl_compacts_voxel_instances_and_camera_updates_reuse_buffers() {
    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    // SAFETY: the OS-owned desktop handle remains valid. WGL creates its own private
    // hidden bootstrap HWND; this test never presents, reads or interacts with the desktop.
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
    verify_gpu_compaction(device, 1);
}

#[test]
#[ignore = "requires Windows D3D11 hardware; profiles many indirect draws without a window"]
fn profile_many_indirect_draws() {
    verify_gpu_compaction(Dx11Device::new(&DeviceDescriptor::default()).unwrap(), 512);
}

fn verify_gpu_compaction<D: Device + TextureTransferDevice>(mut device: D, repetitions: usize) {
    let mut prepared = prepared();
    let mut arena = MeshArena::default();
    let mut meshes = HashMap::new();
    for draw in &prepared.draws {
        if meshes.contains_key(&(draw.mesh.id(), true)) {
            continue;
        }
        let (mesh, upload) =
            gpu::create_mesh(&mut device, &draw.mesh, D::BACKEND_KIND, true, &mut arena).unwrap();
        if let Some(upload) = upload {
            ResourceDevice::write_buffer(&mut device, upload.buffer, upload.offset, &upload.bytes)
                .unwrap();
        }
        meshes.insert((draw.mesh.id(), true), mesh);
    }
    let batches = group_voxel_batches(&mut prepared.draws, &meshes);
    assert_eq!(prepared.draws.len(), 2049);
    assert_eq!(
        batches.len(),
        3,
        "quad ranges share two draws; the indexed triangle stays separate"
    );
    let mut mirrored = prepared.draws.clone();
    assert_eq!(mirrored[0].raster_front_face(), Some(FrontFace::Ccw));
    mirrored[0].world = Transform {
        scale: Vec3::new(-1.0, 1.0, 1.0),
        ..Transform::IDENTITY
    }
    .matrix();
    assert_eq!(mirrored[0].raster_front_face(), Some(FrontFace::Cw));
    let mirrored_batches = group_voxel_batches(&mut mirrored, &meshes);
    assert_eq!(mirrored_batches.len(), batches.len() + 1);
    for batch in mirrored_batches {
        let draws = &mirrored[batch.first_instance..batch.first_instance + batch.instance_count];
        assert!(
            draws
                .iter()
                .all(|draw| draw.is_mirrored() == draws[0].is_mirrored())
        );
    }
    let mut blended = prepared.draws.clone();
    for draw in &mut blended {
        Arc::make_mut(&mut draw.material).alpha_mode = crate::AlphaMode::Blend;
    }
    let order = blended
        .iter()
        .map(|draw| (draw.node, draw.part))
        .collect::<Vec<_>>();
    assert_eq!(group_voxel_batches(&mut blended, &meshes).len(), 2049);
    assert_eq!(
        blended
            .iter()
            .map(|draw| (draw.node, draw.part))
            .collect::<Vec<_>>(),
        order
    );
    let mut mixed = prepared.draws.clone();
    mixed.extend(blended.iter().take(4).cloned());
    let mixed_batches = group_voxel_batches(&mut mixed, &meshes);
    assert_eq!(mixed_batches.len(), batches.len() + 4);
    assert_eq!(
        mixed[mixed.len() - 4..]
            .iter()
            .map(|draw| (draw.node, draw.part))
            .collect::<Vec<_>>(),
        order[..4]
    );
    let mut culling = CullResources::create(
        &mut device,
        &prepared.draws,
        &batches,
        D::BACKEND_KIND,
        None,
    )
    .unwrap();
    let cs = shader(&mut device, ShaderStage::Compute, "cs_cull");
    // Two read-only slots may share a newly allocated resource. D3D12 must
    // transition that resource once, even before its first upload.
    let shared_uniform = ResourceDevice::create_buffer(
        &mut device,
        &BufferDescriptor {
            label: None,
            size: FRAME_PARAMS_STRIDE as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
        },
    )
    .unwrap();
    let bindings = [
        compute_binding(
            0,
            shared_uniform,
            FRAME_PARAMS_STRIDE as u64,
            ComputeBufferAccess::Uniform,
        ),
        compute_binding(
            1,
            culling.bounds,
            prepared.draws.len() as u64 * BOUNDS_STRIDE as u64,
            ComputeBufferAccess::ReadOnly,
        ),
        compute_binding(
            2,
            culling.arguments,
            batches.len() as u64 * ARGUMENT_STRIDE,
            ComputeBufferAccess::ReadWrite,
        ),
        compute_binding(
            3,
            culling.visible_instances,
            prepared.draws.len() as u64 * 4,
            ComputeBufferAccess::ReadWrite,
        ),
        compute_binding(
            4,
            culling.batches,
            batches.len() as u64 * ARGUMENT_STRIDE,
            ComputeBufferAccess::ReadOnly,
        ),
        compute_binding(5, shared_uniform, 32, ComputeBufferAccess::Uniform),
        compute_binding(
            6,
            culling.batches,
            batches.len() as u64 * ARGUMENT_STRIDE,
            ComputeBufferAccess::ReadOnly,
        ),
        compute_binding(
            7,
            culling.hiz.depths,
            (super::super::hiz::WORDS * 4) as u64,
            ComputeBufferAccess::ReadOnly,
        ),
    ];
    ResourceDevice::dispatch_compute(
        &mut device,
        &ComputeDispatch {
            shader: cs,
            bindings: &bindings,
            workgroups: [batches.len() as u32, 1, 1],
        },
    )
    .unwrap();
    ResourceDevice::destroy_buffer(&mut device, shared_uniform).unwrap();
    let vs = shader(&mut device, ShaderStage::Vertex, "vs_voxel_gpu");
    let fs = shader(&mut device, ShaderStage::Fragment, "fs_voxel");
    let draws = gpu::encode_draws(&prepared).unwrap();
    let mut instances = gpu::encode_instances(&prepared).unwrap();
    for (index, draw) in prepared.draws.iter().enumerate() {
        let offset = index * 144;
        let resource = &meshes[&(draw.mesh.id(), true)];
        let base = if resource.procedural_quads {
            instances[offset + 64..offset + 68]
                .copy_from_slice(&draw.part.index_count().to_le_bytes());
            resource.vertex_base + draw.part.first_index() / 6 * 4 | 0x8000_0000
        } else {
            resource.vertex_base
        };
        instances[offset + 140..offset + 144].copy_from_slice(&base.to_le_bytes());
    }
    let draw_buffer = upload(&mut device, &draws, BufferUsage::STORAGE);
    let instance_buffer = upload(&mut device, &instances, BufferUsage::STORAGE);
    let frame_buffer = upload(&mut device, &frame(Mat4::IDENTITY), BufferUsage::UNIFORM);
    let page = &meshes[&(prepared.draws[0].mesh.id(), true)];
    let (group, layout) = graphics_group(
        &mut device,
        &[
            (0, page.vertices, page.vertex_binding_bytes as u64),
            (1, draw_buffer, draws.len() as u64),
            (8, instance_buffer, instances.len() as u64),
            (9, frame_buffer, FRAME_PARAMS_STRIDE as u64),
            (12, draw_buffer, draws.len() as u64),
            (13, page.vertices, page.vertex_binding_bytes as u64),
            (14, draw_buffer, draws.len() as u64),
            (
                11,
                culling.visible_instances,
                culling.instance_capacity as u64 * 4,
            ),
        ],
    );
    let extent = Extent2d::new(16, 16).unwrap();
    let mut targets = Vec::new();
    for _ in 0..5 {
        let target = ResourceDevice::create_texture(
            &mut device,
            &TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                format: Format::Rgba8Unorm,
                usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
            },
        )
        .unwrap();
        let view = ResourceDevice::create_texture_view(
            &mut device,
            &TextureViewDescriptor {
                label: None,
                texture: target,
                base_mip_level: 0,
                mip_level_count: 1,
                format: Format::Rgba8Unorm,
            },
        )
        .unwrap();
        targets.push((target, view));
    }
    let pass = PipelineDevice::create_render_pass(
        &mut device,
        &RenderPassDescriptor {
            label: None,
            color_attachment: ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            },
            depth_attachment: None,
        },
    )
    .unwrap();
    let pipeline = PipelineDevice::create_render_pipeline(
        &mut device,
        &RenderPipelineDescriptor {
            label: None,
            vertex_shader: vs,
            vertex_entry_point: "vs_voxel_gpu".into(),
            fragment_shader: fs,
            fragment_entry_point: "fs_voxel".into(),
            vertex_buffers: vec![],
            render_pass: pass,
            pipeline_layout: Some(layout),
            color_format: Format::Rgba8Unorm,
            blend_mode: BlendMode::Replace,
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState {
                front_face: FrontFace::Ccw,
                cull_mode: Some(Face::Back),
            },
            depth_state: None,
        },
        extent,
    )
    .unwrap();
    let commands = batches
        .iter()
        .enumerate()
        .map(|(index, batch)| {
            let draw = &prepared.draws[batch.first_instance];
            let mesh = &meshes[&(draw.mesh.id(), true)];
            if mesh.procedural_quads {
                RenderCommand::DrawIndirect(DrawIndirectCommand {
                    pipeline,
                    bind_groups: bind_group_list([group]),
                    arguments: culling.arguments,
                    offset: index as u64 * ARGUMENT_STRIDE,
                    scissor: None,
                })
            } else {
                RenderCommand::DrawIndexed(DrawIndexedCommand {
                    pipeline,
                    bind_groups: bind_group_list([group]),
                    index_buffer: IndexBufferBinding {
                        buffer: mesh.indices,
                        format: mesh.index_format,
                        offset: mesh.index_offset,
                    },
                    index_count: draw.part.index_count(),
                    first_index: draw.part.first_index(),
                    base_vertex: 0,
                    instance_count: 1,
                    first_instance: batch.first_instance as u32,
                    scissor: None,
                })
            }
        })
        .collect::<Vec<_>>();
    let commands = std::iter::repeat_n(commands.as_slice(), repetitions)
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let before = DiagnosticsDevice::resource_stats(&device);
    let started = std::time::Instant::now();
    let mut expected = Vec::new();
    for ((target, view), (matrix, range, visible)) in targets.into_iter().zip([
        (Mat4::IDENTITY, None, true),
        (
            Transform {
                translation: Vec3::new(0.0, 20.0, 0.0),
                ..Transform::IDENTITY
            }
            .matrix(),
            None,
            false,
        ),
        (
            Mat4::IDENTITY,
            Aabb::new(Vec3::new(0.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
            true,
        ),
        (
            Mat4::IDENTITY,
            Aabb::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(0.0, 1.0, 1.0)),
            false,
        ),
        (Mat4::IDENTITY, None, true),
    ]) {
        ResourceDevice::write_buffer(&mut device, frame_buffer, 0, &frame(matrix)).unwrap();
        culling
            .dispatch(&mut device, cs, frame_buffer, range, None)
            .unwrap();
        // Keep graphics projection unchanged: an empty second frame must come from
        // GPU-generated zero instance count, rather than normal graphics clipping.
        ResourceDevice::write_buffer(&mut device, frame_buffer, 0, &frame(Mat4::IDENTITY)).unwrap();
        PresentationDevice::render_to_textures(
            &mut device,
            &[RenderPassCommands {
                texture_view: view,
                render_pass: pass,
                commands: &commands,
                color_load_op: LoadOp::Clear(ClearColor::default()),
                depth_attachment: None,
                clear_region: None,
                viewport: None,
            }],
        )
        .unwrap();
        expected.push((target, visible));
    }
    // Submit all five frames before the first readback so uniform/output reuse is
    // exercised with pending GPU consumers, rather than serialized by readback.
    for (target, visible) in expected {
        let pixels = TextureTransferDevice::read_texture(&mut device, target).unwrap();
        for (x, color) in [(4, [0, 255, 0, 255]), (12, [0, 0, 255, 255])] {
            assert_eq!(
                &pixels.bytes[(8 * 16 + x) * 4..][..4],
                if visible { &color } else { &[0, 0, 0, 255] }
            );
        }
        assert_eq!(
            &pixels.bytes[(2 * 16 + 8) * 4..][..4],
            &[255, 0, 255, 255],
            "indexed fallback must bypass the compacted instance list"
        );
    }
    let after = DiagnosticsDevice::resource_stats(&device);
    if repetitions > 1 {
        eprintln!(
            "indirect workload: commands_per_frame={} frames=5 elapsed_ms={:.2}",
            commands.len(),
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }
    assert_eq!(
        before.buffers, after.buffers,
        "camera-only compute must not allocate new buffers"
    );
    // Exercise contention across lanes and multiple loop iterations, then ensure
    // an empty dispatch clears the previously published workgroup totals.
    let batch = &batches[0];
    assert!(batch.instance_count >= 129);
    let visible_bounds = Aabb::new(Vec3::new(-0.2, -0.2, 0.2), Vec3::new(0.2, 0.2, 0.4)).unwrap();
    let hidden_bounds = Aabb::new(Vec3::new(10.0, 10.0, 0.2), Vec3::new(11.0, 11.0, 0.4)).unwrap();
    for draw in &mut prepared.draws {
        draw.bounds = hidden_bounds;
        draw.culling_origin = None;
    }
    let visible = &mut prepared.draws[batch.first_instance..batch.first_instance + 129];
    for draw in visible.iter_mut() {
        draw.bounds = visible_bounds;
    }
    let vertices = visible
        .iter()
        .map(|draw| draw.part.index_count())
        .max()
        .unwrap();
    culling
        .update(&mut device, &prepared.draws, &batches, None)
        .unwrap();
    for (matrix, instances, vertices) in [
        (Mat4::IDENTITY, 129, vertices),
        (
            Transform {
                translation: Vec3::new(0.0, 20.0, 0.0),
                ..Transform::IDENTITY
            }
            .matrix(),
            0,
            0,
        ),
    ] {
        ResourceDevice::write_buffer(&mut device, frame_buffer, 0, &frame(matrix)).unwrap();
        culling
            .dispatch(&mut device, cs, frame_buffer, None, None)
            .unwrap();
        assert_eq!(
            super::super::generated::tests::counts(
                &mut device,
                culling.arguments,
                batches.len() as u64 * ARGUMENT_STRIDE
            ),
            [
                vertices as u8,
                (vertices >> 8) as u8,
                instances,
                batch.first_instance as u8
            ]
        );
    }
    SubmissionDevice::wait_queue_idle(&mut device).unwrap();
    culling.destroy(&mut device).unwrap();
}

#[test]
fn culling_bounds_preserve_vertex_counts_in_padding() {
    let prepared = prepared();
    let bytes = encode_bounds(&prepared.draws);
    assert_eq!(bytes.len(), prepared.draws.len() * BOUNDS_STRIDE);
    for (draw, record) in prepared.draws.iter().zip(bytes.chunks_exact(BOUNDS_STRIDE)) {
        assert_eq!(&record[12..16], &draw.part.index_count().to_le_bytes());
        assert_eq!(&record[0..4], &draw.bounds.min.x.to_le_bytes());
        assert_eq!(&record[16..20], &draw.bounds.max.x.to_le_bytes());
        let has_origin = if draw.culling_origin.is_some() {
            1.0_f32
        } else {
            0.0
        };
        assert_eq!(record[44..48], has_origin.to_le_bytes());
    }
}
