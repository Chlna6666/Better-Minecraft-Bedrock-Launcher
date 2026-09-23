use std::rc::Rc;

use super::*;
use crate::element::{CompositeLayerExt, ParentElement, StatefulInteractiveElement};
use crate::{
    AnyWindowHandle, AppContext, InteractiveElement, TestAppContext, WindowOptions, point, px,
};

struct AbsoluteCachedLeafView {
    bounds: Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    revision: usize,
}

impl Render for AbsoluteCachedLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.bounds.clone();
        crate::div()
            .absolute()
            .inset_0()
            .child(
                crate::canvas(
                    move |canvas_bounds, _window, _cx| {
                        bounds.borrow_mut().push(canvas_bounds);
                    },
                    |_, (), _window, _cx| {},
                )
                .size_full(),
            )
            .child(format!("revision-{}", self.revision))
    }
}

struct AbsoluteCachedRootView {
    leaf: Entity<AbsoluteCachedLeafView>,
}

impl Render for AbsoluteCachedRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .relative()
            .size_full()
            .child(AnyView::from(self.leaf.clone()).cached_absolute_by(&"absolute-leaf"))
    }
}

struct RenderCountCachedLeafView {
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for RenderCountCachedLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().child("cached-leaf")
    }
}

struct DirtySiblingView {
    revision: usize,
}

impl Render for DirtySiblingView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().child(format!("sibling-{}", self.revision))
    }
}

struct CachedSiblingRootView {
    leaf: Entity<RenderCountCachedLeafView>,
    sibling: Entity<DirtySiblingView>,
}

impl Render for CachedSiblingRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .child(AnyView::from(self.leaf.clone()).cached(StyleRefinement::default()))
            .child(self.sibling.clone())
    }
}

struct RefreshRetainedCachedRootView {
    leaf: Entity<RenderCountCachedLeafView>,
    sibling: Entity<DirtySiblingView>,
}

impl Render for RefreshRetainedCachedRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .child(
                AnyView::from(self.leaf.clone())
                    .cached(StyleRefinement::default())
                    .reuse_on_window_refresh(),
            )
            .child(self.sibling.clone())
    }
}

struct ProgressiveCachedRootView {
    leaf: Entity<RenderCountCachedLeafView>,
    expire_budget: bool,
}

impl Render for ProgressiveCachedRootView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.expire_budget {
            window.test_expire_draw_budget();
        }

        crate::div().child(
            AnyView::from(self.leaf.clone())
                .cached(StyleRefinement::default())
                .progressive(),
        )
    }
}

struct HitboxCachedLeafView {
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for HitboxCachedLeafView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div()
            .id("cached-hitbox-leaf")
            .w(px(1.))
            .h(px(1.))
            .bg(crate::white())
            .on_click(cx.listener(|_, _, _, _| {}))
    }
}

struct DegradedCachedRootView {
    leaf: Entity<HitboxCachedLeafView>,
    include_prefix_hitbox: bool,
    expire_budget: bool,
}

impl Render for DegradedCachedRootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.expire_budget {
            window.test_expire_draw_budget();
        }

        let mut root = crate::div().relative();
        if self.include_prefix_hitbox {
            root = root.child(
                crate::div()
                    .id("prefix-hitbox")
                    .absolute()
                    .w(px(1.))
                    .h(px(1.))
                    .on_click(cx.listener(|_, _, _, _| {})),
            );
        }
        root.child(AnyView::from(self.leaf.clone()).cached(StyleRefinement::default()))
            .child(crate::deferred(crate::div().w(px(1.)).h(px(1.))))
    }
}

struct DeferredCachedLeafView;

impl Render for DeferredCachedLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().child(crate::deferred(
            crate::div().w(px(1.)).h(px(1.)).bg(crate::white()),
        ))
    }
}

struct DeferredCachedRootView {
    leaf: Entity<DeferredCachedLeafView>,
}

impl Render for DeferredCachedRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().child(
            AnyView::from(self.leaf.clone())
                .cached(StyleRefinement::default())
                .reuse_on_window_refresh(),
        )
    }
}

struct SelectiveLeafView {
    renders: Rc<std::cell::Cell<usize>>,
    revision: usize,
}

impl Render for SelectiveLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div()
            .w(px(120.))
            .h(px(32.))
            .child(format!("leaf-{}", self.revision))
    }
}

struct SelectiveParentView {
    renders: Rc<std::cell::Cell<usize>>,
    leaf: Entity<SelectiveLeafView>,
}

impl Render for SelectiveParentView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().child(
            AnyView::from(self.leaf.clone()).cached(
                StyleRefinement::default().w(px(120.)).h(px(32.)),
            ),
        )
    }
}

struct SelectiveRootView {
    renders: Rc<std::cell::Cell<usize>>,
    parent: Entity<SelectiveParentView>,
}

impl Render for SelectiveRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().child(
            AnyView::from(self.parent.clone()).cached(
                StyleRefinement::default().w(px(120.)).h(px(32.)),
            ),
        )
    }
}


struct SelectiveFallbackParentView {
    renders: Rc<std::cell::Cell<usize>>,
    leaf: Entity<SelectiveLeafView>,
}

impl Render for SelectiveFallbackParentView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        // Intentionally leave the leaf uncached. A leaf notify must be promoted to this cached
        // parent boundary instead of forcing the window root to render.
        crate::div().child(self.leaf.clone())
    }
}

struct SelectiveFallbackRootView {
    renders: Rc<std::cell::Cell<usize>>,
    parent: Entity<SelectiveFallbackParentView>,
}

impl Render for SelectiveFallbackRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().child(
            AnyView::from(self.parent.clone()).cached(
                StyleRefinement::default().w(px(120.)).h(px(32.)),
            ),
        )
    }
}


struct SelectiveBudgetRootView {
    renders: Rc<std::cell::Cell<usize>>,
    parent: Entity<SelectiveParentView>,
    expire_budget: bool,
}

impl Render for SelectiveBudgetRootView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        if self.expire_budget {
            window.test_expire_draw_budget();
        }
        crate::div().child(
            AnyView::from(self.parent.clone()).cached(
                StyleRefinement::default().w(px(120.)).h(px(32.)),
            ),
        )
    }
}


struct SelectiveMultiRootView {
    renders: Rc<std::cell::Cell<usize>>,
    left: Entity<SelectiveLeafView>,
    right: Entity<SelectiveLeafView>,
}

impl Render for SelectiveMultiRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div()
            .flex()
            .child(
                AnyView::from(self.left.clone()).cached(
                    StyleRefinement::default().w(px(120.)).h(px(32.)),
                ),
            )
            .child(
                AnyView::from(self.right.clone()).cached(
                    StyleRefinement::default().w(px(120.)).h(px(32.)),
                ),
            )
    }
}

struct SelectiveCompositeRootView {
    renders: Rc<std::cell::Cell<usize>>,
    parent: Entity<SelectiveParentView>,
}

impl Render for SelectiveCompositeRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        crate::div().child(
            AnyView::from(self.parent.clone())
                .cached(StyleRefinement::default().w(px(120.)).h(px(32.)))
                .composite_layer(),
        )
    }
}

#[gpui::test]
fn traversal_ancestor_splices_one_dirty_cached_descendant_without_parent_render(
    cx: &mut TestAppContext,
) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let parent_renders = Rc::new(std::cell::Cell::new(0));
    let leaf_renders = Rc::new(std::cell::Cell::new(0));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| SelectiveLeafView {
            renders: leaf_renders.clone(),
            revision: 0,
        });
        let parent = cx.new(|_| SelectiveParentView {
            renders: parent_renders.clone(),
            leaf: leaf.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SelectiveRootView {
                    renders: root_renders.clone(),
                    parent,
                })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();
    let root_baseline = root_renders.get();
    let parent_baseline = parent_renders.get();
    let leaf_baseline = leaf_renders.get();
    assert!(root_baseline > 0 && parent_baseline > 0 && leaf_baseline > 0);

    for revision in 1..=2 {
        leaf.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();

        assert_eq!(
            root_renders.get(),
            root_baseline,
            "TraversalAncestor must not call the window root Render"
        );
        assert_eq!(
            parent_renders.get(),
            parent_baseline,
            "TraversalAncestor must not call the clean cached parent Render"
        );
        assert_eq!(
            leaf_renders.get(),
            leaf_baseline + revision,
            "the direct dirty child must still rebuild on every notify"
        );
    }
}

#[gpui::test]
fn selective_splice_survives_expired_draw_budget(
    cx: &mut TestAppContext,
) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let parent_renders = Rc::new(std::cell::Cell::new(0));
    let leaf_renders = Rc::new(std::cell::Cell::new(0));
    let (root, leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| SelectiveLeafView {
            renders: leaf_renders.clone(),
            revision: 0,
        });
        let parent = cx.new(|_| SelectiveParentView {
            renders: parent_renders.clone(),
            leaf: leaf.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SelectiveBudgetRootView {
                    renders: root_renders.clone(),
                    parent,
                    expire_budget: false,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _| root).unwrap();
        (root, leaf, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let root_baseline = root_renders.get();
    let parent_baseline = parent_renders.get();
    let leaf_baseline = leaf_renders.get();

    // Force the root to render so it can expire the generation deadline before GPUI reaches the
    // cached parent. The parent is still only a TraversalAncestor of the dirty leaf.
    root.update(cx, |root, cx| {
        root.expire_budget = true;
        cx.notify();
    });
    leaf.update(cx, |leaf, cx| {
        leaf.revision = 1;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    assert_eq!(root_renders.get(), root_baseline + 1);
    assert_eq!(
        parent_renders.get(),
        parent_baseline,
        "expired generation budget must not disable selective splice and rerender the ancestor"
    );
    assert_eq!(
        leaf_renders.get(),
        leaf_baseline + 1,
        "the direct dirty leaf must still rebuild after the deadline expires"
    );
}

#[gpui::test]
fn generic_dirty_descendant_promotes_to_nearest_cached_parent_without_root_render(
    cx: &mut TestAppContext,
) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let parent_renders = Rc::new(std::cell::Cell::new(0));
    let leaf_renders = Rc::new(std::cell::Cell::new(0));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| SelectiveLeafView {
            renders: leaf_renders.clone(),
            revision: 0,
        });
        let parent = cx.new(|_| SelectiveFallbackParentView {
            renders: parent_renders.clone(),
            leaf: leaf.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SelectiveFallbackRootView {
                    renders: root_renders.clone(),
                    parent,
                })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let root_baseline = root_renders.get();
    let parent_baseline = parent_renders.get();
    let leaf_baseline = leaf_renders.get();
    assert!(root_baseline > 0 && parent_baseline > 0 && leaf_baseline > 0);

    leaf.update(cx, |leaf, cx| {
        leaf.revision = 1;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    assert_eq!(
        root_renders.get(),
        root_baseline,
        "a non-cached dirty descendant must not widen past its nearest cached parent"
    );
    assert_eq!(
        parent_renders.get(),
        parent_baseline + 1,
        "the promoted cached boundary must render fresh"
    );
    assert_eq!(
        leaf_renders.get(),
        leaf_baseline + 1,
        "fresh parent rendering must rebuild the original dirty child"
    );
}

#[gpui::test]
fn traversal_ancestor_splices_two_dirty_cached_siblings_without_root_render(
    cx: &mut TestAppContext,
) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let left_renders = Rc::new(std::cell::Cell::new(0));
    let right_renders = Rc::new(std::cell::Cell::new(0));
    let (left, right, window) = cx.update(|cx| {
        let left = cx.new(|_| SelectiveLeafView {
            renders: left_renders.clone(),
            revision: 0,
        });
        let right = cx.new(|_| SelectiveLeafView {
            renders: right_renders.clone(),
            revision: 0,
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SelectiveMultiRootView {
                    renders: root_renders.clone(),
                    left: left.clone(),
                    right: right.clone(),
                })
            })
            .unwrap();
        (left, right, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();
    let root_baseline = root_renders.get();
    let left_baseline = left_renders.get();
    let right_baseline = right_renders.get();
    assert!(root_baseline > 0 && left_baseline > 0 && right_baseline > 0);

    for revision in 1..=2 {
        left.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        right.update(cx, |leaf, cx| {
            leaf.revision = revision;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();

        assert_eq!(
            root_renders.get(),
            root_baseline,
            "two independent DirectDirty children must not rerender the window root"
        );
        assert_eq!(left_renders.get(), left_baseline + revision);
        assert_eq!(right_renders.get(), right_baseline + revision);
    }
}

#[gpui::test]
fn selective_splice_falls_back_when_dirty_view_is_inside_composite_capture(
    cx: &mut TestAppContext,
) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let parent_renders = Rc::new(std::cell::Cell::new(0));
    let leaf_renders = Rc::new(std::cell::Cell::new(0));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| SelectiveLeafView {
            renders: leaf_renders.clone(),
            revision: 0,
        });
        let parent = cx.new(|_| SelectiveParentView {
            renders: parent_renders.clone(),
            leaf: leaf.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| SelectiveCompositeRootView {
                    renders: root_renders.clone(),
                    parent,
                })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let root_baseline = root_renders.get();
    let parent_baseline = parent_renders.get();
    let leaf_baseline = leaf_renders.get();
    assert!(root_baseline > 0 && parent_baseline > 0 && leaf_baseline > 0);

    leaf.update(cx, |leaf, cx| {
        leaf.revision = 1;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    assert!(
        root_renders.get() > root_baseline,
        "window-root selective splice must fall back when the dirty route crosses a composite capture"
    );
    assert!(
        parent_renders.get() > parent_baseline,
        "a cached ancestor inside a composite capture must rebuild instead of detached scene replay"
    );
    assert_eq!(
        leaf_renders.get(),
        leaf_baseline + 1,
        "the direct dirty leaf must still rebuild"
    );
}

#[gpui::test]
fn cached_view_infers_stable_fingerprint(cx: &mut TestAppContext) {
    let view = cx.update(|cx| cx.new(|_| EmptyView));

    let cached_view = AnyView::from(view.clone()).cached(StyleRefinement::default());
    let same_cached_view = AnyView::from(view).cached(StyleRefinement::default());

    assert_eq!(
        cached_view.cache_fingerprint(),
        same_cached_view.cache_fingerprint()
    );
    assert!(cached_view.cache_fingerprint().is_some());
}

#[gpui::test]
fn cached_view_preserves_explicit_fingerprint_through_weak_upgrade(cx: &mut TestAppContext) {
    let view = cx.update(|cx| cx.new(|_| EmptyView));
    let cached_view = AnyView::from(view).cached_with_fingerprint(StyleRefinement::default(), 42);
    let weak_view = cached_view.downgrade();
    let upgraded = weak_view.upgrade().expect("view should still be alive");

    assert_eq!(upgraded.cache_fingerprint(), Some(42));
}

#[gpui::test]
fn cached_view_hashes_semantic_key_in_framework(cx: &mut TestAppContext) {
    let view = cx.update(|cx| cx.new(|_| EmptyView));
    let cached_view = AnyView::from(view).cached_by(StyleRefinement::default(), &("route", 7_u64));

    assert_eq!(
        cached_view.cache_fingerprint(),
        Some(render_fingerprint(&("route", 7_u64)))
    );
}

#[gpui::test]
fn cached_view_reuses_after_sibling_notify(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (sibling, window) = cx.update(|cx| {
        let leaf = cx.new(|_| RenderCountCachedLeafView {
            renders: renders.clone(),
        });
        let sibling = cx.new(|_| DirtySiblingView { revision: 0 });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| CachedSiblingRootView {
                    leaf: leaf.clone(),
                    sibling: sibling.clone(),
                })
            })
            .unwrap();
        (sibling, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    sibling.update(cx, |sibling, cx| {
        sibling.revision = sibling.revision.saturating_add(1);
        cx.notify();
    });

    assert_eq!(renders.get(), baseline);
}

#[gpui::test]
fn cached_view_reuses_during_platform_refreshing_frame(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (sibling, window) = cx.update(|cx| {
        let leaf = cx.new(|_| RenderCountCachedLeafView {
            renders: renders.clone(),
        });
        let sibling = cx.new(|_| DirtySiblingView { revision: 0 });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| CachedSiblingRootView {
                    leaf: leaf.clone(),
                    sibling: sibling.clone(),
                })
            })
            .unwrap();
        (sibling, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    cx.update_window(window, |_, window, cx| {
        sibling.update(cx, |sibling, _cx| {
            sibling.revision = sibling.revision.saturating_add(1);
        });
        window.dirty_views.insert(sibling.entity_id());
        window.invalidator.set_dirty(true);
        window.refreshing = true;
        window.draw(cx).clear();
    })
    .unwrap();

    assert_eq!(renders.get(), baseline);
}

#[gpui::test]
fn window_refresh_forces_cached_view_to_render(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let window = cx.update(|cx| {
        let leaf = cx.new(|_| RenderCountCachedLeafView {
            renders: renders.clone(),
        });
        let sibling = cx.new(|_| DirtySiblingView { revision: 0 });
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| CachedSiblingRootView { leaf, sibling })
        })
        .unwrap()
    });
    let window = AnyWindowHandle::from(window);

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    cx.update_window(window, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear();
    })
    .unwrap();

    assert!(renders.get() > baseline);
}

#[gpui::test]
fn reuse_on_window_refresh_cached_view_reuses_during_refresh(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let window = cx.update(|cx| {
        let leaf = cx.new(|_| RenderCountCachedLeafView {
            renders: renders.clone(),
        });
        let sibling = cx.new(|_| DirtySiblingView { revision: 0 });
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| RefreshRetainedCachedRootView { leaf, sibling })
        })
        .unwrap()
    });
    let window = AnyWindowHandle::from(window);

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    cx.update_window(window, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear();
    })
    .unwrap();

    assert_eq!(renders.get(), baseline);
}

#[gpui::test]
fn progressive_cached_dirty_view_reuses_when_budget_is_exhausted(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (root, leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| RenderCountCachedLeafView {
            renders: renders.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| ProgressiveCachedRootView {
                    leaf: leaf.clone(),
                    expire_budget: false,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _cx| root).unwrap();
        (root, leaf, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, _cx| {
            root.expire_budget = true;
        });
        window.dirty_views.insert(leaf.entity_id());
        window.invalidator.set_dirty(true);
        window.draw(cx).clear();
        assert!(window.draw_was_degraded());
    })
    .unwrap();

    assert_eq!(renders.get(), baseline);
}

#[gpui::test]
fn degraded_draw_does_not_publish_discarded_cached_view_ranges(cx: &mut TestAppContext) {
    let renders = Rc::new(std::cell::Cell::new(0));
    let (root, window) = cx.update(|cx| {
        let leaf = cx.new(|_| HitboxCachedLeafView {
            renders: renders.clone(),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DegradedCachedRootView {
                    leaf: leaf.clone(),
                    include_prefix_hitbox: false,
                    expire_budget: false,
                })
            })
            .unwrap();
        let root = cx.read_window(&window, |root, _cx| root).unwrap();
        (root, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
        assert_eq!(window.rendered_frame.hitboxes.len(), 1);
    })
    .unwrap();

    let baseline = renders.get();
    assert!(baseline > 0, "cached leaf should render at least once");

    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, _cx| {
            root.include_prefix_hitbox = true;
            root.expire_budget = true;
        });
        window.refresh();
        window.draw(cx).clear();
        assert!(window.draw_was_degraded());
        assert!(window.test_recovering_degraded_draw());
        assert!(!window.test_force_full_redraw());

        root.update(cx, |root, _cx| {
            root.expire_budget = false;
        });
        window.invalidator.set_dirty(true);
        window.draw(cx).clear();
        assert!(!window.draw_was_degraded());
        assert_eq!(window.rendered_frame.hitboxes.len(), 2);
        assert!(!window.test_render_dirty_region_is_full());
    })
    .unwrap();
}

#[gpui::test]
fn cached_deferred_draw_reuse_records_one_scene_segment(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        let leaf = cx.new(|_| DeferredCachedLeafView);
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| DeferredCachedRootView { leaf })
        })
        .unwrap()
    });
    let window = AnyWindowHandle::from(window);

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
        assert_ne!(window.rendered_frame.scene.len(), 0);
        assert_eq!(window.rendered_frame.retained_scene_segments.len(), 1);

        window.refresh();
        window.draw(cx).clear();
        assert_ne!(window.rendered_frame.scene.len(), 0);
        assert_eq!(window.rendered_frame.retained_scene_segments.len(), 1);
    })
    .unwrap();
}

#[gpui::test]
fn cached_absolute_view_lays_out_nested_absolute_root_to_cache_bounds(cx: &mut TestAppContext) {
    let recorded_bounds = Rc::new(std::cell::RefCell::new(Vec::new()));
    let (leaf, window) = cx.update(|cx| {
        let leaf = cx.new(|_| AbsoluteCachedLeafView {
            bounds: recorded_bounds.clone(),
            revision: 0,
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| AbsoluteCachedRootView { leaf: leaf.clone() })
            })
            .unwrap();
        (leaf, AnyWindowHandle::from(window))
    });

    let expected_size = cx
        .update_window(window, |_, window, _| window.bounds().size)
        .unwrap();

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();

    cx.update(|cx| {
        leaf.update(cx, |leaf, cx| {
            leaf.revision = leaf.revision.saturating_add(1);
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
    });

    let bounds = recorded_bounds.borrow();
    let latest = bounds.last().expect("absolute cached view should prepaint");
    assert_eq!(latest.origin, point(px(0.0), px(0.0)));
    assert_eq!(latest.size, expected_size);
    assert!(latest.size.width > px(0.0));
    assert!(latest.size.height > px(0.0));
}

#[gpui::test]
fn progressive_cached_view_preserves_flag_through_weak_upgrade(cx: &mut TestAppContext) {
    let view = cx.update(|cx| cx.new(|_| EmptyView));
    let cached_view = AnyView::from(view)
        .cached(StyleRefinement::default())
        .progressive();
    let weak_view = cached_view.downgrade();
    let upgraded = weak_view.upgrade().expect("view should still be alive");

    assert!(upgraded.progressive);
    assert!(upgraded.cache_fingerprint().is_some());
}

#[gpui::test]
fn reuse_on_window_refresh_cached_view_preserves_flag_through_weak_upgrade(
    cx: &mut TestAppContext,
) {
    let view = cx.update(|cx| cx.new(|_| EmptyView));
    let cached_view = AnyView::from(view)
        .cached(StyleRefinement::default())
        .reuse_on_window_refresh();
    let weak_view = cached_view.downgrade();
    let upgraded = weak_view.upgrade().expect("view should still be alive");

    assert!(upgraded.reuse_on_window_refresh);
    assert!(upgraded.cache_fingerprint().is_some());
}
