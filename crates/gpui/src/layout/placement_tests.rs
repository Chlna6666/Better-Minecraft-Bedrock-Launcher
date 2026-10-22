use super::{AvailableSpace, TaffyLayoutEngine};
use crate::{
    AppContext as _, ElementId, GlobalElementId, Style, TestAppContext, WindowOptions, px, size,
};
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn interaction_flags_preserve_native_pure_measure_cache(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let count = Rc::new(Cell::new(0));
            let scale_factor = window.scale_factor();
            let owner = GlobalElementId::from_path(&[ElementId::from("pure-measure")]);
            let request = |engine: &mut TaffyLayoutEngine, allow_scroll: bool| {
                let mut style = Style::default();
                style.allow_concurrent_scroll = allow_scroll;
                let count = count.clone();
                engine.request_fingerprinted_layout(
                    Some(owner.clone()),
                    style,
                    px(16.),
                    scale_factor,
                    Some(1),
                    move |_, _, _, _| {
                        count.set(count.get() + 1);
                        size(px(20.), px(10.))
                    },
                )
            };
            let first = request(&mut engine, false);
            engine.compute_layout(first, AvailableSpace::min_size(), window, cx);
            let measured = count.get();
            assert!(measured > 0);
            engine.clear();
            let second = request(&mut engine, true);
            assert_eq!(first, second);
            assert!(!engine.taffy.dirty(second.into()).unwrap());
            engine.compute_layout(second, AvailableSpace::min_size(), window, cx);
            assert_eq!(count.get(), measured);
            assert_eq!(engine.layout_cache_metrics(), (1, 0));
        })
        .unwrap();
}

#[gpui::test]
fn measured_root_bounds_follow_later_parent_placement(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let mut child_style = Style::default();
            child_style.size.width = px(20.).into();
            child_style.size.height = px(10.).into();
            let child = engine.request_layout(None, child_style, px(16.), 1., &[]);
            engine.compute_layout(child, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_bounds(child, 1.).origin.x, px(0.));

            let mut parent_style = Style::default();
            parent_style.padding.left = px(15.).into();
            let parent = engine.request_layout(None, parent_style, px(16.), 1., &[child]);
            engine.compute_layout(parent, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_bounds(child, 1.).origin.x, px(15.));
            assert_eq!(
                engine.layout_bounds(parent, 1.).size,
                size(px(35.), px(10.))
            );
        })
        .unwrap();
}

#[gpui::test]
fn standalone_child_compute_invalidates_cached_parent_placement(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
            .unwrap()
    });
    window
        .update(cx, |_, window, cx| {
            let mut engine = TaffyLayoutEngine::new();
            let mut child_style = Style::default();
            child_style.size.width = px(20.).into();
            child_style.size.height = px(10.).into();
            let child = engine.request_layout(None, child_style, px(16.), 1., &[]);
            let mut parent_style = Style::default();
            parent_style.padding.left = px(15.).into();
            let parent = engine.request_layout(None, parent_style, px(16.), 1., &[child]);
            engine.compute_layout(parent, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_bounds(child, 1.).origin.x, px(15.));

            engine.compute_layout(child, AvailableSpace::min_size(), window, cx);
            assert!(engine.taffy.dirty(parent.into()).unwrap());
            engine.compute_layout(parent, AvailableSpace::min_size(), window, cx);
            assert_eq!(engine.layout_bounds(child, 1.).origin.x, px(15.));
        })
        .unwrap();
}
