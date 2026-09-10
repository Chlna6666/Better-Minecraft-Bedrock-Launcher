use std::{
    borrow::Cow,
    collections::BTreeMap,
    error::Error,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, Application, AssetSource, Bounds, Context, PerformanceMetricsSnapshot, RenderPolicy,
    RendererBackend, RendererOptions, SharedString, Window, WindowBounds, WindowOptions, div, hsla,
    img, performance_metrics_snapshot, prelude::*, px, rgb, size,
};
use serde::Serialize;

const DEFAULT_FRAMES: usize = 600;
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
    scenario: Scenario,
    refresh_rate: f32,
    frames: usize,
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
    schema_version: u32,
    backend: &'static str,
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
    summary: BTreeMap<String, PercentileSummary>,
    samples: Vec<FrameSample>,
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
    samples: Vec<FrameSample>,
    finished: bool,
}

impl Render for PerfLab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            self.finished = true;
            print_report(&self.config, std::mem::take(&mut self.samples));
            cx.spawn(async move |_view, cx| {
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
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
            Scenario::Animation => {
                let elapsed = window
                    .animation_time()
                    .saturating_duration_since(self.animation_started_at)
                    .as_secs_f32();
                animation(elapsed)
            }
        }
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
                .backdrop_blur(px(24.0))
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
        render_policy: RenderPolicy::Continuous {
            max_fps: config.refresh_rate,
        },
        frame_metrics: true,
        ..RendererOptions::default()
    };
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");

    Application::with_renderer_options(options)
        .with_assets(Assets { examples })
        .run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(800.0), px(520.0)), cx);
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..WindowOptions::default()
                },
                |_, cx| {
                    cx.new(|_| PerfLab {
                        config: config.clone(),
                        animation_started_at: Instant::now(),
                        rendered_frames: 0,
                        samples: Vec::with_capacity(config.frames),
                        finished: false,
                    })
                },
            );
            if let Err(error) = result {
                eprintln!("failed to open gpui_perf_lab: {error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
    Ok(())
}

fn parse_config() -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        backend: default_backend(),
        scenario: Scenario::StaticIdle,
        refresh_rate: DEFAULT_REFRESH_RATE,
        frames: DEFAULT_FRAMES,
    };

    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--backend=") {
            config.backend = value.parse()?;
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
    }

    if !matches!(
        config.backend,
        RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
    ) {
        return Err("--backend must be nova-dx12 or nova-vulkan".into());
    }
    if !config.refresh_rate.is_finite() || config.refresh_rate <= 0.0 {
        return Err("--refresh-rate must be a positive finite number".into());
    }
    if config.frames == 0 {
        return Err("--frames must be greater than zero".into());
    }
    Ok(config)
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

fn print_report(config: &Config, samples: Vec<FrameSample>) {
    let snapshot = performance_metrics_snapshot();
    let summary = summarize(&samples);
    let report = LabReport {
        schema_version: 1,
        backend: backend_label(config.backend),
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
        RendererBackend::NovaDx12 => "nova-dx12",
        RendererBackend::NovaVulkan => "nova-vulkan",
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
        "gpui_perf_lab --backend=nova-dx12|nova-vulkan \
         --scenario=static-idle|single-dirty|scroll-10k|cjk-cold|cjk-hot|texture-stress|overdraw-modal|effects|animation \
         --refresh-rate=120 --frames=600"
    );
}
