use super::*;

fn retained_root_view(renderer: &NovaRenderer) -> TextureViewId {
    let index = renderer
        .frame_upload
        .retained_root_blur
        .expect("a completed popup must leave static window color resident");
    let config = renderer
        .frame_upload
        .backdrop_blur_config_for_index(index)
        .expect("root config");
    renderer
        .filters
        .targets
        .as_ref()
        .expect("targets")
        .direct_composite_target(config, renderer.current_frame_resource_index)
        .expect("root target")
        .texture_view
}

pub(super) fn verify(
    renderer: &mut NovaRenderer,
    open: &Arc<crate::Scene>,
    target: (TextureId, TextureViewId),
) {
    pixels(
        renderer,
        open,
        target,
        0.,
        [popup_sample(0.8, PopupPlacement::Sibling)],
        false,
    );
    let original = retained_root_view(renderer);
    let mut closed = page(false, renderer.current_size, PopupPlacement::Closed);
    Arc::get_mut(&mut closed)
        .expect("new closed scene is uniquely owned")
        .revision = 2;
    for _ in 0..3 {
        let local = pixels(renderer, &closed, target, 0., [], false);
        assert!(
            renderer.frame_upload.retained_root_blur.is_some(),
            "removing a finished popup must keep static window color resident"
        );
        assert_eq!(
            retained_root_view(renderer),
            original,
            "closing the popup must preserve the static root texture"
        );
        let source = crate::diagnostics::performance_metrics::performance_metrics_snapshot()
            .backdrop_blur_source_pixels;
        assert!(
            source
                < renderer.current_size.width as usize * renderer.current_size.height as usize / 4,
            "close must stay local: {source}"
        );
        let full = pixels(renderer, &closed, target, 0., [], true);
        assert!(local == full, "popup removal must erase all old pixels");
        let local = pixels(
            renderer,
            open,
            target,
            0.,
            [popup_sample(0.8, PopupPlacement::Sibling)],
            false,
        );
        assert_eq!(
            retained_root_view(renderer),
            original,
            "reopening the popup must reuse the static root texture"
        );
        let source = crate::diagnostics::performance_metrics::performance_metrics_snapshot()
            .backdrop_blur_source_pixels;
        assert!(
            source
                < renderer.current_size.width as usize * renderer.current_size.height as usize / 4,
            "reopen must stay local: {source}"
        );
        let full = pixels(
            renderer,
            open,
            target,
            0.8,
            [popup_sample(0.8, PopupPlacement::Sibling)],
            true,
        );
        assert!(local == full, "reopened popup must match a complete redraw");
    }
    verify_root_reactivation(renderer, open, target);
}

fn verify_root_reactivation(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::Scene>,
    target: (TextureId, TextureViewId),
) {
    present_auto_static(renderer, target);

    let local = pixels(
        renderer,
        scene,
        target,
        0.8,
        [
            popup_sample(0.8, PopupPlacement::Sibling),
            panel_sample(1.0),
        ],
        false,
    );
    assert!(renderer.frame_upload.retained_root_blur.is_some());
    assert!(renderer.filters.root_color_content_version().is_some());
    let full = pixels(
        renderer,
        scene,
        target,
        0.8,
        [
            popup_sample(0.8, PopupPlacement::Sibling),
            panel_sample(1.0),
        ],
        true,
    );
    assert!(
        local == full,
        "reactivated Root must contain freshly rendered pixels"
    );
}

fn present_auto_static(renderer: &mut NovaRenderer, target: (TextureId, TextureViewId)) {
    let direct = static_background::page(renderer.current_size, 0, crate::rgb(0x406080), true);
    let reference = pixels(
        renderer,
        &direct,
        target,
        0.8,
        [popup_sample(0.8, PopupPlacement::Sibling)],
        true,
    );
    let auto_static = static_background::page(renderer.current_size, 3, crate::rgb(0x406080), true);
    let captured = pixels(
        renderer,
        &auto_static,
        target,
        0.8,
        [popup_sample(0.8, PopupPlacement::Sibling)],
        true,
    );
    assert!(renderer.frame_upload.retained_root_blur.is_none());
    assert!(renderer.filters.root_color_content_version().is_none());
    assert!(
        captured == reference,
        "AutoStatic and independent Popup must match direct pixels"
    );
}
