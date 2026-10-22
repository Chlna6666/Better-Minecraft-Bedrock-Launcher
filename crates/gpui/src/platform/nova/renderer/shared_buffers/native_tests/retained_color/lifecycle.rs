use super::*;

fn retained_root_view(renderer: &NovaRenderer) -> TextureViewId {
    renderer
        .frame_upload
        .root_color
        .map(|root| root.index)
        .expect("a completed popup must leave static window color resident");
    renderer
        .root_color
        .source_target(renderer.current_frame_resource_index)
        .expect("root target")
        .texture_view
}

pub(super) fn verify(
    renderer: &mut NovaRenderer,
    open: &Arc<crate::window::Paint>,
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
        .expect("new closed Paint is uniquely owned")
        .scene
        .revision = 2;
    for _ in 0..3 {
        let local = pixels(renderer, &closed, target, 0., [], false);
        assert!(
            renderer.frame_upload.root_color.is_some(),
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
    verify_root_without_filters(renderer, target);
}

fn verify_root_without_filters(renderer: &mut NovaRenderer, target: (TextureId, TextureViewId)) {
    renderer
        .wait_pending_submissions()
        .expect("retire Filter readers");
    renderer.destroy_backdrop_blur_targets();
    let viewport = bounds(
        0.,
        0.,
        renderer.current_size.width as f32,
        renderer.current_size.height as f32,
    );
    let mut scene = crate::scene::Painter::default();
    scene.insert_primitive(quad(viewport, crate::rgb(0x204060)));
    scene.insert_primitive(quad(viewport, crate::rgb(0x6080a0)));
    scene.insert_animated_primitive(
        quad(bounds(20., 20., 16., 16.), crate::rgb(0xff0000)),
        crate::SceneAnimationId(1),
    );
    scene.insert_primitive(quad(bounds(22., 22., 4., 4.), crate::rgb(0x00ffff)));
    scene.finish();
    let scene = Arc::new(crate::window::Paint::new(
        scene.into_scene(),
        Arc::default(),
        Default::default(),
    ));
    let expected = pixels(renderer, &scene, target, 0.8, [sample(0.8)], true);
    assert!(renderer.frame_upload.root_color.is_some());
    assert!(renderer.frame_upload.backdrop_blur_configs().is_empty());
    assert!(
        renderer.filters.targets.is_none(),
        "Root-only needs no Filter target chain"
    );
    let version = renderer
        .root_color
        .content_version()
        .expect("initialized Root");
    renderer.filters.invalidate();
    let actual = pixels(renderer, &scene, target, 0.8, [sample(0.8)], false);
    assert_eq!(actual, expected);
    assert_eq!(
        renderer.root_color.content_version(),
        Some(version),
        "Filter invalidation must not rasterize unchanged Root"
    );
    assert_eq!(
        crate::diagnostics::performance_metrics::performance_metrics_snapshot()
            .backdrop_blur_source_pixels,
        0,
    );
    assert_eq!(renderer.command_scratch.commands().len(), 1);
    let local = pixels(renderer, &scene, target, 0.8, [sample(0.2)], false);
    let complete = pixels(renderer, &scene, target, 0.2, [sample(0.2)], true);
    assert_eq!(
        local, complete,
        "Root-owned samples must repair animation damage without Filters"
    );
    eprintln!(
        "root-only gate backend={} filter_targets=0 unchanged_source_pixels=0 native_present=full pixels_equal=true",
        renderer.backend_info.label()
    );
}

fn verify_filter_retirement_keeps_root(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
) {
    let identity = renderer.root_color.identity();
    let root = renderer
        .root_color
        .target
        .as_ref()
        .expect("window Root owner");
    let pass_sets = root.pass_resource_sets.clone();
    let composite_sets = root.composite_resource_sets.clone();
    let baseline = pixels(renderer, scene, target, 0.8, [sample(0.8)], false);
    let version = renderer.root_color.content_version();
    renderer
        .wait_pending_submissions()
        .expect("retire readers before Filter destruction");
    renderer.destroy_backdrop_blur_targets();
    assert!(renderer.filters.targets.is_none());
    assert_eq!(renderer.root_color.identity(), identity);
    assert_eq!(renderer.root_color.content_version(), version);
    let rebuilt = pixels(renderer, scene, target, 0.8, [sample(0.8)], false);
    assert_eq!(
        baseline, rebuilt,
        "recreated Filters must preserve Root pixels"
    );
    assert_eq!(renderer.root_color.identity(), identity);
    let root = renderer
        .root_color
        .target
        .as_ref()
        .expect("same window Root owner");
    assert_eq!(root.pass_resource_sets, pass_sets);
    assert_eq!(root.composite_resource_sets, composite_sets);
    eprintln!(
        "root lifetime gate backend={:?} filter_chain_recreated=true root_texture_reallocated=false root_descriptors_rebuilt=false pixels_equal=true",
        renderer.backend_info.label()
    );
}

fn verify_root_reactivation(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
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
    assert!(renderer.frame_upload.root_color.is_some());
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
    assert!(renderer.frame_upload.root_color.is_none());
    assert!(renderer.root_color.content_version().is_none());
    assert!(
        captured == reference,
        "AutoStatic and independent Popup must match direct pixels"
    );
}
