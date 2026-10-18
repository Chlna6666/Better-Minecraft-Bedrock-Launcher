//! Compare production indexed geometry against a static geometry reference on each backend.
use super::*;

macro_rules! backend_test {
    ($name:ident, $feature:literal, $backend:ident) => {
        #[test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $name() {
            verify(RendererBackend::$backend);
        }
    };
}
backend_test!(dx11_vertical_edges_pixels, "nova-gfx-dx11", NovaDx11);
backend_test!(dx12_vertical_edges_pixels, "nova-gfx-dx12", NovaDx12);
backend_test!(vulkan_vertical_edges_pixels, "nova-gfx-vulkan", NovaVulkan);
backend_test!(opengl_vertical_edges_pixels, "nova-gfx-opengl", NovaOpenGl);

fn value(progress: f32, scale: f32) -> crate::SceneAnimationValue {
    crate::SceneAnimationValue {
        animation_id: crate::SceneAnimationId(1),
        property: crate::TransitionProperty::VerticalEdges,
        from: [32. * scale, 0., 0., 0.],
        to: [0.; 4],
        progress,
    }
}

fn page(scale: f32, scroll: f32, reference: Option<f32>) -> Arc<crate::window::Paint> {
    let viewport = bounds(0., 0., 64., 64.);
    let mut painter = crate::scene::Painter::default();
    painter.insert_primitive(quad(viewport, crate::rgb(0x204060)));
    let mut bar = quad(
        bounds(12. * scale, 8. * scale - scroll, 14. * scale, 32. * scale),
        crate::rgb(0x40e080),
    );
    bar.corner_radii = crate::Corners::all(crate::ScaledPixels(4. * scale));
    bar.content_mask = crate::ContentMask::new(bounds(4., 12., 56., 40.));
    // A rounded ancestor clip is independent of the shape's moving edges.
    bar.content_mask.corner_bounds = bar.content_mask.bounds;
    bar.content_mask.corner_radii = crate::Corners::all(crate::ScaledPixels(6.));
    if let Some(progress) = reference {
        let mut primitive = crate::Primitive::Quad(bar);
        crate::platform::nova::frame_upload::apply_value(&mut primitive, &value(progress, scale));
        let crate::Primitive::Quad(sampled) = primitive else {
            unreachable!()
        };
        bar = sampled;
    } else {
        bar.animation_id = Some(crate::SceneAnimationId(1));
    }
    painter.insert_primitive(bar);
    painter.finish();
    Arc::new(crate::window::Paint::new(
        painter.into_scene(),
        Arc::default(),
        Default::default(),
    ))
}

fn renderer(window: &native_window::Window, backend: RendererBackend) -> NovaRenderer {
    NovaRenderer::with_atlas(
        window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(64), DevicePixels(64)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer")
}

fn verify_scale(renderer: &mut NovaRenderer, reference_renderer: &mut NovaRenderer, scale: f32) {
    let target = super::super::target(renderer);
    let reference_target = super::super::target(reference_renderer);
    let scene = page(scale, 0., None);
    let mut previous = 0.;
    let mut stable_quads = None;
    for progress in [0., 0.01, 0.25, 0.5, 1., 1.15, 0.3, 1.] {
        let actual = pixels(
            renderer,
            &scene,
            target,
            previous,
            [value(progress, scale)],
            stable_quads.is_none(),
        );
        if let Some(bytes) = &stable_quads {
            assert_eq!(
                &renderer.frame_upload.quads.to_vec(),
                bytes,
                "stable Quad upload stream"
            );
        } else {
            stable_quads = Some(renderer.frame_upload.quads.to_vec());
        }
        let reference = page(scale, 0., Some(progress));
        let expected = pixels(
            reference_renderer,
            &reference,
            reference_target,
            0.,
            [],
            true,
        );
        assert_eq!(actual, expected, "scale={scale}, progress={progress}");
        previous = progress;
    }
    // A completed binding can remain in a retained primitive after its value is removed.
    // The indexed slot must disable itself, restoring exactly the stable base geometry.
    let cleared = pixels(renderer, &scene, target, 1., [], true);
    let terminal = page(scale, 0., Some(1.));
    let expected = pixels(
        reference_renderer,
        &terminal,
        reference_target,
        0.,
        [],
        true,
    );
    assert_eq!(
        cleared, expected,
        "removed completion value restores static Quad"
    );
    // Completion leaves an identity sample. Subsequent scroll/replay must match an unbound
    // static Quad, including both rounded corners and the unchanged ancestor mask.
    for scroll in [7.5, 24., -4., 0.] {
        let replay = page(scale, scroll, None);
        let actual = pixels(renderer, &replay, target, 1., [value(1., scale)], true);
        let reference = page(scale, scroll, Some(1.));
        let expected = pixels(
            reference_renderer,
            &reference,
            reference_target,
            0.,
            [],
            true,
        );
        assert_eq!(actual, expected, "completed scroll={scroll}");
    }
    release_target(renderer, target);
    release_target(reference_renderer, reference_target);
}

fn release_target(renderer: &mut NovaRenderer, target: (TextureId, TextureViewId)) {
    renderer
        .wait_for_pending_submissions()
        .expect("complete readback");
    device!(renderer, device, {
        device.destroy_texture_view(target.1).expect("destroy view");
        device.destroy_texture(target.0).expect("destroy target");
    });
}

fn verify(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    let reference_window = native_window::Window::renderer_fixture();
    let mut animated = renderer(&window, backend);
    let mut reference = renderer(&reference_window, backend);
    for scale in [1., 1.25, 2.] {
        verify_scale(&mut animated, &mut reference, scale);
    }
    animated.destroy();
    reference.destroy();
}
