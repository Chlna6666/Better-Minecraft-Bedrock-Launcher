//! Text uniform changes must update cached native glyph pixels even in the same Buffer.
use super::*;
use crate::{AtlasKey, FontId, GlyphId, RenderGlyphParams};
use std::borrow::Cow;

macro_rules! backend_test {
    ($name:ident, $feature:literal, $backend:ident) => {
        #[test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $name() {
            for isolated in [false, true] {
                verify(RendererBackend::$backend, isolated);
            }
        }
    };
}
backend_test!(
    dx11_text_raster_refreshes_cached_pixels,
    "nova-gfx-dx11",
    NovaDx11
);
backend_test!(
    dx12_text_raster_refreshes_cached_pixels,
    "nova-gfx-dx12",
    NovaDx12
);
backend_test!(
    vulkan_text_raster_refreshes_cached_pixels,
    "nova-gfx-vulkan",
    NovaVulkan
);
backend_test!(
    opengl_text_raster_refreshes_cached_pixels,
    "nova-gfx-opengl",
    NovaOpenGl
);

fn glyph(renderer: &NovaRenderer) -> crate::MonochromeSprite {
    let key = AtlasKey::Glyph(RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(2),
        font_size: crate::px(14.),
        subpixel_variant: crate::point(0, 0),
        scale_factor: 1.,
        grayscale_antialiasing: true,
        is_emoji: false,
        is_cjk: false,
    });
    let tile = renderer
        .atlas
        .ensure_tile_with(key, &mut || {
            Ok(Some((
                crate::size(DevicePixels(4), DevicePixels(4)),
                Cow::Owned(vec![96; 16]),
            )))
        })
        .expect("glyph tile")
        .expect("coverage");
    let area = bounds(80., 80., 16., 16.);
    crate::MonochromeSprite {
        order: 0,
        pad: 0,
        animation_id: None,
        bounds: area,
        content_mask: crate::ContentMask::new(area),
        // Enhanced contrast is brightness-weighted; white has zero weight.
        color: crate::rgb(0x202020),
        tile,
        transformation: Default::default(),
    }
}

fn page(renderer: &NovaRenderer, isolated: bool) -> Arc<crate::window::Paint> {
    let mut painter = crate::scene::Painter::default();
    let viewport = bounds(
        0.,
        0.,
        renderer.current_size.width as f32,
        renderer.current_size.height as f32,
    );
    // Static raster work admits Root independently of the optional glyph capture.
    painter.insert_primitive(quad(viewport, crate::rgb(0x204060)));
    painter.insert_primitive(quad(viewport, crate::rgb(0x406080)));
    if isolated {
        let area = bounds(80., 80., 16., 16.);
        painter.begin_blur(crate::BlurCapture {
            animation_id: None,
            layer_id: Some(crate::GlobalElementId::from_path(&["glyph".into()])),
            bounds: area,
            content_mask: crate::ContentMask::new(viewport),
            radius: crate::ScaledPixels(0.),
            opacity: 1.,
        });
    }
    painter.insert_primitive(glyph(renderer));
    if isolated {
        painter.end_blur();
    }
    popup(&mut painter, viewport);
    painter.finish();
    painter.revision = 1;
    Arc::new(crate::window::Paint::new(
        painter.into_scene(),
        Arc::default(),
        Default::default(),
    ))
}

fn verify_samples(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
) {
    let before = pixels(renderer, scene, target, 0.8, [sample(0.8)], true);
    assert!(renderer.frame_upload.root_color.is_some());
    renderer.wait_pending_submissions().expect("idle aliases");
    let buffer = renderer.text_raster_buffer;
    renderer.rendering_parameters.grayscale_enhanced_contrast = 0.8;
    renderer.retained_upload.invalidate_encode_key();
    // Tiny popup damage must not restrict presentation of changed glyphs elsewhere.
    let actual = pixels(renderer, scene, target, 0.8, [sample(0.9)], false);
    assert!(renderer.command_scratch.submitted_present_damage.is_none());
    assert_eq!(
        renderer.text_raster_buffer, buffer,
        "exercise in-place uniform update"
    );
    assert!(
        crate::performance_metrics_snapshot().backdrop_blur_source_pixels > 0,
        "changed text shader context must repaint retained glyphs"
    );
    let expected = pixels(renderer, scene, target, 0.9, [sample(0.9)], true);
    assert!(
        actual == expected,
        "cached text must match full raster at new parameters"
    );
    let glyph_pixel = (84 * renderer.current_size.width as usize + 84) * 4;
    assert_ne!(
        &actual[glyph_pixel..glyph_pixel + 4],
        &before[glyph_pixel..glyph_pixel + 4],
        "contrast change must reach real glyph pixels outside popup damage"
    );
    let unchanged = pixels(renderer, scene, target, 0.9, [sample(0.9)], false);
    assert!(unchanged == actual);
    assert_eq!(
        crate::performance_metrics_snapshot().backdrop_blur_source_pixels,
        0,
        "submitted text context must not force subsequent refreshes"
    );
}

fn verify(backend: RendererBackend, isolated: bool) {
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
    renderer.rendering_parameters.grayscale_enhanced_contrast = 0.;
    let scene = page(&renderer, isolated);
    let target = super::super::target(&renderer);
    while renderer.swapchain_warmup_frames > 0 {
        pixels(&mut renderer, &scene, target, 0.8, [sample(0.8)], true);
    }
    verify_samples(&mut renderer, &scene, target);
    eprintln!(
        "text raster gate backend={backend:?} isolated={isolated} buffer_unchanged=true pixels_equal=true"
    );
    renderer
        .wait_pending_submissions()
        .expect("retire readback");
    device!(renderer, device, {
        device.destroy_texture_view(target.1).expect("destroy view");
        device.destroy_texture(target.0).expect("destroy texture");
    });
    renderer.destroy();
}
