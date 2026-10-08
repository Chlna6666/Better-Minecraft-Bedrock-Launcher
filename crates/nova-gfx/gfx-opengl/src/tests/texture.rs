use super::*;

#[test]
#[ignore = "requires Windows native OpenGL 4.5 driver"]
fn sampled_mip_upload_and_fixed_sampler_reach_native_pixels() {
    let mut device = open_device();
    let (target, view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        1,
    );
    let (image, _) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        2,
    );
    let image_view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture: image,
            format: Format::Rgba8Unorm,
            base_mip_level: 0,
            mip_level_count: 2,
        })
        .expect("mip SRV");
    let mut pixels = vec![0; 8];
    pixels.extend([32u8, 64, 192, 255].repeat(8 * 8));
    let upload = TextureWriteDescriptor {
        texture: image,
        mip_level: 1,
        origin: Origin2d::default(),
        size: Extent2d::new(8, 8).expect("mip extent"),
        layout: TextureDataLayout::new(8, 32, 8).expect("pitch"),
    };
    device
        .write_texture(upload, &pixels)
        .expect("offset mip upload");
    let sampler = device
        .create_sampler(&SamplerDescriptor::default())
        .expect("sampler");
    let layout = device
        .create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: None,
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 3,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        })
        .expect("sample layout");
    let set = device
        .create_resource_set(&ResourceSetDescriptor {
            label: None,
            layout,
            bindings: vec![
                ResourceBinding {
                    binding: 3,
                    resource: BindingResource::Texture(TextureBinding {
                        texture_view: image_view,
                    }),
                },
                ResourceBinding {
                    binding: 4,
                    resource: BindingResource::Sampler(SamplerBinding { sampler }),
                },
            ],
        })
        .expect("sample set");
    let layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            resource_set_layouts: vec![layout],
        })
        .expect("pipeline layout");
    let source = "@group(0) @binding(3) var image: texture_2d<f32>; @group(0) @binding(4) var image_sampler: sampler; @vertex fn vs(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> { let points = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0)); return vec4<f32>(points[v],0.0,1.0); } @fragment fn fs() -> @location(0) vec4<f32> { return textureSampleLevel(image, image_sampler, vec2<f32>(0.5),1.0); }";
    let pass = pass(&mut device, false);
    let pipeline = pipeline(
        &mut device,
        source,
        pass,
        layout,
        gfx_core::BlendMode::Replace,
        false,
    );
    device
        .draw_steps_to_texture(
            view,
            pass,
            &[DrawStepDescriptor {
                pipeline,
                resource_sets: resource_set_list([set]),
                vertex_count: 3,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }],
            LoadOp::Clear(CLEAR),
        )
        .expect("sample draw");
    let result = device.read_texture(target).expect("sample readback");
    assert!(
        result
            .bytes
            .chunks_exact(4)
            .all(|pixel| pixel == [32, 64, 192, 255]),
        "sample output: {:?}",
        &result.bytes[..16]
    );
}
