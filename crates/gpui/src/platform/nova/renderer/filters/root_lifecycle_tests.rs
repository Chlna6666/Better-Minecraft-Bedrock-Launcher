use super::*;

fn test_resource_set_id(index: u32) -> ResourceSetId {
    ResourceSetId::from_parts(index, 1)
}

fn test_texture_id(index: u32) -> TextureId {
    TextureId::from_parts(index, 1)
}

fn test_texture_view_id(index: u32) -> TextureViewId {
    TextureViewId::from_parts(index, 1)
}

#[test]
fn warmed_zero_radius_popup_targets_rebind_without_reallocating_textures() {
    let make_variant = |index: u32| BackdropBlurVariantTargets {
        config: test_zero_radius_blur_config(index),
        levels: vec![BackdropBlurLevelTarget {
            byte_size: 64 * 32 * 4,
            texture: test_texture_id(index + 1),
            texture_view: test_texture_view_id(index + 1),
            pass_resource_sets: vec![test_resource_set_id(index + 1)],
        }],
        target_resource_sets: vec![test_resource_set_id(index + 11)],
    };
    let mut targets = BackdropBlurTargets {
        source: None,
        source_pass_resource_sets: Vec::new(),
        isolated_sources: Vec::new(),
        active_isolated_source_count: 0,
        variants: (0..6).map(make_variant).collect(),
        active_variant_count: 6,
        retained_root_variant: Some(5),
        root_binding: None,
    };
    let original_root_view = targets.variants[5].levels[0].texture_view;
    assert!(
        targets.root_color_texture_identity().is_none(),
        "a logical Filter slot cannot claim native Root residency without its owner"
    );
    let root_level = &targets.variants[5].levels[0];
    let original_root_identity = (root_level.texture, root_level.texture_view);
    let mut residency = crate::platform::nova::renderer::root_color::RootColor::default();
    residency.target = Some(RootColorTarget {
        target: RenderTarget {
            byte_size: root_level.byte_size,
            texture: root_level.texture,
            texture_view: root_level.texture_view,
        },
        pass_resource_sets: root_level.pass_resource_sets.clone(),
        composite_resource_sets: targets.variants[5].target_resource_sets.clone(),
    });
    targets.root_binding = residency.target.as_ref().map(|root| root.target);
    // Match the production constructor: only native RootColorTarget owns
    // ResourceSet IDs. The zero-radius Filter slot borrows Texture/View only.
    targets.variants[5].levels[0].pass_resource_sets.clear();
    targets.variants[5].target_resource_sets.clear();
    assert_eq!(
        targets.retained_root_texture_view(),
        Some(original_root_view)
    );
    assert!(targets.can_preserve_root_color_capture());
    assert!(
        residency.target.is_some(),
        "the native Root allocation must have one dedicated owner"
    );
    assert_eq!(
        targets.root_color_texture_identity(),
        Some(original_root_identity)
    );
    assert_eq!(
        targets.byte_size(),
        5 * 64 * 32 * 4,
        "a borrowed zero-radius Root descriptor must not double-count GPU residency"
    );
    assert!(targets.variants[5].levels[0].pass_resource_sets.is_empty());
    assert!(targets.variants[5].target_resource_sets.is_empty());
    assert_eq!(
        residency
            .target
            .as_ref()
            .unwrap()
            .pass_resource_sets
            .len(),
        1
    );
    assert_eq!(
        residency
            .target
            .as_ref()
            .unwrap()
            .composite_resource_sets
            .len(),
        1
    );
    residency.record_present(Some(original_root_identity), true);
    let root_version = residency
        .content_version()
        .expect("submitted Root");
    assert!(residency.resident_on(targets.root_color_texture_identity()));
    for (count, root) in [(4_u32, 3_u32), (5, 4), (6, 5), (4, 3)] {
        let configs = (0..count)
            .map(test_zero_radius_blur_config)
            .collect::<Vec<_>>();
        assert!(targets.rebind_zero_radius_layout(&configs, false, Some(root), None));
        assert_eq!(targets.active_variant_count, count as usize);
        assert_eq!(targets.retained_root_variant, Some(root as usize));
        assert_eq!(
            targets.retained_root_texture_view(),
            Some(original_root_view)
        );
        assert_eq!(
            targets.root_color_texture_identity(),
            Some(original_root_identity)
        );
        assert!(residency.resident_on(targets.root_color_texture_identity()));
        let native_owner = residency.target.as_ref().unwrap();
        let logical = &targets.variants[root as usize];
        assert!(
            logical.levels[0].pass_resource_sets.is_empty(),
            "Root Graph must not mirror native pass ResourceSets"
        );
        assert!(
            logical.target_resource_sets.is_empty(),
            "Root Graph must not mirror native composite ResourceSets"
        );
        assert_eq!(native_owner.pass_resource_sets.len(), 1);
        assert_eq!(native_owner.composite_resource_sets.len(), 1);
        residency.record_present(targets.root_color_texture_identity(), false);
        assert_eq!(residency.content_version(), Some(root_version));
        assert!(
            targets.can_preserve_root_color_capture(),
            "zero-radius popup count churn must not discard root GPU identity"
        );
        assert_eq!(
            targets.variants[root as usize].levels[0].texture_view,
            original_root_view,
        );
        assert_eq!(targets.variants.len(), 6);
        assert!(!targets.rebind_zero_radius_layout(
            &[test_backdrop_blur_config(2, 2)],
            false,
            Some(0),
            None,
        ));
    }
    // Root can disappear while the physical allocation remains in a warm
    // spare slot. Its later return must borrow that same Texture/View, and
    // any old content epoch must be cleared while no Root is presented.
    let temporarily_no_root = (0..3).map(test_zero_radius_blur_config).collect::<Vec<_>>();
    assert!(!targets.root_capture_is_compatible(&temporarily_no_root, None));
    assert!(targets.rebind_zero_radius_layout(&temporarily_no_root, false, None, None,));
    assert!(targets.root_capture_is_compatible(&temporarily_no_root, None));
    assert_eq!(
        residency
            .target
            .as_ref()
            .unwrap()
            .target
            .texture_view,
        original_root_view
    );
    residency.record_present(None, false);
    assert_eq!(residency.content_version(), None);
    let reintroduced_root = (0..4).map(test_zero_radius_blur_config).collect::<Vec<_>>();
    assert!(!targets.root_capture_is_compatible(&reintroduced_root, Some(3)));
    assert!(targets.rebind_zero_radius_layout(&reintroduced_root, false, Some(3), None,));
    assert!(targets.root_capture_is_compatible(&reintroduced_root, Some(3)));
    assert_eq!(
        targets.root_color_texture_identity(),
        Some(original_root_identity)
    );
    residency.record_present(Some(original_root_identity), true);
    assert!(residency.resident_on(targets.root_color_texture_identity()));
    let mut without_owner = targets.clone();
    without_owner.root_binding = None;
    assert!(
        !without_owner.root_capture_is_compatible(&reintroduced_root, Some(3)),
        "a Filter scratch allocation must not silently become a Native Root"
    );

    // Binding growth replaces descriptor IDs only on the native owner.
    // Both Render Graph consumers must read the new sets without any
    // descriptor IDs cached on the logical Filter Variant.
    let new_pass_set = test_resource_set_id(901);
    let new_composite_set = test_resource_set_id(902);
    let logical_config = test_zero_radius_blur_config(3);
    residency
        .target
        .as_mut()
        .unwrap()
        .pass_resource_sets[0] = new_pass_set;
    residency
        .target
        .as_mut()
        .unwrap()
        .composite_resource_sets[0] = new_composite_set;
    assert!(targets.variants[3].levels[0].pass_resource_sets.is_empty());
    assert!(targets.variants[3].target_resource_sets.is_empty());
    assert_eq!(
        targets.resource_set_for_config(logical_config, 0, residency.target.as_ref()),
        Some(new_composite_set)
    );
    let composite = targets
        .direct_composite_target(logical_config, 0, residency.target.as_ref())
        .expect("the Render Graph resolves the native Root Color");
    assert_eq!(composite.source_resource_set, new_pass_set);
    assert_eq!(composite.texture_view, original_root_view);
    assert!(targets.variants[3].levels[0].pass_resource_sets.is_empty());
    assert!(targets.variants[3].target_resource_sets.is_empty());
    assert_eq!(
        residency
            .target
            .as_ref()
            .unwrap()
            .pass_resource_sets[0],
        new_pass_set
    );
    assert_eq!(
        residency
            .target
            .as_ref()
            .unwrap()
            .composite_resource_sets[0],
        new_composite_set
    );

    // Mixed zero-radius popup layouts may reassign a spare native target.
    // This fallback must compare against the Root owner's binding capacity,
    // not empty descriptor mirrors on the logical root capture.
    let remapped = [0, 1, 42, 3].map(test_zero_radius_blur_config);
    assert!(
        targets.reuse_layout(&remapped, &[], false, Some(3), None),
        "Root-owned descriptor capacity must not disable warmed slot reuse"
    );
    assert_eq!(
        targets.root_color_texture_identity(),
        Some(original_root_identity)
    );
    assert!(targets.variants[3].levels[0].pass_resource_sets.is_empty());
    assert!(targets.variants[3].target_resource_sets.is_empty());
    assert_eq!(targets.variants.len(), 6);
    assert_eq!(
        targets.resource_set_for_config(remapped[3], 0, residency.target.as_ref()),
        Some(new_composite_set)
    );
    assert_eq!(
        targets
            .direct_composite_target(remapped[3], 0, residency.target.as_ref())
            .unwrap()
            .source_resource_set,
        new_pass_set
    );

    // Replacing the underlying native allocation without a source pass
    // cannot import the old Root Color version, even if a flattened capture
    // index and the target's other descriptor metadata are unchanged.
    let mut different_native = targets.clone();
    different_native.variants[3].levels[0].texture = test_texture_id(777);
    assert!(
        different_native.root_color_texture_identity().is_none(),
        "the borrowed Filter variant may never impersonate a different Root allocation"
    );
    assert!(!residency.resident_on(different_native.root_color_texture_identity()));
    residency.record_present(different_native.root_color_texture_identity(), false);
    assert_eq!(residency.content_version(), None);
    // Only a genuine Native owner replacement, accompanied by a new Root
    // rendering pass, can establish a version for the replacement Texture.
    different_native.root_binding.as_mut().unwrap().texture = test_texture_id(777);
    residency.target.as_mut().unwrap().target.texture = test_texture_id(777);
    assert!(different_native.root_color_texture_identity().is_some());
    residency.record_present(different_native.root_color_texture_identity(), true);
    assert!(residency.resident_on(different_native.root_color_texture_identity()));
    assert_ne!(residency.content_version(), Some(root_version));
    different_native.variants[3].levels[0].texture_view = test_texture_view_id(778);
    assert!(!residency.resident_on(different_native.root_color_texture_identity()));
    residency.record_present(None, false);
    assert_eq!(residency.content_version(), None);

    // A closed Gaussian/isolated popup can leave scratch GPU textures warm.
    // They are independent from the Root Color physical view and must not
    // invalidate its resident content version simply by being allocated.
    targets.variants[4].config = test_backdrop_blur_config(4, 2);
    targets.isolated_sources.push(IsolatedBlurSource {
        index: 4,
        target: RenderTarget {
            byte_size: 64 * 32 * 4,
            texture: test_texture_id(90),
            texture_view: test_texture_view_id(90),
        },
        pass_resource_sets: vec![test_resource_set_id(90)],
    });
    assert_eq!(targets.active_variant_count, 4);
    assert_eq!(targets.active_isolated_source_count, 0);
    assert!(
        targets.can_preserve_root_color_capture(),
        "inactive Gaussian/scratch residency must not discard Root Color"
    );
    targets.variants[1].config = test_backdrop_blur_config(1, 2);
    assert!(
        !targets.can_preserve_root_color_capture(),
        "a live Gaussian source still requires the conservative root proof"
    );
    targets.variants[1].config = test_zero_radius_blur_config(1);
    assert!(targets.can_preserve_root_color_capture());
    targets.isolated_sources[0].target.texture_view = original_root_view;
    assert!(
        !targets.can_preserve_root_color_capture(),
        "aliased scratch View cannot share Root Color residency"
    );
    targets.isolated_sources[0].target.texture_view = test_texture_view_id(90);
    targets.isolated_sources[0].target.texture = targets.variants[3].levels[0].texture;
    assert!(
        !targets.can_preserve_root_color_capture(),
        "two Views of one native Texture cannot be independent Root owners"
    );
}
