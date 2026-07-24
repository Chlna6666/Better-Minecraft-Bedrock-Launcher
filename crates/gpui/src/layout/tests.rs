use super::engine::TaffyLayoutEngine;
use super::metrics::AvailableSpace;
use crate::{AppContext as _, Style, TestAppContext, WindowOptions, px, size};
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn retained_layout_cache_reuses_clean_style_tree(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (first_hits, first_misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            let metrics = engine.layout_cache_metrics();
            engine.clear();
            metrics
        })
        .unwrap();

    assert_eq!(first_hits, 0);
    assert_eq!(first_misses, 1);

    let (second_hits, second_misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_cache_metrics()
        })
        .unwrap();

    assert_eq!(second_hits, 1);
    assert_eq!(second_misses, 0);
}

#[gpui::test]
fn retained_layout_cache_restores_large_tree_in_one_pass(cx: &mut TestAppContext) {
    const CHILD_COUNT: usize = 1_500;

    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (cache_metrics, frame_metrics, restored_child_count) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let children = (0..CHILD_COUNT)
                .map(|_| engine.request_layout(Style::default(), px(16.), 1.0, &[]))
                .collect::<Vec<_>>();
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &children);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            for child in &children {
                engine.layout_bounds(*child, 1.0);
            }
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let children = (0..CHILD_COUNT)
                .map(|_| engine.request_layout(Style::default(), px(16.), 1.0, &[]))
                .collect::<Vec<_>>();
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &children);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);

            let restored_child_count = children
                .iter()
                .filter(|child| engine.absolute_layout_bounds.contains_key(child))
                .count();
            (
                engine.layout_cache_metrics(),
                engine.frame_metrics(),
                restored_child_count,
            )
        })
        .unwrap();

    assert_eq!(cache_metrics, (1, 0));
    assert_eq!(frame_metrics.cache_reused_roots, 1);
    assert_eq!(frame_metrics.cache_saved_roots, CHILD_COUNT);
    assert_eq!(restored_child_count, CHILD_COUNT);
}

#[gpui::test]
fn retained_layout_cache_misses_when_available_space_changes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (hits, misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
            engine.compute_layout(
                root,
                size(
                    AvailableSpace::Definite(px(100.)),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
            engine.compute_layout(
                root,
                size(
                    AvailableSpace::Definite(px(200.)),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            engine.layout_cache_metrics()
        })
        .unwrap();

    assert_eq!(hits, 0);
    assert_eq!(misses, 1);
}

#[gpui::test]
fn retained_layout_cache_ignores_paint_only_style_changes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (hits, misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let mut style = Style::default();
            style.background = Some(crate::red().into());
            let root = engine.request_layout(style.clone(), px(16.), 1.0, &[]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            style.background = Some(crate::blue().into());
            let root = engine.request_layout(style, px(16.), 1.0, &[]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_cache_metrics()
        })
        .unwrap();

    assert_eq!(hits, 1);
    assert_eq!(misses, 0);
}

#[gpui::test]
fn retained_layout_cache_misses_when_measured_fingerprint_changes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (hits, misses, second_bounds) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(1),
                |_, _, _, _| size(px(10.), px(10.)),
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(2),
                |_, _, _, _| size(px(10.), px(40.)),
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let second_bounds = engine.layout_bounds(child, 1.0);
            let (hits, misses) = engine.layout_cache_metrics();
            (hits, misses, second_bounds)
        })
        .unwrap();

    assert_eq!(hits, 0);
    assert_eq!(misses, 1);
    assert!(second_bounds.size.height > px(10.));
}

#[gpui::test]
fn retained_layout_cache_reuses_fingerprinted_measured_subtrees(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (hits, misses, first_bounds, second_bounds) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(11),
                |_, _, _, _| size(px(20.), px(12.)),
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let first_bounds = engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(11),
                |_, _, _, _| size(px(20.), px(12.)),
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let second_bounds = engine.layout_bounds(child, 1.0);
            let (hits, misses) = engine.layout_cache_metrics();
            (hits, misses, first_bounds, second_bounds)
        })
        .unwrap();

    assert_eq!((hits, misses), (1, 0));
    assert_eq!(second_bounds, first_bounds);
}

#[gpui::test]
fn retained_layout_cache_replays_impure_measured_nodes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let first_measure_count = Rc::new(Cell::new(0));
    let second_measure_count = Rc::new(Cell::new(0));

    let (hits, misses, first_bounds, second_bounds) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_impure_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                11,
                {
                    let first_measure_count = first_measure_count.clone();
                    move |_, _, _, _| {
                        first_measure_count.set(first_measure_count.get() + 1);
                        size(px(20.), px(12.))
                    }
                },
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let first_bounds = engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child = engine.request_impure_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                11,
                {
                    let second_measure_count = second_measure_count.clone();
                    move |_, _, _, _| {
                        second_measure_count.set(second_measure_count.get() + 1);
                        size(px(20.), px(12.))
                    }
                },
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            let second_bounds = engine.layout_bounds(child, 1.0);
            let (hits, misses) = engine.layout_cache_metrics();
            (hits, misses, first_bounds, second_bounds)
        })
        .unwrap();

    assert_eq!((hits, misses), (1, 0));
    assert_eq!(second_bounds, first_bounds);
    assert!(first_measure_count.get() > 0);
    assert_eq!(second_measure_count.get(), 1);
}

#[gpui::test]
fn retained_layout_cache_skips_pure_measured_nodes(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let first_measure_count = Rc::new(Cell::new(0));
    let second_measure_count = Rc::new(Cell::new(0));

    let (hits, misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(11),
                {
                    let first_measure_count = first_measure_count.clone();
                    move |_, _, _, _| {
                        first_measure_count.set(first_measure_count.get() + 1);
                        size(px(20.), px(12.))
                    }
                },
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child = engine.request_measured_layout_with_fingerprint(
                Style::default(),
                px(16.),
                1.0,
                Some(11),
                {
                    let second_measure_count = second_measure_count.clone();
                    move |_, _, _, _| {
                        second_measure_count.set(second_measure_count.get() + 1);
                        size(px(20.), px(12.))
                    }
                },
            );
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            let (hits, misses) = engine.layout_cache_metrics();
            (hits, misses)
        })
        .unwrap();

    assert_eq!((hits, misses), (1, 0));
    assert!(first_measure_count.get() > 0);
    assert_eq!(second_measure_count.get(), 0);
}

#[gpui::test]
fn retained_layout_cache_skips_unfingerprinted_measured_subtrees(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });

    let (hits, misses) = window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let child =
                engine.request_measured_layout(Style::default(), px(16.), 1.0, |_, _, _, _| {
                    size(px(20.), px(12.))
                });
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_bounds(child, 1.0);
            engine.layout_bounds(root, 1.0);
            engine.clear();

            let child =
                engine.request_measured_layout(Style::default(), px(16.), 1.0, |_, _, _, _| {
                    size(px(20.), px(12.))
                });
            let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
            engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
            engine.layout_cache_metrics()
        })
        .unwrap();

    assert_eq!((hits, misses), (0, 1));
}
