#![cfg(target_os = "windows")]

use gfx_core::*;
use gfx_dx11::Dx11Device;
use windows::{
    Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCompile},
    core::PCSTR,
};

fn compile_shader_binary(
    source: &str,
    stage: ShaderStage,
    entry: &str,
) -> ShaderBinary {
    let module = gfx_shader::WgslModule::parse(source).expect("WGSL");
    let ShaderCode::Hlsl(hlsl) = module.compile_hlsl_dx11(stage, entry).expect("HLSL").code else {
        panic!("HLSL");
    };
    let profile: &[u8] = match stage {
        ShaderStage::Vertex => b"vs_5_0\0",
        ShaderStage::Fragment => b"ps_5_0\0",
        ShaderStage::Compute => b"cs_5_0\0",
    };
    let entry_c = std::ffi::CString::new(entry).expect("entry");
    let (mut code, mut errors) = (None, None);
    // SAFETY: Source and entry/profile byte strings remain live for this compiler call.
    let compiled = unsafe {
        D3DCompile(
            hlsl.as_ptr().cast(),
            hlsl.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry_c.as_ptr().cast()),
            PCSTR(profile.as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = compiled {
        // SAFETY: Compiler diagnostics own the returned immutable byte range.
        let message = errors.map(|blob| unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                blob.GetBufferPointer().cast(),
                blob.GetBufferSize(),
            ))
            .into_owned()
        });
        panic!("{error}: {message:?}\n{hlsl}");
    }
    let code = code.expect("bytecode");
    // SAFETY: Copy live compiler bytecode before releasing its COM blob.
    let bytes = unsafe {
        std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()).to_vec()
    };
    ShaderBinary::dx_bytecode(stage, entry, bytes)
}

fn shader(
    device: &mut Dx11Device,
    source: &str,
    stage: ShaderStage,
    entry: &str,
) -> ShaderModuleId {
    device
        .create_shader_module(&ShaderModuleDescriptor {
            label: None,
            binary: compile_shader_binary(source, stage, entry),
        })
        .expect("native shader")
}

fn buffer(device: &mut Dx11Device, size: u64, usage: BufferUsage) -> BufferId {
    device
        .create_buffer(&BufferDescriptor {
            label: None,
            size,
            usage,
            memory_location: MemoryLocation::GpuOnly,
        })
        .expect("buffer")
}

const COMPUTE: &str = r"
@group(0) @binding(0) var<storage, read_write> vertices: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> args: array<vec4<u32>>;
@group(0) @binding(2) var<uniform> count: vec4<u32>;
@group(0) @binding(3) var<storage, read> source: array<vec4<f32>>;
@compute @workgroup_size(1) fn cs(@builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) grid: vec3<u32>) {
    if (group.x != 0u || group.y != 0u) { return; }
    vertices[0] = source[0]; vertices[1] = source[1]; vertices[2] = source[2];
    args[1] = vec4<u32>(3u, count.x, grid.x + 1u, grid.y);
    args[2] = vec4<u32>(3u, count.x, 0u, 0u);
}
";

const GRAPHICS: &str = r"
@group(0) @binding(0) var<storage, read> vertices: array<vec4<f32>>;
@vertex fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> @builtin(position) vec4<f32> {
    if (i != 2u || v < 3u || v > 5u) { return vec4<f32>(4.0, 4.0, 0.0, 1.0); }
    return vertices[v - 3u];
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 1.0, 0.0, 1.0); }
";

#[test]
#[ignore = "requires Windows D3D11 hardware; renders only to an offscreen texture"]
fn compute_vertices_and_indirect_offsets_reach_pixels() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("hardware device");
    assert!(device.compute_capabilities().indirect_draw);
    let vertices = buffer(
        &mut device,
        48,
        BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
    );
    let args = buffer(
        &mut device,
        48,
        BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
    );
    let count = buffer(
        &mut device,
        16,
        BufferUsage::UNIFORM | BufferUsage::COPY_DST,
    );
    let source = buffer(
        &mut device,
        48,
        BufferUsage::STORAGE | BufferUsage::COPY_DST,
    );
    let positions = [
        -1.0_f32, -1.0, 0.0, 1.0, 3.0, -1.0, 0.0, 1.0, -1.0, 3.0, 0.0, 1.0,
    ];
    device
        .write_buffer(
            source,
            0,
            &positions
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .expect("source upload");
    let cs = shader(&mut device, COMPUTE, ShaderStage::Compute, "cs");
    let vs = shader(&mut device, GRAPHICS, ShaderStage::Vertex, "vs");
    let fs = shader(&mut device, GRAPHICS, ShaderStage::Fragment, "fs");
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            }],
        })
        .expect("layout");
    let group = device
        .create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: vertices,
                    offset: 0,
                    size: 48,
                    stride: None,
                }),
            }],
        })
        .expect("group");
    let pipeline_layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: vec![layout],
        })
        .expect("pipeline layout");
    let extent = Extent2d::new(16, 16).expect("extent");
    let target = device
        .create_texture(&TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })
        .expect("target");
    let view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture: target,
            format: Format::Rgba8Unorm,
            base_mip_level: 0,
            mip_level_count: 1,
        })
        .expect("view");
    let pass = device
        .create_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: None,

            sample_count: 1,
            resolve_target: None,

        })
        .expect("pass");
    let mut pipeline_desc = RenderPipelineDescriptor {
        label: None,
        vertex_shader: vs,
        vertex_entry_point: "vs".into(),
        fragment_shader: fs,
        fragment_entry_point: "fs".into(),
        vertex_buffers: vec![],
        render_pass: pass,
        pipeline_layout: Some(pipeline_layout),
        color_targets: vec![ColorTargetState::new(Format::Rgba8Unorm, BlendMode::Replace)],
        primitive_topology: PrimitiveTopology::TriangleList,
        raster: Default::default(),
        depth_state: None,
    };
    let pipeline = device
        .create_render_pipeline(&pipeline_desc, extent)
        .expect("pipeline");
    let bindings = [
        ComputeBufferBinding {
            binding: 0,
            buffer: BufferBinding {
                buffer: vertices,
                offset: 0,
                size: 48,
                stride: None,
            },
            access: ComputeBufferAccess::ReadWrite,
        },
        ComputeBufferBinding {
            binding: 1,
            buffer: BufferBinding {
                buffer: args,
                offset: 0,
                size: 48,
                stride: None,
            },
            access: ComputeBufferAccess::ReadWrite,
        },
        ComputeBufferBinding {
            binding: 2,
            buffer: BufferBinding {
                buffer: count,
                offset: 0,
                size: 16,
                stride: None,
            },
            access: ComputeBufferAccess::Uniform,
        },
        ComputeBufferBinding {
            binding: 3,
            buffer: BufferBinding {
                buffer: source,
                offset: 0,
                size: 48,
                stride: None,
            },
            access: ComputeBufferAccess::ReadOnly,
        },
    ];
    let mut commands = vec![RenderCommand::DrawIndirect(DrawIndirectCommand {
        pipeline,
        bind_groups: bind_group_list([group]),
        arguments: args,
        offset: 16,
        scissor: Some(ScissorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }),
    })];
    // Include a skipped draw and cross the 256-slot page boundary. The last draw
    // has different base offsets and is clipped by GRAPHICS; it must not overwrite
    // the valid offsets used by earlier draws during parameter preparation.
    for index in 0..258 {
        commands.push(RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline,
            bind_groups: bind_group_list([group]),
            arguments: args,
            offset: if index % 2 == 0 { 16 } else { 32 },
            scissor: None,
        }));
    }
    for enabled in [1_u32, 0, 1] {
        device
            .write_buffer(count, 0, &enabled.to_le_bytes())
            .expect("count upload");
        device
            .dispatch_compute(&ComputeDispatch {
                shader: cs,
                bindings: &bindings,
                workgroups: [2, 2, 1],
            })
            .expect("dispatch");
        device
            .render_to_textures(&[RenderPassCommands {
                color_attachments: std::slice::from_ref(&view),
                render_pass: pass,
                commands: &commands,
                color_load_op: LoadOp::Clear(ClearColor::default()),
                depth_attachment: None,
                clear_region: None,
                viewport: None,
            resolve_target: None,

        }])
            .expect("indirect draw");
        let pixels = device.read_texture(target).expect("pixels");
        assert_eq!(
            &pixels.bytes[(8 * 16 + 8) * 4..][..4],
            if enabled == 1 {
                &[0, 255, 0, 255]
            } else {
                &[0, 0, 0, 255]
            }
        );
    }
    for reversed in [false, true] {
        let positions = if reversed {
            [
                -1.0_f32, -1.0, 0.0, 1.0, -1.0, 3.0, 0.0, 1.0, 3.0, -1.0, 0.0, 1.0,
            ]
        } else {
            positions
        };
        device
            .write_buffer(
                source,
                0,
                &positions
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        device
            .dispatch_compute(&ComputeDispatch {
                shader: cs,
                bindings: &bindings,
                workgroups: [2, 2, 1],
            })
            .unwrap();
        for front_face in [FrontFace::Ccw, FrontFace::Cw] {
            for cull_mode in [None, Some(Face::Back), Some(Face::Front)] {
                pipeline_desc.raster = RasterState {
                    front_face,
                    cull_mode,
                };
                let raster_pipeline = device
                    .create_render_pipeline(&pipeline_desc, extent)
                    .unwrap();
                let commands = [RenderCommand::DrawIndirect(DrawIndirectCommand {
                    pipeline: raster_pipeline,
                    bind_groups: bind_group_list([group]),
                    arguments: args,
                    offset: 16,
                    scissor: None,
                })];
                device
                    .render_to_textures(&[RenderPassCommands {
                        color_attachments: std::slice::from_ref(&view),
                        render_pass: pass,
                        commands: &commands,
                        color_load_op: LoadOp::Clear(ClearColor::default()),
                        depth_attachment: None,
                        clear_region: None,
                        viewport: None,
            resolve_target: None,

        }])
                    .unwrap();
                let is_front = (front_face == FrontFace::Ccw) != reversed;
                let visible = match cull_mode {
                    None => true,
                    Some(Face::Back) => is_front,
                    Some(Face::Front) => !is_front,
                };
                let pixels = device.read_texture(target).unwrap();
                assert_eq!(
                    &pixels.bytes[(8 * 16 + 8) * 4..][..4],
                    if visible {
                        &[0, 255, 0, 255]
                    } else {
                        &[0, 0, 0, 255]
                    },
                    "reversed={reversed} front={front_face:?} cull={cull_mode:?}"
                );
                device.destroy_render_pipeline(raster_pipeline).unwrap();
            }
        }
    }
    device.wait_queue_idle().expect("completion");
    device
        .destroy_render_pipeline(pipeline)
        .expect("pipeline cleanup");
    device.destroy_render_pass(pass).expect("pass cleanup");
    device.destroy_texture_view(view).expect("view cleanup");
    device.destroy_texture(target).expect("target cleanup");
    device.destroy_bind_group(group).expect("group cleanup");
    device
        .destroy_pipeline_layout(pipeline_layout)
        .expect("pipeline layout cleanup");
    device
        .destroy_bind_group_layout(layout)
        .expect("layout cleanup");
    for id in [cs, vs, fs] {
        device.destroy_shader_module(id).expect("shader cleanup");
    }
    for id in [vertices, args, count, source] {
        device.destroy_buffer(id).expect("buffer cleanup");
    }
}


/// Native hardware gate for mixed resource-set compute. The GPUI extension
/// path uses exactly this API; no CPU emulation or staging draw is involved.
#[test]
#[ignore = "requires Windows D3D11 hardware and a native texture readback"]
fn compute_resource_set_writes_storage_image_and_buffer() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("D3D11 device");
    let data = buffer(
        &mut device, 16,
        BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
    );
    let image = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent2d::new(16, 16).unwrap(),
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::STORAGE | TextureUsage::COPY_SRC,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
        sample_count: 1,
    }).expect("storage image");
    let image_view = device.create_texture_view(&TextureViewDescriptor {
        label: None,
        texture: image,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
    }).expect("storage image UAV");
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None,
        entries: vec![
            BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageBufferReadWrite,
                stages: ShaderStages::COMPUTE,
            },
            BindGroupLayoutEntry {
                binding: 1,
                binding_type: BindingType::StorageTexture {
                    format: Format::Rgba8Unorm,
                    read_only: false,
                },
                stages: ShaderStages::COMPUTE,
            },
        ],
    }).expect("compute-only layout");
    let set = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout,
        entries: vec![
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: data, offset: 0, size: 16, stride: None,
                }),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: image_view,
                }),
            },
        ],
    }).expect("native compute UAV bind group");
    let source = r#"
@group(0) @binding(0) var<storage, read_write> words: array<vec4<u32>>;
@group(0) @binding(1) var target: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(1)
fn cs(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x == 0u && id.y == 0u) {
        words[0] = vec4<u32>(1u, 2u, 3u, 4u);
    }
    textureStore(target, vec2<i32>(id.xy), vec4<f32>(0.0, 1.0, 0.0, 1.0));
}
"#;
    let compute = shader(&mut device, source, ShaderStage::Compute, "cs");
    assert!(device.compute_bind_group_capabilities().supports(
        &BindGroupLayoutDescriptor {
            label: None,
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::StorageBufferReadWrite,
                    stages: ShaderStages::COMPUTE,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    binding_type: BindingType::StorageTexture {
                        format: Format::Rgba8Unorm, read_only: false,
                    },
                    stages: ShaderStages::COMPUTE,
                },
            ],
        }
    ));
    device.dispatch_compute_bind_group(&ComputeBindGroupDispatch {
        shader: compute,
        bind_group: set,
        workgroups: [16, 16, 1],
    }).expect("native compute dispatch");
    let pixels = device.read_texture(image).expect("GPU texture readback");
    let probe = (8 * 16 + 8) * 4;
    assert_eq!(&pixels.bytes[probe..probe + 4], &[0, 255, 0, 255]);
    device.wait_queue_idle().expect("GPU completion");
}

#[test]
#[ignore = "requires D3D11 hardware and native GPU readback"]
fn compute_uav_to_srv_to_storage_image_roundtrip() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("D3D11 device");
    let writer = compile_shader_binary(
        gfx_core::conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
        ShaderStage::Compute,
        "write_data",
    );
    let reader = compile_shader_binary(
        gfx_core::conformance::COMPUTE_RESOURCE_SET_READER_WGSL,
        ShaderStage::Compute,
        "write_image",
    );
    gfx_core::conformance::compute_resource_set_roundtrip(&mut device, writer, reader)
        .expect("native D3D11 UAV/SRV image visibility and readback");
}
