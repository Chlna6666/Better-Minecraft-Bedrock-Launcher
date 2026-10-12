//! Compare ordinary and cached nested views with a leaf-only content update.
//! The frame callback changes only the leaf; it never notifies the root/parent.
//! GPU metrics are latest asynchronous observations, not GPU duration samples.

use std::{cell::Cell, error::Error, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, Entity, PerformanceMetricsSnapshot, Render, RendererBackend,
    RendererFallback, RendererOptions, StyleRefinement, Window, WindowBounds, WindowOptions, div,
    hsla, performance_metrics_snapshot, prelude::*, px, rgb, size,
};
use serde::Serialize;

const WARMUP: usize = 120;

struct FrameLog;

impl log::Log for FrameLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("gpui")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            if message.contains("complete_frame")
                || message.contains("gpui frame request:")
                || message.contains("gpui selective splice blocked:")
                || message.contains("gpui draw degraded:")
                || record.level() <= log::Level::Warn
            {
                eprintln!("{message}");
            }
        }
    }

    fn flush(&self) {}
}

#[derive(Clone, Copy, Debug, Serialize)]
enum Cache {
    Plain,
    Cached,
}

#[derive(Clone, Copy, Debug, Serialize)]
enum Blur {
    None,
    Behind,
    Ahead,
    ElementSmall,
    ElementFull,
}

#[derive(Clone, Debug, Serialize)]
struct Config {
    backend: RendererBackend,
    cache: Cache,
    blur: Blur,
    frames: usize,
    trace: bool,
}

#[derive(Default, Serialize)]
struct Counts {
    root: Cell<usize>,
    parent: Cell<usize>,
    leaf: Cell<usize>,
}

#[derive(Serialize)]
struct Sample {
    build_us: u128,
    layout_nodes: usize,
    dirty_pixels: usize,
    commands: usize,
    upload_bytes: usize,
    blur_source_pixels: usize,
    blur_filter_pixels: usize,
    main_passes: usize,
}

impl From<PerformanceMetricsSnapshot> for Sample {
    fn from(metrics: PerformanceMetricsSnapshot) -> Self {
        Self {
            build_us: metrics.frame_build_time.map_or(0, |time| time.as_micros()),
            layout_nodes: metrics.layout_nodes,
            dirty_pixels: metrics.dirty_rect_area,
            commands: metrics.prepared_command_count,
            upload_bytes: metrics.upload_bytes,
            blur_source_pixels: metrics.backdrop_blur_source_pixels,
            blur_filter_pixels: metrics.backdrop_blur_target_pixels,
            main_passes: metrics.main_pass_count,
        }
    }
}

struct Page {
    parent: Entity<Parent>,
    leaf: Entity<Leaf>,
    config: Config,
    counts: Rc<Counts>,
    scheduled: bool,
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.counts.root.set(self.counts.root.get() + 1);
        if !self.scheduled {
            self.scheduled = true;
            Run {
                config: self.config.clone(),
                counts: self.counts.clone(),
                leaf: self.leaf.clone(),
                samples: Vec::new(),
                frame: 0,
            }
            .schedule(window);
        }
        let mut page = div()
            .id("page")
            .relative()
            .size_full()
            .bg(rgb(0x0f172a))
            .children((0..384).map(|index| {
                div()
                    .id(("tile", index as usize))
                    .absolute()
                    .left(px((index % 24) as f32 * 32.0))
                    .top(px((index / 24) as f32 * 28.0))
                    .w(px(30.0))
                    .h(px(26.0))
                    .bg(rgb(if index % 2 == 0 { 0x334155 } else { 0x1e293b }))
            }));
        if matches!(self.config.blur, Blur::Behind) {
            page = page.child(glass());
        }
        let parent = match self.config.cache {
            Cache::Plain => self.parent.clone().into_any_element(),
            Cache::Cached => self
                .parent
                .clone()
                .cached(parent_style())
                .into_any_element(),
        };
        page = page.child(parent);
        if matches!(self.config.blur, Blur::Ahead) {
            page = page.child(glass());
        }
        if matches!(self.config.blur, Blur::ElementFull) {
            page = page.filter_blur(px(16.0));
        }
        page
    }
}

fn parent_style() -> StyleRefinement {
    StyleRefinement::default()
        .absolute()
        .left(px(96.0))
        .top(px(100.0))
        .w(px(240.0))
        .h(px(160.0))
}

fn glass() -> impl IntoElement {
    div()
        .id("glass")
        .absolute()
        .inset_0()
        .background_blur(px(16.0))
        .bg(hsla(0.6, 0.2, 0.4, 0.12))
}

struct Parent {
    leaf: Entity<Leaf>,
    cache: Cache,
    counts: Rc<Counts>,
}

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.counts.parent.set(self.counts.parent.get() + 1);
        let leaf = match self.cache {
            Cache::Plain => self.leaf.clone().into_any_element(),
            Cache::Cached => self
                .leaf
                .clone()
                .cached(StyleRefinement::default().w(px(80.0)).h(px(40.0)))
                .into_any_element(),
        };
        let mut parent = div().id("parent");
        if matches!(self.cache, Cache::Plain) {
            parent = parent
                .absolute()
                .left(px(96.0))
                .top(px(100.0))
                .w(px(240.0))
                .h(px(160.0));
        } else {
            parent = parent.size_full();
        }
        parent.child(
            div()
                .id("nested")
                .p_4()
                .child(div().id("inner").child(leaf)),
        )
    }
}

struct Leaf {
    revision: usize,
    counts: Rc<Counts>,
    blur: bool,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.counts.leaf.set(self.counts.leaf.get() + 1);
        let leaf = div()
            .id("leaf")
            .w(px(80.0))
            .h(px(40.0))
            .bg(rgb(match self.revision % 3 {
                0 => 0x2563eb,
                1 => 0xf97316,
                _ => 0x10b981,
            }))
            .child("Leaf");
        if self.blur {
            leaf.filter_blur(px(16.0))
        } else {
            leaf
        }
    }
}

struct Run {
    config: Config,
    counts: Rc<Counts>,
    leaf: Entity<Leaf>,
    samples: Vec<Sample>,
    frame: usize,
}

impl Run {
    fn schedule(mut self, window: &Window) {
        window.on_next_frame(move |window, cx| {
            if self.frame == WARMUP {
                self.counts.root.set(0);
                self.counts.parent.set(0);
                self.counts.leaf.set(0);
            } else if self.frame > WARMUP {
                self.samples.push(performance_metrics_snapshot().into());
            }
            if self.samples.len() == self.config.frames {
                report(&self.config, &self.counts, &self.samples, cx);
                cx.quit();
                return;
            }
            self.leaf.update(cx, |leaf, cx| {
                leaf.revision += 1;
                cx.notify();
            });
            self.frame += 1;
            self.schedule(window);
        });
    }
}

fn report(config: &Config, counts: &Counts, samples: &[Sample], cx: &App) {
    let metrics = performance_metrics_snapshot();
    let report = serde_json::json!({
        "config": config,
        "resolved_backend": metrics.renderer_backend,
        "adapter": metrics.gpu_adapter_name,
        "partial_redraws": metrics.partial_redraw_count,
        "full_redraw_fallbacks": metrics.full_redraw_fallback_count,
        "counts": counts,
        "samples": samples,
        "window_metrics": gpui::window_metrics_snapshot(),
        "gpu_owner": gpui::gpu_owner_metrics_snapshot(),
        "memory": cx.gpui_memory_snapshot(),
        "renderer_memory": gpui::memory_profile_snapshot().renderers,
        "metrics_note": "CPU and GPU latest samples are asynchronous; command/pixel counts are not GPU timings",
    });
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(error) => eprintln!("failed to serialize nested_dirty report: {error}"),
    }
}

fn config() -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        backend: RendererBackend::NovaDx12,
        cache: Cache::Plain,
        blur: Blur::None,
        frames: 120,
        trace: false,
    };
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--backend=") {
            config.backend = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--cache=") {
            config.cache = match value {
                "plain" => Cache::Plain,
                "cached" => Cache::Cached,
                _ => return Err("--cache=plain|cached".into()),
            };
        } else if let Some(value) = argument.strip_prefix("--blur=") {
            config.blur = match value {
                "none" => Blur::None,
                "behind" => Blur::Behind,
                "ahead" => Blur::Ahead,
                "element-small" => Blur::ElementSmall,
                "element-full" => Blur::ElementFull,
                _ => return Err("--blur=none|behind|ahead|element-small|element-full".into()),
            };
        } else if let Some(value) = argument.strip_prefix("--frames=") {
            config.frames = value.parse()?;
        } else if argument == "--trace" {
            config.trace = true;
        } else {
            return Err(format!("unknown argument: {argument}").into());
        }
    }
    if config.frames == 0 {
        return Err("--frames must be positive".into());
    }
    Ok(config)
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = config()?;
    log::set_logger(&FrameLog)?;
    log::set_max_level(if config.trace {
        log::LevelFilter::Trace
    } else {
        log::LevelFilter::Warn
    });
    let options = RendererOptions {
        backend: config.backend,
        fallback: RendererFallback::Disabled,
        frame_metrics: true,
        ..RendererOptions::default()
    };
    Application::with_renderer_options(options).run(move |cx| {
            let counts = Rc::new(Counts::default());
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(800.0), px(520.0)),
                        cx,
                    ))),
                    ..WindowOptions::default()
                },
                |_, cx| {
                    let leaf = cx.new(|_| Leaf {
                        revision: 0,
                        counts: counts.clone(),
                        blur: matches!(config.blur, Blur::ElementSmall),
                    });
                    let parent = cx.new(|_| Parent {
                        leaf: leaf.clone(),
                        cache: config.cache,
                        counts: counts.clone(),
                    });
                    cx.new(|_| Page {
                        parent,
                        leaf,
                        config,
                        counts,
                        scheduled: false,
                    })
                },
            );
            if let Err(error) = result {
                eprintln!("failed to open nested_dirty: {error:#}");
                cx.quit();
            } else {
                cx.activate(true);
            }
        });
    Ok(())
}
