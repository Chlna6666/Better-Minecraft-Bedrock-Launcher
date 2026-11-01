//! Headless production-shader checks for conservative occlusion and stale-depth removal.
use super::*;

fn verify<D: Device + TextureTransferDevice>(mut device: D) {
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_transform(Transform {
                    translation: Vec3::new(0., 0., 0.7),
                    scale: Vec3::new(0.1, 0.1, 0.1),
                    ..Transform::IDENTITY
                }),
        )
        .unwrap();
    let mut prepared = PreparedScene::default();
    prepared.set_gpu_culling(true);
    prepared
        .update(
            &scene,
            Camera::perspective(Vec3::new(0., 0., 2.), Vec3::ZERO, Vec3::Y, 1., 0.1, 10.),
            1.,
        )
        .unwrap();
    let batches = vec![super::super::super::scene::DrawBatchRange {
        first_instance: 0,
        instance_count: 1,
    }];
    let mut culling = CullResources::create(
        &mut device,
        &prepared.draws,
        &batches,
        D::BACKEND_KIND,
        None,
    )
    .unwrap();
    let cs = shader(&mut device, ShaderStage::Compute, "cs_cull");
    let eye = Vec3::new(0., 0., -1.);
    let mut bytes = frame(Mat4::IDENTITY);
    for (dst, value) in bytes[64..76].chunks_exact_mut(4).zip([eye.x, eye.y, eye.z]) {
        dst.copy_from_slice(&value.to_le_bytes());
    }
    let frame_buffer = upload(&mut device, &bytes, BufferUsage::UNIFORM);
    let solid = |min: [f32; 3], max: [f32; 3]| {
        let bounds = Aabb::new(
            Vec3::new(min[0], min[1], min[2]),
            Vec3::new(max[0], max[1], max[2]),
        )
        .unwrap();
        crate::Occluder { bounds, faces: 63 }
    };
    let front = solid([-0.8, -0.8, 0.2], [0.8, 0.8, 0.3]);
    let count = prepared.draws[0].part.index_count();
    for (name, boxes, bias, offset, visible) in [
        ("covered", vec![front], 0., Vec2::ZERO, false),
        (
            "unrendered entry face",
            vec![crate::Occluder { faces: 32, ..front }],
            0.,
            Vec2::ZERO,
            true,
        ),
        ("removed", vec![], 0., Vec2::ZERO, true),
        (
            "side opening",
            vec![solid([0.2, -0.8, 0.2], [0.8, 0.8, 0.3])],
            0.,
            Vec2::ZERO,
            true,
        ),
        (
            "near intersection",
            vec![solid([-0.8, -0.8, -0.1], [0.8, 0.8, 0.3])],
            0.,
            Vec2::ZERO,
            true,
        ),
        (
            "self",
            vec![solid([-0.05, -0.05, 0.65], [0.05, 0.05, 0.75])],
            0.,
            Vec2::ZERO,
            true,
        ),
        (
            "partially covered",
            vec![solid([-0.005, -0.8, 0.2], [0.005, 0.8, 0.3])],
            0.,
            Vec2::ZERO,
            true,
        ),
        ("depth bias", vec![front], -0.6, Vec2::ZERO, true),
        ("pixel offset", vec![front], 0., Vec2::new(16., 0.), true),
        ("covered again", vec![front], 0., Vec2::ZERO, false),
    ] {
        prepared.draws[0].depth_bias = bias;
        prepared.draws[0].pixel_offset = offset;
        culling
            .update(&mut device, &prepared.draws, &batches, None)
            .unwrap();
        culling
            .hiz
            .update(
                &mut device,
                boxes.into_iter(),
                eye,
                Vec3::Z,
                name != "covered again",
            )
            .unwrap();
        culling
            .dispatch(&mut device, cs, frame_buffer, None, None)
            .unwrap();
        let actual =
            super::super::super::generated::tests::counts(&mut device, culling.arguments, 16);
        assert_eq!(
            actual,
            if visible {
                [count as u8, (count >> 8) as u8, 1, 0]
            } else {
                [0; 4]
            },
            "{name}"
        );
    }
    culling.destroy(&mut device).unwrap();
    ResourceDevice::destroy_buffer(&mut device, frame_buffer).unwrap();
    PipelineDevice::destroy_shader_module(&mut device, cs).unwrap();
}

#[test]
#[ignore = "requires Windows D3D11 hardware; headless Hi-Z occlusion"]
fn hiz_dx11() {
    verify(Dx11Device::new(&DeviceDescriptor::default()).unwrap());
}

#[test]
#[ignore = "requires Vulkan hardware; headless Hi-Z occlusion"]
fn hiz_vulkan() {
    verify(gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap());
}

#[test]
#[ignore = "requires Windows D3D12 hardware; headless Hi-Z occlusion"]
fn hiz_dx12() {
    verify(gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap());
}
