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
        cx.new(|_| TestView(state.clone()))
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
        cx.new(|_| TestView(state.clone()))
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
        cx.new(|_| TestView(state.clone()))
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
