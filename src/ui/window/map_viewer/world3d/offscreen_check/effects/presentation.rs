//! Presents the fixed world fixture through a real Windows GPU Owner.
use super::{Camera, Duration, Extent2d, Instant, SceneView, Vec3, percentile};
use gpui::{
    App, Application, Bounds, Context, FrameClock, GpuPowerPreference, RendererBackend,
    RendererFallback, RendererOptions, Timer, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, size,
};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

struct MovingWorld(Arc<SceneView>);

impl Render for MovingWorld {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(gpui_3d::scene_view::element(Arc::clone(&self.0)))
    }
}

fn presents() -> usize {
    let metrics = gpui::diagnostics::performance_metrics::snapshot();
    metrics.direct_present_count + metrics.retained_present_count
}

pub(super) fn profile(view: SceneView, camera: Camera, extent: Extent2d) {
    let started = OnceLock::new();
    let view = Arc::new(view.with_camera_sampler(move |now| {
        let angle = now
            .saturating_duration_since(*started.get_or_init(|| now))
            .as_secs_f32()
            * 1.7;
        Camera {
            eye: camera.eye + Vec3::new(angle.sin() * 4., 0., angle.cos() * 4.),
            ..camera
        }
    }));
    let observed = Arc::new(AtomicBool::new(false));
    let window_observed = observed.clone();
    Application::with_renderer_options(RendererOptions {
        backend: RendererBackend::NovaDx11,
        fallback: RendererFallback::Disabled,
        power_preference: GpuPowerPreference::HighPerformance,
        frame_metrics: true,
        ..RendererOptions::default()
    })
    .run(move |cx: &mut App| {
        let bounds = Bounds::centered(
            None,
            size(px(extent.width() as f32), px(extent.height() as f32)),
            cx,
        );
        let baseline = presents();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..WindowOptions::default()
            },
            move |window, cx| {
                window.set_frame_clock(FrameClock::fixed_fps(120).unwrap());
                cx.new(|_| MovingWorld(view))
            },
        )
        .expect("open DX11 world fixture");
        cx.activate(true);
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while presents() == baseline && Instant::now() < deadline {
                Timer::after(Duration::from_millis(100)).await;
            }
            if presents() > baseline {
                Timer::after(Duration::from_millis(500)).await;
                let baseline = presents();
                use gpui::diagnostics::gpu_owner::{GpuOwnerJobOutcome, metrics_snapshot, samples_since};
                let owner = metrics_snapshot()
                    .windows
                    .into_iter()
                    .find(|owner| owner.window_id.is_some())
                    .map(|owner| {
                        let id = owner.owner_id;
                        let baseline_job = samples_since(id, 0).last().map_or(0, |sample| sample.job_id);
                        (id, baseline_job)
                    });
                let start = Instant::now();
                Timer::after(Duration::from_secs(6)).await;
                let count = presents() - baseline;
                let fps = count as f64 / start.elapsed().as_secs_f64();
                if let Some((owner, baseline_job)) = owner {
                    let samples = samples_since(owner, baseline_job);
                    let mut duration = Vec::new();
                    let mut gaps = Vec::new();
                    let mut previous = None;
                    for sample in samples.iter().filter(|s| s.outcome == GpuOwnerJobOutcome::Submitted) {
                        duration.push(sample.owner_job_duration_us as f64 / 1000.);
                        if let Some(previous) = previous {
                            gaps.push(sample.completed_at_us.saturating_sub(previous) as f64 / 1000.);
                        }
                        previous = Some(sample.completed_at_us);
                    }
                    if !gaps.is_empty() {
                        eprintln!(
                            "world3d window presents={count} fps={fps:.2} owner_p95_ms={:.2} gap_p95_ms={:.2} gap_p99_ms={:.2}",
                            percentile(&mut duration, 0.95),
                            percentile(&mut gaps, 0.95),
                            percentile(&mut gaps, 0.99),
                        );
                    }
                }
                window_observed.store(count > 0, Ordering::Release);
            }
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    assert!(
        observed.load(Ordering::Acquire),
        "world fixture did not present"
    );
}
