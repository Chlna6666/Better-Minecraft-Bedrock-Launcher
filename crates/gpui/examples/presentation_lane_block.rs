//! Native presentation-lane gate: the renderer must advance while UI Render is blocked.
//!
//! Run with `--backend=nova-dx12` or `--backend=nova-vulkan`. The process exits with status 1
//! unless at least two distinct animation samples are presented during a 200 ms UI Render block.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use gpui::{
    Animation, AnimationExt as _, AnimationGroup, App, Application, Bounds, Context, Point,
    RendererBackend, RendererOptions, Window, WindowBounds, WindowOptions, div,
    performance_metrics_snapshot, prelude::*, px, rgb, size,
};

const BLOCK_DURATION: Duration = Duration::from_millis(200);
const INITIAL_PRESENT_TIMEOUT: Duration = Duration::from_secs(30);
const UI_BLOCK_TIMEOUT: Duration = Duration::from_secs(35);

fn native_presents() -> (usize, usize) {
    let metrics = performance_metrics_snapshot();
    (
        metrics.direct_present_count + metrics.retained_present_count,
        metrics.presentation_animation_distinct_sample_count,
    )
}

struct BlockView {
    block_next_render: bool,
    block_probe: Option<mpsc::Sender<BlockProbe>>,
    render_count: Arc<AtomicUsize>,
}

struct BlockProbe {
    time: Instant,
    presents: (usize, usize),
    renders: usize,
}

impl BlockProbe {
    fn capture(render_count: &AtomicUsize) -> Self {
        Self {
            time: Instant::now(),
            presents: native_presents(),
            renders: render_count.load(Ordering::Relaxed),
        }
    }
}

impl Render for BlockView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count.fetch_add(1, Ordering::Relaxed);
        if self.block_next_render {
            self.block_next_render = false;
            if let Some(sender) = self.block_probe.take() {
                if sender
                    .send(BlockProbe::capture(&self.render_count))
                    .is_err()
                {
                    eprintln!("presentation observer exited before UI Render blocked");
                }
                // Deliberately block the UI owner to verify that the native owner keeps presenting.
                std::thread::sleep(BLOCK_DURATION);
                if sender
                    .send(BlockProbe::capture(&self.render_count))
                    .is_err()
                {
                    eprintln!("presentation observer exited before UI Render unblocked");
                }
            }
        }

        div()
            .size_full()
            .bg(rgb(0xffffff))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div().size(px(180.)).bg(rgb(0x0088ff)).with_animation_group(
                    "presentation-lane-multitrack",
                    AnimationGroup::parallel([
                        Animation::new(Duration::from_millis(700))
                            .repeat()
                            .with_opacity(0.15, 1.0),
                        Animation::new(Duration::from_millis(950))
                            .repeat()
                            .with_easing(|progress| progress * progress)
                            .with_translation(Point::default(), gpui::point(px(32.), px(0.))),
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(|progress| 1.0 - (1.0 - progress).powi(3))
                            .with_scale(0.8, 1.0),
                    ])
                    .expect("the presentation gate uses three distinct visual properties"),
                ),
            )
    }
}

fn main() {
    let backend = match std::env::args().nth(1) {
        Some(argument) => match argument
            .strip_prefix("--backend=")
            .and_then(|value| value.parse().ok())
        {
            Some(backend @ (RendererBackend::NovaDx12 | RendererBackend::NovaVulkan)) => backend,
            _ => {
                eprintln!("expected --backend=nova-dx12 or --backend=nova-vulkan");
                std::process::exit(2);
            }
        },
        None => {
            eprintln!("expected --backend=nova-dx12 or --backend=nova-vulkan");
            std::process::exit(2);
        }
    };

    let (block_probe, receiver) = mpsc::channel::<BlockProbe>();
    let render_count = Arc::new(AtomicUsize::new(0));
    let observer_renders = render_count.clone();
    std::thread::spawn(move || {
        let before = match receiver.recv_timeout(UI_BLOCK_TIMEOUT) {
            Ok(probe) => probe,
            Err(error) => {
                eprintln!("FAIL: UI Render block did not start: {error}");
                std::process::exit(1);
            }
        };
        std::thread::sleep(BLOCK_DURATION / 2);
        let middle = native_presents();
        let middle_renders = observer_renders.load(Ordering::Relaxed);
        let after = match receiver.recv_timeout(UI_BLOCK_TIMEOUT) {
            Ok(probe) => probe,
            Err(error) => {
                eprintln!("FAIL: UI Render block did not finish: {error}");
                std::process::exit(1);
            }
        };
        let elapsed = after.time.duration_since(before.time);
        let passed = elapsed >= BLOCK_DURATION
            && after.presents.0 >= before.presents.0 + 2
            && middle.0 > before.presents.0
            && after.presents.1 >= before.presents.1 + 2
            && before.renders == middle_renders
            && before.renders == after.renders;
        println!(
            "{}: presents during UI Render block ({elapsed:?}): {} -> {} -> {}; distinct animation samples: {} -> {} -> {}; Render calls: {} -> {} -> {}",
            if passed { "PASS" } else { "FAIL" },
            before.presents.0,
            middle.0,
            after.presents.0,
            before.presents.1,
            middle.1,
            after.presents.1,
            before.renders,
            middle_renders,
            after.renders,
        );
        std::process::exit(if passed { 0 } else { 1 });
    });

    let options = RendererOptions {
        backend,
        ..Default::default()
    };
    if let Err(error) = Application::run_separate(
        options.clone(),
        move || Application::with_renderer_options(options),
        move |cx: &mut App| {
            let view = cx.new(|_| BlockView {
                block_next_render: false,
                block_probe: Some(block_probe),
                render_count,
            });
            let window_view = view.clone();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(480.), px(320.)),
                    cx,
                ))),
                ..Default::default()
            };
            if let Err(error) = cx.open_window(options, move |_, _| window_view) {
                eprintln!("failed to open presentation-lane gate window: {error:#}");
                std::process::exit(1);
            }
            cx.activate(true);
            cx.spawn(async move |cx| {
                let deadline = Instant::now() + INITIAL_PRESENT_TIMEOUT;
                while native_presents().0 < 2 {
                    if Instant::now() >= deadline {
                        eprintln!("FAIL: native renderer did not present the initial animation");
                        std::process::exit(1);
                    }
                    gpui::Timer::after(Duration::from_millis(50)).await;
                }
                gpui::Timer::after(Duration::from_millis(100)).await;
                if let Err(error) = cx.update(|cx| {
                    view.update(cx, |view, cx| {
                        view.block_next_render = true;
                        cx.notify();
                    })
                }) {
                    eprintln!("failed to schedule UI Render block: {error:#}");
                    std::process::exit(1);
                }
            })
            .detach();
        },
    ) {
        eprintln!("failed to run presentation-lane gate: {error:#}");
        std::process::exit(1);
    }
}
