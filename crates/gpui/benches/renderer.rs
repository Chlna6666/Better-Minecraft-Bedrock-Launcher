use gpui::{BenchAppContext, Context, IntoElement, Render, Styled, Window, div, px};

struct RendererBenchView {
    revision: u64,
}

impl Render for RendererBenchView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.0 + (self.revision & 1) as f32))
            .h(px(180.0))
    }
}

#[gpui::bench]
fn frame_driven_root_update(cx: &mut BenchAppContext<'_, '_>) {
    let window = cx.add_window(|_, _| RendererBenchView { revision: 0 });

    cx.bench_renderer(window, |view, _window, cx| {
        view.revision = view.revision.wrapping_add(1);
        cx.notify();
    });
}

gpui::bench_group!(gpui_renderer, frame_driven_root_update);
gpui::bench_main!(gpui_renderer);
