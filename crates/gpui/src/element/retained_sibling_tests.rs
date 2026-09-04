use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, Render, Styled,
    TestAppContext, Window, WindowOptions, point, px, size,
};

struct PaintCountLeaf {
    id: &'static str,
    inner: crate::Div,
    paints: Rc<Cell<usize>>,
}

impl IntoElement for PaintCountLeaf {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PaintCountLeaf {
    type RequestLayoutState = <crate::Div as Element>::RequestLayoutState;
    type PrepaintState = <crate::Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.into())
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
        self.paints.set(self.paints.get().saturating_add(1));
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

struct CapturedDiv {
    id: &'static str,
    inner: crate::Div,
    path: Rc<RefCell<Option<GlobalElementId>>>,
}

impl IntoElement for CapturedDiv {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CapturedDiv {
    type RequestLayoutState = <crate::Div as Element>::RequestLayoutState;
    type PrepaintState = <crate::Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.into())
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
        *self.path.borrow_mut() = window.current_retained_element_id();
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

struct SiblingIsolationRoot {
    left_paints: Rc<Cell<usize>>,
    right_paints: Rc<Cell<usize>>,
    dirty_path: Rc<RefCell<Option<GlobalElementId>>>,
    dirty_width: f32,
}

impl Render for SiblingIsolationRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .relative()
            .w(px(240.0))
            .h(px(80.0))
            .child(PaintCountLeaf {
                id: "retained-static-left",
                inner: crate::div()
                    .absolute()
                    .left(px(0.0))
                    .w(px(20.0))
                    .h(px(20.0))
                    .bg(crate::white()),
                paints: self.left_paints.clone(),
            })
            .child(CapturedDiv {
                id: "retained-dirty-middle",
                inner: crate::div()
                    .absolute()
                    .left(px(80.0))
                    .w(px(self.dirty_width))
                    .h(px(20.0))
                    .bg(crate::red()),
                path: self.dirty_path.clone(),
            })
            .child(PaintCountLeaf {
                id: "retained-static-right",
                inner: crate::div()
                    .absolute()
                    .left(px(180.0))
                    .w(px(20.0))
                    .h(px(20.0))
                    .bg(crate::white()),
                paints: self.right_paints.clone(),
            })
    }
}

struct ClipDependencyRoot {
    child_paints: Rc<Cell<usize>>,
    clip_path: Rc<RefCell<Option<GlobalElementId>>>,
    clip_width: f32,
}

impl Render for ClipDependencyRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().relative().w(px(200.0)).h(px(80.0)).child(CapturedDiv {
            id: "retained-changing-clip",
            inner: crate::div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(self.clip_width))
                .h(px(20.0))
                .overflow_hidden()
                .child(PaintCountLeaf {
                    id: "retained-clipped-child",
                    inner: crate::div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(100.0))
                        .h(px(20.0))
                        .bg(crate::white()),
                    paints: self.child_paints.clone(),
                }),
            path: self.clip_path.clone(),
        })
    }
}

fn draw_window(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();
}

#[gpui::test]
fn targeted_child_change_keeps_unrelated_same_parent_siblings_retained(
    cx: &mut TestAppContext,
) {
    let left_paints = Rc::new(Cell::new(0));
    let right_paints = Rc::new(Cell::new(0));
    let dirty_path = Rc::new(RefCell::new(None));

    let (root, window) = cx.update(|cx| {
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SiblingIsolationRoot {
                    left_paints: left_paints.clone(),
                    right_paints: right_paints.clone(),
                    dirty_path: dirty_path.clone(),
                    dirty_width: 20.0,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _cx| root).unwrap();
        (root, AnyWindowHandle::from(window))
    });

    draw_window(window, cx);
    let left_baseline = left_paints.get();
    let right_baseline = right_paints.get();
    assert!(left_baseline > 0 && right_baseline > 0);

    root.update(cx, |root, _cx| root.dirty_width = 48.0);
    let path = dirty_path
        .borrow()
        .clone()
        .expect("dirty child should publish retained path");
    cx.update_window(window, |_, window, cx| {
        window.notify_interactive_region_scoped(
            root.entity_id(),
            Some(&path),
            Bounds::new(point(px(80.0), px(0.0)), size(px(48.0), px(20.0))),
            false,
            cx,
        );
        window.draw(cx).clear();
    })
    .unwrap();

    assert_eq!(
        left_paints.get(),
        left_baseline,
        "unrelated left sibling must remain retained"
    );
    assert_eq!(
        right_paints.get(),
        right_baseline,
        "unrelated right sibling must remain retained"
    );
}

#[gpui::test]
fn inherited_clip_change_repaints_only_the_dependent_descendant(cx: &mut TestAppContext) {
    let child_paints = Rc::new(Cell::new(0));
    let clip_path = Rc::new(RefCell::new(None));

    let (root, window) = cx.update(|cx| {
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| ClipDependencyRoot {
                    child_paints: child_paints.clone(),
                    clip_path: clip_path.clone(),
                    clip_width: 60.0,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _cx| root).unwrap();
        (root, AnyWindowHandle::from(window))
    });

    draw_window(window, cx);
    let baseline = child_paints.get();
    assert!(baseline > 0);

    root.update(cx, |root, _cx| root.clip_width = 30.0);
    let path = clip_path
        .borrow()
        .clone()
        .expect("clip parent should publish retained path");
    cx.update_window(window, |_, window, cx| {
        window.notify_interactive_region_scoped(
            root.entity_id(),
            Some(&path),
            Bounds::new(point(px(0.0), px(0.0)), size(px(60.0), px(20.0))),
            false,
            cx,
        );
        window.draw(cx).clear();
    })
    .unwrap();

    assert!(
        child_paints.get() > baseline,
        "a changed inherited overflow mask must invalidate the clipped descendant"
    );
}
