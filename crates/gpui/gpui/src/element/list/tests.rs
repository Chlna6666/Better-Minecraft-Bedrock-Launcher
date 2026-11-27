use gpui::{ScrollDelta, ScrollWheelEvent};

use crate::{self as gpui, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;

#[gpui::test]
fn native_scroll_reuses_measured_list_rows(cx: &mut TestAppContext) {
    use crate::{Context, Element, ListAlignment, ListState, PlatformInput, Render, Styled, Window, div, list, point, px};

    struct Messages {
        state: ListState,
        builds: Rc<RefCell<Vec<usize>>>,
    }
    impl Render for Messages {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl crate::IntoElement {
            let builds = self.builds.clone();
            list(self.state.clone(), move |row, _, _| {
                builds.borrow_mut().push(row);
                div()
                    .w_full()
                    .h(px(if row % 2 == 0 { 20. } else { 30. }))
                    .bg(if row % 2 == 0 { crate::red() } else { crate::blue() })
                    .into_any()
            })
            .w(px(200.))
            .h(px(55.))
        }
    }
    let builds = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(|_, _| Messages {
        state: ListState::new(50, ListAlignment::Top, px(0.)).measure_all(),
        builds: builds.clone(),
    });
    let wheel = |dy| PlatformInput::ScrollWheel(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        ..Default::default()
    });
    cx.update(|window, app| {
        window.draw(app).clear();
        window.dispatch_event(wheel(-20.), app);
        window.draw(app).clear();
        builds.borrow_mut().clear();

        window.dispatch_event(wheel(-30.), app);
        window.draw(app).clear();
        assert_eq!(&*builds.borrow(), &[4]);

        let geometry = |window: &Window| window.rendered_frame.scene().quads.iter()
            .map(|quad| (quad.bounds, quad.content_mask, quad.background))
            .collect::<Vec<_>>();
        let retained = geometry(window);
        window.refresh();
        window.draw(app).clear();
        assert_eq!(retained, geometry(window));
    });
}

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
    use crate::{AppContext, Context, Element, IntoElement, ListState, Render, Styled, Window, div, list, point, px, size};
    use super::tree::Count;
    use std::cell::Cell;
    use sum_tree::Bias;

    let cx = cx.add_empty_window();
    let state = ListState::new(12, crate::ListAlignment::Top, px(0.)).measure_all();
    let taller = Rc::new(Cell::new(false));
    let builds = Rc::new(Cell::new(0));
    struct Rows(ListState, Rc<Cell<bool>>, Rc<Cell<usize>>);
    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let taller = self.1.clone();
            let builds = self.2.clone();
            list(self.0.clone(), move |index, _, _| {
                builds.set(builds.get() + 1);
                div().w_full().h(if matches!(index, 1 | 3) && taller.get() { px(30.) } else { px(20.) }).into_any()
            }).w_full().h_full()
        }
    }
    let mut draw = || {
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(60.)), |_, cx| {
            cx.new(|_| Rows(state.clone(), taller.clone(), builds.clone())).into_element()
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

#[test]
fn sparse_width_invalidation_preserves_unmeasured_subtrees() {
    use super::{state::ListItem, tree::Count};
    use crate::{ListAlignment, ListState, px, size};
    use sum_tree::{Bias, SumTree};
    let state = ListState::new(4096, ListAlignment::Top, px(0.));
    let mut state = state.0.borrow_mut();
    state.items = SumTree::from_iter((0..4096).map(|index| {
        if index == 0 || index == 2000 {
            ListItem::Measured { size: size(px(100.), px(20.)), focus_handle: None }
        } else {
            ListItem::Unmeasured { focus_handle: None }
        }
    }), ());
    let previous = state.items.clone();
    state.invalidate_width_measurements();
    assert_eq!(state.items.summary().rendered_count, 0);
    assert_eq!(state.items.summary().count, 4096);
    let mut old = previous.cursor::<Count>(());
    let mut new = state.items.cursor::<Count>(());
    old.seek(&Count(3500), Bias::Right);
    new.seek(&Count(3500), Bias::Right);
    assert!(std::ptr::eq(old.item().unwrap(), new.item().unwrap()));
}

#[test]
fn disjoint_item_invalidation_keeps_scroll_anchor_and_other_measurements() {
    use super::{state::ListItem, tree::Count};
    use crate::{ListAlignment, ListOffset, ListState, px, size};
    use sum_tree::{Bias, SumTree};
    let list = ListState::new(4096, ListAlignment::Top, px(0.));
    let top = ListOffset { item_ix: 100, offset_in_item: px(5.) };
    {
        let mut state = list.0.borrow_mut();
        state.items = SumTree::from_iter((0..4096).map(|_| ListItem::Measured {
            size: size(px(100.), px(20.)), focus_handle: None,
        }), ());
        state.logical_scroll_top = Some(top);
    }
    let previous = list.0.borrow().items.clone();
    list.invalidate_items(&[10..12, 100..101]);
    let state = list.0.borrow();
    assert_eq!(state.logical_scroll_top, Some(top));
    assert_eq!(state.items.summary().count, 4096);
    assert_eq!(state.items.summary().rendered_count, 4093);
    assert_eq!(state.items.summary().height, px(4093. * 20.));
    let mut old = previous.cursor::<Count>(());
    let mut new = state.items.cursor::<Count>(());
    old.seek(&Count(3500), Bias::Right);
    new.seek(&Count(3500), Bias::Right);
    assert!(std::ptr::eq(old.item().unwrap(), new.item().unwrap()));
}


#[test]
fn broad_sparse_item_invalidation_preserves_unselected_measured_and_unmeasured_nodes() {
    use super::{state::ListItem, tree::Count};
    use crate::{ListAlignment, ListOffset, ListState, px, size};
    use sum_tree::{Bias, SumTree};
    let list = ListState::new(8192, ListAlignment::Top, px(0.));
    let top = ListOffset { item_ix: 4000, offset_in_item: px(3.) };
    {
        let mut state = list.0.borrow_mut();
        state.items = SumTree::from_iter((0..8192).map(|index| {
            if [10, 2000, 4000, 7000].contains(&index) {
                ListItem::Measured { size: size(px(100.), px(20.)), focus_handle: None }
            } else {
                ListItem::Unmeasured { focus_handle: None }
            }
        }), ());
        state.logical_scroll_top = Some(top);
    }
    let previous = list.0.borrow().items.clone();
    list.invalidate_items(&[0..3500, 3500..3500, 6500..8192]);
    let state = list.0.borrow();
    assert_eq!(state.logical_scroll_top, Some(top));
    assert_eq!(state.items.summary().count, 8192);
    assert_eq!(state.items.summary().rendered_count, 1);
    assert_eq!(state.items.summary().height, px(20.));
    let mut old = previous.cursor::<Count>(());
    let mut new = state.items.cursor::<Count>(());
    old.seek(&Count(4000), Bias::Right);
    new.seek(&Count(4000), Bias::Right);
    assert!(std::ptr::eq(old.item().unwrap(), new.item().unwrap()));
}


#[test]
fn variable_placement_membership_requires_stable_measured_geometry() {
    use crate::{ListState, px};
    let list = ListState::new(5, crate::ListAlignment::Top, px(0.));
    {
        let mut state = list.0.borrow_mut();
        state.items = sum_tree::SumTree::from_iter([10., 20., 30., 40., 50.].into_iter().map(|height| {
            super::state::ListItem::Measured {
                size: crate::size(px(100.), px(height)), focus_handle: None
            }
        }), ());
        state.last_layout_bounds = Some(crate::Bounds::new(crate::point(px(0.), px(0.)), crate::size(px(100.), px(45.))));
        state.last_padding = Some(Default::default());
    }
    list.scroll_to(crate::ListOffset { item_ix: 0, offset_in_item: px(1.) });
    let proof = list.placement_proof(list.logical_scroll_top(), 0..3).unwrap();
    list.scroll_by(px(2.));
    assert_eq!(proof.sample().unwrap().0.y, px(-3.));
    list.scroll_by(px(8.));
    assert!(proof.sample().is_none());
    list.scroll_to(crate::ListOffset { item_ix: 0, offset_in_item: px(1.) });
    assert!(proof.sample().is_some());
    list.invalidate_items(&[4..5]);
    assert!(proof.sample().is_none());
    // A fresh capture can certify the same viewport even with an unknown
    // remote row, but must not cross an unknown required row.
    let proof = list.placement_proof(list.logical_scroll_top(), 0..3).unwrap();
    assert!(proof.sample().is_some());
    list.invalidate_items(&[2..3]);
    assert!(proof.sample().is_none());
    assert!(list.placement_proof(list.logical_scroll_top(), 0..3).is_none());
}


#[test]
fn list_wheel_events_use_live_anchor_and_reverse_at_clamp() {
    use crate::{ListState, ListAlignment, point, px, size};
    let list = ListState::new(10, ListAlignment::Top, px(0.));
    let mut state = list.0.borrow_mut();
    state.items = sum_tree::SumTree::from_iter((0..10).map(|_| super::state::ListItem::Measured {
        size: size(px(100.), px(10.)), focus_handle: None,
    }), ());
    let delta = |y| point(px(0.), px(y));
    state.apply_live_scroll(px(20.), delta(-3.)).unwrap();
    state.apply_live_scroll(px(20.), delta(-4.)).unwrap();
    assert_eq!(state.logical_scroll_top().offset_in_item, px(7.));
    state.apply_live_scroll(px(20.), delta(20.)).unwrap();
    assert_eq!(state.logical_scroll_top().offset_in_item, px(0.));
    state.apply_live_scroll(px(20.), delta(-1.)).unwrap();
    assert_eq!(state.logical_scroll_top().offset_in_item, px(1.));
}


#[test]
fn bottom_pinned_live_wheel_uses_current_measured_extent() {
    use crate::{ListState, ListAlignment, point, px, size};
    let list = ListState::new(10, ListAlignment::Bottom, px(0.));
    let mut state = list.0.borrow_mut();
    state.items = sum_tree::SumTree::from_iter((0..10).map(|_| super::state::ListItem::Measured {
        size: size(px(100.), px(10.)), focus_handle: None,
    }), ());
    let delta = |y| point(px(0.), px(y));
    state.apply_live_scroll(px(20.), delta(3.)).unwrap();
    assert_eq!(state.logical_scroll_top().item_ix, 7);
    assert_eq!(state.logical_scroll_top().offset_in_item, px(7.));
    state.apply_live_scroll(px(20.), delta(-5.)).unwrap();
    assert!(state.logical_scroll_top.is_none());
    state.apply_live_scroll(px(20.), delta(1.)).unwrap();
    assert_eq!(state.logical_scroll_top().item_ix, 7);
    assert_eq!(state.logical_scroll_top().offset_in_item, px(9.));
}
