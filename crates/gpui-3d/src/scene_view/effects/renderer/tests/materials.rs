//! Material transmission, cloud/alpha shadows and failed-shadow retry.
use super::*;

pub(super) fn verify_material_reflections<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    base: &SceneView,
    target: &Target,
    depth: &Target,
) {
    let mut red = Vec::new();
    for (roughness, metallic, color) in [(1., 0., 0.), (0.08, 0., 0.), (0.9, 1., 0.8)] {
        let mut material = Material::new();
        material.roughness = roughness;
        material.metallic = metallic;
        material.base_color = [color, color, color, 1.];
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let view = base
            .with_scene(Arc::new(scene))
            .with_screen_effects(Some(ScreenEffects {
                reflections: true,
                background_color: Some([1., 0., 1.]),
                ..base.screen_effects.unwrap()
            }))
            .unwrap();
        let pixels = draw(device, renderer, context, &view, target, depth);
        red.push(pixels[(32 * 64 + 32) * 4]);
    }
    assert!(
        red[0] <= 4 && red[1] > red[0] + 20 && red[2] > red[1] + 30,
        "rough dielectrics, polished dielectrics and rough metals need distinct environment responses: {red:?}"
    );
}

pub(super) fn verify_hidden_lines<D: Device + TextureTransferDevice>(
    device: &mut D,
    context: &RendererExtensionContext,
    base: &SceneView,
    target: &Target,
    depth: &Target,
) {
    let view = base
        .with_screen_effects(None)
        .unwrap()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Unlit,
            display: crate::VoxelDisplay::Wireframe,
            ..VoxelShading::default()
        })
        .unwrap();
    // The public renderer composites a separate sidecar color target and
    // deliberately does not write GPUI's depth attachment. Exercise the same
    // production geometry pipeline directly on the depth image under test.
    let mut resources = crate::scene_view::resources::RendererResources::new(device, context).unwrap();
    let mut scene = crate::scene_view::scene::SceneResources::new(device, D::BACKEND_KIND).unwrap();
    scene.prepare(device, &mut resources, &view, context).unwrap();
    let mut commands = Vec::new();
    scene.append_steps(&resources, view.voxel_shader, &mut commands).unwrap();
    assert!(!commands.is_empty(), "wireframe geometry must produce draw commands");
    PresentationDevice::render_to_textures(device, &[RenderPassCommands {
        color_attachments: &[target.view],
        render_pass: context.render_pass(),
        commands: &commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view, depth_load_op: LoadOp::Clear(1.), stencil_load_op: None,
        }),
        clear_region: None, viewport: None, resolve_target: None,
    }]).unwrap();
    let pixels = TextureTransferDevice::read_texture(device, target.texture).unwrap().bytes;
    let values = TextureTransferDevice::read_texture(device, depth.texture).unwrap();
    let at = (32 * 64 + 32) * 4;
    let z = f32::from_le_bytes(values.bytes[at..at + 4].try_into().unwrap());
    assert!(z < 1., "wireframe interiors must occlude deeper edges: {z}");
    assert!(
        pixels.chunks_exact(4).any(|pixel| pixel[0] > 80),
        "visible quad edges must remain"
    );
    scene.destroy(device).unwrap();
    resources.destroy(device).unwrap();
}

pub(super) fn verify_water_animation<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    base: &SceneView,
    target: &Target,
    depth: &Target,
) {
    let strip =
        Arc::new(crate::TextureAsset::rgba8(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap());
    let mut material = Material::new();
    material.surface = crate::SurfaceKind::Water;
    material.alpha_mode = AlphaMode::Blend;
    material.albedo_texture = Some(strip.id());
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(material)]),
        )
        .unwrap();
    let view = base
        .with_scene(Arc::new(scene))
        .with_textures([strip.clone()])
        .unwrap()
        .with_screen_effects(Some(ScreenEffects {
            water: true,
            water_texture: Some(crate::FlipbookTexture {
                texture: strip.id(),
                seconds_per_frame: 0.5,
            }),
            background_color: Some([0.; 3]),
            ..base.screen_effects.unwrap()
        }))
        .unwrap();
    let mut colors = Vec::new();
    for milliseconds in [0, 600] {
        renderer.epoch = context.frame_time() - std::time::Duration::from_millis(milliseconds);
        let pixels = draw(device, renderer, context, &view, target, depth);
        colors.push(pixels[(32 * 64 + 32) * 4..][..4].to_vec());
    }
    assert!(
        colors[0][0] > colors[0][2] + 20 && colors[1][2] > colors[1][0] + 20,
        "water must play both strip frames in the resolve: {colors:?}"
    );
    let mut commands = Vec::new();
    renderer
        .render(&view, device, context.clone(), &mut commands)
        .unwrap();
    let mut passes = Vec::new();
    renderer.drain_render_passes(&mut passes);
    assert_eq!(
        passes.len(),
        1,
        "water animation must reuse the geometry capture"
    );
}

pub(super) fn verify_sky_transmission<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
    target: &Target,
    depth: &Target,
) {
    let mut material = Material::new();
    material.alpha_mode = AlphaMode::Blend;
    material.surface = crate::SurfaceKind::Glass;
    material.base_color = [1., 1., 1., 0.1];
    let mut scene = Scene::new();
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(material)]),
        )
        .unwrap();
    let view = view
        .with_scene(Arc::new(scene))
        .with_screen_effects(Some(ScreenEffects {
            background_color: Some([0.03, 0.12, 0.4]),
            ..view.screen_effects.unwrap()
        }))
        .unwrap();
    let pixels = draw(device, renderer, context, &view, target, depth);
    let center = &pixels[(32 * 64 + 32) * 4..][..4];
    assert!(
        center[2] > 140 && center[2] > center[0] + 40,
        "transmission must sample the sky when no opaque background exists: {center:?}"
    );
    let values = TextureTransferDevice::read_texture(device, depth.texture).unwrap();
    let at = (32 * 64 + 32) * 4;
    let z = f32::from_le_bytes(values.bytes[at..at + 4].try_into().unwrap());
    assert_eq!(
        z, 1.,
        "transparent surfaces must retain opaque depth for overlays"
    );
}

pub(super) fn verify_shadow_retry(
    device: &mut dyn ExtensionDevice,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
) {
    let state = renderer.scenes.get_mut(&view.id).unwrap();
    // Fail after the projection key changes, as can also happen during shadow encoding.
    let mut invalid = view.clone();
    invalid.voxel_shading.sun_direction = Vec3::ZERO;
    invalid.voxel_shading.shadow_strength = 1.;
    state.shadow_valid.fill(true);
    for _ in 0..2 {
        let mut passes = Vec::new();
        assert!(
            state
                .shadow_pass(
                    device,
                    &renderer.shadow_resources,
                    &invalid,
                    context,
                    context.render_pass(),
                    &mut passes
                )
                .is_err()
        );
        assert!(
            state.shadow_valid.iter().all(|&valid| !valid),
            "failed shadows must remain eligible for retry"
        );
        assert!(passes.is_empty());
    }
    let invalid = view.with_camera(Camera {
        eye: view.camera.target,
        ..view.camera
    });
    let mut steps = Vec::new();
    assert!(
        renderer
            .render(&invalid, device, context.clone(), &mut steps)
            .is_err()
    );
    assert!(
        !renderer.scenes[&view.id].targets_valid,
        "failed scene preparation must invalidate the capture before mutating resources"
    );
}

pub(super) fn verify_shadows<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    target: &Target,
    depth: &Target,
) {
    let opaque = Arc::new(crate::TextureAsset::linear_rgba8(2, 1, vec![255; 8]).unwrap());
    let cutout = Arc::new(
        crate::TextureAsset::linear_rgba8(2, 1, vec![255, 255, 255, 255, 255, 255, 255, 0])
            .unwrap(),
    );
    let mut counts = Vec::new();
    for texture in [opaque.clone(), cutout] {
        let mut scene = Scene::new();
        let mut material = Material::new();
        material.alpha_mode = AlphaMode::Mask;
        material.albedo_texture = Some(texture.id());
        material.double_sided = true;
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::plane(24., 24.).unwrap()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let view = SceneView::new(
            Arc::new(scene),
            Camera::perspective(Vec3::new(0., 20., 0.), Vec3::ZERO, Vec3::Z, 1., 0.1, 100.),
        )
        .with_textures([texture])
        .unwrap()
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
            volumetric_fog: false,
            clouds: false,
            ..ScreenEffects::default()
        }))
        .unwrap();
        draw(device, renderer, context, &view, target, depth);
        let shadow = renderer.scenes[&view.id].shadow[0].texture;
        let values = TextureTransferDevice::read_texture(device, shadow).unwrap();
        counts.push(
            values
                .bytes
                .chunks_exact(4)
                .filter(|bytes| {
                    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) < 0.999
                })
                .count(),
        );
        if counts.len() == 1 {
            let clear =
                Arc::new(crate::TextureAsset::linear_rgba8(1, 1, vec![255, 255, 255, 0]).unwrap());
            let mut luminance = Vec::new();
            let solid = Arc::new(crate::TextureAsset::linear_rgba8(1, 1, vec![255; 4]).unwrap());
            for cloud in [clear, solid] {
                let settings = ScreenEffects {
                    clouds: true,
                    cloud_texture: Some(cloud.id()),
                    cloud_height: 64.,
                    ..view.screen_effects.unwrap()
                };
                let cloudy = view
                    .with_textures([opaque.clone(), cloud])
                    .unwrap()
                    .with_screen_effects(Some(settings))
                    .unwrap();
                let pixels = draw(device, renderer, context, &cloudy, target, depth);
                luminance.push(pixels[(32 * 64 + 32) * 4]);
            }
            // The output target contains the ACES-fit + sRGB display value,
            // not linear radiance. Undo both transforms before measuring
            // extinction; a fixed byte delta depends on exposure and tone
            // mapping compression and overstates the required cloud shadow.
            let decode_radiance = |byte: u8| {
                assert!(byte < 255, "cloud shadow fixture must not saturate");
                let srgb = f32::from(byte) / 255.0;
                let mapped = if srgb <= 0.04045 {
                    srgb / 12.92
                } else {
                    ((srgb + 0.055) / 1.055).powf(2.4)
                };
                let a = 2.51 - 2.43 * mapped;
                let b = 0.03 - 0.59 * mapped;
                let c = -0.14 * mapped;
                (-b + (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
            };
            let clear = decode_radiance(luminance[0]);
            let solid = decode_radiance(luminance[1]);
            assert!(clear > 0.0, "cloud shadow fixture must receive sunlight");
            // A fully covered 4-block slab at vertical incidence attenuates
            // direct radiance by exp(-4 * 0.09). Ambient radiance is not
            // attenuated, so total contrast is smaller. Require at least
            // half the predicted direct contrast and reject over-extinction
            // beyond a small readback/quantization tolerance.
            let expected = 1.0 - (-4.0_f32 * 0.09).exp();
            let contrast = (clear - solid) / clear;
            eprintln!("NOVA_CLOUD_SHADOW display={luminance:?} linear=[{clear}, {solid}] contrast={contrast}");
            assert!(
                contrast >= expected * 0.5 && contrast <= expected + 0.04,
                "cloud alpha must cast a physically bounded ground shadow: {luminance:?}, linear contrast={contrast}"
            );
        }
    }
    assert!(
        counts[0] > 100 && counts[1] > 0 && counts[1] * 4 < counts[0] * 3,
        "masked texels must leave holes in the real geometry shadow: {counts:?}"
    );
}
