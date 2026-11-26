//! Sloped receivers must not shadow themselves or accumulate darkness across frames.
use super::*;

pub(super) fn verify_shadow_coverage<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
) {
    // The last CSM receiver split is 296 world units, with a 16-unit
    // fade-out. A 240-unit viewport cannot reach that boundary at all.
    // Cover both sides and resolve the fade with enough native pixels.
    let extent = Extent2d::new(256, 256).unwrap();
    let target = Target::create(device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(device, extent, Format::Depth32Float, true, 1).unwrap();
    let context = RendererExtensionContext::for_target(
        D::BACKEND_KIND, context.render_pass(), Format::Rgba8Unorm, extent);
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.base_color = [0.18, 0.18, 0.18, 1.];
    material.roughness = 1.;
    material.double_sided = true;
    let material = Arc::new(material);
    let mesh = Arc::new(Mesh::plane(1024., 1024.).unwrap());
    for height in [0., 20.] {
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([material.clone()])
                    .with_transform(crate::Transform {
                        translation: Vec3::new(0., height, 0.),
                        ..crate::Transform::IDENTITY
                    }),
            )
            .unwrap();
    }
    // The canopy is behind the main camera but covers the entire shadow map.
    let view = SceneView::new(
        Arc::new(scene),
        Camera::orthographic(Vec3::new(0., 4., 0.), Vec3::ZERO, Vec3::Z, 680., 0.1, 1000.),
    )
    .with_voxel_shader()
    .with_voxel_shading(VoxelShading {
        illumination: VoxelIllumination::Sunlight,
        sun_direction: Vec3::Y,
        sun_intensity: 1.,
        shadow_strength: 1.,
        ..VoxelShading::default()
    })
    .unwrap()
    .with_screen_effects(Some(ScreenEffects {
        resolution_scale: 1.,
        reflections: false,
        water: false,
        clouds: false,
        volumetric_fog: false,
        ..ScreenEffects::default()
    }))
    .unwrap();
    let pixels = draw(device, renderer, &context, &view, &target, &depth);
    let row = (0..128)
        .map(|x| pixels[(128 * 256 + x) * 4])
        .collect::<Vec<_>>();
    let outside = row[0];
    let inside = row[127];
    eprintln!("NOVA_CSM_COVERAGE outside={outside} inside={inside} row={row:?}");
    target.destroy(device).unwrap();
    depth.destroy(device).unwrap();
    assert!(
        outside > inside + 30,
        "coverage fixture needs a shadow: {row:?}"
    );
    let transitional = row
        .iter()
        .filter(|v| **v > inside + 5 && **v < outside - 5)
        .count();
    assert!(
        transitional >= 3,
        "finite shadow coverage must fade instead of creating a hard light boundary: {row:?}"
    );
}

pub(super) fn verify_receiver_planes<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    target: &Target,
    depth: &Target,
) {
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.base_color = [0.18, 0.18, 0.18, 1.];
    material.roughness = 1.;
    let material = Arc::new(material);
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::plane(80., 80.).unwrap()))
                .with_materials([material.clone()]),
        )
        .unwrap();
    let view = SceneView::new(
        Arc::new(scene),
        Camera::perspective(Vec3::new(0., 16., 20.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.),
    )
    .with_voxel_shader()
    .with_voxel_shading(VoxelShading {
        illumination: VoxelIllumination::Sunlight,
        sun_direction: Vec3::new(0.4, 0.8, 0.3).normalized().unwrap(),
        sun_intensity: 1.,
        shadow_strength: 0.,
        ..VoxelShading::default()
    })
    .unwrap()
    .with_screen_effects(Some(ScreenEffects {
        resolution_scale: 1.,
        reflections: false,
        water: false,
        clouds: false,
        volumetric_fog: false,
        ..ScreenEffects::default()
    }))
    .unwrap();
    let reference = draw(device, renderer, context, &view, target, depth);
    let shadowed = view
        .with_voxel_shading(VoxelShading {
            shadow_strength: 1.,
            ..view.voxel_shading
        })
        .unwrap();
    let pixels = draw(device, renderer, context, &shadowed, target, depth);
    let mut darkened = 0;
    let mut maximum = 0;
    for y in 12..52 {
        for x in 12..52 {
            let at = (y * 64 + x) * 4;
            let delta = reference[at].saturating_sub(pixels[at]);
            maximum = maximum.max(delta);
            darkened += usize::from(delta > 5);
        }
    }
    if D::BACKEND_KIND == BackendKind::Vulkan {
        let state = &renderer.scenes[&shadowed.id];
        if ResourceDevice::sample_zero_position(device, 4).is_some() {
            assert_eq!(state.msaa_sample_count, 4, "standard Vulkan MSAA fixture must exercise 4x depth");
        }
        // The host composite writes sample zero of opaque depth into the
        // single-sampled destination. This fixture is entirely opaque, so it
        // is also the surface depth. Never copy an MSAA image to a buffer:
        // Vulkan requires the source of vkCmdCopyImageToBuffer to be single-sampled.
        let receiver = device.read_texture(depth.texture).unwrap();
        let read_depth = |bytes: &[u8], pitch: u32, x: usize, y: usize| {
            let offset = y * pitch as usize + x * 4;
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };
        let x = 32;
        let y = 32;
        let depth_value = read_depth(&receiver.bytes, receiver.bytes_per_row, x, y);
        let inverse = (shadowed.camera.projection_matrix(1.).unwrap()
            * shadowed.camera.view().unwrap()).inverse().unwrap();
        let reconstruct = |sample: [f32; 2]| inverse.transform_point(Vec3::new(
            (x as f32 + sample[0]) / 64.0 * 2.0 - 1.0,
            1.0 - (y as f32 + sample[1]) / 64.0 * 2.0,
            depth_value,
        ));
        let sample = ResourceDevice::sample_zero_position(device, state.msaa_sample_count).unwrap();
        let p = reconstruct(sample);
        let center = reconstruct([0.5, 0.5]);
        eprintln!("NOVA_CSM_RECEIVER samples={} sample_zero={sample:?} depth={depth_value} world={p:?} center_world={center:?} reference={:?} shadowed={:?}",
            state.msaa_sample_count, &reference[(y * 64 + x) * 4..][..4], &pixels[(y * 64 + x) * 4..][..4]);
        if darkened != 0 || p.y.abs() >= 0.01 {
            for (cascade, target) in state.shadow.iter().enumerate() {
                let map = device.read_texture(target.texture).unwrap();
                let q = state.shadow_matrix[cascade].transform_point(p);
                let width = map.size.width() as usize;
                let height = map.size.height() as usize;
                let sx = ((q.x * 0.5 + 0.5) * width as f32) as usize;
                let sy = ((0.5 - q.y * 0.5) * height as f32) as usize;
                let sx = sx.min(width - 1);
                let sy = sy.min(height - 1);
                let mut neighborhood = Vec::with_capacity(9);
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        neighborhood.push(read_depth(&map.bytes, map.bytes_per_row,
                            sx.saturating_add_signed(dx).min(width - 1),
                            sy.saturating_add_signed(dy).min(height - 1)));
                    }
                }
                eprintln!("NOVA_CSM_CASCADE={cascade} clip={q:?} texel=({sx},{sy}) depths={neighborhood:?}");
            }
        }
        assert!(p.y.abs() < 0.01, "sample-zero depth must reconstruct the Y=0 receiver: {p:?}");
    }
    assert_eq!(
        darkened, 0,
        "unobstructed planar receiver self-shadows: pixels={darkened}, max_delta={maximum}"
    );
    let repeated = draw(device, renderer, context, &shadowed, target, depth);
    assert_eq!(
        pixels, repeated,
        "static shadows must not accumulate darkness"
    );
    // Compare the complete image as before, but retain the legal resolved
    // opaque depth so a failure can distinguish coverage from CSM visibility.
    // Other backends do not necessarily expose depth readback (Metal rejects
    // it explicitly). The existing image assertion is backend-independent.
    let single_depth = if D::BACKEND_KIND == BackendKind::Vulkan {
        Some(device.read_texture(depth.texture).unwrap())
    } else {
        None
    };
    let mut tiled = Scene::new();
    let mesh = Arc::new(Mesh::plane(20., 20.).unwrap());
    for x in 0..4 {
        for z in 0..4 {
            tiled
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_materials([material.clone()])
                        .with_transform(crate::Transform {
                            translation: Vec3::new(x as f32 * 20. - 30., 0., z as f32 * 20. - 30.),
                            ..crate::Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
    }
    let tiled_view = shadowed.with_scene(Arc::new(tiled));
    let tiled = draw(device, renderer, context, &tiled_view, target, depth);
    if D::BACKEND_KIND == BackendKind::Vulkan {
        assert!(renderer.scenes[&tiled_view.id].scene.test_has_remapped_standard_draw(),
            "the retained tiled receiver must exercise non-identity PBR draw slots");
    }
    let maximum = tiled
        .iter()
        .zip(&pixels)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    if maximum > 5 && let Some(single_depth) = single_depth {
        renderer.scenes[&tiled_view.id].scene.trace_test_geometry();
        let tiled_depth = device.read_texture(depth.texture).unwrap();
        let tiled_unshadowed = tiled_view.with_voxel_shading(VoxelShading {
            shadow_strength: 0.,
            ..tiled_view.voxel_shading
        }).unwrap();
        let tiled_reference = draw(device, renderer, context, &tiled_unshadowed, target, depth);
        let no_hiz = draw(device, renderer, context, &tiled_unshadowed.with_culling_disabled(), target, depth);
        let mut fresh_renderer = EffectsRenderer::new(device, context.clone(), tiled_view.screen_effects.unwrap()).unwrap();
        let fresh = draw(device, &mut fresh_renderer, context, &tiled_unshadowed, target, depth);
        let delta_count = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4))
            .filter(|(a,b)| a.iter().zip(*b).any(|(a,b)| a.abs_diff(*b)>5)).count();
        eprintln!("NOVA_CSM_TILED_ISOLATE no_hiz_changed={} fresh_changed={} fresh_vs_single={}",
            delta_count(&no_hiz,&tiled_reference),delta_count(&fresh,&tiled_reference),delta_count(&fresh,&reference));
        let read = |image: &TextureReadback, x: usize, y: usize| {
            let offset = y * image.bytes_per_row as usize + x * 4;
            f32::from_le_bytes(image.bytes[offset..offset + 4].try_into().unwrap())
        };
        let mut changed = 0usize;
        let mut coverage_changed = 0usize;
        let mut baseline_changed = 0usize;
        let mut tiled_self_shadow = 0usize;
        let mut largest = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let at = (y * 64 + x) * 4;
                let rgba = &pixels[at..at+4];
                let split = &tiled[at..at+4];
                let original = &reference[at..at+4];
                let unshadowed = &tiled_reference[at..at+4];
                let delta = rgba.iter().zip(split).map(|(a,b)| a.abs_diff(*b)).max().unwrap();
                let a = read(&single_depth, x, y);
                let b = read(&tiled_depth, x, y);
                coverage_changed += usize::from((a >= 1.) != (b >= 1.));
                baseline_changed += usize::from(original.iter().zip(unshadowed).any(|(a,b)| a.abs_diff(*b) > 5));
                tiled_self_shadow += usize::from(unshadowed[0].saturating_sub(split[0]) > 5);
                if delta > 5 {
                    changed += 1;
                    largest.push((delta,x,y,a,b,rgba,split,original,unshadowed));
                }
            }
        }
        largest.sort_unstable_by_key(|p| std::cmp::Reverse(p.0));
        eprintln!("NOVA_CSM_TILED changed={changed} coverage_changed={coverage_changed} baseline_changed={baseline_changed} tiled_self_shadow={tiled_self_shadow}");
        for (delta,x,y,a,b,single,split,original,unshadowed) in largest.into_iter().take(12) {
            eprintln!("NOVA_CSM_TILED_PIXEL ({x},{y}) delta={delta} single_depth={a} tiled_depth={b} single={single:?} tiled={split:?} single_unshadowed={original:?} tiled_unshadowed={unshadowed:?}");
        }
    }
    assert!(
        maximum <= 5,
        "splitting a receiver into tiles must not introduce shadow seams: max_delta={maximum}"
    );
}
