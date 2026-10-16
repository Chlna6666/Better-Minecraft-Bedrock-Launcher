use super::*;

struct Reader {
    source: Entity<PlainLeafView>,
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for Reader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .w(px(80.))
            .h(px(40.))
            .bg(crate::rgb(if self.source.read(cx).revision == 0 {
                0xffffff
            } else {
                0xaa00aa
            }))
    }
}

struct Root {
    source: Entity<PlainLeafView>,
    reader: Entity<Reader>,
    independent: Entity<PlainLeafView>,
}

struct DataReaderRoot {
    reader: Entity<DataRootView>,
    independent: Entity<PlainLeafView>,
    renders: Rc<std::cell::Cell<usize>>,
}

impl Render for DataReaderRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .flex()
            .child(self.reader.clone())
            .child(self.independent.clone())
    }
}

#[gpui::test]
fn data_dependency_rebuilds_only_its_reader(cx: &mut TestAppContext) {
    let root_renders = Rc::new(std::cell::Cell::new(0));
    let reader_renders = Rc::new(std::cell::Cell::new(0));
    let independent_renders = Rc::new(std::cell::Cell::new(0));
    let (data, window) = cx.update(|cx| {
        let data = cx.new(|_| Data { blue: false });
        let reader = cx.new(|_| DataRootView {
            data: data.clone(),
            renders: reader_renders.clone(),
        });
        let independent = cx.new(|_| PlainLeafView {
            renders: independent_renders.clone(),
            revision: 0,
            width: px(80.),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DataReaderRoot {
                    reader,
                    independent,
                    renders: root_renders.clone(),
                })
            })
            .unwrap();
        (data, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let root_baseline = root_renders.get();
    let reader_baseline = reader_renders.get();
    let independent_baseline = independent_renders.get();
    data.update(cx, |data, cx| {
        data.blue = true;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    assert_eq!(root_renders.get(), root_baseline);
    assert_eq!(reader_renders.get(), reader_baseline + 1);
    assert_eq!(independent_renders.get(), independent_baseline);
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| { quad.background.solid == crate::rgb(0x0000ff) })
        );
        assert!(
            !window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| { quad.background.solid == crate::rgb(0xff0000) })
        );
    })
    .unwrap();
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(
                AnyView::from(self.source.clone())
                    .cached(StyleRefinement::default().w(px(80.)).h(px(40.))),
            )
            .child(
                AnyView::from(self.reader.clone())
                    .cached(StyleRefinement::default().w(px(80.)).h(px(40.))),
            )
            .child(
                AnyView::from(self.independent.clone())
                    .cached(StyleRefinement::default().w(px(80.)).h(px(40.))),
            )
    }
}

#[gpui::test]
fn sibling_dependency_rebuilds_reader_without_refreshing_unrelated_cache(cx: &mut TestAppContext) {
    let reader_renders = Rc::new(std::cell::Cell::new(0));
    let independent_renders = Rc::new(std::cell::Cell::new(0));
    let (source, window) = cx.update(|cx| {
        let source = cx.new(|_| PlainLeafView {
            renders: Rc::new(std::cell::Cell::new(0)),
            revision: 0,
            width: px(80.),
        });
        let reader = cx.new(|_| Reader {
            source: source.clone(),
            renders: reader_renders.clone(),
        });
        let independent = cx.new(|_| PlainLeafView {
            renders: independent_renders.clone(),
            revision: 0,
            width: px(80.),
        });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Root {
                    source: source.clone(),
                    reader,
                    independent,
                })
            })
            .unwrap();
        (source, AnyWindowHandle::from(window))
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let reader_baseline = reader_renders.get();
    let independent_baseline = independent_renders.get();
    source.update(cx, |source, cx| {
        source.revision = 1;
        cx.notify();
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    assert_eq!(reader_renders.get(), reader_baseline + 1);
    assert_eq!(independent_renders.get(), independent_baseline);
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| quad.background.solid == crate::rgb(0xaa00aa))
        );
        assert!(
            !window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| quad.background.solid == crate::rgb(0xffffff))
        );
    })
    .unwrap();
}
