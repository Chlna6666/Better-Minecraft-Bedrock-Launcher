use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    App, Application, Bounds, Context, ImagePixelFormat, RenderImage, RendererBackend,
    RendererFallback, RendererOptions, Timer, Window, WindowBounds, WindowOptions, div, img,
    prelude::*, px, rgb, size,
};
use sysinfo::{ProcessesToUpdate, System};

struct SmokeWindow {
    image: Arc<RenderImage>,
}

impl Render for SmokeWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(12.0))
            .gap(px(12.0))
            .bg(rgb(0x0010_1214))
            .text_color(rgb(0xffffff))
            .child("nova-gfx text and image")
            .child(img(self.image.clone()).size(px(64.0)))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let started_at = Instant::now();
    let backend = requested_backend()?;
    reject_conflicting_gpui_renderer_env(backend)?;
    let first_frame_printed = Arc::new(AtomicBool::new(false));
    let initial_presents = gpui::performance_metrics_snapshot().direct_present_count;
    let image = Arc::new(RenderImage::from_raw_pixels(
        2,
        2,
        ImagePixelFormat::Rgba8,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )?);
    if let Some(timeout) = auto_exit_timeout()? {
        spawn_exit_fallback(started_at, first_frame_printed.clone(), timeout);
    }
    println!("selected_backend={backend}");
    println!("renderer_path=nova-gfx");
    println!("startup_time_ms={}", started_at.elapsed().as_millis());

    Application::with_renderer_options(RendererOptions {
        backend,
        fallback: RendererFallback::Disabled,
        ..Default::default()
    })
    .run({
        let first_frame_printed = first_frame_printed.clone();
        move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(320.0), px(200.0)), cx);
            if let Err(error) = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |_, cx| cx.new(|_| SmokeWindow { image }),
            ) {
                eprintln!("failed to open nova-gfx GPUI smoke window: {error:#}");
                cx.quit();
                return;
            }

            cx.spawn({
                let first_frame_printed = first_frame_printed.clone();
                async move |cx| {
                    for _ in 0..200 {
                        Timer::after(Duration::from_millis(50)).await;
                        let metrics = gpui::performance_metrics_snapshot();
                        if metrics.direct_present_count > initial_presents {
                            first_frame_printed.store(true, Ordering::SeqCst);
                            println!(
                                "first_present_observed_time_ms={}",
                                started_at.elapsed().as_millis()
                            );
                            println!(
                                "native_presents={}",
                                metrics.direct_present_count - initial_presents
                            );
                            println!("actual_backend={}", metrics.renderer_backend);
                            println!("process_memory_kib={}", process_memory_kib());
                            break;
                        }
                    }
                    Timer::after(Duration::from_millis(750)).await;
                    let _ = cx.update(|cx| cx.quit());
                }
            })
            .detach();

            cx.activate(true);
        }
    });

    if first_frame_printed.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err("smoke did not observe a native present".into())
    }
}

fn requested_backend() -> Result<RendererBackend, Box<dyn Error>> {
    let backend = std::env::args()
        .skip(1)
        .find_map(|arg| arg.strip_prefix("--backend=").map(str::to_string))
        .or_else(|| std::env::var(RendererBackend::ENV_VAR).ok())
        .unwrap_or_else(default_backend_name);
    let backend = backend.parse::<RendererBackend>()?;
    if is_supported_smoke_backend(backend) {
        Ok(backend)
    } else {
        Err(format!(
            "nova-gpui-minimal-window-smoke requires nova-opengl, nova-dx11, nova-dx12, nova-vulkan, or nova-metal on macOS; got {backend}"
        )
        .into())
    }
}

fn default_backend_name() -> String {
    #[cfg(target_os = "macos")]
    {
        "nova-metal".to_string()
    }
    #[cfg(target_os = "windows")]
    {
        "nova-dx12".to_string()
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        "nova-vulkan".to_string()
    }
}

fn is_supported_smoke_backend(backend: RendererBackend) -> bool {
    match backend {
        RendererBackend::NovaOpenGl
        | RendererBackend::NovaDx11
        | RendererBackend::NovaDx12
        | RendererBackend::NovaVulkan => true,
        #[cfg(target_os = "macos")]
        RendererBackend::NovaMetal => true,
        _ => false,
    }
}

fn auto_exit_timeout() -> Result<Option<Duration>, Box<dyn Error>> {
    let value = std::env::args()
        .skip(1)
        .find_map(|arg| arg.strip_prefix("--auto-exit-ms=").map(str::to_string))
        .or_else(|| std::env::var("NOVA_GPUI_SMOKE_AUTO_EXIT_MS").ok());
    let Some(value) = value else {
        return Ok(None);
    };
    let millis = value.parse::<u64>()?;
    Ok(Some(Duration::from_millis(millis)))
}

fn reject_conflicting_gpui_renderer_env(backend: RendererBackend) -> Result<(), Box<dyn Error>> {
    let Ok(value) = std::env::var(RendererBackend::ENV_VAR) else {
        return Ok(());
    };
    let env_backend = value.parse::<RendererBackend>()?;
    if env_backend == backend {
        return Ok(());
    }
    Err(format!(
        "{}={} would override --backend={} and leave the smoke unable to prove the nova-gfx path",
        RendererBackend::ENV_VAR,
        value,
        backend
    )
    .into())
}

fn process_memory_kib() -> u64 {
    let mut system = System::new();
    let Ok(pid) = sysinfo::get_current_pid() else {
        return 0;
    };
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or(0, sysinfo::Process::memory)
}

fn spawn_exit_fallback(
    started_at: Instant,
    first_frame_printed: Arc<AtomicBool>,
    timeout: Duration,
) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        if !first_frame_printed.load(Ordering::SeqCst) {
            eprintln!(
                "native present timeout after {} ms",
                started_at.elapsed().as_millis()
            );
            std::process::exit(1);
        }
        std::process::exit(0);
    });
}
