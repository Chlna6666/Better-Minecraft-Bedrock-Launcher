use super::*;
use crate::element::InteractiveElement;
use crate::{AnyWindowHandle, AppContext, Context, TestAppContext, WindowOptions, px};

struct PlainLeafView {
    renders: Rc<std::cell::Cell<usize>>,
    revision: usize,
    width: Pixels,
}

impl Render for PlainLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .w(self.width)
            .h(px(40.))
            .bg(crate::rgb(if self.revision % 2 == 0 {
                0xf00000
            } else {
                0x0000f0
            }))
            .child("Leaf")
    }
}

struct PlainParentView {
    renders: Rc<std::cell::Cell<usize>>,
    leaf: Entity<PlainLeafView>,
}

impl Render for PlainParentView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .flex()
            .child(div().w(px(20.)).h(px(40.)).bg(crate::rgb(0x00f000)))
            .child(div().id("plain-nested").p(px(4.)).child(self.leaf.clone()))
            .child(div().w(px(20.)).h(px(40.)).bg(crate::rgb(0xf0f000)))
    }
}

struct PlainRootView {
    renders: Rc<std::cell::Cell<usize>>,
    parent: Entity<PlainParentView>,
}

struct IndependentRootView {
    renders: Rc<std::cell::Cell<usize>>,
    left: Entity<PlainLeafView>,
    right: Entity<PlainLeafView>,
}

struct Data {
    blue: bool,
}

struct DependentRootView {
    leaf: Entity<PlainLeafView>,
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for DependentRootView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .w(px(160.))
            .h(px(80.))
            .bg(crate::rgb(if self.leaf.read(cx).revision == 0 {
                0xffffff
            } else {
                0xaa00aa
            }))
            .child(self.leaf.clone())
    }
}

#[gpui::test]
fn rendered_leaf_notify_updates_parent_that_reads_its_state(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| PlainLeafView {
            renders: Rc::new(std::cell::Cell::new(0)),
            revision: 0,
            width: px(80.),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DependentRootView {
                    leaf: leaf.clone(),
                    renders: renders.clone(),
                })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let baseline = renders.get();
    leaf.update(cx, |leaf, cx| {
        leaf.revision = 1;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    assert!(
        renders.get() > baseline,
        "parent's own dependency must invalidate it"
    );
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .rendered_frame
                .scene
                .quads
                .iter()
                .any(|quad| { quad.background.solid == crate::rgb(0xaa00aa) })
        );
        assert!(
            !window
                .rendered_frame
                .scene
                .quads
                .iter()
                .any(|quad| { quad.background.solid == crate::rgb(0xffffff) })
        );
    })
    .unwrap();
}

struct DataRootView {
    data: Entity<Data>,
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for DataRootView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size(px(20.))
            .bg(crate::rgb(if self.data.read(cx).blue {
                0x0000ff
            } else {
                0xff0000
            }))
    }
}

#[gpui::test]
fn non_view_entity_notify_cannot_replay_stale_root(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (data, window) = cx.update(|cx| {
        let data = cx.new(|_| Data { blue: false });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DataRootView {
                    data: data.clone(),
                    renders: renders.clone(),
                })
            })
            .unwrap();
        (data, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let baseline = renders.get();
    data.update(cx, |data, cx| {
        data.blue = true;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    assert!(renders.get() > baseline);
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .rendered_frame
                .scene
                .quads
                .iter()
                .any(|quad| quad.background.solid == crate::rgb(0x0000ff))
        );
        assert!(
            !window
                .rendered_frame
                .scene
                .quads
                .iter()
                .any(|quad| quad.background.solid == crate::rgb(0xff0000))
        );
    })
    .unwrap();
}

impl Render for IndependentRootView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .flex()
            .child(self.left.clone())
            .child(self.right.clone())
    }
}

#[gpui::test]
fn unrelated_plain_entities_update_independently(cx: &mut TestAppContext) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let left_renders = Rc::new(std::cell::Cell::new(0));
    let right_renders = Rc::new(std::cell::Cell::new(0));
    let (left, right, window) = cx.update(|cx| {
        let left = cx.new(|_| PlainLeafView {
            renders: left_renders.clone(),
            revision: 0,
            width: px(80.),
        });
        let right = cx.new(|_| PlainLeafView {
            renders: right_renders.clone(),
            revision: 0,
            width: px(80.),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| IndependentRootView {
                    renders: root_renders.clone(),
                    left: left.clone(),
                    right: right.clone(),
                })
            })
            .unwrap();
        (left, right, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let baseline = (root_renders.get(), left_renders.get(), right_renders.get());
    let original = cx
        .update_window(window, |_, window, _| {
            window.rendered_frame.scene.snapshot()
        })
        .unwrap();
    let bounds: Vec<_> = original
        .quads
        .iter()
        .filter(|quad| quad.background.solid == crate::rgb(0xf00000))
        .map(|quad| quad.bounds)
        .collect();
    assert_eq!(bounds.len(), 2);
    for revision in 1..=4 {
        left.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| window.draw(cx).clear())
            .unwrap();
        assert_eq!(root_renders.get(), baseline.0);
        assert_eq!(left_renders.get(), baseline.1 + revision);
        assert_eq!(right_renders.get(), baseline.2 + revision - 1);
        cx.update_window(window, |_, window, _| {
            let current = window
                .rendered_frame
                .scene
                .quads
                .iter()
                .find(|quad| quad.bounds == bounds[1])
                .unwrap();
            let color = if (revision - 1) % 2 == 0 {
                0xf00000
            } else {
                0x0000f0
            };
            assert_eq!(current.background.solid, crate::rgb(color));
        })
        .unwrap();
        right.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| window.draw(cx).clear())
            .unwrap();
        assert_eq!(root_renders.get(), baseline.0);
        assert_eq!(left_renders.get(), baseline.1 + revision);
        assert_eq!(right_renders.get(), baseline.2 + revision);
        cx.update_window(window, |_, window, _| {
            let current = window
                .rendered_frame
                .scene
                .quads
                .iter()
                .find(|quad| quad.bounds == bounds[0])
                .unwrap();
            let color = if revision % 2 == 0 {
                0xf00000
            } else {
                0x0000f0
            };
            assert_eq!(current.background.solid, crate::rgb(color));
        })
        .unwrap();
    }
}

impl Render for PlainRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().relative().p(px(12.)).child(self.parent.clone())
    }
}

#[gpui::test]
fn plain_nested_leaf_update_preserves_ancestors_and_sibling_geometry(cx: &mut TestAppContext) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let parent_renders = Rc::new(std::cell::Cell::new(0));
    let leaf_renders = Rc::new(std::cell::Cell::new(0));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| PlainLeafView {
            renders: leaf_renders.clone(),
            revision: 0,
            width: px(80.),
        });
        let parent = cx.new(|_| PlainParentView {
            renders: parent_renders.clone(),
            leaf: leaf.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| PlainRootView {
                    renders: root_renders.clone(),
                    parent,
                })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let baseline = (root_renders.get(), parent_renders.get(), leaf_renders.get());
    let before = cx
        .update_window(window, |_, window, _| {
            window.rendered_frame.scene.snapshot()
        })
        .unwrap();
    for revision in 1..=4 {
        leaf.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| window.draw(cx).clear())
            .unwrap();
        assert_eq!(root_renders.get(), baseline.0);
        assert_eq!(parent_renders.get(), baseline.1);
        assert_eq!(leaf_renders.get(), baseline.2 + revision);
        cx.update_window(window, |_, window, _| {
            for color in [0x00f000, 0xf0f000] {
                let previous = before
                    .quads
                    .iter()
                    .find(|quad| quad.background.solid == crate::rgb(color))
                    .unwrap();
                let current = window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .find(|quad| quad.background.solid == crate::rgb(color))
                    .unwrap();
                assert_eq!(current.bounds, previous.bounds);
            }
            let color = if revision % 2 == 0 {
                0xf00000
            } else {
                0x0000f0
            };
            let current = window
                .rendered_frame
                .scene
                .quads
                .iter()
                .find(|quad| quad.background.solid == crate::rgb(color))
                .unwrap();
            let previous = before
                .quads
                .iter()
                .find(|quad| quad.background.solid == crate::rgb(0xf00000))
                .unwrap();
            assert_eq!(current.bounds, previous.bounds);
        })
        .unwrap();
    }
    leaf.update(cx, |leaf, cx| {
        leaf.width = px(100.);
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    assert!(
        root_renders.get() > baseline.0,
        "geometry changes require structural layout"
    );
    assert!(parent_renders.get() > baseline.1);
    cx.update_window(window, |_, window, _| {
        let previous = before
            .quads
            .iter()
            .find(|quad| quad.background.solid == crate::rgb(0xf0f000))
            .unwrap();
        let current = window
            .rendered_frame
            .scene
            .quads
            .iter()
            .find(|quad| quad.background.solid == crate::rgb(0xf0f000))
            .unwrap();
        assert_eq!(
            current.bounds.origin.x.0 - previous.bounds.origin.x.0,
            20. * window.scale_factor()
        );
    })
    .unwrap();
}
