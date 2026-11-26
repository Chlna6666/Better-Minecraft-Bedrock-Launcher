use super::engine::TaffyLayoutEngine;
use super::metrics::AvailableSpace;
use crate::{
    AppContext as _, ElementId, GlobalElementId, Style, TestAppContext, WindowOptions, px, size,
};
use std::{cell::Cell, rc::Rc};

/// Test identity for the native key.
fn element_identity(name: &'static str) -> GlobalElementId {
    GlobalElementId::from_path(&[ElementId::from(name)])
}

/// The integer key the layout engine claims.
fn layout_identity(name: &'static str) -> Option<u64> {
    let id = element_identity(name);
    Some(crate::fast::layout_key::key_for_path(id.0.as_ref()))
}

#[gpui::test]
fn taffy_014_leaf_layout_applies_fixed_size_without_measure_context(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let bounds = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let mut style = Style::default();
            style.size.width = px(80.).into();
            style.size.height = px(30.).into();

            let root = engine.request_layout(None, false,style, px(16.), 1.0, &[]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(root, 1.0)
        })
        .unwrap();

    assert_eq!(bounds.size, size(px(80.), px(30.)));
}

#[gpui::test]
fn taffy_014_measured_leaf_keeps_style_constraints(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let bounds = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let mut style = Style::default();
            style.size.width = px(80.).into();
            style.min_size.height = px(30.).into();

            let root = engine.request_measured_layout(None, false,style, px(16.), 1.0, |_, _, _, _| {
                size(px(20.), px(12.))
            });
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(root, 1.0)
        })
        .unwrap();

    assert_eq!(bounds.size, size(px(80.), px(30.)));
}

#[gpui::test]
fn native_taffy_reuses_stable_element_nodes_and_updates_dirty_geometry(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine, width: f32| {
                let mut moving = Style::default();
                moving.size.width = px(width).into();
                moving.size.height = px(20.).into();
                let left = engine.request_layout(layout_identity("left"), false, moving, px(16.), 1., &[]);
                let mut fixed = Style::default();
                fixed.size.width = px(45.).into();
                fixed.size.height = px(20.).into();
                let right =
                    engine.request_layout(layout_identity("right"), false, fixed, px(16.), 1., &[]);
                let root = engine.request_layout(
                    layout_identity("root"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[left, right],
                );
                (left, right, root)
            };
            let first = build(&mut engine, 20.);
            engine.compute_layout(first.2, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.taffy.total_node_count(), 3);
            engine.clear();

            let same = build(&mut engine, 20.);
            assert_eq!(first, same);
            engine.compute_layout(same.2, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_cache_metrics(), (1, 0));
            engine.clear();

            let changed = build(&mut engine, 70.);
            assert_eq!(first, changed);
            assert!(engine.taffy.dirty(changed.2.into()).unwrap());
            engine.compute_layout(changed.2, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_bounds(changed.0, 1.).size.width, px(70.));
            assert_eq!(engine.layout_bounds(changed.1, 1.).size.width, px(45.));
            assert_eq!(engine.taffy.total_node_count(), 3);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_preserves_identity_across_sibling_insertion_and_removal(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine, middle: bool| {
                let left = engine.request_layout(
                    layout_identity("left"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let center = middle.then(|| {
                    engine.request_layout(
                        layout_identity("center"), false,
                        Style::default(),
                        px(16.),
                        1.,
                        &[],
                    )
                });
                let right = engine.request_layout(
                    layout_identity("right"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let children = [Some(left), center, Some(right)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                let root = engine.request_layout(
                    layout_identity("root"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &children,
                );
                (left, right, root)
            };
            let first = build(&mut engine, false);
            engine.compute_layout(first.2, AvailableSpace::min_size(), window, cx);
            engine.clear();

            let inserted = build(&mut engine, true);
            assert_eq!((inserted.0, inserted.1, inserted.2), first);
            engine.compute_layout(inserted.2, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.taffy.children(inserted.2.into()).unwrap().len(), 3);
            engine.clear();

            let removed = build(&mut engine, false);
            assert_eq!(removed, first);
            engine.compute_layout(removed.2, AvailableSpace::min_size(), window, cx);
            engine.clear();
            assert_eq!(engine.taffy.total_node_count(), 3);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_compares_stable_child_order_without_allocating_a_child_vector(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine, reverse: bool| {
                let left = engine.request_layout(
                    layout_identity("left"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let right = engine.request_layout(
                    layout_identity("right"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let children = if reverse {
                    [right, left]
                } else {
                    [left, right]
                };
                let root = engine.request_layout(
                    layout_identity("root"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &children,
                );
                (left, right, root)
            };

            let first = build(&mut engine, false);
            engine.compute_layout(first.2, AvailableSpace::min_size(), window, cx);
            engine.clear();

            let same = build(&mut engine, false);
            assert_eq!(same, first);
            assert!(!engine.taffy.dirty(same.2.into()).unwrap());
            engine.compute_layout(same.2, AvailableSpace::min_size(), window, cx);
            engine.clear();

            let reversed = build(&mut engine, true);
            assert_eq!(reversed, first);
            assert!(engine.taffy.dirty(reversed.2.into()).unwrap());
            assert_eq!(
                engine.taffy.child_at_index(reversed.2.into(), 0).unwrap(),
                reversed.1.into(),
            );
            engine.compute_layout(reversed.2, AvailableSpace::min_size(), window, cx);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_keeps_unchanged_size_measure_when_absolute_sibling_changes(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine, sibling_height: f32| {
                let leaf = engine.request_size_layout(
                    layout_identity("leaf"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    super::SizeMeasurement::Intrinsic(size(px(36.), px(12.))),
                );
                let mut style = Style::default();
                style.position = crate::Position::Absolute;
                style.size.height = px(sibling_height).into();
                let sibling =
                    engine.request_layout(layout_identity("overlay"), false, style, px(16.), 1., &[]);
                let root = engine.request_layout(
                    layout_identity("root"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    &[leaf, sibling],
                );
                (root, leaf)
            };
            let (root, leaf) = build(&mut engine, 20.);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let bounds = engine.layout_bounds(leaf, 1.);
            engine.clear();
            let (root, reused_leaf) = build(&mut engine, 70.);
            assert_eq!(leaf, reused_leaf);
            assert!(!engine.taffy.dirty(leaf.into()).unwrap());
            assert!(engine.taffy.dirty(root.into()).unwrap());
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            assert_eq!(bounds.size, engine.layout_bounds(leaf, 1.).size);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_keeps_claimed_subtrees_and_releases_unclaimed_nodes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window.update(cx, |_, window, cx| {
        let mut engine = TaffyLayoutEngine::new();
        let recording = engine.claim_log_len();
        let child = engine.request_layout(layout_identity("child"), false, Style::default(), px(16.), 1., &[]);
        let root = engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[child]);
        engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
        let claims = super::LayoutClaimBlocks::request(recording..engine.claim_log_len());
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 2);
        assert!(engine.try_keep_retained(&claims, root).is_some());
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 2);
        assert_eq!(engine.taffy.children(root.into()).unwrap().len(), 1);
        assert_eq!(engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[]), root);
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 1);
    }).unwrap();
}

#[gpui::test]
fn native_taffy_abort_discards_native_claims_without_reusing_ids(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window.update(cx, |_, _, _| {
        let mut engine = TaffyLayoutEngine::new();
        let recording = engine.claim_log_len();
        let root = engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[]);
        let claims = super::LayoutClaimBlocks::request(recording..engine.claim_log_len());
        engine.clear();
        assert!(engine.try_keep_retained(&claims, root).is_some());
        engine.abort_frame();
        assert_eq!(engine.taffy.total_node_count(), 0);
        assert!(engine.try_keep_retained(&claims, root).is_none());
        let replacement = engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[]);
        assert_ne!(root, replacement);
    }).unwrap();
}

#[gpui::test]
fn native_taffy_reused_node_changes_between_measured_leaf_and_container(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let scale_factor = window.scale_factor();
            let first = engine.request_size_layout(
                layout_identity("slot"), false,
                Style::default(),
                px(16.),
                scale_factor,
                super::SizeMeasurement::Intrinsic(size(px(20.), px(15.))),
            );
            engine.compute_layout(first, AvailableSpace::min_size(), window, cx);
            assert_eq!(
                engine.layout_bounds(first, scale_factor).size.width,
                px(20.)
            );
            engine.clear();

            let mut style = Style::default();
            style.size.width = px(80.).into();
            style.size.height = px(30.).into();
            let reused =
                engine.request_layout(layout_identity("slot"), false, style, px(16.), scale_factor, &[]);
            assert_eq!(first, reused);
            assert!(engine.taffy.get_node_context(reused.into()).is_none());
            assert!(engine.taffy.dirty(reused.into()).unwrap());
            engine.compute_layout(reused, AvailableSpace::min_size(), window, cx);
            assert_eq!(
                engine.layout_bounds(reused, scale_factor).size.width,
                px(80.)
            );
            engine.clear();

            let restored = engine.request_size_layout(
                layout_identity("slot"), false,
                Style::default(),
                px(16.),
                scale_factor,
                super::SizeMeasurement::Intrinsic(size(px(36.), px(12.))),
            );
            assert_eq!(restored, first);
            assert!(engine.taffy.dirty(restored.into()).unwrap());
            engine.compute_layout(restored, AvailableSpace::min_size(), window, cx);
            assert_eq!(
                engine.layout_bounds(restored, scale_factor).size.width,
                px(36.)
            );
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_marks_impure_measure_dirty_on_every_commit(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    let count = Rc::new(Cell::new(0));
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine| {
                let counter = count.clone();
                engine.request_measured_layout(
                    layout_identity("impure"), false,
                    Style::default(),
                    px(16.),
                    1.,
                    move |_, _, _, _| {
                        counter.set(counter.get() + 1);
                        size(px(22.), px(14.))
                    },
                )
            };
            let root = build(&mut engine);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let first = count.get();
            assert!(first > 0);
            engine.clear();
            let same = build(&mut engine);
            assert_eq!(same, root);
            assert!(engine.taffy.dirty(same.into()).unwrap());
            engine.compute_layout(same, AvailableSpace::min_size(), window, cx);
            assert!(count.get() > first);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_recomputes_when_root_constraints_change(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let id =
                engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[]);
            engine.compute_layout(
                id,
                size(
                    AvailableSpace::Definite(px(100.)),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            engine.clear();
            let reused =
                engine.request_layout(layout_identity("root"), false, Style::default(), px(16.), 1., &[]);
            assert_eq!(id, reused);
            engine.compute_layout(
                reused,
                size(
                    AvailableSpace::Definite(px(200.)),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            assert_eq!(engine.layout_cache_metrics(), (0, 1));
        })
        .unwrap();
}

#[test]
fn claim_normalization_compacts_phases_without_crossing_the_boundary() {
    use super::LayoutClaimBlocks;

    let mut claims = LayoutClaimBlocks::default();
    for span in [4..6, 0..2, 2..4, 10..12, 11..13] {
        claims.push_request(span);
    }
    for span in [15..17, 13..15, 6..7] {
        claims.push_prepaint(span);
    }
    claims.normalize();
    assert_eq!(claims.request_blocks(), &[0..6, 10..12, 11..13]);
    assert_eq!(claims.prepaint_blocks(), &[6..7, 13..17]);

    let mut merged = LayoutClaimBlocks::default();
    for index in (0..32).rev() {
        merged.push_request(index..index + 1);
        merged.push_prepaint(index + 32..index + 33);
    }
    assert!(merged.blocks.spilled());
    merged.normalize();
    assert_eq!(merged.request_blocks(), &[0..32]);
    assert_eq!(merged.prepaint_blocks(), &[32..64]);
    assert!(!merged.blocks.spilled());
}
