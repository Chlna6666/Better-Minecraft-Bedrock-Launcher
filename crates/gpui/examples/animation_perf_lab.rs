//! Retained animation stress gallery and presentation-lane gate.
//!
//! On Windows, run with `--backend=nova-dx12` and `--backend=nova-vulkan`.
//! Visual tracks run forever in alternating directions so values remain
//! continuous at each cycle boundary. The example blocks UI `Render` for
//! 200 ms, then measures presentation continuity, interval percentiles, and
//! skipped frames over a sustained run. Layout/color fallback tracks start
//! after the compositor-only interval so their UI work can be compared.
//!
//! The gallery covers opacity, translation, relative translation, scale,
//! rotation, blur, horizontal/vertical reveal, captured subtree translation,
//! scale-opacity, translation-opacity, parallel groups, spring, custom easing,
//! repeated retargeting, and a layout/color callback fallback. `--copies=N`
//! multiplies each visual track to expose CPU and upload scaling.

use std::{
    borrow::Cow,
    error::Error,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{
    App, Application, AssetSource, Bounds, Context, RendererBackend, RendererOptions, SharedString,
    Timer, TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowOptions, div,
    prelude::*, px, rgb, size, window_metrics_snapshot,
};

mod animation_perf_lab_support;

const UI_RENDER_BLOCK: Duration = Duration::from_millis(200);
const GATE_WINDOW: Duration = Duration::from_secs(35);
const INITIAL_PRESENT_TIMEOUT: Duration = Duration::from_secs(20);
const SAMPLE_HISTORY_CAPACITY: usize = 256;
const DEFAULT_MEASUREMENT: Duration = Duration::from_secs(30);
const DEFAULT_COPIES: usize = 4;
const MAX_COPIES: usize = 64;

#[derive(Clone, Copy)]
struct Config {
    backend: RendererBackend,
    copies: usize,
    measurement: Duration,
    visual_only: bool,
    client_titlebar: bool,
}

struct ExampleAssets {
    examples: PathBuf,
}

impl AssetSource for ExampleAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let path = if path == "animation-perf-icon.png" {
            self.examples.join("image/app-icon.png")
        } else {
            self.examples.join(path)
        };
        Ok(Some(Cow::Owned(std::fs::read(path)?)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

struct AnimationLab {
    copies: usize,
    client_titlebar: bool,
    window_id: u64,
    phase: bool,
    axis_comparison_enabled: bool,
    spring_comparison_enabled: bool,
    layout_fallback_enabled: bool,
    block_next_render: bool,
    baseline_presentation_rate: f64,
    sample_interval_p50_micros: u64,
    block_started: Option<mpsc::Sender<animation_perf_lab_support::BlockEvent>>,
    render_count: Arc<AtomicUsize>,
}

impl Render for AnimationLab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count.fetch_add(1, Ordering::Relaxed);
        if self.block_next_render {
            self.block_next_render = false;
            let block_sender = self.block_started.take();
            let block_metrics = window_metrics_snapshot()
                .into_iter()
                .find(|metrics| metrics.window_id == self.window_id);
            let started_at = Instant::now();
            if let Some(sender) = &block_sender
                && sender
                    .send(animation_perf_lab_support::BlockEvent::Started(
                        animation_perf_lab_support::GateBaseline {
                            window_id: self.window_id,
                            sample_rate: self.baseline_presentation_rate,
                            sample_interval_p50_micros: self.sample_interval_p50_micros,
                            sample_sequence: block_metrics.as_ref().map_or(0, |metrics| {
                                metrics.animation_sampled_present_count as u64
                            }),
                            changed_sample_count: block_metrics.as_ref().map_or(0, |metrics| {
                                metrics.animation_sample_changed_present_count as u64
                            }),
                            unchanged_sample_count: block_metrics.as_ref().map_or(0, |metrics| {
                                metrics.animation_sample_unchanged_present_count as u64
                            }),
                            skipped_frame_count: block_metrics
                                .as_ref()
                                .map_or(0, |metrics| metrics.skipped_frame_count as u64),
                            started_at,
                            render_count: self.render_count.load(Ordering::Relaxed),
                        },
                    ))
                    .is_err()
            {
                eprintln!("animation observer exited before the UI Render block");
            }
            // The native presentation owner must keep sampling the committed scene here.
            thread::sleep(UI_RENDER_BLOCK);
            let ended_at = Instant::now();
            if let Some(sender) = block_sender
                && sender
                    .send(animation_perf_lab_support::BlockEvent::Finished {
                        ended_at,
                        render_count: self.render_count.load(Ordering::Relaxed),
                    })
                    .is_err()
            {
                eprintln!("animation observer exited before the UI Render block ended");
            }
        }

        let mut cards =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(animation_perf_lab_support::visual_cards(
                    self.copies,
                    self.phase,
                ));
        if self.axis_comparison_enabled {
            cards = cards.child(animation_perf_lab_support::axis_comparison_card(self.phase));
        }
        if self.spring_comparison_enabled {
            cards = cards.child(animation_perf_lab_support::spring_comparison_card(
                self.phase,
            ));
        }
        if self.layout_fallback_enabled {
            cards = cards
                .child(animation_perf_lab_support::layout_fallback_card())
                .child(animation_perf_lab_support::color_fallback_card());
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x111827))
            .p_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .when(self.client_titlebar, |header| header.h(px(48.0)))
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgb(0xf8fafc))
                            .when(self.client_titlebar, |title| {
                                title
                                    .flex_1()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .window_control_area(WindowControlArea::Drag)
                            })
                            .child("GPUI retained animation performance lab"),
                    )
                    .child(
                        div()
                            .id("retarget-all")
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .px_3()
                            .py_2()
                            .text_color(rgb(0xffffff))
                            .child("Retarget all tracks")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.phase = !view.phase;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().text_sm().text_color(rgb(0xcbd5e1)).child(format!(
                "{} copies per visual track · click Retarget while running · UI Render calls: {}",
                self.copies,
                self.render_count.load(Ordering::Relaxed),
            )))
            .child(
                div()
                    .id("animation-gallery-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(cards),
            )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_config()?;
    let options = RendererOptions {
        backend: config.backend,
        frame_metrics: true,
        ..RendererOptions::default()
    };
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let (block_started, block_receiver) = mpsc::channel();
    let render_count = Arc::new(AtomicUsize::new(0));
    let observed_render_count = render_count.clone();
    thread::spawn(move || {
        animation_perf_lab_support::observe(config, block_receiver, observed_render_count);
    });

    #[cfg(target_os = "windows")]
    {
        Application::run_separate(
            options.clone(),
            move || {
                Application::with_renderer_options(options).with_assets(ExampleAssets { examples })
            },
            move |cx| open_lab(cx, config, block_started, render_count),
        )?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        Application::with_renderer_options(options)
            .with_assets(ExampleAssets { examples })
            .run(move |cx| open_lab(cx, config, block_started, render_count));
    }
    Ok(())
}

fn open_lab(
    cx: &mut App,
    config: Config,
    block_started: mpsc::Sender<animation_perf_lab_support::BlockEvent>,
    render_count: Arc<AtomicUsize>,
) {
    let view = cx.new(|_| AnimationLab {
        copies: config.copies,
        client_titlebar: config.client_titlebar,
        window_id: 0,
        phase: false,
        axis_comparison_enabled: false,
        spring_comparison_enabled: false,
        layout_fallback_enabled: false,
        block_next_render: false,
        baseline_presentation_rate: 60.0,
        sample_interval_p50_micros: 16_667,
        block_started: Some(block_started),
        render_count,
    });
    let window_view = view.clone();
    let window_bounds = Bounds::centered(None, size(px(1120.0), px(820.0)), cx);
    let window_handle = match cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: config.client_titlebar.then(|| TitlebarOptions {
                appears_transparent: true,
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        },
        move |_, _| window_view,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("failed to open animation performance lab: {error:#}");
            cx.quit();
            return;
        }
    };
    let window_id = window_handle.window_id().as_u64();
    view.update(cx, |view, _| view.window_id = window_id);
    cx.activate(true);

    cx.spawn(async move |cx| {
        let initial_present_deadline = Instant::now() + INITIAL_PRESENT_TIMEOUT;
        while animation_sample_count(window_id) < 2 {
            if Instant::now() >= initial_present_deadline {
                eprintln!(
                    "presentation lane did not present the initial scene in {INITIAL_PRESENT_TIMEOUT:?}"
                );
                return;
            }
            Timer::after(Duration::from_millis(50)).await;
        }

        let history_deadline = Instant::now() + GATE_WINDOW;
        loop {
            let Some(metrics) = window_metrics_snapshot()
                .into_iter()
                .find(|window| window.window_id == window_id)
            else {
                eprintln!("FAIL: could not read animation window warm-up metrics");
                return;
            };
            if metrics.animation_sample_interval_sample_count >= SAMPLE_HISTORY_CAPACITY {
                break;
            }
            if Instant::now() >= history_deadline {
                eprintln!(
                    "FAIL: animation sample history did not fill during warm-up ({}/{})",
                    metrics.animation_sample_interval_sample_count, SAMPLE_HISTORY_CAPACITY
                );
                return;
            }
            Timer::after(Duration::from_millis(50)).await;
        }

        let baseline_started = Instant::now();
        let baseline_sample_count = animation_sample_count(window_id);
        Timer::after(Duration::from_millis(500)).await;
        let Some(baseline_metrics) = window_metrics_snapshot()
            .into_iter()
            .find(|window| window.window_id == window_id)
        else {
            eprintln!("FAIL: could not read animation window cadence baseline");
            return;
        };
        let baseline_presentation_rate = animation_sample_count(window_id)
            .saturating_sub(baseline_sample_count) as f64
            / baseline_started.elapsed().as_secs_f64();
        let sample_interval_p50_micros = baseline_metrics.animation_sample_interval_p50_micros;
        if baseline_presentation_rate <= 0.0 || sample_interval_p50_micros == 0 {
            eprintln!("FAIL: animation samples did not establish a frame cadence baseline");
            return;
        }
        let target = window_handle.update(cx, |view, window, cx| {
            let Some(refresh_interval) = window.display(cx).and_then(|display| display.refresh_interval()) else {
                eprintln!("FAIL: display mode refresh period is unavailable; cannot validate native refresh continuity");
                cx.quit();
                return;
            };
            view.baseline_presentation_rate = 1.0 / refresh_interval.as_secs_f64();
            view.sample_interval_p50_micros = refresh_interval.as_micros() as u64;
            eprintln!("display target_hz={:.3} target_interval_ms={:.3} measured_baseline_hz={:.3} measured_p50_ms={:.3}",
                view.baseline_presentation_rate, refresh_interval.as_secs_f64() * 1000.0,
                baseline_presentation_rate, sample_interval_p50_micros as f64 / 1000.0);
            view.block_next_render = true;
            cx.notify();
        });
        if target.is_err() {
            return;
        }
        if config.visual_only {
            return;
        }
        Timer::after(Duration::from_millis(1800)).await;
        if cx
            .update(|cx| {
                view.update(cx, |view, cx| {
                    view.axis_comparison_enabled = true;
                    cx.notify();
                })
            })
            .is_err()
        {
            return;
        }
        eprintln!("axis A/B phase started: compositor and layout X/Y translation only");
        Timer::after(Duration::from_secs(3)).await;
        if cx
            .update(|cx| {
                view.update(cx, |view, cx| {
                    view.axis_comparison_enabled = false;
                    view.spring_comparison_enabled = true;
                    cx.notify();
                })
            })
            .is_err()
        {
            return;
        }
        eprintln!("spring A/B phase started: same default spring on compositor and layout X/Y tracks");
        Timer::after(Duration::from_secs(3)).await;
        if cx
            .update(|cx| {
            view.update(cx, |view, cx| {
                view.layout_fallback_enabled = true;
                cx.notify();
            })
            })
            .is_err()
        {
            return;
        }
        eprintln!("layout/color fallback phase started after isolated axis A/B interval");
    })
    .detach();
}

fn animation_sample_count(window_id: u64) -> usize {
    window_metrics_snapshot()
        .into_iter()
        .find(|window| window.window_id == window_id)
        .map_or(0, |window| window.animation_sampled_present_count)
}

fn parse_config() -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        backend: default_backend(),
        copies: DEFAULT_COPIES,
        measurement: DEFAULT_MEASUREMENT,
        visual_only: false,
        client_titlebar: false,
    };
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--backend=") {
            config.backend = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--copies=") {
            config.copies = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--seconds=") {
            config.measurement = Duration::from_secs(value.parse()?);
        } else if argument == "--visual-only" {
            config.visual_only = true;
        } else if argument == "--client-titlebar" {
            config.client_titlebar = true;
        } else if argument == "--help" || argument == "-h" {
            print_help();
            std::process::exit(0);
        } else {
            return Err(format!("unknown argument: {argument}").into());
        }
    }
    if !matches!(
        config.backend,
        RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
    ) {
        return Err("--backend must be nova-dx12 or nova-vulkan".into());
    }
    if !(1..=MAX_COPIES).contains(&config.copies) {
        return Err(format!("--copies must be between 1 and {MAX_COPIES}").into());
    }
    if config.measurement < Duration::from_secs(4) {
        return Err("--seconds must be at least 4".into());
    }
    Ok(config)
}

fn print_help() {
    println!(
        "animation_perf_lab [--backend=nova-dx12|nova-vulkan] [--copies=1..{MAX_COPIES}] [--seconds=4+] [--visual-only] [--client-titlebar]\n\
         Default: {DEFAULT_COPIES} copies per visual track, {seconds}s measurement.\n\
         The output includes a 200 ms presentation-lane gate, window frame/present percentiles,\n\
         latest CPU-stage timings, uploads, passes and skipped frames.",
        seconds = DEFAULT_MEASUREMENT.as_secs(),
    );
}

fn default_backend() -> RendererBackend {
    if cfg!(target_os = "windows") {
        RendererBackend::NovaDx12
    } else {
        RendererBackend::NovaVulkan
    }
}

fn backend_label(backend: RendererBackend) -> &'static str {
    match backend {
        RendererBackend::NovaDx12 => "nova-dx12",
        RendererBackend::NovaVulkan => "nova-vulkan",
        _ => "unsupported",
    }
}

fn duration_us(duration: Option<Duration>) -> u128 {
    duration.map_or(0, |duration| duration.as_micros())
}

fn duration_ms(duration: Option<Duration>) -> f64 {
    duration.map_or(0.0, |duration| duration.as_secs_f64() * 1000.0)
}

fn micros_to_millis(micros: usize) -> f64 {
    micros as f64 / 1000.0
}
