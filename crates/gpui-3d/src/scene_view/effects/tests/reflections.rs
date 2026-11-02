//! Reflection controls and alpha coverage must not turn ordinary surfaces into glass.
use super::*;

pub(super) fn verify_reflection_controls<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    base: &SceneView,
    target: &Target,
    depth: &Target,
) {
    for voxel in [false, true] {
        for blended in [false, true] {
            let mut material = Material::new();
            material.roughness = if blended { 1. } else { 0.08 };
            material.base_color = if blended {
                [0.2, 0.2, 0.2, 0.999]
            } else {
                [0., 0., 0., 1.]
            };
            material.alpha_mode = if blended {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
            let mut scene = Scene::new();
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(Arc::new(Mesh::cube()))
                        .with_materials([Arc::new(material)]),
                )
                .unwrap();
            let mut view = base
                .with_scene(Arc::new(scene))
                .with_screen_effects(Some(ScreenEffects {
                    reflections: false,
                    background_color: Some([1., 0., 1.]),
                    ..base.screen_effects.unwrap()
                }))
                .unwrap();
            view.voxel_shader = voxel;
            let pixels = draw(device, renderer, context, &view, target, depth);
            let center = &pixels[(32 * 64 + 32) * 4..][..3];
            if blended {
                assert!(
                    center[1] > 4,
                    "ordinary blended material must remain visible: {center:?}"
                );
                assert!(
                    center[0].abs_diff(center[1]) <= 5 && center[2].abs_diff(center[1]) <= 5,
                    "ordinary almost-opaque alpha coverage must not become magenta glass (voxel={voxel}): {center:?}"
                );
            } else {
                assert!(
                    center.iter().all(|value| *value <= 4),
                    "disabled reflections must omit sky/IBL as well as SSR: {center:?}"
                );
            }
        }
    }
}
