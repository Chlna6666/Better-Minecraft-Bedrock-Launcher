//! Material transmission, cloud/alpha shadows and failed-shadow retry.
use super::*;

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
    state.shadow_valid = true;
    for _ in 0..2 {
        let mut passes = Vec::new();
        assert!(
            state
                .shadow_pass(
                    device,
                    &renderer.shadow_resources,
                    &invalid,
                    context,
                    &mut passes
                )
                .is_err()
        );
        assert!(
            !state.shadow_valid,
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
        let shadow = renderer.scenes[&view.id].shadow.texture;
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
            assert!(
                luminance[0] > luminance[1] + 25,
                "cloud alpha must cast a material-dependent ground shadow: {luminance:?}"
            );
        }
    }
    assert!(
        counts[0] > 100 && counts[1] > 0 && counts[1] * 4 < counts[0] * 3,
        "masked texels must leave holes in the real geometry shadow: {counts:?}"
    );
}
