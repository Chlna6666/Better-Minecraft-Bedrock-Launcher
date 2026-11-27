//! GPUI sidecar rendering path.
//!
//! Opens a native GPUI window, presents the shared world through a `SceneView` element, and installs
//! a camera sampler so WASD / mouse-drag / wheel input moves the camera without rebuilding the UI
//! element tree. While the window runs, GPUI performance metrics (present FPS, native presents,
//! GPU submission waits) are sampled and summarized on exit.
//!
//! `ScreenEffects` (analytic sky + material-aware shadows + fog/clouds) are opt-in via
//! `--screen-effects` because the effects renderer requires ordered compute + indirect draw
//! capabilities and falls back to no presents on devices lacking them. The sidecar path then uses
//! the plain scene-view renderer with a flat background; the direct path always renders a real
//! environment-map sky, so compare sky/shadow numbers with that difference in mind.

use crate::{
    camera::{FlightCamera, SharedCamera},
    scene, trace, Config,
};
use gpui::{
    App, Bounds, Context, FocusHandle, FrameClock, KeyDownEvent, MouseButton,
    RendererBackend, RendererFallback, RendererOptions, ScrollDelta, Timer, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_3d::{SceneView, ScreenEffects};
use std::error::Error;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct SidecarExample {
    scene_view: Arc<SceneView>,
    focus_handle: FocusHandle,
}

impl Render for SidecarExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x121a2c))
            .track_focus(&self.focus_handle)
            .on_key_down(move |event: &KeyDownEvent, _, _| {
                let key = event.keystroke.key.as_str();
                let step = if event.keystroke.modifiers.shift { 4.0 } else { 1.2 };
                let camera = CAMERA.get().expect("camera installed before window open");
                camera.update(|flight| match key {
                    "w" => flight.move_forward(step),
                    "s" => flight.move_forward(-step),
                    "a" => flight.strafe(-step),
                    "d" => flight.strafe(step),
                    "q" => flight.move_up(-step),
                    "e" => flight.move_up(step),
                    _ => {}
                });
            })
            .child({
                let camera = CAMERA.get().expect("camera installed before window open").clone();
                let scroll_camera = camera.clone();
                div()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        DRAGGING.store(true, Ordering::Relaxed);
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, _| {
                        DRAGGING.store(false, Ordering::Relaxed);
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
                        DRAGGING.store(false, Ordering::Relaxed);
                    })
                    .on_mouse_move(move |event: &gpui::MouseMoveEvent, _, _| {
                        if DRAGGING.load(Ordering::Relaxed) {
                            let (dx, dy) = mouse_delta(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            );
                            camera.update(|flight| flight.look(dx, dy));
                        }
                    })
                    .on_scroll_wheel(move |event: &gpui::ScrollWheelEvent, _, _| {
                        let delta = match event.delta {
                            ScrollDelta::Pixels(point) => -f32::from(point.y),
                            ScrollDelta::Lines(point) => -point.y * 24.0,
                        };
                        scroll_camera.update(|flight| flight.move_forward(delta));
                    })
                    .child(gpui_3d::scene_view::element(self.scene_view.clone()))
            })
    }
}

static DRAGGING: AtomicBool = AtomicBool::new(false);
static CAMERA: std::sync::OnceLock<SharedCamera> = std::sync::OnceLock::new();
static LAST_MOUSE: std::sync::OnceLock<Mutex<(f32, f32)>> = std::sync::OnceLock::new();

fn mouse_delta(x: f32, y: f32) -> (f32, f32) {
    let slot = LAST_MOUSE.get_or_init(|| Mutex::new((x, y)));
    let mut last = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let delta = (x - last.0, y - last.1);
    *last = (x, y);
    // Sensitivity matching FlightCamera::look.
    (delta.0 * 1.5, delta.1 * 1.5)
}

/// Runs the sidecar window benchmark. Returns after the window closes (or `--auto-exit-ms`).
pub(crate) fn run(config: &Config) -> Result<std::process::ExitCode, Box<dyn Error>> {
    trace::event("sidecar.run", "begin", Some(&config.backend));

    let world = scene::build(&config.world)?;
    println!("world_bench: source = {}", world.description);
    println!(
        "world_bench: nodes={} radius={:.2} center=({:.2},{:.2},{:.2})",
        world.scene.len(),
        world.radius,
        world.center.x,
        world.center.y,
        world.center.z
    );

    let framed = scene::starting_camera(&world, config.yaw, config.pitch);
    let shared = FlightCamera::from_camera(framed).shared();
    // Publish the camera state once so GPUI input handlers (which only get &mut Window / &mut App)
    // can reach the same state the renderer-thread sampler reads.
    let _ = CAMERA.set(shared.clone());

    // The sampler runs on the renderer thread. It advances the flight camera at a steady angular
    // rate so every presented frame is a real re-render (not the static capture-reuse path), then
    // samples the shared state that GPUI input handlers mutate.
    let sampler_camera = shared.clone();
    let degrees_per_sec = config.orbit_degrees;
    let last_sample = Arc::new(Mutex::new(None::<Instant>));
    let sampler_last = last_sample.clone();
    let mut scene_view = SceneView::new(Arc::new(world.scene), framed)
        .with_textures(world.textures.values().cloned().collect::<Vec<_>>())?
        .with_camera_sampler(move |now| {
            let mut slot = sampler_last.lock().ok();
            if let Some(slot) = slot.as_deref_mut() {
                let dt = slot.map_or(0.0, |previous| previous.elapsed().as_secs_f32());
                *slot = Some(now);
                if degrees_per_sec != 0.0 {
                    sampler_camera.update(|flight| flight.orbit(degrees_per_sec.to_radians() * dt));
                }
            }
            sampler_camera.sample()
        });
    if config.screen_effects {
        // Sidecar sky + material-aware shadows + volumetric fog/clouds (analytic sky, no IBL map).
        // Opt-in: the effects renderer requires ordered compute + indirect draw capabilities.
        scene_view = scene_view.with_screen_effects(Some(ScreenEffects::default()))?;
    }
    let scene_view = scene_view;

    trace::event("sidecar.sceneview", "created + element ready", None);

    let backend = config
        .backend
        .parse::<RendererBackend>()
        .map_err(|error| format!("invalid renderer backend '{}': {error}", config.backend))?;
    let timeout = config.auto_exit_ms.map(Duration::from_millis);
    let observed = Arc::new(AtomicBool::new(false));
    let observed_by_task = observed.clone();
    let baseline = gpui::diagnostics::performance_metrics::snapshot();
    // Continuous presentation may present via the retained full-window target instead of a direct
    // swapchain present, so consider both counters as evidence that frames are flowing.
    let initial_presents = (baseline.direct_present_count, baseline.retained_present_count);
    let metrics_interval = config.metrics_interval_ms.max(50);
    let json_path = config.json.clone();
    let source_description = world.description.clone();
    let backend_name = config.backend.clone();
    let screen_effects = config.screen_effects;
    let width = config.width;
    let height = config.height;
    let fps_cap = config.fps_cap;

    gpui_platform::application_with_renderer_options(RendererOptions {
        backend,
        fallback: RendererFallback::Disabled,
        ..Default::default()
    })
    .run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                let scene_view = scene_view.clone();
                move |window, cx| {
                    if let Some(cap) = fps_cap {
                        if let Ok(clock) = FrameClock::fixed_fps(cap) {
                            window.set_frame_clock(clock);
                        }
                    }
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window, cx);
                    cx.new(|_| SidecarExample {
                        scene_view: Arc::new(scene_view),
                        focus_handle,
                    })
                }
            },
        ) {
            eprintln!("world_bench sidecar: failed to open window: {error:#}");
            cx.quit();
            return;
        }
        if let Some(timeout) = timeout {
            sample_and_quit(
                cx,
                observed_by_task,
                initial_presents,
                timeout,
                Duration::from_millis(metrics_interval),
                json_path,
                source_description,
                backend_name,
                screen_effects,
                width,
                height,
            );
        }
        cx.activate(true);
    });

    if timeout.is_some() && !observed.load(Ordering::Acquire) {
        return Err("sidecar example did not observe a native present".into());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn sample_and_quit(
    cx: &mut App,
    observed: Arc<AtomicBool>,
    initial_presents: (usize, usize),
    timeout: Duration,
    interval: Duration,
    json_path: Option<String>,
    source_description: String,
    backend_name: String,
    screen_effects: bool,
    width: u32,
    height: u32,
) {
    cx.spawn(async move |cx| {
        let started_at = Instant::now();
        let mut fps_samples = Vec::new();
        let mut wait_samples = Vec::new();
        let mut total_presents = 0_usize;
        let mut last_direct = initial_presents.0;
        let mut last_retained = initial_presents.1;
        while started_at.elapsed() < timeout {
            Timer::after(interval).await;
            let metrics = gpui::diagnostics::performance_metrics::snapshot();
            let direct = metrics.direct_present_count;
            let retained = metrics.retained_present_count;
            total_presents +=
                direct.saturating_sub(last_direct) + retained.saturating_sub(last_retained);
            last_direct = direct;
            last_retained = retained;
            if metrics.present_fps > 0.0 {
                fps_samples.push(metrics.present_fps);
            }
            if let Some(wait) = metrics.gpu_submission_wait_time {
                wait_samples.push(wait.as_secs_f64() * 1_000_000.0);
            }
            if total_presents > 0 {
                observed.store(true, Ordering::Release);
            }
        }
        let final_metrics = gpui::diagnostics::performance_metrics::snapshot();
        let directs = final_metrics.direct_present_count.saturating_sub(initial_presents.0);
        let retained = final_metrics.retained_present_count.saturating_sub(initial_presents.1);
        if directs + retained > 0 {
            observed.store(true, Ordering::Release);
        }
        // GPUI's per-window metrics are the decisive 120 FPS evidence: present-interval
        // percentiles are measured from real submitted presentations, and the extension
        // breakdown isolates the 3D sidecar's CPU cost from GPU presentation cadence.
        let windows = gpui::diagnostics::performance_metrics::window::snapshot();
        let w = windows.iter().max_by_key(|w| w.present_fps_milli);
        println!(
            "world_bench: present counters: direct={} retained={} fps={:.2} prepared_commands={}",
            directs, retained, final_metrics.present_fps, final_metrics.prepared_command_count
        );
        if let Some(w) = w {
            println!(
                "world_bench: window present_fps={:.2} interval_p50={:.3}ms p95={:.3}ms p99={:.3}ms (n={}) draws={}",
                w.present_fps_milli as f32 / 1000.0,
                w.present_interval_p50_micros as f64 / 1000.0,
                w.present_interval_p95_micros as f64 / 1000.0,
                w.present_interval_p99_micros as f64 / 1000.0,
                w.present_interval_sample_count,
                w.draw_count,
            );
            println!(
                "world_bench: extension_prepare_p95={:.3}ms frame_upload_p95={:.3}ms backend_present_p95={:.3}ms total_p95={:.3}ms (n={})",
                w.extension_present.extension_prepare_p95_micros as f64 / 1000.0,
                w.extension_present.frame_upload_p95_micros as f64 / 1000.0,
                w.extension_present.backend_present_p95_micros as f64 / 1000.0,
                w.extension_present.total_p95_micros as f64 / 1000.0,
                w.extension_present.sample_count,
            );
        }
        print_summary(
            &backend_name,
            &source_description,
            width,
            height,
            total_presents,
            screen_effects,
            &fps_samples,
            &wait_samples,
            w,
        );
        if let Some(path) = json_path {
            let json = json_report(
                &backend_name,
                &source_description,
                width,
                height,
                total_presents,
                screen_effects,
                &fps_samples,
                &wait_samples,
                w,
            );
            if let Err(error) = std::fs::write(&path, json) {
                eprintln!("world_bench sidecar: write {path}: {error}");
            } else {
                println!("world_bench: wrote {path}");
            }
        }
        trace::event("sidecar.run", "quit", None);
        if let Err(error) = cx.update(|cx| cx.quit()) {
            eprintln!("world_bench sidecar: exit failed: {error:#}");
        }
    })
    .detach();
}

#[allow(clippy::too_many_arguments)]
fn print_summary(
    backend: &str,
    source: &str,
    width: u32,
    height: u32,
    presents: usize,
    screen_effects: bool,
    fps: &[f32],
    waits_us: &[f64],
    window: Option<&gpui::diagnostics::performance_metrics::window::MetricsSnapshot>,
) {
    let fps_avg = fps.iter().copied().sum::<f32>() / fps.len().max(1) as f32;
    let wait_avg = waits_us.iter().copied().sum::<f64>() / waits_us.len().max(1) as f64;
    println!(
        "path=sidecar backend={} size={}x{} presents={} present_fps_avg={:.2} gpu_wait_avg_us={:.1}",
        backend, width, height, presents, fps_avg, wait_avg
    );
    match window {
        Some(w) if w.extension_present.sample_count > 0 => {
            // Headless windows are not marked visible, so GPUI leaves the per-window
            // present_fps / interval fields at zero. The extension timing samples ARE recorded and
            // are the faithful per-frame CPU cost of the 3D sidecar. total_p95 is the whole
            // owner-side frame from start through submission; compare it to the 8.333ms budget.
            let total = w.extension_present.total_p95_micros;
            println!(
                "120fps_check: gpu_owner_total_p95={:.3}ms (budget=8.333ms) {} | ext_prepare_p95={:.3}ms frame_upload_p95={:.3}ms backend_present_p95={:.3}ms (n={}) global_fps={:.1}",
                total as f64 / 1000.0,
                if total <= 8_333 { "PASS" } else { "FAIL" },
                w.extension_present.extension_prepare_p95_micros as f64 / 1000.0,
                w.extension_present.frame_upload_p95_micros as f64 / 1000.0,
                w.extension_present.backend_present_p95_micros as f64 / 1000.0,
                w.extension_present.sample_count,
                fps_avg,
            );
        }
        _ => println!(
            "120fps_check: no extension timing samples; global_fps={:.1} (present count based)",
            fps_avg
        ),
    }
    println!("world_bench: source = {source}");
    if screen_effects {
        println!("sky: screen_effects analytic sky + material-aware shadows + fog/clouds (sidecar path)");
    } else {
        println!("sky: flat background (enable --screen-effects for analytic sky; needs compute+indirect)");
    }
    println!("note: frame timing is GPUI Present-driven; compare with direct render/capture percentiles carefully");
}

fn json_report(
    backend: &str,
    source: &str,
    width: u32,
    height: u32,
    presents: usize,
    screen_effects: bool,
    fps: &[f32],
    waits_us: &[f64],
    window: Option<&gpui::diagnostics::performance_metrics::window::MetricsSnapshot>,
) -> String {
    let fps_avg = fps.iter().copied().sum::<f32>() / fps.len().max(1) as f32;
    let wait_avg = waits_us.iter().copied().sum::<f64>() / waits_us.len().max(1) as f64;
    let sky = if screen_effects {
        "screen_effects"
    } else {
        "flat_background"
    };
    let (window_fps, present_p50, present_p95, present_p99, ext_prepare_p95, total_p95) =
        window.map_or((0.0, 0, 0, 0, 0, 0), |w| {
            (
                w.present_fps_milli as f64 / 1000.0,
                w.present_interval_p50_micros,
                w.present_interval_p95_micros,
                w.present_interval_p99_micros,
                w.extension_present.extension_prepare_p95_micros,
                w.extension_present.total_p95_micros,
            )
        });
    format!(
        concat!(
            "{{\n",
            "  \"path\": \"sidecar\",\n",
            "  \"backend\": \"{}\",\n",
            "  \"source\": \"{}\",\n",
            "  \"width\": {},\n",
            "  \"height\": {},\n",
            "  \"presents\": {},\n",
            "  \"present_fps_avg\": {:.3},\n",
            "  \"gpu_wait_avg_us\": {:.3},\n",
            "  \"window_present_fps\": {:.3},\n",
            "  \"present_interval_p50_ns\": {},\n",
            "  \"present_interval_p95_ns\": {},\n",
            "  \"present_interval_p99_ns\": {},\n",
            "  \"extension_prepare_p95_ns\": {},\n",
            "  \"total_p95_ns\": {},\n",
            "  \"budget_120fps_ns\": 8333333,\n",
            "  \"sky\": \"{}\"\n",
            "}}\n"
        ),
        backend,
        source.replace('"', "\\\""),
        width,
        height,
        presents,
        fps_avg,
        wait_avg,
        window_fps,
        present_p50.saturating_mul(1000),
        present_p95.saturating_mul(1000),
        present_p99.saturating_mul(1000),
        ext_prepare_p95.saturating_mul(1000),
        total_p95.saturating_mul(1000),
        sky,
    )
}
