//! Direct (headless offscreen) rendering path.
//!
//! Creates a native Nova device, renders the shared world through `OffscreenRenderer` with a
//! scripted moving camera, times render + capture per frame, and prints the same percentile and
//! counter report as `scene_lab`. This is the "without GPUI sidecar" side of the comparison.

#![allow(
    unsafe_code,
    reason = "Creating a WGL OpenGL device requires a hidden Win32 window and audited win32 FFI; example-only, never part of the gpui_3d library."
)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::borrow_as_ptr,
    reason = "Timing samples and validated positive pixel centers are narrowed for reporting only."
)]

use crate::{camera::FlightCamera, scene, trace, Config};
use gpui_3d::{
    CapturedImage, FrameStatistics, OffscreenRenderer, RenderOptions, RenderQuality,
    RendererStatistics,
};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Instant;

struct FrameSample {
    render_nanos: u64,
    capture_nanos: u64,
}

struct BackendReport {
    image: CapturedImage,
    first_frame: FrameStatistics,
    steady_frame: FrameStatistics,
    renderer: RendererStatistics,
    samples: Vec<FrameSample>,
    render_nanos: u128,
    capture_nanos: u128,
    frames: u32,
}

/// Runs the direct benchmark and returns a process exit code.
pub(crate) fn run(config: &Config) -> Result<ExitCode, String> {
    trace::event("direct.run", "begin", Some(&config.backend));

    let mut world = scene::build(&config.world).map_err(|error| format!("build world: {error}"))?;
    println!("world_bench: source = {}", world.description);
    println!(
        "world_bench: nodes={} radius={:.2} center=({:.2},{:.2},{:.2})",
        world.scene.len(), world.radius, world.center.x, world.center.y, world.center.z
    );

    let framed = scene::starting_camera(&world, config.yaw, config.pitch);
    let mut flight = FlightCamera::from_camera(framed);

    let mut options = config
        .quality
        .map_or_else(RenderOptions::default, RenderQuality::options);
    options.shadows.enabled = config.shadows;
    if let Some(resolution) = config.shadow_resolution {
        options.shadows.resolution = resolution;
    }
    options.exposure = config.exposure;
    // Attach the sky/IBL environment directly to the scene for the offscreen path.
    world.scene.set_environment(Some(scene::environment()));

    let request = RenderRequest {
        scene: &world.scene,
        textures: &world.textures,
        options: &options,
        width: config.width,
        height: config.height,
        frames: config.frames,
        render_scale: config.render_scale,
        orbit_per_frame: config.orbit_degrees.to_radians(),
        flight: &mut flight,
    };

    let backend_name = config.backend.as_str();
    trace::event("direct.render", backend_name, None);
    let report = render(backend_name, request)?;
    trace::event("direct.render", "done", None);

    if let Some(path) = &config.out {
        save_png(path, &report.image)?;
        println!("world_bench: wrote {path}");
    }
    print_report(&report, &options);
    if let Some(path) = &config.json {
        let json = report_json(&report, &options, &world.description);
        std::fs::write(path, json).map_err(|error| format!("write {path}: {error}"))?;
        println!("world_bench: wrote {path}");
    }
    Ok(ExitCode::SUCCESS)
}

struct RenderRequest<'a> {
    scene: &'a gpui_3d::Scene,
    textures: &'a HashMap<gpui_3d::TextureAssetId, std::sync::Arc<gpui_3d::TextureAsset>>,
    options: &'a RenderOptions,
    width: u32,
    height: u32,
    frames: u32,
    render_scale: f32,
    orbit_per_frame: f32,
    flight: &'a mut FlightCamera,
}

fn available_backends() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut backends = Vec::new();
    #[cfg(windows)]
    {
        backends.push("dx12");
        backends.push("dx11");
        backends.push("vulkan");
        backends.push("opengl");
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        backends.push("vulkan");
        backends.push("opengl");
    }
    backends
}

fn descriptor() -> gfx_core::DeviceDescriptor {
    gfx_core::DeviceDescriptor {
        application_name: "gpui_3d world bench".to_string(),
        adapter_name: None,
        power_preference: gfx_core::PowerPreference::HighPerformance,
        pipeline_cache_dir: None,
        tuning: gfx_core::DeviceTuning::default(),
    }
}

fn render(backend: &str, request: RenderRequest<'_>) -> Result<BackendReport, String> {
    match backend {
        #[cfg(windows)]
        "dx12" => {
            let mut device = gfx_dx12::Dx12Device::new(&descriptor())
                .map_err(|error| format!("create DX12 device: {error}"))?;
            run_generic(&mut device, request)
        }
        #[cfg(windows)]
        "dx11" => {
            let mut device = gfx_dx11::Dx11Device::new(&descriptor())
                .map_err(|error| format!("create DX11 device: {error}"))?;
            run_generic(&mut device, request)
        }
        #[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
        "vulkan" => {
            let mut device = gfx_vulkan::VulkanDevice::new(&descriptor())
                .map_err(|error| format!("create Vulkan device: {error}"))?;
            run_generic(&mut device, request)
        }
        #[cfg(windows)]
        "opengl" => {
            let window = HiddenWindow::new()?;
            let display_handle = raw_window_handle::RawDisplayHandle::Windows(
                raw_window_handle::WindowsDisplayHandle::new(),
            );
            // SAFETY: the hidden window outlives the device and its native handles come from this
            // process.
            let device = unsafe {
                gfx_opengl::OpenGlDevice::new(&descriptor(), display_handle, window.window_handle()?)
            }
            .map_err(|error| format!("create OpenGL device: {error}"))?;
            let mut device = device;
            let report = run_generic(&mut device, request);
            let _window_lifetime = window;
            report
        }
        other => Err(format!(
            "backend '{other}' is not available in this build (available: {})",
            available_backends().join(", ")
        )),
    }
}

fn run_generic<D>(device: &mut D, request: RenderRequest<'_>) -> Result<BackendReport, String>
where
    D: gfx_core::Device + gfx_core::TextureTransferDevice + ?Sized,
{
    let mut renderer = OffscreenRenderer::new(device, request.width, request.height)
        .map_err(|error| format!("create offscreen renderer: {error}"))?;
    renderer
        .set_render_scale(device, request.render_scale)
        .map_err(|error| format!("set render scale: {error}"))?;

    let frames = request.frames.max(1);
    let mut first_frame = FrameStatistics::default();
    let mut steady_frame = FrameStatistics::default();
    let mut image = None;
    let mut samples = Vec::with_capacity(frames as usize);
    let mut render_nanos = 0_u128;
    let mut capture_nanos = 0_u128;
    for frame in 0..frames {
        // Move the camera every frame so the renderer does not take the static-frame reuse path.
        request.flight.orbit(request.orbit_per_frame);
        let camera = request.flight.camera();

        trace::event("direct.render.frame", "render", Some(&frame.to_string()));
        let start = Instant::now();
        let statistics = renderer
            .render(
                device,
                request.scene,
                request.textures,
                camera,
                request.options,
            )
            .map_err(|error| format!("render frame {frame}: {error}"))?;
        let render_elapsed = start.elapsed().as_nanos();
        render_nanos += render_elapsed;
        if frame == 0 {
            first_frame = statistics;
        }
        steady_frame = statistics;

        let capture_start = Instant::now();
        image = Some(
            renderer
                .capture(device)
                .map_err(|error| format!("capture frame {frame}: {error}"))?,
        );
        let capture_elapsed = capture_start.elapsed().as_nanos();
        capture_nanos += capture_elapsed;
        samples.push(FrameSample {
            render_nanos: render_elapsed as u64,
            capture_nanos: capture_elapsed as u64,
        });
    }
    let renderer_statistics = renderer.statistics();
    trace::event("direct.renderer.destroy", "destroy", None);
    renderer
        .destroy(device)
        .map_err(|error| format!("destroy renderer: {error}"))?;

    Ok(BackendReport {
        image: image.ok_or_else(|| "renderer produced no frame".to_string())?,
        first_frame,
        steady_frame,
        renderer: renderer_statistics,
        samples,
        render_nanos,
        capture_nanos,
        frames,
    })
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) * percent) / 100]
}

fn print_report(report: &BackendReport, options: &RenderOptions) {
    let frames = report.frames.max(1);
    println!(
        "path=direct backend=direct frames={} size={}x{} render_avg={:.2}ms capture_avg={:.2}ms",
        report.frames,
        report.image.width(),
        report.image.height(),
        report.render_nanos as f64 / f64::from(frames) / 1.0e6,
        report.capture_nanos as f64 / f64::from(frames) / 1.0e6,
    );
    let first = report.first_frame;
    let steady = report.steady_frame;
    println!(
        "first_frame: draws={} shadow_draws={} triangles={} instances={} mesh_uploads={} texture_uploads={} upload_bytes={} prepare={:.2}ms submit={:.2}ms",
        first.draw_calls, first.shadow_draw_calls, first.triangles, first.instances,
        first.mesh_uploads, first.texture_uploads, first.upload_bytes,
        first.cpu_prepare_nanos as f64 / 1.0e6, first.submit_nanos as f64 / 1.0e6,
    );
    println!(
        "steady_frame: draws={} shadow_draws={} triangles={} instances={} mesh_uploads={} texture_uploads={} upload_bytes={} prepare={:.2}ms submit={:.2}ms reused={} camera_only={}",
        steady.draw_calls, steady.shadow_draw_calls, steady.triangles, steady.instances,
        steady.mesh_uploads, steady.texture_uploads, steady.upload_bytes,
        steady.cpu_prepare_nanos as f64 / 1.0e6, steady.submit_nanos as f64 / 1.0e6,
        steady.frame_reused, steady.camera_only_update,
    );
    let r = report.renderer;
    println!(
        "memory: resident={}KB peak={}KB shadow={}x{} render_target={}x{} cache_hits(mesh={},texture={}) evictions={}",
        (r.resident_buffer_bytes + r.resident_texture_bytes) / 1024,
        r.peak_resident_bytes / 1024, r.shadow_resolution, r.shadow_resolution,
        r.render_target_width, r.render_target_height,
        r.mesh_cache_hits, r.texture_cache_hits, r.evictions,
    );
    if report.frames > 1 {
        let mut render_samples = report.samples.iter().map(|s| s.render_nanos).collect::<Vec<_>>();
        let mut capture_samples = report.samples.iter().map(|s| s.capture_nanos).collect::<Vec<_>>();
        render_samples.sort_unstable();
        capture_samples.sort_unstable();
        println!(
            "frame_times: render p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms | capture p50={:.2}ms p95={:.2}ms p99={:.2}ms",
            percentile(&render_samples, 50) as f64 / 1.0e6,
            percentile(&render_samples, 95) as f64 / 1.0e6,
            percentile(&render_samples, 99) as f64 / 1.0e6,
            render_samples.last().copied().unwrap_or(0) as f64 / 1.0e6,
            percentile(&capture_samples, 50) as f64 / 1.0e6,
            percentile(&capture_samples, 95) as f64 / 1.0e6,
            percentile(&capture_samples, 99) as f64 / 1.0e6,
        );
    }
    println!(
        "sky: environment_map=procedural_studio shadows={} (direct renderer shadow pass)",
        options.shadows.enabled
    );
}

fn report_json(report: &BackendReport, options: &RenderOptions, description: &str) -> String {
    let render = sorted_percentiles(report, |s| s.render_nanos);
    let capture = sorted_percentiles(report, |s| s.capture_nanos);
    let frame = report.steady_frame;
    format!(
        concat!(
            "{{\n",
            "  \"path\": \"direct\",\n",
            "  \"source\": \"{}\",\n",
            "  \"frames\": {},\n",
            "  \"width\": {},\n",
            "  \"height\": {},\n",
            "  \"render_nanos_total\": {},\n",
            "  \"capture_nanos_total\": {},\n",
            "  \"render_p50\": {},\n",
            "  \"render_p95\": {},\n",
            "  \"render_p99\": {},\n",
            "  \"capture_p50\": {},\n",
            "  \"capture_p95\": {},\n",
            "  \"capture_p99\": {},\n",
            "  \"steady_draw_calls\": {},\n",
            "  \"steady_triangles\": {},\n",
            "  \"steady_instances\": {},\n",
            "  \"steady_prepare_nanos\": {},\n",
            "  \"steady_submit_nanos\": {},\n",
            "  \"shadows\": {},\n",
            "  \"sky\": \"environment_map\"\n",
            "}}\n"
        ),
        json_escape(description),
        report.frames,
        report.image.width(),
        report.image.height(),
        report.render_nanos,
        report.capture_nanos,
        render.0, render.1, render.2,
        capture.0, capture.1, capture.2,
        frame.draw_calls, frame.triangles, frame.instances,
        frame.cpu_prepare_nanos, frame.submit_nanos,
        options.shadows.enabled,
    )
}

fn sorted_percentiles(report: &BackendReport, selector: fn(&FrameSample) -> u64) -> (u64, u64, u64) {
    let mut values = report.samples.iter().map(selector).collect::<Vec<_>>();
    values.sort_unstable();
    (
        percentile(&values, 50),
        percentile(&values, 95),
        percentile(&values, 99),
    )
}

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn save_png(path: &str, image: &CapturedImage) -> Result<(), String> {
    image::save_buffer_with_format(
        path,
        image.rgba(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| format!("write PNG {path}: {error}"))
}

#[cfg(windows)]
unsafe extern "system" fn hidden_window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

/// Minimal hidden Win32 window used only to create a WGL context for the OpenGL backend.
#[cfg(windows)]
struct HiddenWindow {
    hwnd: windows::Win32::Foundation::HWND,
}

#[cfg(windows)]
impl HiddenWindow {
    fn new() -> Result<Self, String> {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CS_OWNDC, CW_USEDEFAULT, CreateWindowExW, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
            WS_OVERLAPPEDWINDOW,
        };
        use windows::core::w;
        // SAFETY: class and window are created on this thread and the process module handle
        // stays valid for the process lifetime.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|error| format!("GetModuleHandleW: {error}"))?;
            let class_name = w!("Gpui3dWorldBench");
            let window_class = WNDCLASSW {
                style: CS_OWNDC,
                lpfnWndProc: Some(hidden_window_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&window_class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("gpui_3d world bench"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                64,
                64,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|error| format!("CreateWindowExW: {error}"))?;
            Ok(Self { hwnd })
        }
    }

    fn window_handle(&self) -> Result<raw_window_handle::RawWindowHandle, String> {
        let handle = std::num::NonZeroIsize::new(self.hwnd.0 as isize)
            .ok_or_else(|| "hidden window handle is null".to_string())?;
        Ok(raw_window_handle::RawWindowHandle::Win32(
            raw_window_handle::Win32WindowHandle::new(handle),
        ))
    }
}

