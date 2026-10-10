use std::{
    borrow::Cow,
    collections::BTreeMap,
    error::Error,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, Application, AssetSource, Bounds, Context, Entity, PerformanceMetricsSnapshot,
    PresentModePreference, RenderPolicy, RendererBackend, RendererOptions, SharedString,
    StyleRefinement, Window, WindowBounds, WindowOptions, div, hsla, img,
    performance_metrics_snapshot, prelude::*, px, rgb, size,
};
use serde::Serialize;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const DEFAULT_FRAMES: usize = 600;
mod gpui_perf_lab_buffer_growth;
const DEFAULT_REFRESH_RATE: f32 = 120.0;
const DEFAULT_WARMUP_FRAMES: usize = 120;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Scenario {
    StaticIdle,
    SingleDirty,
    Scroll10k,
    CjkCold,
    CjkHot,
    TextureStress,
    OverdrawModal,
    Effects,
    Animation,
    TraversalAncestor,
    BufferGrowth,
}

impl Scenario {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "static-idle" => Ok(Self::StaticIdle),
            "single-dirty" => Ok(Self::SingleDirty),
            "scroll-10k" => Ok(Self::Scroll10k),
            "cjk-cold" => Ok(Self::CjkCold),
            "cjk-hot" => Ok(Self::CjkHot),
            "texture-stress" => Ok(Self::TextureStress),
            "overdraw-modal" => Ok(Self::OverdrawModal),
            "effects" => Ok(Self::Effects),
            "animation" => Ok(Self::Animation),
            "traversal-ancestor" => Ok(Self::TraversalAncestor),
            "buffer-growth" => Ok(Self::BufferGrowth),
            _ => Err(format!("unknown scenario: {value}").into()),
        }
    }

    fn warmup_frames(self) -> usize {
        if matches!(self, Self::CjkCold) {
            0
        } else {
            DEFAULT_WARMUP_FRAMES
        }
    }
}

#[derive(Clone)]
struct Config {
    backend: RendererBackend,
    host: Host,
    present_mode: PresentModePreference,
    scenario: Scenario,
    refresh_rate: f32,
    frames: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Host {
    Single,
    Separate,
}

#[derive(Serialize)]
struct FrameSample {
    build_us: u64,
    layout_us: u64,
    prepaint_us: u64,
    paint_us: u64,
    scene_finish_us: u64,
    backend_draw_us: u64,
    pack_us: u64,
    encode_us: u64,
    signature_us: u64,
    buffer_upload_us: u64,
    buffer_upload_batches: usize,
    buffer_upload_requested_writes: usize,
    buffer_upload_requested_bytes: usize,
    buffer_upload_writes: usize,
    buffer_upload_bytes: usize,
    buffer_upload_backend_calls: usize,
    buffer_upload_backend_bytes: usize,
    frame_slot_wait_us: u64,
    gpu_submission_wait_us: u64,
    hashed_bytes: usize,
    uploaded_bytes: usize,
    retained_chunk_hits: usize,
    retained_chunk_misses: usize,
    retained_chunk_reused_bytes: usize,
    retained_key_hit: bool,
    static_stream_hits: usize,
    static_stream_misses: usize,
}

impl From<&PerformanceMetricsSnapshot> for FrameSample {
    fn from(snapshot: &PerformanceMetricsSnapshot) -> Self {
        Self {
            build_us: duration_us(snapshot.frame_build_time),
            layout_us: duration_us(snapshot.frame_layout_time),
            prepaint_us: duration_us(snapshot.frame_prepaint_time),
            paint_us: duration_us(snapshot.frame_paint_time),
            scene_finish_us: duration_us(snapshot.frame_scene_finish_time),
            backend_draw_us: duration_us(snapshot.frame_backend_draw_time),
            pack_us: duration_us(snapshot.scene_pack_time),
            encode_us: duration_us(snapshot.scene_encode_time),
            signature_us: duration_us(snapshot.scene_signature_time),
            buffer_upload_us: duration_us(snapshot.buffer_upload_time),
            buffer_upload_batches: snapshot.buffer_upload_batches,
            buffer_upload_requested_writes: snapshot.buffer_upload_requested_writes,
            buffer_upload_requested_bytes: snapshot.buffer_upload_requested_bytes,
            buffer_upload_writes: snapshot.buffer_upload_writes,
            buffer_upload_bytes: snapshot.buffer_upload_bytes,
            buffer_upload_backend_calls: snapshot.buffer_upload_backend_calls,
            buffer_upload_backend_bytes: snapshot.buffer_upload_backend_bytes,
            frame_slot_wait_us: duration_us(snapshot.frame_slot_wait_time),
            gpu_submission_wait_us: duration_us(snapshot.gpu_submission_wait_time),
            hashed_bytes: snapshot.scene_hashed_bytes,
            uploaded_bytes: snapshot.upload_bytes,
            retained_chunk_hits: snapshot.retained_chunk_hits,
            retained_chunk_misses: snapshot.retained_chunk_misses,
            retained_chunk_reused_bytes: snapshot.retained_chunk_reused_bytes,
            retained_key_hit: snapshot.retained_upload_key_hit,
            static_stream_hits: snapshot.static_stream_hits,
            static_stream_misses: snapshot.static_stream_misses,
        }
    }
}

#[derive(Serialize)]
struct LabReport {
    window_metrics: Vec<gpui::WindowMetricsSnapshot>,
    text_metrics: Vec<gpui::TextOperationMetricsSnapshot>,
    gpu_owner: gpui::GpuOwnerMetricsSnapshot,
    gpu_owner_samples: Vec<gpui::GpuOwnerJobSample>,
    memory: gpui::GpuiMemorySnapshot,
    process_memory: Option<gpui::ProcessMemorySnapshot>,
    process_memory_error: Option<String>,
    schema_version: u32,
    backend: &'static str,
    host: Host,
    requested_present_mode: PresentModePreference,
    scenario: Scenario,
    refresh_rate: f32,
    warmup_frames: usize,
    sample_frames: usize,
    width: u32,
    height: u32,
    adapter_name: String,
    adapter_type: String,
    surface_format: String,
    surface_alpha_mode: String,
    surface_present_mode: String,
    traversal_render_counts: Option<TraversalRenderCounts>,
    summary: BTreeMap<String, PercentileSummary>,
    samples: Vec<FrameSample>,
}

#[derive(Clone, Copy, Serialize)]
struct TraversalRenderCounts {
    root: usize,
    parent: usize,
    leaf: usize,
}

#[derive(Serialize)]
struct PercentileSummary {
    p50: u64,
    p95: u64,
    p99: u64,
    max: u64,
}

struct Assets {
    examples: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let path = if path.starts_with("perf-texture-") {
            self.examples.join("image/app-icon.png")
        } else {
            self.examples.join(path)
        };
        Ok(Some(Cow::Owned(fs::read(path)?)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

struct PerfLab {
    config: Config,
    animation_started_at: Instant,
    rendered_frames: usize,
    traversal_parent: Entity<TraversalParentView>,
    traversal_leaf: Entity<TraversalLeafView>,
    traversal: Rc<RefCell<TraversalRun>>,
    samples: Vec<FrameSample>,
    finished: bool,
}

impl Render for PerfLab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(self.config.scenario, Scenario::TraversalAncestor) {
            if !self.finished {
                let should_schedule = {
                    let mut traversal = self.traversal.borrow_mut();
                    traversal
                        .root_renders
                        .set(traversal.root_renders.get().saturating_add(1));
                    if traversal.scheduled {
                        false
                    } else {
                        traversal.scheduled = true;
                        true
                    }
                };
                if should_schedule {
                    schedule_traversal_frame(
                        self.traversal.clone(),
                        self.config.clone(),
                        self.traversal_leaf.clone(),
                        window,
                    );
                }
            }
        } else {
            if !self.finished {
                window.request_animation_frame();
            }
            if self.rendered_frames > self.config.scenario.warmup_frames()
                && self.samples.len() < self.config.frames
            {
                self.samples
                    .push(FrameSample::from(&performance_metrics_snapshot()));
            }
            if self.samples.len() == self.config.frames && !self.finished {
                self.finish(cx);
            }
        }
        self.rendered_frames = self.rendered_frames.saturating_add(1);
        self.workload(window)
    }
}

impl PerfLab {
    fn workload(&self, window: &Window) -> gpui::AnyElement {
        match self.config.scenario {
            Scenario::StaticIdle => rows(128, 0),
            Scenario::SingleDirty => single_dirty(self.rendered_frames),
            Scenario::Scroll10k => rows(10_000, self.rendered_frames % 100),
            Scenario::CjkCold | Scenario::CjkHot => cjk_rows(),
            Scenario::TextureStress => texture_grid(),
            Scenario::OverdrawModal => overdraw_modal(),
            Scenario::Effects => effects(),
            Scenario::BufferGrowth => gpui_perf_lab_buffer_growth::workload(self.rendered_frames),
            Scenario::Animation => {
                let elapsed = window
                    .animation_time()
                    .saturating_duration_since(self.animation_started_at)
                    .as_secs_f32();
                animation(elapsed)
            }
            Scenario::TraversalAncestor => {
                let mut content = div().size_full();
                for index in 0..256 {
                    content = content.child(div().h(px(1.0)).bg(rgb(if index % 2 == 0 {
                        0x172033
                    } else {
                        0x1e293b
                    })));
                }
                content
                    .child(
                        self.traversal_parent
                            .clone()
                            .cached(StyleRefinement::default().w(px(180.0)).h(px(40.0))),
                    )
                    .into_any_element()
            }
        }
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.finished = true;
        print_report(
            &self.config,
            std::mem::take(&mut self.samples),
            None,
            cx.gpui_memory_snapshot(),
        );
        cx.spawn(async move |_view, cx| {
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

#[derive(Default)]
struct TraversalRun {
    frames: usize,
    samples: Vec<FrameSample>,
    root_renders: Rc<Cell<usize>>,
    parent_renders: Rc<Cell<usize>>,
    leaf_renders: Rc<Cell<usize>>,
    initial_render_counts: Option<TraversalRenderCounts>,
    scheduled: bool,
}

fn schedule_traversal_frame(
    traversal: Rc<RefCell<TraversalRun>>,
    config: Config,
    leaf: Entity<TraversalLeafView>,
    window: &Window,
) {
    window.on_next_frame(move |window, cx| {
        let report = {
            let mut traversal = traversal.borrow_mut();
            let sampled_frame = traversal.frames;
            traversal.frames = traversal.frames.saturating_add(1);
            let current_render_counts = TraversalRenderCounts {
                root: traversal.root_renders.get(),
                parent: traversal.parent_renders.get(),
                leaf: traversal.leaf_renders.get(),
            };
            let initial_render_counts = *traversal
                .initial_render_counts
                .get_or_insert(current_render_counts);
            if sampled_frame > config.scenario.warmup_frames()
                && traversal.samples.len() < config.frames
            {
                traversal
                    .samples
                    .push(FrameSample::from(&performance_metrics_snapshot()));
            }
            if traversal.samples.len() == config.frames {
                Some((
                    std::mem::take(&mut traversal.samples),
                    TraversalRenderCounts {
                        root: current_render_counts
                            .root
                            .saturating_sub(initial_render_counts.root),
                        parent: current_render_counts
                            .parent
                            .saturating_sub(initial_render_counts.parent),
                        leaf: current_render_counts
                            .leaf
                            .saturating_sub(initial_render_counts.leaf),
                    },
                ))
            } else {
                None
            }
        };
        if let Some((samples, render_counts)) = report {
            print_report(
                &config,
                samples,
                Some(render_counts),
                cx.gpui_memory_snapshot(),
            );
            cx.quit();
            return;
        }

        leaf.update(cx, |leaf, cx| {
            leaf.revision = leaf.revision.saturating_add(1);
            cx.notify();
        });
        schedule_traversal_frame(traversal, config, leaf, window);
    });
}

struct TraversalParentView {
    leaf: Entity<TraversalLeafView>,
    renders: Rc<Cell<usize>>,
}

impl Render for TraversalParentView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        div().size_full().bg(rgb(0x0f172a)).child(
            self.leaf
                .clone()
                .cached(StyleRefinement::default().w(px(180.0)).h(px(40.0))),
        )
    }
}

struct TraversalLeafView {
    revision: usize,
    renders: Rc<Cell<usize>>,
}

impl Render for TraversalLeafView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get().saturating_add(1));
        div()
            .size_full()
            .bg(rgb(if self.revision % 2 == 0 {
                0x2563eb
            } else {
                0x0891b2
            }))
            .child(format!("Target revision {}", self.revision))
    }
}

fn rows(count: usize, offset: usize) -> gpui::AnyElement {
    div()
        .size_full()
        .overflow_hidden()
        .bg(rgb(0x0f172a))
        .child(
            div()
                .relative()
                .top(px(-(offset as f32 * 20.0)))
                .children((0..count).map(|index| {
                    div()
                        .h_5()
                        .bg(rgb(if index % 2 == 0 { 0x172033 } else { 0x1e293b }))
                        .child(format!("Row {index}"))
                })),
        )
        .into_any_element()
}

fn single_dirty(frame: usize) -> gpui::AnyElement {
    let color = if frame % 2 == 0 { 0x2563eb } else { 0x0891b2 };
    div()
        .size_full()
        .bg(rgb(0x0f172a))
        .p_4()
        .child(div().size_12().rounded_md().bg(rgb(color)))
        .children((0..128).map(|index| {
            div()
                .h_5()
                .mt_1()
                .bg(rgb(0x1e293b))
                .child(format!("Retained sibling {index}"))
        }))
        .into_any_element()
}

fn cjk_rows() -> gpui::AnyElement {
    div()
        .size_full()
        .overflow_hidden()
        .bg(rgb(0x111827))
        .p_4()
        .children((0..320).map(|index| {
            div().h_6().child(format!(
                "冷文本：启动器、世界、资源包与字体回退 {index} — GPUI 🚀"
            ))
        }))
        .into_any_element()
}

fn texture_grid() -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .flex_wrap()
        .content_start()
        .bg(rgb(0x111827))
        .children((0..512).map(|index| {
            img(format!("perf-texture-{index}.png"))
                .id(SharedString::from(format!("texture-{index}")))
                .size_8()
        }))
        .into_any_element()
}

fn overdraw_modal() -> gpui::AnyElement {
    div()
        .relative()
        .size_full()
        .bg(rgb(0x0f172a))
        .children((0..96).map(|index| {
            let inset = (index % 24) as f32 * 3.0;
            div()
                .absolute()
                .top(px(inset))
                .left(px(inset))
                .w(px(720.0 - inset))
                .h(px(480.0 - inset))
                .bg(hsla(0.58, 0.55, 0.35, 0.12))
        }))
        .child(
            div()
                .absolute()
                .top(px(100.0))
                .left(px(160.0))
                .w(px(420.0))
                .h(px(260.0))
                .rounded_lg()
                .bg(rgb(0x1f2937))
                .child("Opaque modal"),
        )
        .into_any_element()
}

fn effects() -> gpui::AnyElement {
    div()
        .relative()
        .size_full()
        .bg(rgb(0x155e75))
        .children((0..32).map(|index| {
            div()
                .absolute()
                .top(px((index % 8) as f32 * 64.0))
                .left(px((index / 8) as f32 * 180.0))
                .size_20()
                .rounded_lg()
                .bg(rgb(0x0e7490))
        }))
        .child(
            div()
                .absolute()
                .top(px(72.0))
                .left(px(96.0))
                .w(px(560.0))
                .h(px(340.0))
                .rounded_lg()
                .background_blur(px(24.0))
                .bg(hsla(0.0, 0.0, 0.12, 0.35))
                .child("Backdrop blur / damage workload"),
        )
        .into_any_element()
}

fn animation(elapsed: f32) -> gpui::AnyElement {
    let x = 40.0 + (elapsed * 160.0) % 640.0;
    div()
        .relative()
        .size_full()
        .bg(rgb(0x111827))
        .child(
            div()
                .absolute()
                .top(px(180.0))
                .left(px(x))
                .size_16()
                .rounded_full()
                .bg(rgb(0x22c55e)),
        )
        .into_any_element()
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_config()?;
    let options = RendererOptions {
        backend: config.backend,
        present_mode: config.present_mode,
        render_policy: RenderPolicy::Continuous {
            max_fps: config.refresh_rate,
        },
        frame_metrics: true,
        ..RendererOptions::default()
    };
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let host = config.host;
    let launch = move |cx: &mut App| open_lab_window(config, cx);
    match host {
        Host::Single => Application::with_renderer_options(options)
            .with_assets(Assets { examples })
            .run(launch),
        Host::Separate => Application::run_separate(
            options.clone(),
            move || Application::with_renderer_options(options).with_assets(Assets { examples }),
            launch,
        )?,
    }
    Ok(())
}

fn open_lab_window(config: Config, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(800.0), px(520.0)), cx);
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        },
        |_, cx| {
            cx.new(|cx| {
                let traversal = Rc::new(RefCell::new(TraversalRun::default()));
                traversal.borrow_mut().samples = Vec::with_capacity(config.frames);
                let traversal_parent_renders = traversal.borrow().parent_renders.clone();
                let traversal_leaf_renders = traversal.borrow().leaf_renders.clone();
                let traversal_leaf = cx.new(|_| TraversalLeafView {
                    revision: 0,
                    renders: traversal_leaf_renders.clone(),
                });
                let traversal_parent = cx.new(|_| TraversalParentView {
                    leaf: traversal_leaf.clone(),
                    renders: traversal_parent_renders.clone(),
                });
                PerfLab {
                    config: config.clone(),
                    animation_started_at: Instant::now(),
                    rendered_frames: 0,
                    traversal_parent,
                    traversal_leaf,
                    traversal,
                    samples: Vec::with_capacity(config.frames),
                    finished: false,
                }
            })
        },
    );
    if let Err(error) = result {
        eprintln!("failed to open gpui_perf_lab: {error:#}");
        cx.quit();
        return;
    }
    cx.activate(true);
}

fn parse_config() -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        backend: default_backend(),
        host: Host::Single,
        present_mode: PresentModePreference::AutoVsync,
        scenario: Scenario::StaticIdle,
        refresh_rate: DEFAULT_REFRESH_RATE,
        frames: DEFAULT_FRAMES,
    };

    for argument in std::env::args().skip(1) {
        parse_argument(&mut config, &argument)?;
    }

    if !matches!(
        config.backend,
        RendererBackend::Auto
            | RendererBackend::NovaDx11
            | RendererBackend::NovaDx12
            | RendererBackend::NovaVulkan
            | RendererBackend::NovaOpenGl
    ) {
        return Err(
            "--backend must be auto, nova-dx11, nova-dx12, nova-vulkan, or nova-opengl".into(),
        );
    }
    if !config.refresh_rate.is_finite() || config.refresh_rate <= 0.0 {
        return Err("--refresh-rate must be a positive finite number".into());
    }
    if config.frames == 0 {
        return Err("--frames must be greater than zero".into());
    }
    Ok(config)
}

fn parse_argument(config: &mut Config, argument: &str) -> Result<(), Box<dyn Error>> {
    if let Some(value) = argument.strip_prefix("--backend=") {
        config.backend = value.parse()?;
    } else if let Some(value) = argument.strip_prefix("--host=") {
        config.host = match value {
            "single" => Host::Single,
            "separate" => Host::Separate,
            _ => return Err("--host must be single or separate".into()),
        };
    } else if let Some(value) = argument.strip_prefix("--present-mode=") {
        config.present_mode = match value {
            "auto-vsync" => PresentModePreference::AutoVsync,
            "mailbox" => PresentModePreference::Mailbox,
            "immediate" => PresentModePreference::Immediate,
            _ => return Err("--present-mode must be auto-vsync, mailbox, or immediate".into()),
        };
    } else if let Some(value) = argument.strip_prefix("--scenario=") {
        config.scenario = Scenario::parse(value)?;
    } else if let Some(value) = argument.strip_prefix("--refresh-rate=") {
        config.refresh_rate = value.parse()?;
    } else if let Some(value) = argument.strip_prefix("--frames=") {
        config.frames = value.parse()?;
    } else if argument == "--help" || argument == "-h" {
        print_help();
        std::process::exit(0);
    } else {
        return Err(format!("unknown argument: {argument}").into());
    }
    Ok(())
}

fn default_backend() -> RendererBackend {
    if cfg!(target_os = "windows") {
        RendererBackend::NovaDx12
    } else {
        RendererBackend::NovaVulkan
    }
}

fn duration_us(duration: Option<Duration>) -> u64 {
    duration.map_or(0, |duration| {
        duration.as_micros().min(u128::from(u64::MAX)) as u64
    })
}

fn print_report(
    config: &Config,
    samples: Vec<FrameSample>,
    traversal_render_counts: Option<TraversalRenderCounts>,
    memory: gpui::GpuiMemorySnapshot,
) {
    let snapshot = performance_metrics_snapshot();
    let gpu_owner = gpui::gpu_owner_metrics_snapshot();
    let gpu_owner_samples = gpu_owner
        .windows
        .iter()
        .flat_map(|window| {
            gpui::gpu_owner_samples_since(window.owner_id, 0)
                .into_iter()
                .filter(|sample| sample.job_id <= window.completed_jobs)
        })
        .collect();
    let summary = summarize(&samples);
    let (process_memory, process_memory_error) = match gpui::process_memory_snapshot() {
        Ok(sample) => (sample, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let report = LabReport {
        window_metrics: gpui::window_metrics_snapshot(),
        text_metrics: gpui::text_metrics_snapshot(),
        schema_version: 5,
        gpu_owner,
        gpu_owner_samples,
        memory,
        process_memory,
        process_memory_error,
        backend: backend_label(snapshot.renderer_backend),
        host: config.host,
        requested_present_mode: config.present_mode,
        scenario: config.scenario,
        refresh_rate: config.refresh_rate,
        warmup_frames: config.scenario.warmup_frames(),
        sample_frames: samples.len(),
        width: 800,
        height: 520,
        adapter_name: snapshot.gpu_adapter_name,
        adapter_type: snapshot.gpu_adapter_type,
        surface_format: snapshot.gpu_surface_format,
        surface_alpha_mode: snapshot.gpu_surface_alpha_mode,
        surface_present_mode: snapshot.gpu_surface_present_mode,
        traversal_render_counts,
        summary,
        samples,
    };
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(error) => eprintln!("failed to serialize gpui_perf_lab report: {error}"),
    }
}

fn backend_label(backend: RendererBackend) -> &'static str {
    match backend {
        RendererBackend::Auto => "auto",
        RendererBackend::NovaDx11 => "nova-dx11",
        RendererBackend::NovaDx12 => "nova-dx12",
        RendererBackend::NovaVulkan => "nova-vulkan",
        RendererBackend::NovaOpenGl => "nova-opengl",
        _ => "unsupported",
    }
}

fn summarize(samples: &[FrameSample]) -> BTreeMap<String, PercentileSummary> {
    let mut summary = BTreeMap::new();
    macro_rules! insert {
        ($name:literal, $field:ident) => {
            summary.insert(
                $name.to_string(),
                distribution(samples.iter().map(|sample| sample.$field as u64)),
            );
        };
    }
    insert!("backend_draw_us", backend_draw_us);
    insert!("buffer_upload_us", buffer_upload_us);
    insert!("buffer_upload_batches", buffer_upload_batches);
    insert!(
        "buffer_upload_requested_writes",
        buffer_upload_requested_writes
    );
    insert!(
        "buffer_upload_requested_bytes",
        buffer_upload_requested_bytes
    );
    insert!("buffer_upload_writes", buffer_upload_writes);
    insert!("buffer_upload_bytes", buffer_upload_bytes);
    insert!("buffer_upload_backend_calls", buffer_upload_backend_calls);
    insert!("buffer_upload_backend_bytes", buffer_upload_backend_bytes);
    insert!("build_us", build_us);
    insert!("encode_us", encode_us);
    insert!("frame_slot_wait_us", frame_slot_wait_us);
    insert!("gpu_submission_wait_us", gpu_submission_wait_us);
    insert!("hashed_bytes", hashed_bytes);
    insert!("layout_us", layout_us);
    insert!("pack_us", pack_us);
    insert!("paint_us", paint_us);
    insert!("prepaint_us", prepaint_us);
    insert!("scene_finish_us", scene_finish_us);
    insert!("signature_us", signature_us);
    insert!("uploaded_bytes", uploaded_bytes);
    summary
}

fn distribution(values: impl Iterator<Item = u64>) -> PercentileSummary {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    PercentileSummary {
        p50: percentile(&values, 50),
        p95: percentile(&values, 95),
        p99: percentile(&values, 99),
        max: values.last().copied().unwrap_or_default(),
    }
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let rank = values
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1);
    values[rank.min(values.len() - 1)]
}

fn print_help() {
    println!(
        "gpui_perf_lab --backend=auto|nova-dx11|nova-dx12|nova-vulkan|nova-opengl \
         --host=single|separate --present-mode=auto-vsync|mailbox|immediate \
         --scenario=static-idle|single-dirty|scroll-10k|cjk-cold|cjk-hot|texture-stress|overdraw-modal|effects|animation|traversal-ancestor|buffer-growth \
         --refresh-rate=120 --frames=600"
    );
}
