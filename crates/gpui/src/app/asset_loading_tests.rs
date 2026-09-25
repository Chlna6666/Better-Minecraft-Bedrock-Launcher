use crate::{AssetLocation, ObjectFit, SharedString, TestAppContext, px, size};

#[test]
fn compressed_image_preload_reuses_and_removes_global_asset() {
    let cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from("missing-background.webp"));

    cx.update(|cx| {
        let initial_assets = cx.asset_entries.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("preload lease should be returned");
        let after_first_preload = cx.asset_entries.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("preload lease should be returned");
        let after_second_preload = cx.asset_entries.len();

        let _ = cx
            .remove_compressed_image_resource(&source)
            .expect("preloaded compressed resource should be removable");
        let after_remove = cx.asset_entries.len();

        let _ = cx
            .preload_compressed_image_resources([source])
            .pop()
            .expect("preload lease should be returned");
        let after_third_preload = cx.asset_entries.len();

        assert_eq!(after_first_preload, initial_assets + 1);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_remove, initial_assets);
        assert_eq!(after_third_preload, initial_assets + 1);
    });
}

#[test]
fn completed_compressed_preload_retires_internal_cache_entry() {
    let mut cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from("missing-completed-background.webp"));
    let initial_assets = cx.read(|cx| cx.asset_entries.len());
    let preload = cx.update(|cx| {
        let preload = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("preload lease should be returned");
        assert_eq!(cx.asset_entries.len(), initial_assets + 1);
        preload
    });

    cx.run_until_parked();

    assert!(
        preload.get().is_some(),
        "preload lease should keep the settled result alive"
    );
    cx.read(|cx| assert_eq!(cx.asset_entries.len(), initial_assets));
    cx.update(|cx| {
        assert!(
            cx.remove_compressed_image_resource(&source).is_none(),
            "completed transient entry should no longer be retained by the app cache"
        );
    });
}

#[test]
fn sized_image_preload_reuses_target_and_compressed_cache() {
    let cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from("missing-target-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.asset_entries.len();

        let _ = cx
            .preload_sized_images([source.clone()], logical_size, 1.0, ObjectFit::Cover)
            .pop()
            .expect("target preload lease should be returned");
        let after_first_preload = cx.asset_entries.len();

        let _ = cx
            .preload_sized_images([source.clone()], logical_size, 1.0, ObjectFit::Cover)
            .pop()
            .expect("target preload lease should be returned");
        let after_second_preload = cx.asset_entries.len();

        let _ = cx
            .remove_sized_image(&source, logical_size, 1.0, ObjectFit::Cover)
            .expect("target preload should be removable");
        let after_target_remove = cx.asset_entries.len();

        let compressed = cx.remove_compressed_image_resource(&source);
        assert!(compressed.is_some());
        assert_eq!(after_first_preload, initial_assets + 2);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_target_remove, initial_assets + 1);
    });
}

#[test]
fn sized_image_preload_reuses_equivalent_scale_factor_targets() {
    let cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from(
        "missing-equivalent-scale-target-background.webp",
    ));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let first_target = cx
            .image_render_request(source.clone(), logical_size, 1.25, ObjectFit::Cover)
            .expect("target source should be created");
        let equivalent_target = cx
            .image_render_request(source.clone(), logical_size, 1.2500001, ObjectFit::Cover)
            .expect("equivalent target source should be created");

        assert_eq!(first_target, equivalent_target);

        let initial_assets = cx.asset_entries.len();
        let _first_preload = cx.preload_sized_image(first_target);
        let after_first_preload = cx.asset_entries.len();

        let _equivalent_preload = cx.preload_sized_image(equivalent_target.clone());
        let after_second_preload = cx.asset_entries.len();

        cx.remove_image_render_request_in(&equivalent_target, None);
        let after_remove = cx.asset_entries.len();

        assert_eq!(after_first_preload, initial_assets + 2);
        assert_eq!(after_second_preload, after_first_preload);
        assert_eq!(after_remove, initial_assets + 1);
        assert!(cx.remove_compressed_image_resource(&source).is_some());
    });
}

#[test]
fn sized_image_preload_allows_releasing_compressed_preload() {
    let cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from("missing-consumed-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.asset_entries.len();

        let _ = cx
            .preload_compressed_image_resources([source.clone()])
            .pop()
            .expect("compressed preload lease should be returned");
        let after_compressed_preload = cx.asset_entries.len();

        let target = cx
            .image_render_request(source.clone(), logical_size, 1.25, ObjectFit::Cover)
            .expect("target source should be created");
        let compressed_resource = target.resource().clone();
        let _target_preload = cx.preload_sized_image(target);
        let after_target_preload = cx.asset_entries.len();
        cx.remove_compressed_image_resource(&compressed_resource);
        let after_compressed_remove = cx.asset_entries.len();

        assert_eq!(after_compressed_preload, initial_assets + 1);
        assert_eq!(after_target_preload, initial_assets + 2);
        assert_eq!(after_compressed_remove, initial_assets + 1);
        assert!(cx.remove_compressed_image_resource(&source).is_none());
    });
}

#[test]
fn sized_image_removal_keeps_compressed_preload_explicit() {
    let cx = TestAppContext::single();
    let source = AssetLocation::Embedded(SharedString::from("missing-target-background.webp"));
    let logical_size = size(px(972.0), px(600.0));

    cx.update(|cx| {
        let initial_assets = cx.asset_entries.len();

        let _ = cx.preload_compressed_image_resources([source.clone()]);
        let _ = cx.preload_sized_images([source.clone()], logical_size, 1.0, ObjectFit::Cover);
        assert_eq!(cx.asset_entries.len(), initial_assets + 2);

        let target = cx.remove_sized_image(&source, logical_size, 1.0, ObjectFit::Cover);
        let after_target_remove = cx.asset_entries.len();
        let compressed = cx.remove_compressed_image_resource(&source);

        assert!(target.is_some());
        assert_eq!(after_target_remove, initial_assets + 1);
        assert!(compressed.is_some());
        assert_eq!(cx.asset_entries.len(), initial_assets);
    });
}
