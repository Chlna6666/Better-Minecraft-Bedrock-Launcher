use super::*;
use crate::{Context, IntoElement, Render, TestAppContext, TestDisplay, Window, div};
use std::cell::RefCell;

struct DisplayProbe {
    intervals: Vec<Option<Duration>>,
    _subscription: Subscription,
}

impl Render for DisplayProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn display(id: u32, hertz: f64) -> TestDisplay {
    let mut display = TestDisplay::with_id(DisplayId(id));
    display.refresh_interval = crate::refresh_interval_from_hz(hertz);
    display
}

fn add_probe(cx: &mut TestAppContext) -> crate::WindowHandle<DisplayProbe> {
    cx.add_window(|window, cx| DisplayProbe {
        intervals: Vec::new(),
        _subscription: cx.observe_window_display(window, |probe: &mut DisplayProbe, window, cx| {
            let interval = window.refresh_interval(cx);
            if let Some(id) = window.display_id {
                assert_eq!(interval, cx.display_refresh_interval(id));
            }
            probe.intervals.push(interval);
        }),
    })
}

#[gpui::test]
fn refresh_notifications_update_the_cache_outside_app_borrows(cx: &mut TestAppContext) {
    cx.test_platform().set_displays(vec![display(1, 60.)]);
    cx.run_until_parked();
    let window = add_probe(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update({
        let events = events.clone();
        move |cx| {
            cx.observe_displays(move |event, cx| {
                if let DisplayEvent::Changed { id, .. } = event {
                    assert_eq!(
                        cx.find_display(id).unwrap().refresh_interval(),
                        cx.display_refresh_interval(id)
                    );
                }
                events.borrow_mut().push(event);
            })
        }
    });
    {
        let app = cx.app.borrow_mut();
        cx.test_platform().set_displays(vec![display(1, 120.)]);
        assert_eq!(
            app.display_refresh_interval(DisplayId(1)),
            crate::refresh_interval_from_hz(60.)
        );
    }
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [DisplayEvent::Changed {
            id: DisplayId(1),
            changes: DisplayChanges::REFRESH_INTERVAL
        }]
    );
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.intervals, [crate::refresh_interval_from_hz(120.)]);
            assert_eq!(
                window.refresh_interval(cx),
                crate::refresh_interval_from_hz(120.)
            );
        })
        .unwrap();
    cx.test_platform().set_displays(vec![display(1, 120.)]);
    cx.run_until_parked();
    assert_eq!(events.borrow().len(), 1, "spurious native notification");
}

#[gpui::test]
fn moving_to_a_new_display_notifies_once_after_app_reconciliation(cx: &mut TestAppContext) {
    cx.test_platform().set_displays(vec![display(1, 60.)]);
    cx.run_until_parked();
    let window = add_probe(cx);
    let next = display(2, 144.);
    // The native window can move before App processes the display notification.
    cx.test_platform()
        .set_displays(vec![display(1, 60.), next.clone()]);
    cx.test_window(window.into())
        .simulate_display_change(Rc::new(next));
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(window.display_id, Some(DisplayId(2)));
            assert_eq!(
                window.refresh_interval(cx),
                crate::refresh_interval_from_hz(144.)
            );
            assert_eq!(view.intervals, [crate::refresh_interval_from_hz(144.)]);
        })
        .unwrap();
}

#[gpui::test]
fn disconnect_and_bounds_changes_are_observable(cx: &mut TestAppContext) {
    let mut first = display(1, 60.);
    cx.test_platform().set_displays(vec![first.clone()]);
    cx.run_until_parked();
    let window = add_probe(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update({
        let events = events.clone();
        move |cx| cx.observe_displays(move |event, _| events.borrow_mut().push(event))
    });
    first.bounds.size.width = crate::px(2560.);
    cx.test_platform()
        .set_displays(vec![first, display(2, 120.)]);
    cx.run_until_parked();
    assert!(events.borrow().contains(&DisplayEvent::Changed {
        id: DisplayId(1),
        changes: DisplayChanges::BOUNDS
    }));
    assert!(events.borrow().contains(&DisplayEvent::Added(DisplayId(2))));
    cx.test_platform().set_displays(Vec::new());
    cx.run_until_parked();
    assert!(
        events
            .borrow()
            .contains(&DisplayEvent::Removed(DisplayId(1)))
    );
    assert!(
        events
            .borrow()
            .contains(&DisplayEvent::Removed(DisplayId(2)))
    );
    window
        .update(cx, |view, window, cx| {
            assert_eq!(window.refresh_interval(cx), None);
            assert_eq!(view.intervals, [crate::refresh_interval_from_hz(60.), None]);
        })
        .unwrap();
}

#[gpui::test]
fn a_display_announced_after_the_move_updates_the_window_observer(cx: &mut TestAppContext) {
    cx.test_platform().set_displays(vec![display(1, 60.)]);
    cx.run_until_parked();
    let window = add_probe(cx);
    let next = display(2, 144.);
    cx.test_window(window.into())
        .simulate_display_change(Rc::new(next.clone()));
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert_eq!(view.intervals, [None]))
        .unwrap();
    cx.test_platform().set_displays(vec![display(1, 60.), next]);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.intervals,
                [None, crate::refresh_interval_from_hz(144.)]
            );
        })
        .unwrap();
}

#[gpui::test]
fn queued_display_observation_allows_window_removal(cx: &mut TestAppContext) {
    let window = add_probe(cx);
    cx.test_window(window.into())
        .simulate_display_change(Rc::new(display(2, 120.)));
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.app.borrow().windows.contains_key(window.window_id()));
}
