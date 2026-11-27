use crate::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, TestAppContext, UniformListScrollHandle,
    Window, canvas, point, px, uniform_list,
};
use std::{cell::RefCell, rc::Rc};

struct LongListView {
    scroll_handle: UniformListScrollHandle,
    item_bounds: Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>>,
}

impl Render for LongListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item_bounds = self.item_bounds.clone();
        uniform_list(
            "entries",
            10_000_000,
            cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                range
                    .map(|ix| {
                        let item_bounds = item_bounds.clone();
                        canvas(
                            move |bounds, _, _| item_bounds.borrow_mut().push((ix, bounds)),
                            |_, _, _, _| {},
                        )
                        .w_full()
                        .h(px(24.0))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll_handle.clone())
        .h(px(200.0))
    }
}

#[gpui::test]
fn item_positions_far_down_long_list_remain_uniform(cx: &mut TestAppContext) {
    let item_bounds = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view(|_, _| LongListView {
        scroll_handle: UniformListScrollHandle::new(),
        item_bounds: item_bounds.clone(),
    });

    // At this depth f32 scroll offsets are quantized to 16px. The offset is
    // 16px into item 9,000,002, but adjacent items must still be 24px apart.
    item_bounds.borrow_mut().clear();
    view.update(cx, |view, cx| {
        view.scroll_handle
            .base_handle()
            .set_offset(point(px(0.0), px(-216_000_064.0)));
        cx.notify();
    });
    cx.run_until_parked();

    let item_tops = item_bounds
        .borrow()
        .iter()
        .map(|(ix, bounds)| (*ix, bounds.origin.y))
        .collect::<Vec<_>>();
    let expected_tops = (9_000_002..9_000_011)
        .map(|ix| (ix, px(-16.0 + 24.0 * (ix - 9_000_002) as f32)))
        .collect::<Vec<_>>();
    assert_eq!(item_tops, expected_tops);
}

struct FixedRows {
    calls: Rc<RefCell<Vec<std::ops::Range<usize>>>>,
    scroll_handle: UniformListScrollHandle,
    hover: bool,
}

impl Render for FixedRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use crate::FluentBuilder;
        let calls = self.calls.clone();
        let hover = self.hover;
        let rows = uniform_list("fixed-rows", 100, move |range, _, _| {
            use crate::{FluentBuilder, InteractiveElement};
            calls.borrow_mut().push(range.clone());
            range
                .map(|row| {
                    crate::div()
                        .id(row)
                        .w_full()
                        .h(px(24.))
                        .bg(if row % 2 == 0 { crate::red() } else { crate::blue() })
                        .when(hover, |row| {
                            row.hover(|style| style.bg(crate::green()))
                                .child(crate::div().overflow_hidden().child("price"))
                        })
                })
                .collect()
        })
        .with_sizing_behavior(crate::ListSizingBehavior::Auto)
        .when(!hover, |rows| rows.with_item_height(px(24.)))
        .track_scroll(self.scroll_handle.clone())
        .w(px(240.))
        .h(px(120.));
        crate::div().w(px(240.)).h(px(120.)).rounded_lg().overflow_hidden().child(rows)
    }
}

#[gpui::test]
fn fixed_rows_do_not_render_a_measurement_sample(cx: &mut TestAppContext) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view(|_, _| FixedRows {
        calls: calls.clone(),
        scroll_handle: UniformListScrollHandle::new(),
        hover: false,
    });
    cx.run_until_parked();
    calls.borrow_mut().clear();
    view.update(cx, |view, cx| {
        view.scroll_handle.base_handle().set_offset(point(px(0.), px(-240.)));
        cx.notify();
    });
    cx.run_until_parked();
    let calls = calls.borrow();
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|range| range.start >= 10 && !range.is_empty()), "{calls:?}");
}

#[gpui::test]
fn native_scroll_reuses_overlapping_uniform_rows(cx: &mut TestAppContext) {
    use crate::{PlatformInput, ScrollDelta, ScrollWheelEvent};

    let calls = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(|_, _| FixedRows {
        calls: calls.clone(),
        scroll_handle: UniformListScrollHandle::new(),
        hover: false,
    });
    let wheel = || PlatformInput::ScrollWheel(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-24.))),
        ..Default::default()
    });
    cx.update(|window, app| {
        window.draw(app).clear();
        window.dispatch_event(wheel(), app);
        window.draw(app).clear();
        calls.borrow_mut().clear();

        window.dispatch_event(wheel(), app);
        window.draw(app).clear();
        assert_eq!(&*calls.borrow(), &[6..7]);

        let geometry = |window: &Window| {
            window.rendered_frame.scene().quads.iter()
                .map(|quad| (quad.bounds, quad.content_mask, quad.background))
                .collect::<Vec<_>>()
        };
        let retained = geometry(window);
        window.refresh();
        window.draw(app).clear();
        assert_eq!(retained, geometry(window));
    });
}

#[gpui::test]
fn native_scroll_repositions_retained_hover_hitboxes(cx: &mut TestAppContext) {
    use crate::{MouseMoveEvent, PlatformInput, ScrollDelta, ScrollWheelEvent};

    let calls = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(|_, _| FixedRows {
        calls: calls.clone(),
        scroll_handle: UniformListScrollHandle::new(),
        hover: true,
    });
    let wheel = || PlatformInput::ScrollWheel(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-24.))),
        ..Default::default()
    });
    cx.update(|window, app| {
        window.draw(app).clear();
        window.dispatch_event(wheel(), app);
        window.draw(app).clear();
        calls.borrow_mut().clear();
        window.dispatch_event(wheel(), app);
        window.draw(app).clear();
        assert_eq!(&*calls.borrow(), &[6..7]);
        let scene = window.rendered_frame.scene();
        assert!(scene.quads.iter().all(|quad| quad.background != crate::green().into()));
        let row = scene.quads.iter().find(|quad| quad.background == crate::red().into()).unwrap();
        assert_ne!(row.content_mask.corner_radii, Default::default());

        let position = point(px(4.), px(50.));
        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent {
            position, ..Default::default()
        }), app);
        window.draw(app).clear();
        let quad = window.rendered_frame.scene().quads.iter()
            .find(|quad| quad.background == crate::green().into())
            .expect("hover style at translated row position");
        assert_eq!(quad.bounds.origin.y, px(48.).scale(window.scale_factor()));
    });
}

#[gpui::test]
fn versioned_uniform_list_rebuilds_only_changed_rows_after_owner_notify(cx: &mut TestAppContext) {
    struct VersionedRows {
        revisions: Rc<RefCell<[u64; 100]>>,
        calls: Rc<RefCell<Vec<std::ops::Range<usize>>>>,
    }
    impl Render for VersionedRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let values = self.revisions.clone();
            let revisions = self.revisions.clone();
            let calls = self.calls.clone();
            uniform_list("versioned-rows", 100, move |range, _, _| {
                calls.borrow_mut().push(range.clone());
                range.map(|index| {
                    crate::div().w_full().h(px(24.)).bg(
                        if values.borrow()[index] == 0 { crate::blue() } else { crate::red() }
                    )
                }).collect()
            })
            .with_item_height(px(24.))
            .with_row_revision(move |index| revisions.borrow()[index])
            .w(px(240.)).h(px(120.))
        }
    }
    let calls = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view(|_, _| VersionedRows {
        revisions: Rc::new(RefCell::new([0; 100])),
        calls: calls.clone(),
    });
    cx.run_until_parked();
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    calls.borrow_mut().clear();
    view.update(cx, |view, cx| {
        view.revisions.borrow_mut()[2] += 1;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(&*calls.borrow(), &[2..3]);
    cx.update(|window, app| {
        let geometry = |window: &Window| {
            window.rendered_frame.scene().quads.iter()
                .map(|quad| (quad.bounds, quad.content_mask, quad.background))
                .collect::<Vec<_>>()
        };
        let retained = geometry(window);
        window.refresh();
        window.draw(app).clear();
        assert_eq!(retained, geometry(window));
    });
}


#[gpui::test]
fn independent_uniform_list_rebuilds_only_notified_model_row(cx: &mut TestAppContext) {
    use crate::{AppContext, Entity, uniform_list_each};
    struct Model(bool);
    struct IndependentRows {
        models: Vec<Entity<Model>>,
        calls: Rc<RefCell<Vec<usize>>>,
    }
    impl Render for IndependentRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let models = self.models.clone();
            let calls = self.calls.clone();
            uniform_list_each("independent-rows", models.len(), move |index, _, cx| {
                calls.borrow_mut().push(index);
                crate::div().w_full().h(px(24.)).bg(
                    if models[index].read(cx).0 { crate::red() } else { crate::blue() }
                )
            })
            .with_item_height(px(24.))
            .w(px(240.)).h(px(120.))
        }
    }
    let calls = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view(|_, cx| IndependentRows {
        models: (0..100).map(|_| cx.new(|_| Model(false))).collect(),
        calls: calls.clone(),
    });
    cx.run_until_parked();
    // Prime the retained row capture through a scoped owner update.
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    calls.borrow_mut().clear();
    view.update(cx, |view, cx| {
        view.models[2].update(cx, |model, cx| {
            model.0 = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(&*calls.borrow(), &[2]);
    cx.update(|window, app| {
        let geometry = |window: &Window| window.rendered_frame.scene().quads.iter()
            .map(|quad| (quad.bounds, quad.content_mask, quad.background))
            .collect::<Vec<_>>();
        let retained = geometry(window);
        window.refresh();
        window.draw(app).clear();
        assert_eq!(retained, geometry(window));
    });
}
