use super::{AvailableSpace, TaffyLayoutEngine};
use crate::{AppContext as _, Style, TestAppContext, WindowOptions, px, size};

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
