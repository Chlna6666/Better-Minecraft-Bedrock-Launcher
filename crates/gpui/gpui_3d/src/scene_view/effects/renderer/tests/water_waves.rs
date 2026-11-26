//! Water waves share a world-space phase across origin rebasing and cached captures.
use super::*;

pub(super) fn verify_water_waves<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    target: &Target,
    depth: &Target,
) {
    let mut scene = Scene::new();
    let mut water = Material::new();
    water.surface = crate::SurfaceKind::Water;
    water.alpha_mode = AlphaMode::Blend;
    water.base_color = [0.2, 0.4, 0.7, 0.7];
    water.roughness = 0.08;
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::plane(32., 32.).unwrap()))
                .with_materials([Arc::new(water)]),
        )
        .unwrap();
    let mut floor = Material::new();
    floor.base_color = [0.4, 0.2, 0.1, 1.];
    floor.roughness = 1.;
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::plane(32., 32.).unwrap()))
                .with_materials([Arc::new(floor)])
                .with_transform(crate::Transform {
                    translation: Vec3::new(0., -2., 0.),
                    ..crate::Transform::IDENTITY
                }),
        )
        .unwrap();
    let camera = Camera::perspective(Vec3::new(0., 4., 6.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
    let view = SceneView::new(Arc::new(scene), camera)
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sun_intensity: 1.,
            sun_direction: Vec3::new(0.4, 0.8, 0.3).normalized().unwrap(),
            shadow_strength: 0.,
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(Some(ScreenEffects {
            resolution_scale: 1.,
            water: true,
            reflections: false,
            clouds: false,
            volumetric_fog: false,
            ..ScreenEffects::default()
        }))
        .unwrap();
    renderer.epoch = context.frame_time();
    let initial = draw(device, renderer, context, &view, target, depth);
    renderer.epoch = context.frame_time() - std::time::Duration::from_millis(600);
    let animated = draw(device, renderer, context, &view, target, depth);
    assert_ne!(initial, animated, "top water surfaces must animate");
    let shifted = view
        .with_scene_transform(crate::Mat4::from_columns([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [-16., 0., 0., 1.],
        ]))
        .unwrap()
        .with_camera(Camera {
            eye: camera.eye - Vec3::new(16., 0., 0.),
            target: camera.target - Vec3::new(16., 0., 0.),
            ..camera
        })
        .with_screen_effects(Some(ScreenEffects {
            world_origin: Vec3::new(16., 0., 0.),
            ..view.screen_effects.unwrap()
        }))
        .unwrap();
    let rebased = draw(device, renderer, context, &shifted, target, depth);
    let error: usize = animated
        .iter()
        .zip(rebased)
        .map(|(a, b)| usize::from(a.abs_diff(b)))
        .sum();
    assert!(
        error < animated.len(),
        "water phase or refraction changed after origin rebasing: error={error}"
    );
}
