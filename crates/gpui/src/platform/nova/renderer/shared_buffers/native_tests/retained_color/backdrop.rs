//! Retained Root must still refresh Gaussian outputs when their input changes.
use super::*;

const SOURCE_ID: crate::SceneAnimationId = crate::SceneAnimationId(1 << 31);

fn sample(progress: f32) -> crate::SceneAnimationValue {
    crate::SceneAnimationValue {
        animation_id: SOURCE_ID,
        ..super::sample(progress)
    }
}

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
backend_test!(dx11_root_backdrop_source, "nova-gfx-dx11", NovaDx11);
backend_test!(dx12_root_backdrop_source, "nova-gfx-dx12", NovaDx12);
backend_test!(vulkan_root_backdrop_source, "nova-gfx-vulkan", NovaVulkan);
backend_test!(opengl_root_backdrop_source, "nova-gfx-opengl", NovaOpenGl);

fn verify_samples(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
) {
    let mut previous = 0.;
    let mut previous_pixels = pixels(renderer, scene, target, 0., [sample(0.)], true);
    for progress in [0.8, 0.2, 1., 0.] {
        let actual = pixels(renderer, scene, target, previous, [sample(progress)], false);
        let metrics = crate::performance_metrics_snapshot();
        assert!(
            renderer.frame_upload.root_color.is_some(),
            "exercise retained Root"
        );
        assert!(
            metrics.backdrop_blur_target_pixels > 0,
            "upstream animation must filter"
        );
        let expected = pixels(renderer, scene, target, progress, [sample(progress)], true);
        assert!(
            actual == expected,
            "source damage must retain the complete Gaussian halo"
        );
        assert!(
            actual != previous_pixels,
            "the changed source must reach visible pixels"
        );
        eprintln!(
            "root backdrop source backend={} progress={progress} filter_pixels={} source_pixels={} pixels_equal=true",
            renderer.backend_info.label(),
            metrics.backdrop_blur_target_pixels,
            metrics.backdrop_blur_source_pixels
        );
        previous = progress;
        previous_pixels = actual;
    }
    // A non-spatial invalidation must override an empty per-order plan.
    renderer.invalidate_backdrop_blur_cache();
    let actual = pixels(renderer, scene, target, 0., [sample(0.)], false);
    assert!(crate::performance_metrics_snapshot().backdrop_blur_target_pixels > 0);
    let expected = pixels(renderer, scene, target, 0., [sample(0.)], true);
    assert!(
        actual == expected,
        "invalidated Gaussian targets must initialize fully"
    );
}

fn verify(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(SIDE as i32), DevicePixels(SIDE as i32)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let size = renderer.current_size;
    let mut source = quad(
        bounds(size.width as f32 * 0.8, size.height as f32 * 0.8, 24., 24.),
        crate::rgb(0xff0000),
    );
    source.animation_id = Some(SOURCE_ID);
    let scene = page_with_source(true, size, PopupPlacement::Sibling, Some(source));
    let target = super::super::target(&renderer);
    while renderer.swapchain_warmup_frames > 0 {
        pixels(&mut renderer, &scene, target, 0., [sample(0.)], true);
    }
    verify_samples(&mut renderer, &scene, target);
    verify_repaired_sample(&mut renderer, &scene, target);
    renderer
        .wait_pending_submissions()
        .expect("retire readback");
    device!(renderer, device, {
        device.destroy_texture_view(target.1).expect("destroy view");
        device.destroy_texture(target.0).expect("destroy texture");
    });
    renderer.destroy();
}

fn verify_repaired_sample(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
) {
    // An independently supplied/replaced packet omits the last GPU sample's
    // damage. The resident Root owns that baseline and must repair BOTH plans.
    let packet = crate::PresentationPacket::new(
        Arc::clone(scene),
        [sample(0.8)],
        [],
        Instant::now(),
        crate::DirtyRegion::empty(),
        Default::default(),
        crate::PartialPresentMode::Partial,
    );
    assert!(
        renderer
            .present_framebuffer_only(packet)
            .expect("repaired sample")
    );
    assert!(
        crate::performance_metrics_snapshot().backdrop_blur_target_pixels > 0,
        "last successful Root sample must repair backdrop source damage"
    );
    // Re-present the same value without any new source damage, preserving
    // exactly the pixels produced by the repaired frame above.
    let actual = pixels(renderer, scene, target, 0.8, [sample(0.8)], false);
    let expected = pixels(renderer, scene, target, 0.8, [sample(0.8)], true);
    assert!(actual == expected, "repaired source must match full redraw");
}
