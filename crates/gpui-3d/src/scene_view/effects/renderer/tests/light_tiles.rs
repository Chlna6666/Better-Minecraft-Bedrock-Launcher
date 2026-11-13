use super::*;
use crate::{Keyframe, NodeId, TransformTrack, Vec3Track};
use std::time::Duration;

pub(super) fn animate_light(view: SceneView, light: NodeId) -> SceneView {
    let translation = Vec3Track::new(vec![
        Keyframe::new(Duration::ZERO, Vec3::ZERO),
        Keyframe::new(Duration::from_secs(1), Vec3::X * 32.),
    ])
    .unwrap();
    view.with_animation(
        vec![TransformTrack::new(light).with_translation(translation)],
        Duration::ZERO,
    )
    .unwrap()
}

pub(super) fn verify_animated_light<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    view: &SceneView,
    target: &Target,
    depth: &Target,
) {
    let initial_key = renderer.scenes[&view.id].tile_key;
    let moved = view.with_animation_time(Duration::from_secs(1));
    let pixels = draw(device, renderer, context, &moved, target, depth);
    let old = &pixels[(32 * 65 + 14) * 4..][..4];
    let lit = &pixels[(32 * 65 + 46) * 4..][..4];
    assert!(i16::from(lit[0]) - i16::from(lit[1]) > 40, "{lit:?}");
    assert!(old[0].abs_diff(old[1]) <= 1, "{old:?}");
    let moved_key = renderer.scenes[&view.id].tile_key;
    assert_ne!(moved_key, initial_key);
    // A new animation sample past the final key has the same evaluated lights.
    let held = view.with_animation_time(Duration::from_secs(2));
    assert_eq!(
        draw(device, renderer, context, &held, target, depth),
        pixels
    );
    assert_eq!(renderer.scenes[&view.id].tile_key, moved_key);
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires D3D11 and a native Froxel volume"]
fn froxel_overflow_dx11() {
    let mut device = gfx_dx11::Dx11Device::new(&DeviceDescriptor::default()).unwrap();
    let extent = Extent2d::new(16, 16).unwrap();
    let target = Target::create(&mut device, extent, Format::Rgba8Unorm, false, 1).unwrap();
    let depth = Target::create(&mut device, extent, Format::Depth32Float, true, 1).unwrap();
    let pass = test_render_pass(&mut device);
    let context =
        RendererExtensionContext::for_target(BackendKind::Dx11, pass, Format::Rgba8Unorm, extent);
    let settings = ScreenEffects {
        resolution_scale: 1.,
        background_color: Some([0.; 3]),
        fog_density: 0.3,
        reflections: false,
        water: false,
        clouds: false,
        ..ScreenEffects::default()
    };
    let mut renderer = EffectsRenderer::new(&mut device, context.clone(), settings).unwrap();
    let mut sample = |view: &SceneView, expected_dispatches| {
        let mut steps = Vec::new();
        renderer
            .render(view, &mut device, context.clone(), &mut steps)
            .unwrap();
        let mut compute = Vec::new();
        renderer.drain_compute_passes(&mut compute);
        assert_eq!(compute.len(), expected_dispatches);
        // This fixture has no shadows. Injection only reads the light bins
        // written by render(), so both kernels can execute before graphics.
        for stage in compute {
            ResourceDevice::dispatch_compute_bind_group(&mut device, &stage.dispatch).unwrap();
        }
        if let Some(id) = submit_draw(
            &mut device,
            &mut renderer,
            &context,
            &target,
            &depth,
            &steps,
        ) {
            SubmissionDevice::wait_submission(&mut device, id).unwrap();
        }
        TextureTransferDevice::read_texture(&mut device, target.texture)
            .unwrap()
            .bytes
    };
    let reference = sample(&fog_lights(settings, 0), 2);
    let crowded = fog_lights(settings, 64);
    let overflow = sample(&crowded, 2);
    assert!(reference.chunks_exact(4).any(|pixel| pixel[0] > 40));
    assert_eq!(overflow, reference);
    let revised = crowded.with_scene(Arc::new(crowded.scene.fork()));
    assert_eq!(sample(&revised, 0), overflow);
    let changed_fog = revised
        .with_screen_effects(Some(ScreenEffects {
            fog_density: 0.1,
            ..settings
        }))
        .unwrap();
    sample(&changed_fog, 2);
    renderer.destroy(&mut device).unwrap();
    target.destroy(&mut device).unwrap();
    depth.destroy(&mut device).unwrap();
    PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
}

#[cfg(target_os = "windows")]
fn fog_lights(settings: ScreenEffects, dark_lights: usize) -> SceneView {
    let mut scene = Scene::new();
    for index in 0..=dark_lights {
        scene
            .insert(
                None,
                Node::new().with_light(Light::Point {
                    position: Vec3::new(0., 0., -3.),
                    color: if index == dark_lights {
                        [1., 0., 0.]
                    } else {
                        [0.; 3]
                    },
                    intensity: 16.,
                    range: 10.,
                }),
            )
            .unwrap();
    }
    let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1., 0.1, 100.);
    SceneView::new(Arc::new(scene), camera)
        .with_screen_effects(Some(settings))
        .unwrap()
}
