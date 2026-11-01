use crate::{
    Bounds, Context, IntoElement, Pixels, Render, Styled, TestAppContext, UniformListScrollHandle,
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
            .0
            .borrow()
            .base_handle
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
