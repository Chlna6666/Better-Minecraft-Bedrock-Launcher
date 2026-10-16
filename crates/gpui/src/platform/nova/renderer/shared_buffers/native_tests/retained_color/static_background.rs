//! Verify automatic page capture through native pixels and actual source-pass work.
use super::*;
mod retained;

pub(super) fn page(
    size: DrawableSize,
    revision: u64,
    background: crate::Rgba,
    captured_foreground: bool,
) -> Arc<crate::window::Paint> {
    let viewport = bounds(0., 0., size.width as f32, size.height as f32);
    let mut scene = crate::scene::Painter::default();
    // A painter-ordered static prefix followed by independently animated foreground.
    scene.push_layer(viewport);
    scene.insert_primitive(quad(viewport, background));
    for index in 1..96 {
        let region = bounds(8. + (index % 3) as f32, 8., 48., 48.);
        let mut color = crate::rgb(if index % 2 == 0 { 0xff8040 } else { 0x4080ff });
        color.a = 0.03;
        scene.push_layer(region);
        scene.insert_primitive(quad(region, color));
        scene.pop_layer();
    }
    scene.pop_layer();
    scene.push_layer(viewport);
    for index in 0..32 {
        scene.insert_primitive(crate::Shadow {
            bounds: bounds(8. + (index % 3) as f32, 8., 48., 48.),
            content_mask: crate::ContentMask::new(viewport),
            blur_radius: crate::ScaledPixels(3.),
            color: crate::Rgba {
                a: 0.025,
                ..crate::rgb(0x102030)
            },
            ..Default::default()
        });
    }
    scene.pop_layer();
    if captured_foreground {
        popup(&mut scene, viewport);
    } else {
        scene.insert_animated_primitive(
            quad(bounds(20., 20., 16., 16.), crate::rgb(0xff0000)),
            crate::SceneAnimationId(1),
        );
    }
    scene.finish();
    scene.revision = revision;
    Arc::new(crate::window::Paint::new(
        scene.into_scene(),
        Arc::default(),
        Default::default(),
    ))
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
    let target = super::super::target(&renderer);
    let first = page(renderer.current_size, 1, crate::rgb(0x204060), false);
    // Revision zero bypasses automatic capture. This is the direct painter
    // stream reference, so both paths cannot share the same grouping mistake.
    let direct = page(renderer.current_size, 0, crate::rgb(0x204060), false);
    while renderer.swapchain_warmup_frames > 0 {
        pixels(&mut renderer, &first, target, 0., [sample(0.)], true);
    }
    pixels(&mut renderer, &first, target, 0., [sample(0.)], true);
    assert!(renderer.frame_upload.root_color.is_none());
    assert!(renderer.frame_upload.has_element_blurs());

    let mut previous = 0.;
    for current in [0.8, 0.2, 1., 0.] {
        let local = pixels(
            &mut renderer,
            &first,
            target,
            previous,
            [sample(current)],
            false,
        );
        let source_pixels = crate::performance_metrics_snapshot().backdrop_blur_source_pixels;
        assert_eq!(
            source_pixels, 0,
            "foreground motion must not rasterize static background"
        );
        let full = pixels(
            &mut renderer,
            &direct,
            target,
            current,
            [sample(current)],
            true,
        );
        assert_eq!(
            local, full,
            "automatic layer must match full raster on {backend}"
        );
        // Re-establish the retained source after rendering the direct reference.
        pixels(
            &mut renderer,
            &first,
            target,
            current,
            [sample(current)],
            true,
        );
        previous = current;
    }

    // An unrelated commit preserves the independent source; an actual source
    // change refreshes it even though the only supplied spatial damage is local.
    let unchanged = page(renderer.current_size, 2, crate::rgb(0x204060), false);
    pixels(&mut renderer, &unchanged, target, 0., [sample(0.8)], false);
    assert_eq!(
        crate::performance_metrics_snapshot().backdrop_blur_source_pixels,
        0
    );
    // A new Paint generation shares real Shadow/operation allocations and
    // preserves the static source through a foreground animation sample.
    let prefix_end = unchanged.len() - 1;
    let mut painter = crate::scene::Painter::default();
    assert!(painter.replay(0..prefix_end, &unchanged, &[]));
    painter.insert_animated_primitive(
        quad(bounds(20., 20., 16., 16.), crate::rgb(0xff0000)),
        crate::SceneAnimationId(1),
    );
    painter.finish();
    painter.revision = 4;
    assert_eq!(
        &painter.shadows[0] as *const _,
        &unchanged.shadows[0] as *const _
    );
    let shared_operation = unchanged
        .paint_operations
        .iter()
        .position(|operation| {
            matches!(
                operation,
                crate::scene::PaintOperation::Primitive(crate::Primitive::Shadow(_))
            )
        })
        .expect("static shadow operation");
    assert_eq!(
        &painter.paint_operations[shared_operation] as *const _,
        &unchanged.paint_operations[shared_operation] as *const _,
    );
    let replayed = Arc::new(crate::window::Paint::new(
        painter.into_scene(),
        Arc::default(),
        Default::default(),
    ));
    let local = pixels(&mut renderer, &replayed, target, 0.8, [sample(0.2)], false);
    assert_eq!(
        crate::performance_metrics_snapshot().backdrop_blur_source_pixels,
        0
    );
    assert_eq!(
        renderer.frame_upload.shadows.len() / PACKED_SHADOW_BYTES,
        32
    );
    assert_eq!(
        renderer.frame_upload.reused_shadow_bytes,
        32 * PACKED_SHADOW_BYTES
    );
    let full = pixels(&mut renderer, &direct, target, 0.2, [sample(0.2)], true);
    assert_eq!(
        local, full,
        "shared Shadow source must match full rebuild on {backend}"
    );
    println!(
        "persistent Shadow source gate backend={backend} cpu_shared_shadows=32 packed_reused_bytes={} source_pixels=0 pixels_equal=true",
        32 * PACKED_SHADOW_BYTES
    );
    let changed = page(renderer.current_size, 3, crate::rgb(0x406080), false);
    let local = pixels(&mut renderer, &changed, target, 0.8, [sample(0.2)], false);
    assert!(crate::performance_metrics_snapshot().backdrop_blur_source_pixels > 0);
    let changed_direct = page(renderer.current_size, 0, crate::rgb(0x406080), false);
    let full = pixels(
        &mut renderer,
        &changed_direct,
        target,
        0.2,
        [sample(0.2)],
        true,
    );
    assert_eq!(
        local, full,
        "changed source must refresh outside foreground damage"
    );
    retained::verify(&mut renderer, target);
    renderer
        .wait_for_pending_submissions()
        .expect("complete readback");
    device!(renderer, device, {
        device
            .destroy_texture_view(target.1)
            .expect("destroy readback view");
        device
            .destroy_texture(target.0)
            .expect("destroy readback texture");
    });
    renderer.destroy();
}

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_automatic_background_preserves_pixels_without_raster() {
    verify(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_automatic_background_preserves_pixels_without_raster() {
    verify(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_automatic_background_preserves_pixels_without_raster() {
    verify(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_automatic_background_preserves_pixels_without_raster() {
    verify(RendererBackend::NovaOpenGl);
}
