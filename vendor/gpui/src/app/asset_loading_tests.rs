use crate::{AppContext, ObjectFit, Resource, SharedString, TestAppContext, px, size};

#[test]
fn compressed_image_preload_reuses_and_removes_global_asset() {
    let cx = TestAppContext::single();
    let source = Resource::Embedded(SharedString::from("missing-background.webp"));

    cx.update(|cx| {
        let initial_assets = cx.loading_assets.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("preload task should be returned");
        let after_first_preload = cx.loading_assets.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("preload task should be returned");
        let after_second_preload = cx.loading_assets.len();

        let _ = cx
            .remove_compressed_image_resource(&source)
            .expect("preloaded compressed resource should be removable");
        let after_remove = cx.loading_assets.len();

        let _ = cx
            .preload_compressed_image_resources([source])
            .pop()
            .expect("preload task should be returned");
        let after_third_preload = cx.loading_assets.len();

        assert_eq!(after_first_preload, initial_assets + 1);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_remove, initial_assets);
        assert_eq!(after_third_preload, initial_assets + 1);
    });
}

#[test]
fn target_size_image_preload_reuses_target_without_implicit_compressed_cache() {
    let cx = TestAppContext::single();
    let source = Resource::Embedded(SharedString::from("missing-target-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.loading_assets.len();

        let _ = cx
            .preload_target_size_images([source.clone()], logical_size, 1.0, ObjectFit::Cover)
            .pop()
            .expect("target preload task should be returned");
        let after_first_preload = cx.loading_assets.len();

        let _ = cx
            .preload_target_size_images([source.clone()], logical_size, 1.0, ObjectFit::Cover)
            .pop()
            .expect("target preload task should be returned");
        let after_second_preload = cx.loading_assets.len();

        let _ = cx
            .remove_target_size_image(&source, logical_size, 1.0, ObjectFit::Cover)
            .expect("target preload should be removable");
        let after_target_remove = cx.loading_assets.len();

        assert!(cx.remove_compressed_image_resource(&source).is_none());
        assert_eq!(after_first_preload, initial_assets + 1);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_target_remove, initial_assets);
    });
}

#[test]
fn target_size_image_preload_reuses_equivalent_scale_factor_targets() {
    let cx = TestAppContext::single();
    let source = Resource::Embedded(SharedString::from(
        "missing-equivalent-scale-target-background.webp",
    ));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let first_target = cx
            .target_size_image_source(source.clone(), logical_size, 1.25, ObjectFit::Cover)
            .expect("target source should be created");
        let equivalent_target = cx
            .target_size_image_source(source.clone(), logical_size, 1.2500001, ObjectFit::Cover)
            .expect("equivalent target source should be created");

        assert_eq!(first_target, equivalent_target);

        let initial_assets = cx.loading_assets.len();
        let _first_task = cx.preload_target_size_image(first_target);
        let after_first_preload = cx.loading_assets.len();

        let _equivalent_task = cx.preload_target_size_image(equivalent_target.clone());
        let after_second_preload = cx.loading_assets.len();

        cx.remove_target_size_image_source_in(&equivalent_target, None);
        let after_remove = cx.loading_assets.len();

        assert_eq!(after_first_preload, initial_assets + 1);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_remove, initial_assets);
    });
}

#[test]
fn target_size_image_preload_allows_releasing_compressed_preload() {
    let cx = TestAppContext::single();
    let source = Resource::Embedded(SharedString::from("missing-consumed-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.loading_assets.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("compressed preload task should be returned");
        let after_compressed_preload = cx.loading_assets.len();

        let target = cx
            .target_size_image_source(source.clone(), logical_size, 1.25, ObjectFit::Cover)
            .expect("target source should be created");
        let compressed_resource = target.resource().clone();
        let _target_task = cx.preload_target_size_image(target);
        let after_target_preload = cx.loading_assets.len();
        cx.remove_compressed_image_resource(&compressed_resource);
        let after_compressed_remove = cx.loading_assets.len();

        assert_eq!(after_compressed_preload, initial_assets + 1);
        assert_eq!(after_target_preload, initial_assets + 2);
        assert_eq!(after_compressed_remove, initial_assets + 1);
        assert!(cx.remove_compressed_image_resource(&source).is_none());
    });
}

#[test]
fn target_size_image_removal_keeps_compressed_preload_explicit() {
    let cx = TestAppContext::single();
    let source = Resource::Embedded(SharedString::from("missing-target-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.loading_assets.len();

        let _ = cx.preload_compressed_image_resources([source.clone()]);
        let _ =
            cx.preload_target_size_images([source.clone()], logical_size, 1.0, ObjectFit::Cover);
        assert_eq!(cx.loading_assets.len(), initial_assets + 2);

        let target = cx.remove_target_size_image(&source, logical_size, 1.0, ObjectFit::Cover);
        let after_target_remove = cx.loading_assets.len();
        let compressed = cx.remove_compressed_image_resource(&source);

        assert!(target.is_some());
        assert_eq!(after_target_remove, initial_assets + 1);
        assert!(compressed.is_some());
        assert_eq!(cx.loading_assets.len(), initial_assets);
    });
}
