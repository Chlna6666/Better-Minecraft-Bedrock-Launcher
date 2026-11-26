use gpui::{ScrollDelta, ScrollWheelEvent};

use crate::{self as gpui, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;

#[gpui::test]
fn test_reset_after_paint_before_scroll(cx: &mut TestAppContext) {
    use crate::{
        AppContext, Context, Element, IntoElement, ListState, Render, Styled, Window, div, list,
        point, px, size,
    };

    let cx = cx.add_empty_window();

    let state = ListState::new(5, crate::ListAlignment::Top, px(10.));

    // Ensure that the list is scrolled to the top
    state.scroll_to(gpui::ListOffset {
        item_ix: 0,
        offset_in_item: px(0.0),
    });

    struct TestView(ListState);
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.0.clone(), |_, _, _| {
                div().h(px(10.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    // Paint
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
        cx.new(|_| TestView(state.clone())).into_element()
    });

    // Reset
    state.reset(5);

    // And then receive a scroll event _before_ the next paint
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(1.), px(1.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-500.))),
        ..Default::default()
    });

    // Scroll position should stay at the top of the list
    assert_eq!(state.logical_scroll_top().item_ix, 0);
    assert_eq!(state.logical_scroll_top().offset_in_item, px(0.));
}

#[gpui::test]
fn test_scroll_by_positive_and_negative_distance(cx: &mut TestAppContext) {
    use crate::{
        AppContext, Context, Element, IntoElement, ListState, Render, Styled, Window, div, list,
        point, px, size,
    };

    let cx = cx.add_empty_window();

    let state = ListState::new(5, crate::ListAlignment::Top, px(10.));

    struct TestView(ListState);
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.0.clone(), |_, _, _| {
                div().h(px(20.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    // Paint
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, cx| {
        cx.new(|_| TestView(state.clone())).into_element()
    });

    // Test positive distance: start at item 1, move down 30px
    state.scroll_by(px(30.));

    // Should move to item 2
    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 1);
    assert_eq!(offset.offset_in_item, px(10.));

    // Test negative distance: start at item 2, move up 30px
    state.scroll_by(px(-30.));

    // Should move back to item 1
    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 0);
    assert_eq!(offset.offset_in_item, px(0.));

    // Test zero distance
    state.scroll_by(px(0.));
    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 0);
    assert_eq!(offset.offset_in_item, px(0.));
}

#[gpui::test]
fn test_scroll_handler_receives_updated_visible_range(cx: &mut TestAppContext) {
    use crate::{
        AppContext, Context, Element, IntoElement, ListOffset, ListState, Render, Styled, Window,
        div, list, point, px, size,
    };

    let cx = cx.add_empty_window();
    let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();
    let visible_range = Rc::new(RefCell::new(None));
    state.set_scroll_handler({
        let visible_range = Rc::clone(&visible_range);
        move |event, _, _| {
            *visible_range.borrow_mut() = Some(event.visible_range.clone());
        }
    });

    struct TestView(ListState);
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.0.clone(), |_, _, _| {
                div().h(px(10.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
        cx.new(|_| TestView(state.clone())).into_element()
    });

    cx.update(|window, cx| {
        state.0.borrow_mut().scroll_for_test(
            &ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            },
            px(20.),
            point(px(0.), px(-20.)),
            window,
            cx,
        );
    });

    assert_eq!(*visible_range.borrow(), Some(2..4));
}

#[gpui::test]
fn large_overdraw_bounds_speculative_measurements_and_keeps_row_indices(cx: &mut TestAppContext) {
    use crate::{
        AppContext, Context, Element, IntoElement, ListOffset, ListState, Render, Styled, Window,
        div, list, point, px, size,
    };

    struct TestView(ListState, Rc<RefCell<Vec<usize>>>);
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let measured = self.1.clone();
            list(self.0.clone(), move |index, _, _| {
                measured.borrow_mut().push(index);
                div().h(px(10.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    let cx = cx.add_empty_window();
    let state = ListState::new(1000, crate::ListAlignment::Top, px(10_000.));
    state.scroll_to(ListOffset { item_ix: 500, offset_in_item: px(0.) });
    let measured = Rc::new(RefCell::new(Vec::new()));
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
        cx.new(|_| TestView(state.clone(), measured.clone())).into_element()
    });

    let mut indices = measured.borrow().clone();
    indices.sort_unstable();
    assert_eq!(indices, (468..534).collect::<Vec<_>>());
    assert_eq!(state.item_count(), 1000);
    assert_eq!(state.logical_scroll_top().item_ix, 500);
    assert_eq!(state.bounds_for_item(500).unwrap().origin.y, px(0.));
    assert_eq!(state.bounds_for_item(501).unwrap().origin.y, px(10.));
    assert!(state.bounds_for_item(534).is_none());
}

#[gpui::test]
fn stable_list_scroll_preserves_measured_tree_and_updates_changed_rows(cx: &mut TestAppContext) {
    use crate::{Element, ListState, Styled, div, list, point, px, size};
    use super::tree::Count;
    use std::cell::Cell;
    use sum_tree::Bias;

    let cx = cx.add_empty_window();
    let state = ListState::new(12, crate::ListAlignment::Top, px(0.)).measure_all();
    let taller = Rc::new(Cell::new(false));
    let builds = Rc::new(Cell::new(0));
    let mut draw = || {
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(60.)), |_, _| {
            let taller = taller.clone();
            let builds = builds.clone();
            list(state.clone(), move |index, _, _| {
                builds.set(builds.get() + 1);
                div().w_full().h(if matches!(index, 1 | 3) && taller.get() { px(30.) } else { px(20.) }).into_any()
            }).w_full().h_full()
        });
    };
    draw();
    let old_tree = state.0.borrow().items.clone();
    let before = builds.get();
    state.scroll_by(px(20.));
    draw();
    assert!(builds.get() > before, "exercise the normal visible-row layout path");
    {
        let state = state.0.borrow();
        let mut old = old_tree.cursor::<Count>(());
        let mut new = state.items.cursor::<Count>(());
        old.seek(&Count(1), Bias::Right);
        new.seek(&Count(1), Bias::Right);
        assert!(std::ptr::eq(old.item().unwrap(), new.item().unwrap()),
            "unchanged measured rows must retain their original tree nodes");
    }

    taller.set(true);
    draw();
    let state = state.0.borrow();
    assert_eq!(state.items.summary().count, 12);
    assert_eq!(state.items.summary().height, px(260.));
}
