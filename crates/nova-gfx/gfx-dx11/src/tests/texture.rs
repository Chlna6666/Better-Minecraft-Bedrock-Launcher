use super::*;

#[test]
#[ignore = "requires Windows hardware D3D11 feature level 11.0"]
fn sampled_mip_upload_and_fixed_sampler_reach_native_pixels() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("hardware DX11 device");
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
        .create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: vec![
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
            ],
        })
        .expect("sample layout");
    let set = device
        .create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Texture(TextureBinding {
                        texture_view: image_view,
                    }),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::Sampler(SamplerBinding { sampler }),
                },
            ],
        })
        .expect("sample set");
    let layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: vec![layout],
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
        .render_to_textures(&[RenderPassCommands {
            color_attachments: std::slice::from_ref(&view),
            render_pass: pass,
            commands: &[RenderCommand::Draw(DrawCommand {
                pipeline,
                bind_groups: bind_group_list([set]),
                vertex_count: 3,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            })],
            color_load_op: LoadOp::Clear(CLEAR),
            clear_region: None,
            depth_attachment: None,
            viewport: None,
            resolve_target: None,

        }])
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
/// Hardware-only regression: verify that a D2Array owns independent
/// layer/mip storage while exposing one sampled texture view.
#[test]
#[ignore = "requires native graphics hardware; D2Array layer upload and view"]
fn sampled_texture_array_uses_distinct_native_subresources() {
    use gfx_core::{TextureArrayLayerUpload, TextureArrayLayerWriteDescriptor};
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("D3D11");
    let caps = device.texture_array_capabilities();
    assert!(caps.array_resources && caps.layer_uploads);
    let desc = TextureDescriptor {
        label: Some("native layered texture test".into()),
        size: Extent2d::new(8, 8).unwrap(),
        mip_level_count: 2,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2Array { layers: 3 },
            sample_count: 1,
        };
    let texture = device.create_texture(&desc).unwrap();
    let view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture,
            base_mip_level: 0,
            mip_level_count: 2,
            format: Format::Rgba8Unorm,
        })
        .unwrap();
    let image = [10, 45, 180, 255].repeat(4 * 4);
    let layer_write = TextureArrayLayerWriteDescriptor {
        texture,
        upload: TextureArrayLayerUpload {
            layer: 2,
            mip_level: 1,
            origin: Origin2d::ZERO,
            extent: Extent2d::new(4, 4).unwrap(),
            layout: TextureDataLayout::new(0, 16, 4).unwrap(),
        },
    };
    device
        .write_texture_array_layer(layer_write, &image)
        .unwrap();
    assert!(
        device
            .write_texture(
                TextureWriteDescriptor {
                    texture,
                    mip_level: 1,
                    origin: Origin2d::ZERO,
                    size: Extent2d::new(4, 4).unwrap(),
                    layout: TextureDataLayout::new(0, 16, 4).unwrap(),
                },
                &image,
            )
            .is_err(),
        "2D uploads must not alias layer zero of a D2Array"
    );
    assert_eq!(layer_write.subresource_index(2).unwrap(), 5);
    device.destroy_texture_view(view).unwrap();
    device.destroy_texture(texture).unwrap();
}
