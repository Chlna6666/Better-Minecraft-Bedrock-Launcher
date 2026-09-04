use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use super::*;
use crate::element::{Element, ParentElement};
use crate::{
    AnyWindowHandle, AppContext, IntoElement, TestAppContext, WindowOptions, point, px, size,
};

struct RangeTrackedLeaf {
    renders: Rc<Cell<usize>>,
}

impl Render for RangeTrackedLeaf {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().w(px(2.)).h(px(2.)).bg(crate::white())
    }
}

struct CapturedVariableDiv {
    inner: crate::Div,
    path: Rc<RefCell<Option<GlobalElementId>>>,
}

impl IntoElement for CapturedVariableDiv {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CapturedVariableDiv {
    type RequestLayoutState = <crate::Div as Element>::RequestLayoutState;
    type PrepaintState = <crate::Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("retained-range-variable-sibling".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        *self.path.borrow_mut() = id.cloned();
        self.inner
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
    }
}

struct FrameLocalBoundaryRoot {
    before: Entity<RangeTrackedLeaf>,
    after: Entity<RangeTrackedLeaf>,
    variable_path: Rc<RefCell<Option<GlobalElementId>>>,
    variable_primitives: usize,
}

impl Render for FrameLocalBoundaryRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut variable = crate::div().flex().flex_col();
        for _ in 0..self.variable_primitives {
            variable = variable.child(
                crate::div()
                    .w(px(1.))
                    .h(px(1.))
                    .bg(crate::white()),
            );
        }

        crate::div()
            // Keep each AnyView below a normal retained ancestor. The regression was not just direct
            // AnyView replay: an unrelated ancestor could previously replay across the cache boundary
            // and skip AnyView::prepaint/paint, leaving its absolute frame-local ranges stale.
            .child(crate::div().child(
                AnyView::from(self.before.clone()).cached(StyleRefinement::default()),
            ))
            .child(CapturedVariableDiv {
                inner: variable,
                path: self.variable_path.clone(),
            })
            .child(crate::div().child(
                AnyView::from(self.after.clone()).cached(StyleRefinement::default()),
            ))
    }
}

fn any_view_scene_ranges(window: &Window) -> Vec<Range<usize>> {
    let mut ranges = window
        .rendered_frame
        .element_states
        .iter()
        .filter_map(|((_, type_id), state)| {
            if *type_id != TypeId::of::<AnyViewState>() {
                return None;
            }
            state
                .inner
                .downcast_ref::<Option<AnyViewState>>()?
                .as_ref()
                .map(|state| state.paint_range.start.scene_index..state.paint_range.end.scene_index)
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    ranges
}

fn targeted_resize(
    root: &Entity<FrameLocalBoundaryRoot>,
    variable_path: &Rc<RefCell<Option<GlobalElementId>>>,
    count: usize,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    root.update(cx, |root, _cx| {
        root.variable_primitives = count;
    });
    let path = variable_path
        .borrow()
        .clone()
        .expect("variable sibling should publish its retained path");
    cx.update_window(window, |_, window, cx| {
        window.notify_interactive_region(
            root.entity_id(),
            Some(&path),
            Bounds::new(point(px(0.), px(0.)), size(px(1.), px(1.))),
            cx,
        );
        window.draw(cx).clear();
    })
    .unwrap();
}

#[gpui::test]
fn frame_local_cache_boundary_rebases_ranges_across_variable_sibling(cx: &mut TestAppContext) {
    let before_renders = Rc::new(Cell::new(0));
    let after_renders = Rc::new(Cell::new(0));
    let variable_path = Rc::new(RefCell::new(None));

    let (root, window) = cx.update(|cx| {
        let before = cx.new(|_| RangeTrackedLeaf {
            renders: before_renders.clone(),
        });
        let after = cx.new(|_| RangeTrackedLeaf {
            renders: after_renders.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| FrameLocalBoundaryRoot {
                    before,
                    after,
                    variable_path: variable_path.clone(),
                    variable_primitives: 10,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _cx| root).unwrap();
        (root, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline_before_renders = before_renders.get();
    let baseline_after_renders = after_renders.get();
    assert!(baseline_before_renders > 0 && baseline_after_renders > 0);

    let ranges_10 = cx
        .update_window(window, |_, window, _| any_view_scene_ranges(window))
        .unwrap();
    assert_eq!(ranges_10.len(), 2);

    targeted_resize(&root, &variable_path, 100, window, cx);
    let ranges_100 = cx
        .update_window(window, |_, window, _| any_view_scene_ranges(window))
        .unwrap();
    assert_eq!(ranges_100.len(), 2);
    assert_eq!(before_renders.get(), baseline_before_renders);
    assert_eq!(after_renders.get(), baseline_after_renders);
    assert_eq!(ranges_100[0].start, ranges_10[0].start);
    assert!(
        ranges_100[1].start > ranges_10[1].start,
        "trailing cached AnyView must rebase after the variable sibling grows"
    );

    targeted_resize(&root, &variable_path, 3, window, cx);
    let ranges_3 = cx
        .update_window(window, |_, window, _| any_view_scene_ranges(window))
        .unwrap();
    assert_eq!(ranges_3.len(), 2);
    assert_eq!(before_renders.get(), baseline_before_renders);
    assert_eq!(after_renders.get(), baseline_after_renders);
    assert_eq!(ranges_3[0].start, ranges_10[0].start);
    assert!(
        ranges_3[1].start < ranges_100[1].start,
        "trailing cached AnyView must rebase after the variable sibling shrinks"
    );
}
