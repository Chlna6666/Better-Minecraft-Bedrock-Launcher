//! Sloped receivers must not shadow themselves or accumulate darkness across frames.
use super::*;

pub(super) fn verify_shadow_coverage<D: Device + TextureTransferDevice>(
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
    material.double_sided = true;
    let material = Arc::new(material);
    let mesh = Arc::new(Mesh::plane(512., 512.).unwrap());
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
        Camera::orthographic(Vec3::new(0., 4., 0.), Vec3::ZERO, Vec3::Z, 240., 0.1, 1000.),
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
    let pixels = draw(device, renderer, context, &view, target, depth);
    let row = (0..32)
        .map(|x| pixels[(32 * 64 + x) * 4])
        .collect::<Vec<_>>();
    let outside = row[0];
    let inside = row[31];
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
    assert_eq!(
        darkened, 0,
        "unobstructed planar receiver self-shadows: pixels={darkened}, max_delta={maximum}"
    );
    let repeated = draw(device, renderer, context, &shadowed, target, depth);
    assert_eq!(
        pixels, repeated,
        "static shadows must not accumulate darkness"
    );
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
    let tiled = draw(
        device,
        renderer,
        context,
        &shadowed.with_scene(Arc::new(tiled)),
        target,
        depth,
    );
    let maximum = tiled
        .iter()
        .zip(&pixels)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        maximum <= 5,
        "splitting a receiver into tiles must not introduce shadow seams: max_delta={maximum}"
    );
}
