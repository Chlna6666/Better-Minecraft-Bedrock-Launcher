//! Compare production compute AO descriptors with the CPU rule, including packed classes.
use super::*;

fn verify<D: Device + TextureTransferDevice>(mut device: D) {
    let shaders = create_shaders(&mut device, D::BACKEND_KIND).unwrap();
    let template = crate::voxel_mesh::tests::source()
        .templates
        .iter()
        .find(|face| face.neighbour == [0, 1, 0])
        .unwrap()
        .clone();
    let corners = template
        .vertices
        .map(|v| [v.position.x, v.position.y, v.position.z]);
    let positions = [[0, 2, 1], [1, 2, 0]];
    let expected = crate::voxel_ambient_light([0, 1, 0], corners, |p| {
        positions.contains(&p.map(|v| v + 1))
    })
    .map(|v| (v * 255.0).round() as u8);
    for opaque in [true, false] {
        for packed in [true, false] {
            let mut classes = vec![0; 27];
            for [x, y, z] in positions {
                classes[((z * 3 + y) * 3 + x) as usize] = if opaque { 16 } else { 2 };
            }
            if !packed {
                classes[0] = 256;
            }
            let source =
                crate::VoxelMeshSource::new([3; 3], crate::Vec3::ZERO, vec![0; 27], vec![])
                    .unwrap()
                    .with_instances(
                        classes,
                        vec![template.clone()],
                        vec![crate::VoxelMeshInstance {
                            position: [1; 3],
                            faces: 0..1,
                            height: 1.0,
                            occlusion_mask: 1,
                        }],
                    )
                    .unwrap()
                    .with_ambient_occlusion(16);
            let cpu = std::array::from_fn(|i| {
                (source.fallback().vertices()[i].color[0] * 255.0).round() as u8
            });
            assert_eq!(cpu, if opaque { expected } else { [255; 4] });
            let mesh = Mesh::from_gpu_voxels(source);
            let resources = create(&mut device, &mesh, &shaders).unwrap();
            assert_eq!(
                tests::counts(
                    &mut device,
                    resources.generated.as_ref().unwrap().arguments,
                    16
                )[0],
                6
            );
            let actual = tests::probe(
                &mut device,
                resources.vertices,
                resources.vertex_binding_bytes as u64,
                "fs_mesh_ao_check",
            );
            assert_eq!(actual, cpu, "packed={packed}, opaque={opaque}");
        }
    }
    let mesh = Mesh::from_gpu_voxels(crate::voxel_mesh::tests::source());
    let resources = create(&mut device, &mesh, &shaders).unwrap();
    // The graphics helper exercises a three-entry culling/argument mapping.
    let bytes = [60u32, 1, 0, 0]
        .repeat(3)
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let arguments = ResourceDevice::create_buffer(
        &mut device,
        &BufferDescriptor {
            label: None,
            size: bytes.len() as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
        },
    )
    .unwrap();
    ResourceDevice::write_buffer(&mut device, arguments, 0, &bytes).unwrap();
    tests::render_generated(
        &mut device,
        &mesh,
        &resources,
        arguments,
        [0, 255, 0, 255],
        crate::DistanceFog::new([0.0, 1.0, 0.0], 0.0, 0.001),
        None,
    );
    let mut probes = [[0u8; 4]; 2];
    for (index, sky_channel) in [false, true].into_iter().enumerate() {
        let source = crate::voxel_mesh::tests::source();
        let levels = (0..36)
            .map(|i| {
                if i % 4 >= 2 {
                    0xf0
                } else if sky_channel {
                    0x70
                } else {
                    7
                }
            })
            .collect();
        let source = source.with_lighting(levels, 0.2).unwrap();
        // Smooth per-corner lighting interpolates vertex colors across each quad, so
        // the rendered probe must land inside the CPU corner range instead of matching
        // one exact per-face value.
        let mut min = 255u8;
        let mut max = 0u8;
        for vertex in source.fallback().vertices() {
            let value = (vertex.color[0] * 255.).round() as u8;
            min = min.min(value);
            max = max.max(value);
        }
        let mesh = Mesh::from_gpu_voxels(source);
        let resources = create(&mut device, &mesh, &shaders).unwrap();
        let pixels = tests::probe_generated(
            &mut device,
            &mesh,
            &resources,
            arguments,
            [max, max, max, 255],
            None,
            None,
            false,
        );
        assert!(
            pixels[0][0] >= min && pixels[0][0] <= max,
            "sky_channel={sky_channel} pixel {} outside CPU corner range [{min},{max}]",
            pixels[0][0]
        );
        probes[index] = pixels[0];
    }
    // The sky nibble adds light (cells with 0xf0 carry sky 15), so counting the sky
    // channel must never darken the probe and should lift it above the ambient floor.
    assert!(
        probes[1][0] >= probes[0][0],
        "sky channel must not darken: {:?} vs {:?}",
        probes[1],
        probes[0]
    );
    assert!(
        probes[1][0] >= 95,
        "sky nibble should contribute: {:?}",
        probes
    );
    let source = crate::voxel_mesh::tests::source()
        .with_lighting(vec![0x70; 36], 0.2)
        .unwrap();
    let mesh = Mesh::from_gpu_voxels(source);
    let resources = create(&mut device, &mesh, &shaders).unwrap();
    for (illumination, display, strength, expected) in [
        (
            crate::VoxelIllumination::Unlit,
            crate::VoxelDisplay::VertexColor,
            0.,
            255,
        ),
        (
            crate::VoxelIllumination::Vertex,
            crate::VoxelDisplay::Textured,
            0.,
            51,
        ),
        (
            crate::VoxelIllumination::Sunlight,
            crate::VoxelDisplay::Textured,
            1.,
            85,
        ),
        (
            crate::VoxelIllumination::Unlit,
            crate::VoxelDisplay::Wireframe,
            0.,
            0,
        ),
        (
            crate::VoxelIllumination::Sunlight,
            crate::VoxelDisplay::VertexColor,
            1.,
            20,
        ),
    ] {
        let style = crate::VoxelShading {
            color_space: crate::TextureColorSpace::Srgb,
            illumination,
            display,
            sky_brightness: 0.,
            sun_direction: -crate::Vec3::Z,
            sun_intensity: 1.,
            shadow_strength: strength,
            ..crate::VoxelShading::default()
        };
        tests::render_generated(
            &mut device,
            &mesh,
            &resources,
            arguments,
            [expected, expected, expected, 255],
            None,
            Some((style, [expected, expected, expected, 255])),
        );
    }
    let linear_style = crate::VoxelShading {
        color_space: crate::TextureColorSpace::Linear,
        illumination: crate::VoxelIllumination::Sunlight,
        sky_brightness: 0.,
        sun_direction: -crate::Vec3::Z,
        sun_intensity: 1.,
        shadow_strength: 1.,
        ..crate::VoxelShading::default()
    };
    tests::render_generated(
        &mut device,
        &mesh,
        &resources,
        arguments,
        [165, 165, 165, 255],
        None,
        Some((linear_style, [165, 165, 165, 255])),
    );
    let face = crate::voxel_mesh::tests::source()
        .templates
        .into_iter()
        .find(|face| face.neighbour == [0, 0, -1])
        .unwrap();
    let mut left = face.clone();
    for vertex in &mut left.vertices {
        vertex.position.x -= 1.;
    }
    let source = crate::VoxelMeshSource::new(
        [4, 3, 3],
        crate::Vec3::new(-2., -1., -1.),
        vec![0; 36],
        vec![],
    )
    .unwrap()
    .with_static_faces(vec![left, face])
    .unwrap()
    .with_lighting(vec![0x70; 36], 0.2)
    .unwrap();
    let mesh = Mesh::from_gpu_voxels(source);
    let resources = create(&mut device, &mesh, &shaders).unwrap();
    ResourceDevice::write_buffer(
        &mut device,
        arguments,
        0,
        &[12u32, 1, 0, 0]
            .repeat(3)
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    tests::render_generated(
        &mut device,
        &mesh,
        &resources,
        arguments,
        [95, 95, 95, 255],
        None,
        Some((crate::VoxelShading::default(), [95, 95, 95, 255])),
    );
    SubmissionDevice::wait_queue_idle(&mut device).unwrap();
}

#[test]
#[ignore = "requires Windows D3D11 hardware; headless AO descriptor readback"]
fn voxel_ao_dx11() {
    verify(gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap());
}

#[test]
#[ignore = "requires Vulkan hardware; headless AO descriptor readback"]
fn voxel_ao_vulkan() {
    verify(gfx_vulkan::VulkanDevice::new(&DeviceDescriptor::default()).unwrap());
}

#[test]
#[ignore = "requires Windows D3D12 hardware; headless light/AO/fog validation"]
fn voxel_lighting_dx12() {
    verify(gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap());
}
