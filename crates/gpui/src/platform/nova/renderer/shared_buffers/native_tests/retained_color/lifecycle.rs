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
        .direct_composite_target(config, renderer.current_frame_resource_index, renderer.root_color.target.as_ref())
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
    verify_filter_retirement_keeps_root(renderer, open, target);
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

fn verify_filter_retirement_keeps_root(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::Scene>,
    target: (TextureId, TextureViewId),
) {
    let identity = renderer.root_color.identity();
    let root = renderer.root_color.target.as_ref().expect("window Root owner");
    let pass_sets = root.pass_resource_sets.clone();
    let composite_sets = root.composite_resource_sets.clone();
    let baseline = pixels(renderer, scene, target, 0.8, [sample(0.8)], false);
    let version = renderer.root_color.content_version();
    renderer.wait_for_pending_submissions().expect("retire readers before Filter destruction");
    renderer.destroy_backdrop_blur_targets();
    assert!(renderer.filters.targets.is_none());
    assert_eq!(renderer.root_color.identity(), identity);
    assert_eq!(renderer.root_color.content_version(), version);
    let rebuilt = pixels(renderer, scene, target, 0.8, [sample(0.8)], false);
    assert_eq!(baseline, rebuilt, "recreated Filters must preserve Root pixels");
    assert_eq!(renderer.root_color.identity(), identity);
    let root = renderer.root_color.target.as_ref().expect("same window Root owner");
    assert_eq!(root.pass_resource_sets, pass_sets);
    assert_eq!(root.composite_resource_sets, composite_sets);
    eprintln!("root lifetime gate backend={:?} filter_chain_recreated=true root_texture_reallocated=false root_descriptors_rebuilt=false pixels_equal=true",
        renderer.backend_info.label());
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
    assert!(renderer.root_color.content_version().is_some());
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
    assert!(renderer.root_color.content_version().is_none());
    assert!(
        captured == reference,
        "AutoStatic and independent Popup must match direct pixels"
    );
}
