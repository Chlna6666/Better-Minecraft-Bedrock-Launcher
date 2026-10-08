use super::*;

#[test]
#[ignore = "requires native Windows graphics hardware"]
fn command_storage_survives_submission_errors_and_pressure_reclaims_it() {
    use gfx_core::{
        BeginRenderPassDescriptor, CommandDevice as _, DrawDescriptor, MemoryTrimLevel,
        RenderTarget, ResourceId,
    };
    let mut device = open_device();
    let encoder = device
        .create_command_encoder(&CommandEncoderDescriptor { label: None })
        .expect("encoder");
    device
        .encoders
        .get_mut(encoder)
        .expect("commands")
        .reserve(64);
    let capacity = device.encoders.get(encoder).expect("commands").capacity();
    device.submit(encoder).expect("empty submission");
    assert_eq!(
        device.encoders.get(encoder).expect("commands").capacity(),
        capacity
    );
    device
        .encoders
        .get_mut(encoder)
        .expect("commands")
        .push(DrawDescriptor {
            pass: BeginRenderPassDescriptor {
                render_pass: ResourceId::new(u64::MAX),
                target: RenderTarget::TextureView(ResourceId::new(u64::MAX)),
                color_load_op: LoadOp::Clear(CLEAR),
            },
            pipeline: ResourceId::new(u64::MAX),
            resource_sets: resource_set_list([]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        });
    assert!(device.submit(encoder).is_err());
    let commands = device.encoders.get(encoder).expect("commands after error");
    assert!(commands.is_empty());
    assert_eq!(commands.capacity(), capacity);
    device.submit(encoder).expect("retry empty submission");
    device
        .trim_memory(MemoryTrimLevel::Moderate)
        .expect("idle trim");
    assert_eq!(
        device
            .encoders
            .get(encoder)
            .expect("live encoder after idle trim")
            .capacity(),
        0
    );
    device
        .encoders
        .get_mut(encoder)
        .expect("commands")
        .reserve(64);
    device
        .trim_memory(MemoryTrimLevel::Aggressive)
        .expect("pressure trim");
    assert_eq!(
        device
            .encoders
            .get(encoder)
            .expect("live encoder after trim")
            .capacity(),
        0
    );
    device
        .destroy_command_encoder(encoder)
        .expect("destroy encoder");
}

#[test]
#[ignore = "requires Windows native OpenGL 4.5 driver"]
fn viewport_scissor_fragment_coordinates_and_texture_rows_share_top_origin() {
    let mut device = open_device();
    let (target, view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        1,
    );
    let pass = pass(&mut device, false);
    let layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            resource_set_layouts: vec![],
        })
        .expect("empty layout");
    let source = "@vertex fn vs(@builtin(vertex_index) v:u32)->@builtin(position) vec4<f32>{ let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));return vec4<f32>(p[v],0.0,1.0); } @fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{if p.y<8.0{return vec4<f32>(1.0,0.0,0.0,1.0);}return vec4<f32>(0.0,0.0,1.0,1.0);}";
    let pipeline = pipeline(
        &mut device,
        source,
        pass,
        layout,
        gfx_core::BlendMode::Replace,
        false,
    );
    let draw = DrawStepDescriptor {
        pipeline,
        resource_sets: resource_set_list([]),
        vertex_count: 3,
        first_vertex: 0,
        instance_count: 1,
        first_instance: 0,
        scissor: Some(gfx_core::ScissorRect {
            x: 0,
            y: 0,
            width: 16,
            height: 4,
        }),
    };
    device
        .draw_steps_to_texture(view, pass, &[draw], LoadOp::Clear(CLEAR))
        .expect("top scissor draw");
    let readback = device.read_texture(target).expect("row readback");
    assert!(
        readback.bytes[..16 * 4 * 4]
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255]),
        "top four rows must be red"
    );
    assert!(
        readback.bytes[16 * 4 * 4..]
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 0]),
        "lower rows must remain clear"
    );
}
#[test]
#[ignore = "requires Windows native OpenGL 4.5 driver"]
fn indexed_offsets_uniform_ranges_depth_and_alpha_reach_native_pixels() {
    let mut device = open_device();
    println!("OpenGL pixel gate adapter: {}", device.adapter_name());
    let (target, view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        1,
    );
    let (_, depth) = texture(
        &mut device,
        Format::Depth32Float,
        TextureUsage::DEPTH_ATTACHMENT,
        1,
    );
    let pass = pass(&mut device, true);
    let layout = device
        .create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: None,
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 119,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
                ResourceSetLayoutEntry {
                    binding: 120,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
            ],
        })
        .expect("layout");
    let pipeline_layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            resource_set_layouts: vec![layout],
        })
        .expect("pipeline layout");
    let pipeline = pipeline(
        &mut device,
        WGSL,
        pass,
        pipeline_layout,
        gfx_core::BlendMode::Alpha,
        true,
    );
    let vertices = buffer(&mut device, 64, BufferUsage::STORAGE);
    // Non-zero byte range plus shader-pulled vertex/instance offsets exercise the native ABI.
    device
        .write_buffer(
            vertices,
            16,
            &floats(&[
                -1.0, -1.0, 0.0, 0.0, 3.0, -1.0, 0.0, 0.0, -1.0, 3.0, 0.0, 0.0,
            ]),
        )
        .expect("vertices");
    let indices = buffer(&mut device, 16, BufferUsage::INDEX);
    device
        .write_buffer(
            indices,
            0,
            &([99u32, 0, 1, 2]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()),
        )
        .expect("indices");
    let constants = buffer(&mut device, 512, BufferUsage::UNIFORM);
    let set = device
        .create_resource_set(&ResourceSetDescriptor {
            label: None,
            layout,
            bindings: vec![
                ResourceBinding {
                    binding: 119,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: vertices,
                        offset: 16,
                        size: 48,
                        stride: Some(16),
                    }),
                },
                ResourceBinding {
                    binding: 120,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: constants,
                        offset: 256,
                        size: 32,
                        stride: None,
                    }),
                },
            ],
        })
        .expect("offset bindings");
    let steps = [RenderStepDescriptor::DrawIndexed(
        DrawIndexedStepDescriptor {
            pipeline,
            resource_sets: resource_set_list([set]),
            index_buffer: IndexBufferBinding {
                buffer: indices,
                format: IndexFormat::Uint32,
                offset: 0,
            },
            index_count: 3,
            first_index: 1,
            base_vertex: 3,
            instance_count: 1,
            first_instance: 2,
            scissor: None,
        },
    )];
    device
        .write_buffer(
            constants,
            256,
            &floats(&[0.0, 1.0, 0.0, 1.0, 0.2, 0.0, 0.0, 0.0]),
        )
        .expect("near green");
    device
        .render_steps_to_texture_compat(
            view,
            pass,
            &steps,
            LoadOp::Clear(CLEAR),
            Some(RenderPassDepthAttachment {
                target: depth,
                depth_load_op: LoadOp::Clear(1.0),
            }),
        )
        .expect("near draw");
    device
        .write_buffer(
            constants,
            256,
            &floats(&[1.0, 0.0, 0.0, 1.0, 0.8, 0.0, 0.0, 0.0]),
        )
        .expect("far red");
    device
        .render_steps_to_texture_compat(
            view,
            pass,
            &steps,
            LoadOp::Load,
            Some(RenderPassDepthAttachment {
                target: depth,
                depth_load_op: LoadOp::Load,
            }),
        )
        .expect("occluded draw");
    let result = device.read_texture(target).expect("native GPU readback");
    assert!(
        result
            .bytes
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 255, 0, 255]),
        "depth/offset output: {:?}",
        &result.bytes[..16]
    );
    // A partial uniform update must retain the previously uploaded depth field.
    device
        .write_buffer(constants, 272, &floats(&[0.1]))
        .expect("partial depth update");
    device
        .write_buffer(constants, 256, &floats(&[0.0, 0.0, 1.0, 0.5]))
        .expect("partial color update");
    device
        .render_steps_to_texture_compat(
            view,
            pass,
            &steps,
            LoadOp::Load,
            Some(RenderPassDepthAttachment {
                target: depth,
                depth_load_op: LoadOp::Load,
            }),
        )
        .expect("alpha draw");
    let result = device.read_texture(target).expect("alpha readback");
    assert!(
        result.bytes.chunks_exact(4).all(|p| p[0] == 0
            && (127..=128).contains(&p[1])
            && (127..=128).contains(&p[2])
            && p[3] == 255),
        "alpha output: {:?}",
        &result.bytes[..16]
    );
}
