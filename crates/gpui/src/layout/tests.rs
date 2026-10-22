use super::engine::TaffyLayoutEngine;
use super::metrics::AvailableSpace;
use crate::{
    AppContext as _, ElementId, GlobalElementId, Style, TestAppContext, WindowOptions, px, size,
};
use std::{cell::Cell, rc::Rc};

fn layout_identity(name: &'static str) -> Option<GlobalElementId> {
    Some(GlobalElementId::from_path(&[ElementId::from(name)]))
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

            let root = engine.request_layout(None, style, px(16.), 1.0, &[]);
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

            let root = engine.request_measured_layout(None, style, px(16.), 1.0, |_, _, _, _| {
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
                let left = engine.request_layout(layout_identity("left"), moving, px(16.), 1., &[]);
                let mut fixed = Style::default();
                fixed.size.width = px(45.).into();
                fixed.size.height = px(20.).into();
                let right =
                    engine.request_layout(layout_identity("right"), fixed, px(16.), 1., &[]);
                let root = engine.request_layout(
                    layout_identity("root"),
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
                    layout_identity("left"),
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let center = middle.then(|| {
                    engine.request_layout(
                        layout_identity("center"),
                        Style::default(),
                        px(16.),
                        1.,
                        &[],
                    )
                });
                let right = engine.request_layout(
                    layout_identity("right"),
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
                    layout_identity("root"),
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
            let center_identity = layout_identity("center").unwrap();
            engine.retain_committed_elements(|id| id != &center_identity, true);
            assert_eq!(engine.taffy.total_node_count(), 3);
        })
        .unwrap();
}

#[test]
fn native_taffy_subtree_snapshot_preserves_preorder_without_child_vec_allocations() {
    let mut engine = TaffyLayoutEngine::new();
    let leaf = engine.request_layout(layout_identity("leaf"), Style::default(), px(16.), 1., &[]);
    let left = engine.request_layout(
        layout_identity("left"),
        Style::default(),
        px(16.),
        1.,
        &[leaf],
    );
    let right = engine.request_layout(layout_identity("right"), Style::default(), px(16.), 1., &[]);
    let root = engine.request_layout(
        layout_identity("root"),
        Style::default(),
        px(16.),
        1.,
        &[left, right],
    );
    let mut preorder = Vec::new();
    engine.collect_subtree_nodes_into(root, &mut preorder);
    assert_eq!(preorder, vec![root, left, leaf, right]);
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
                    layout_identity("left"),
                    Style::default(),
                    px(16.),
                    1.,
                    &[],
                );
                let right = engine.request_layout(
                    layout_identity("right"),
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
                    layout_identity("root"),
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
fn native_taffy_keeps_unchanged_pure_measure_when_absolute_sibling_changes(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    let count = Rc::new(Cell::new(0));
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let build = |engine: &mut TaffyLayoutEngine, sibling_height: f32| {
                let counter = count.clone();
                let leaf = engine.request_fingerprinted_layout(
                    layout_identity("leaf"),
                    Style::default(),
                    px(16.),
                    1.,
                    Some(41),
                    move |_, _, _, _| {
                        counter.set(counter.get() + 1);
                        size(px(36.), px(12.))
                    },
                );
                let mut style = Style::default();
                style.position = crate::Position::Absolute;
                style.size.height = px(sibling_height).into();
                let sibling =
                    engine.request_layout(layout_identity("overlay"), style, px(16.), 1., &[]);
                engine.request_layout(
                    layout_identity("root"),
                    Style::default(),
                    px(16.),
                    1.,
                    &[leaf, sibling],
                )
            };
            let root = build(&mut engine, 20.);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            assert!(count.get() > 0);
            let measured = count.get();
            engine.clear();
            let root = build(&mut engine, 70.);
            assert!(engine.taffy.dirty(root.into()).unwrap());
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            assert_eq!(measured, count.get());
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_keeps_cached_subtrees_until_committed_unmount(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child_identity = layout_identity("child").unwrap();
            let root_identity = layout_identity("root").unwrap();
            let child = engine.request_layout(
                Some(child_identity.clone()),
                Style::default(),
                px(16.),
                1.,
                &[],
            );
            let root = engine.request_layout(
                Some(root_identity.clone()),
                Style::default(),
                px(16.),
                1.,
                &[child],
            );
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.clear();
            engine.retain_committed_elements(
                |id| id == &root_identity || id == &child_identity,
                true,
            );
            assert_eq!(engine.taffy.total_node_count(), 2);

            // The committed view is replayed with no request_layout calls.
            // It is still mounted, so native Taffy nodes and cached data survive.
            engine.clear();
            engine.retain_committed_elements(
                |id| id == &root_identity || id == &child_identity,
                true,
            );
            assert_eq!(engine.taffy.total_node_count(), 2);
            assert_eq!(engine.taffy.children(root.into()).unwrap().len(), 1);

            // Only the exact unmounted element is reclaimed.
            engine.retain_committed_elements(|id| id == &root_identity, true);
            assert_eq!(engine.taffy.total_node_count(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn native_taffy_prunes_unused_layout_slots_but_not_replayed_or_degraded_slots(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, _, _| {
            let mut engine = TaffyLayoutEngine::new();
            let owner = layout_identity("multi-layout-owner").unwrap();
            let request = |engine: &mut TaffyLayoutEngine| {
                engine.request_layout(Some(owner.clone()), Style::default(), px(16.), 1., &[])
            };

            let first = request(&mut engine);
            let second = request(&mut engine);
            assert_ne!(first, second);
            engine.clear();
            engine.retain_committed_elements(|id| id == &owner, true);
            assert_eq!(engine.taffy.total_node_count(), 2);

            // A cached subtree can paint without requesting any layout this frame.
            engine.clear();
            engine.retain_committed_elements(|id| id == &owner, true);
            assert_eq!(engine.taffy.total_node_count(), 2);

            // A failed/degraded frame may have visited only some of its slots.
            assert_eq!(request(&mut engine), first);
            engine.clear();
            engine.retain_committed_elements(|id| id == &owner, false);
            assert_eq!(engine.taffy.total_node_count(), 2);

            // The next committed frame visits that owner with only one slot:
            // its old extra Taffy allocation must be released.
            assert_eq!(request(&mut engine), first);
            engine.clear();
            engine.retain_committed_elements(|id| id == &owner, true);
            assert_eq!(engine.taffy.total_node_count(), 1);
        })
        .unwrap();
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
            let first = engine.request_fingerprinted_layout(
                layout_identity("slot"),
                Style::default(),
                px(16.),
                scale_factor,
                Some(1),
                |_, _, _, _| size(px(20.), px(15.)),
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
                engine.request_layout(layout_identity("slot"), style, px(16.), scale_factor, &[]);
            assert_eq!(first, reused);
            assert!(engine.taffy.get_node_context(reused.into()).is_none());
            assert!(engine.taffy.dirty(reused.into()).unwrap());
            engine.compute_layout(reused, AvailableSpace::min_size(), window, cx);
            assert_eq!(
                engine.layout_bounds(reused, scale_factor).size.width,
                px(80.)
            );
            engine.clear();

            let restored = engine.request_fingerprinted_layout(
                layout_identity("slot"),
                Style::default(),
                px(16.),
                scale_factor,
                Some(2),
                |_, _, _, _| size(px(36.), px(12.)),
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
                engine.request_impure_measured_layout(
                    layout_identity("impure"),
                    Style::default(),
                    px(16.),
                    1.,
                    99,
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
                engine.request_layout(layout_identity("root"), Style::default(), px(16.), 1., &[]);
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
                engine.request_layout(layout_identity("root"), Style::default(), px(16.), 1., &[]);
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
