#![cfg(target_vendor = "apple")]

use gfx_core::*;
use gfx_metal::MetalDevice;
use gfx_shader::{MslVersion, WgslModule};

const DRAW: &str = r#"
struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) layer: u32,
}
@vertex fn vs_main(@builtin(vertex_index) vertex: u32,
                   @builtin(instance_index) instance: u32) -> Varying {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: Varying;
    out.position = vec4<f32>(positions[vertex], 0.0, 1.0);
    out.layer = instance;
    return out;
}
@group(0) @binding(0) var layers: texture_2d_array<f32>;
@fragment fn fs_main(in: Varying) -> @location(0) vec4<f32> {
    return textureLoad(layers, vec2<i32>(0, 0), i32(in.layer), 0);
}
"#;

const GENERATE: &str = r#"
@group(0) @binding(0) var<storage, read_write> args: array<u32, 9>;
@compute @workgroup_size(1) fn generate() {
    // DrawPrimitives: vertex count, instance count, vertex start, base instance.
    args[0] = 3u; args[1] = 1u; args[2] = 0u; args[3] = 1u;
    // DrawIndexedPrimitives: index count, instance count, index start,
    // signed base vertex (bit representation), base instance.
    args[4] = 3u; args[5] = 1u; args[6] = 0u; args[7] = 0u; args[8] = 0u;
}
"#;

const GENERATE_COUNTED: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32, 23>;
@compute @workgroup_size(1) fn generate_counted() {
    // Prefix padding tests a nonzero, four-byte-aligned indirect offset.
    data[0] = 0u;
    data[1] = 3u; data[2] = 1u; data[3] = 0u; data[4] = 0u;
    data[5] = 3u; data[6] = 1u; data[7] = 0u; data[8] = 1u;
    // Indexed commands use base instances 0 and 1 and the index binding's
    // nonzero offset. Their packed ABI begins at byte 36.
    data[9] = 3u; data[10] = 1u; data[11] = 0u; data[12] = 0u; data[13] = 0u;
    data[14] = 3u; data[15] = 1u; data[16] = 0u; data[17] = 0u; data[18] = 1u;
    data[19] = 0u; data[20] = 1u; data[21] = 2u; data[22] = 99u;
}
"#;

fn shader(device: &mut MetalDevice, source: &str, stage: ShaderStage, entry: &str) -> ShaderModuleId {
    let binary = WgslModule::parse(source).expect("WGSL")
        .compile_msl_versioned(stage, entry, MslVersion::V2_0).expect("reflected MSL");
    if let ShaderCode::Msl(source) = &binary.code {
        eprintln!("NOVA_GFX_METAL_MSL entry={entry}\n{source}");
    }
    device.create_shader_module(&ShaderModuleDescriptor { label: Some(entry.into()), binary })
        .expect("native Metal shader")
}

#[test]
fn gpu_generated_indirect_draws_select_array_layers_without_cpu_readback() {
    let mut device = match MetalDevice::new(&DeviceDescriptor::default()) {
        Ok(device) => device,
        Err(Error::Unavailable(reason)) => {
            eprintln!("NOVA_GFX_SKIP=metal unavailable: {reason}");
            return;
        }
        Err(error) => panic!("Metal initialization: {error}"),
    };
    let one = Extent2d::new(1, 1).unwrap();
    let extent = Extent2d::new(8, 8).unwrap();
    let format = Format::Rgba8Unorm;
    let array = device.create_texture(&TextureDescriptor {
        label: None, size: one, format, mip_level_count: 1, sample_count: 1,
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2Array { layers: 2 },
    }).expect("array texture");
    for (layer, pixel) in [(0, [255u8, 0, 0, 255]), (1, [0u8, 255, 0, 255])] {
        device.write_texture_array_layer(TextureArrayLayerWriteDescriptor {
            texture: array, upload: TextureArrayLayerUpload {
                layer, mip_level: 0, origin: Origin2d::ZERO, extent: one,
                layout: TextureDataLayout::new(0, 4, 1).unwrap(),
            },
        }, &pixel).expect("upload array layer");
    }
    let array_view = device.create_texture_view(&TextureViewDescriptor {
        label: None, texture: array, format, base_mip_level: 0, mip_level_count: 1,
    }).expect("array view");
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None, entries: vec![BindGroupLayoutEntry {
            binding: 0, binding_type: BindingType::SampledTexture, stages: ShaderStages::FRAGMENT,
        }],
    }).unwrap();
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None, layout, entries: vec![BindGroupEntry {
            binding: 0, resource: BindingResource::Texture(TextureBinding { texture_view: array_view }),
        }],
    }).unwrap();
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: None, bind_group_layouts: vec![layout],
    }).unwrap();
    let target = device.create_texture(&TextureDescriptor {
        label: None, size: extent, format, mip_level_count: 1, sample_count: 1,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2,
    }).unwrap();
    let target_view = device.create_texture_view(&TextureViewDescriptor {
        label: None, texture: target, format, base_mip_level: 0, mip_level_count: 1,
    }).unwrap();
    let render_pass = device.create_render_pass(&RenderPassDescriptor {
        label: None, color_attachments: vec![ColorAttachmentDescriptor { format }],
        depth_attachment: None, sample_count: 1, resolve_target: None,
    }).unwrap();
    let vs = shader(&mut device, DRAW, ShaderStage::Vertex, "vs_main");
    let fs = shader(&mut device, DRAW, ShaderStage::Fragment, "fs_main");
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: None, vertex_shader: vs, vertex_entry_point: "vs_main".into(),
        fragment_shader: fs, fragment_entry_point: "fs_main".into(),
        vertex_buffers: vec![], render_pass, pipeline_layout: Some(pipeline_layout),
        color_targets: vec![ColorTargetState::new(format, BlendMode::Replace)],
        primitive_topology: PrimitiveTopology::TriangleList, raster: RasterState::default(),
        depth_state: None,
    }, extent).expect("render pipeline");
    let args = device.create_buffer(&BufferDescriptor {
        label: None, size: 36,
        usage: BufferUsage::INDIRECT | BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        memory_location: MemoryLocation::GpuOnly,
    }).unwrap();
    let indices = device.create_buffer(&BufferDescriptor {
        label: None, size: 16, usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    }).unwrap();
    let index_bytes: Vec<u8> = [99u32, 0, 1, 2].into_iter().flat_map(u32::to_le_bytes).collect();
    device.write_buffer(indices, 0, &index_bytes).expect("private index upload");
    // First isolate direct array reads without any compute producer in flight.
    // Isolate attachment store, texture layer upload / shader binding, and
    // indirect command generation. The original GPU-only draw remains tested.
    let clear = LoadOp::Clear(ClearColor { red: 0.0, green: 0.0, blue: 1.0, alpha: 1.0 });
    let verify = |device: &mut MetalDevice, label: &str, draws: &[RenderCommand], expected: [u8; 4]| {
        let submission = device.render_to_textures(&[RenderPassCommands {
            color_attachments: &[target_view], render_pass, commands: draws,
            color_load_op: clear, clear_region: None, depth_attachment: None,
            viewport: None, resolve_target: None,
        }]).expect("record native render").expect("tracked Metal render submission");
        // Check the draw's own command buffer before issuing a readback; on a
        // GPU fault the latter can otherwise hide the producer's native error.
        // This is a test-only wait after the entire producer/consumer sequence.
        device.wait_submission(submission).unwrap_or_else(|error| panic!("{label}: {error}"));
        let pixels = device.read_texture(target).expect("read rendered pixels");
        assert_eq!(&pixels.bytes[(4 * 8 + 4) * 4..(4 * 8 + 4) * 4 + 4], &expected, "{label}");
    };
    verify(&mut device, "clear/store", &[], [0, 0, 255, 255]);
    for (layer, expected) in [(0, [255, 0, 0, 255]), (1, [0, 255, 0, 255])] {
        verify(&mut device, "direct array layer", &[RenderCommand::Draw(DrawCommand {
            pipeline, bind_groups: bind_group_list([group]), vertex_count: 3,
            first_vertex: 0, instance_count: 1, first_instance: layer, scissor: None,
        })], expected);
    }
    eprintln!("NOVA_GFX_METAL_DIRECT_ARRAY_DRAW=pass");
    let compute = shader(&mut device, GENERATE, ShaderStage::Compute, "generate");
    device.dispatch_compute(&ComputeDispatch {
        shader: compute,
        bindings: &[ComputeBufferBinding {
            binding: 0, access: ComputeBufferAccess::ReadWrite,
            buffer: BufferBinding { buffer: args, offset: 0, size: 36, stride: None },
        }],
        workgroups: [1, 1, 1],
    }).expect("GPU writes native indirect arguments");
    // Deliberately bypass conservative capability advertisement. Never read
    // the GPU-generated argument buffer back to the CPU.
    for (draw, expected) in [
        (RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline, bind_groups: bind_group_list([group]), arguments: args, offset: 0, scissor: None,
        }), [0u8, 255, 0, 255]),
        (RenderCommand::DrawIndexedIndirect(DrawIndexedIndirectCommand {
            pipeline, bind_groups: bind_group_list([group]), arguments: args, offset: 16,
            index_buffer: IndexBufferBinding { buffer: indices, format: IndexFormat::Uint32, offset: 4 },
            scissor: None,
        }), [255u8, 0, 0, 255]),
    ] {
        let label = match &draw {
            RenderCommand::DrawIndirect(_) => "GPU non-indexed indirect",
            _ => "GPU indexed indirect with nonzero offset",
        };
        verify(&mut device, label, &[draw], expected);
    }
    eprintln!("NOVA_GFX_METAL_GPU_DRIVEN_ARRAY_DRAW=pass");

    if std::env::var_os("NOVA_GFX_METAL_ICB").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    assert!(device.indirect_draw_count_limit().is_some(), "requested experimental ICB is unavailable");
    let counted_args = device.create_buffer(&BufferDescriptor {
        label: Some("Metal GPU counted draw test".into()), size: 92,
        usage: BufferUsage::INDIRECT | BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        memory_location: MemoryLocation::GpuOnly,
    }).unwrap();
    let generate = shader(&mut device, GENERATE_COUNTED, ShaderStage::Compute, "generate_counted");
    device.dispatch_compute(&ComputeDispatch {
        shader: generate,
        bindings: &[ComputeBufferBinding {
            binding: 0, access: ComputeBufferAccess::ReadWrite,
            buffer: BufferBinding { buffer: counted_args, offset: 0, size: 92, stride: None },
        }],
        workgroups: [1, 1, 1],
    }).unwrap();
    let draws = [
        RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline, bind_groups: bind_group_list([group]), arguments: counted_args, offset: 4, scissor: None,
        }),
        RenderCommand::DrawIndexedIndirect(DrawIndexedIndirectCommand {
            pipeline, bind_groups: bind_group_list([group]), arguments: counted_args, offset: 36,
            index_buffer: IndexBufferBinding { buffer: indices, format: IndexFormat::Uint32, offset: 4 },
            scissor: None,
        }),
    ];
    for (draw, stride) in draws.iter().zip([16, 20]) {
        let pass = RenderPassCommands {
            color_attachments: &[target_view], render_pass, commands: std::slice::from_ref(draw),
            color_load_op: clear, clear_region: None, depth_attachment: None, viewport: None, resolve_target: None,
        };
        let count = |word| IndirectCountBinding {
            command_index: 0, count_buffer: counted_args, count_offset: word * 4,
            max_draw_count: 2, argument_stride: stride,
        };
        // Reject a count whose argument range crosses the buffer boundary.
        let invalid = IndirectCountBinding { max_draw_count: 32, ..count(21) };
        assert!(device.render_to_textures_counted_profiled(&[pass], &[&[invalid]], &[None]).is_err());
        // A later draw overwrites the first, so the pixel proves count and
        // ordered base-instance execution. Oversized counts clamp on GPU.
        for (word, expected) in [
            (19, [0u8, 0, 255, 255]), (20, [255, 0, 0, 255]),
            (21, [0, 255, 0, 255]), (22, [0, 255, 0, 255]),
        ] {
            let submitted = device.render_to_textures_counted_profiled(&[pass], &[&[count(word)]], &[None])
                .expect("native GPU counted ICB").expect("tracked submission");
            device.wait_submission(submitted).expect("counted ICB completed");
            let pixels = device.read_texture(target).expect("read ICB output");
            assert_eq!(&pixels.bytes[(4 * 8 + 4) * 4..(4 * 8 + 4) * 4 + 4], &expected,
                "Metal ICB stride={stride} count word={word}");
        }
    }
    eprintln!("NOVA_GFX_METAL_GPU_COUNTED_ICB=pass");
}
