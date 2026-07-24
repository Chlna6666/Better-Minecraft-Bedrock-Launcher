mod startup_report;

use gpui::{
    App, Application, Bounds, Context, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use std::time::Duration;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

struct MinimalWindow;

impl Render for MinimalWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x101214))
            .text_color(rgb(0xe7ecef))
            .child("minimal")
    }
}

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::builder()
        .file_name("target/gpui-minimal-dhat-heap.json")
        .trim_backtraces(Some(20))
        .build();

    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(320.0), px(200.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| MinimalWindow),
        ) {
            eprintln!("failed to open minimal window: {error:#}");
        }
        #[cfg(feature = "dhat-heap")]
        cx.spawn(async move |cx| {
            gpui::Timer::after(std::time::Duration::from_secs(5)).await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();

        if startup_report::report_path().is_some() {
            cx.spawn(async move |cx| {
                gpui::Timer::after(Duration::from_secs(10)).await;
                let _ = cx.update(|cx| {
                    startup_report::write_if_requested();
                    cx.quit();
                });
            })
            .detach();
        }

        cx.activate(true);
    });
}
