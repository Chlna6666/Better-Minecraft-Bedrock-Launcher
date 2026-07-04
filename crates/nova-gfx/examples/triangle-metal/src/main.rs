#![cfg_attr(
    target_vendor = "apple",
    expect(
        unsafe_code,
        reason = "the macOS Metal triangle example owns the AppKit and raw-handle boundary"
    )
)]

#[cfg(target_vendor = "apple")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos_example::run()
}

#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("nova-triangle-metal is only available on Apple targets");
}

#[cfg(target_vendor = "apple")]
mod macos_example {
    use std::{num::NonZeroU32, ptr::NonNull, thread, time::Duration, time::Instant};

    use gfx_core::{
        BlendMode, ClearColor, ColorAttachmentDesc, DeviceDesc, Format, GfxPipelineDevice,
        GfxPresentationDevice, GfxSurfaceDevice, PresentMode, RenderPassDesc, RenderPassId,
        RenderPipelineDesc, RenderPipelineId, ShaderBinary, ShaderModuleDesc, ShaderStage,
        SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId,
    };
    use gfx_metal::MetalDevice;
    use gfx_shader::compile_wgsl_to_msl;
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEventMask, NSView,
        NSWindow, NSWindowStyleMask,
    };
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSString};
    use raw_window_handle::{
        AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
        RawWindowHandle, WindowHandle,
    };
    use sysinfo::{Pid, ProcessesToUpdate, System};

    const TRIANGLE_WGSL: &str = r"
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(0.0, -0.5),
        vec2<f32>(0.5, 0.5),
        vec2<f32>(-0.5, 0.5),
    );
    var colors = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.color = colors[vertex_index];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
";

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let started_at = Instant::now();
        let main_thread = MainThreadMarker::new()
            .ok_or("nova-triangle-metal must be launched on the macOS main thread")?;
        let app = NSApplication::sharedApplication(main_thread);
        let _activation_changed = app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        // SAFETY: The application is initialized on the main thread before windows are shown.
        unsafe {
            app.finishLaunching();
        }

        let mut window = NativeWindow::new(main_thread, 960.0, 540.0);
        let size = window.size();
        let vertex_shader = compile_wgsl_to_msl(TRIANGLE_WGSL, ShaderStage::Vertex, "vs_main")?;
        let fragment_shader = compile_wgsl_to_msl(TRIANGLE_WGSL, ShaderStage::Fragment, "fs_main")?;
        let mut surface_config =
            SurfaceConfig::new(size.width.max(1), size.height.max(1), Format::Bgra8Unorm)?;
        surface_config.present_mode = PresentMode::Fifo;
        let mut renderer =
            TriangleRenderer::new(&window, surface_config, vertex_shader, fragment_shader)?;
        println!(
            "startup_time_ms={}",
            renderer.metrics().startup_time.as_millis()
        );
        renderer.draw_triangle()?;
        let mut system = System::new();
        print_first_frame_metrics(renderer.metrics(), started_at, &mut system);
        app.activateIgnoringOtherApps(true);
        run_event_loop(&app, &window, &mut renderer)
    }

    #[derive(Default)]
    struct BaselineMetrics {
        startup_time: Duration,
        first_frame_time: Option<Duration>,
        submitted_frames: u64,
    }

    struct NativeWindow {
        window: Retained<NSWindow>,
        view: Retained<NSView>,
        last_size: WindowSize,
    }

    impl NativeWindow {
        fn new(main_thread: MainThreadMarker, width: f64, height: f64) -> Self {
            let frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width, height },
            };
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;
            // SAFETY: NSWindow and NSView are allocated and initialized on the main thread.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(main_thread),
                    frame,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            let view = unsafe { NSView::initWithFrame(NSView::alloc(main_thread), frame) };
            view.setWantsLayer(true);
            window.setTitle(&NSString::from_str("nova-gfx triangle metal"));
            window.setContentView(Some(&view));
            window.setOpaque(true);
            window.setHasShadow(true);
            window.setAcceptsMouseMovedEvents(true);
            // SAFETY: The example owns a retained NSWindow for the process lifetime.
            unsafe {
                window.setReleasedWhenClosed(false);
            }
            window.center();
            window.makeKeyAndOrderFront(None);
            Self {
                window,
                view,
                last_size: WindowSize {
                    width: width.round() as u32,
                    height: height.round() as u32,
                },
            }
        }

        fn size(&self) -> WindowSize {
            let frame = self.view.visibleRect();
            WindowSize {
                width: f64_to_nonzero_u32(frame.size.width),
                height: f64_to_nonzero_u32(frame.size.height),
            }
        }

        fn poll_size_change(&mut self) -> Option<WindowSize> {
            let size = self.size();
            if size == self.last_size {
                None
            } else {
                self.last_size = size;
                Some(size)
            }
        }
    }

    impl HasWindowHandle for NativeWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let ns_view = NonNull::from(self.view.as_ref()).cast();
            let handle = AppKitWindowHandle::new(ns_view);
            // SAFETY: The borrowed raw handle is tied to self and self owns the live NSView.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::AppKit(handle)) })
        }
    }

    impl HasDisplayHandle for NativeWindow {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::appkit())
        }
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct WindowSize {
        width: u32,
        height: u32,
    }

    struct TriangleRenderer {
        device: MetalDevice,
        surface: SurfaceId,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        pipeline: RenderPipelineId,
        metrics_started_at: Instant,
        metrics: BaselineMetrics,
    }

    impl TriangleRenderer {
        fn new(
            window: &NativeWindow,
            surface_config: SurfaceConfig,
            vertex_shader: ShaderBinary,
            fragment_shader: ShaderBinary,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let metrics_started_at = Instant::now();
            let mut device = MetalDevice::new(&DeviceDesc {
                application_name: "nova-gfx triangle metal".to_string(),
                ..DeviceDesc::default()
            })?;
            let surface = device.create_surface(window, &SurfaceDesc { label: None })?;
            let swapchain = device.create_swapchain(surface, surface_config)?;
            let vertex_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("triangle metal vertex shader".to_string()),
                binary: vertex_shader,
            })?;
            let fragment_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("triangle metal fragment shader".to_string()),
                binary: fragment_shader,
            })?;
            let render_pass = device.create_render_pass(&RenderPassDesc {
                label: Some("triangle metal render pass".to_string()),
                color_attachment: ColorAttachmentDesc {
                    format: surface_config.format,
                },
                depth_attachment: None,
            })?;
            let pipeline = device.create_render_pipeline(
                &RenderPipelineDesc {
                    label: Some("triangle metal pipeline".to_string()),
                    vertex_shader,
                    vertex_entry_point: "vs_main".to_string(),
                    fragment_shader,
                    fragment_entry_point: "fs_main".to_string(),
                    vertex_buffers: Vec::new(),
                    render_pass,
                    pipeline_layout: None,
                    color_format: surface_config.format,
                    blend_mode: BlendMode::Replace,
                    primitive_topology: gfx_core::PrimitiveTopology::TriangleList,
                    depth_state: None,
                },
                surface_config.size,
            )?;
            let metrics = BaselineMetrics {
                startup_time: metrics_started_at.elapsed(),
                ..BaselineMetrics::default()
            };
            Ok(Self {
                device,
                surface,
                swapchain,
                render_pass,
                pipeline,
                metrics_started_at,
                metrics,
            })
        }

        fn resize(&mut self, width: u32, height: u32) -> gfx_core::Result<()> {
            self.device.resize_swapchain(self.swapchain, width, height)
        }

        fn draw_triangle(&mut self) -> gfx_core::Result<()> {
            self.device.draw_and_present(
                self.swapchain,
                self.render_pass,
                self.pipeline,
                clear_color(),
            )?;
            self.metrics.submitted_frames = self.metrics.submitted_frames.saturating_add(1);
            if self.metrics.first_frame_time.is_none() {
                self.metrics.first_frame_time = Some(self.metrics_started_at.elapsed());
            }
            Ok(())
        }

        fn metrics(&self) -> &BaselineMetrics {
            let _ = self.surface;
            &self.metrics
        }
    }

    fn clear_color() -> ClearColor {
        ClearColor {
            red: 0.02,
            green: 0.025,
            blue: 0.035,
            alpha: 1.0,
        }
    }

    fn run_event_loop(
        app: &NSApplication,
        window: &mut NativeWindow,
        renderer: &mut TriangleRenderer,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            while let Some(event) = next_pending_event(app) {
                // SAFETY: The event came from NSApplication and is dispatched on the main thread.
                unsafe {
                    app.sendEvent(&event);
                }
            }
            // SAFETY: Window updates are dispatched on the main AppKit thread.
            unsafe {
                app.updateWindows();
            }
            if let Some(size) = window.poll_size_change() {
                renderer.resize(size.width, size.height)?;
            }
            renderer.draw_triangle()?;
            thread::sleep(Duration::from_millis(16));
        }
    }

    fn next_pending_event(app: &NSApplication) -> Option<Retained<objc2_app_kit::NSEvent>> {
        let expiration = NSDate::distantPast();
        // SAFETY: Event polling is performed on the main AppKit thread, using the default run-loop
        // mode and a nonblocking expiration date.
        unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&expiration),
                NSDefaultRunLoopMode,
                true,
            )
        }
    }

    fn f64_to_nonzero_u32(value: f64) -> u32 {
        let rounded = if value.is_finite() {
            value.round()
        } else {
            1.0
        };
        NonZeroU32::new(rounded.max(1.0) as u32).map_or(1, NonZeroU32::get)
    }

    fn print_first_frame_metrics(
        metrics: &BaselineMetrics,
        started_at: Instant,
        system: &mut System,
    ) {
        let first_frame_ms = metrics
            .first_frame_time
            .unwrap_or_else(|| started_at.elapsed())
            .as_millis();
        let process_memory_kib = process_memory_kib(system).unwrap_or_default();
        println!("first_frame_time_ms={first_frame_ms}");
        println!("submitted_frames={}", metrics.submitted_frames);
        println!("process_memory_kib={process_memory_kib}");
    }

    fn process_memory_kib(system: &mut System) -> Option<u64> {
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).map(sysinfo::Process::memory)
    }
}
