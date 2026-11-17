//! Standalone (NO GPUI) real-time presenter for a gpui-3d large world.
//!
//! This binary is built with `--no-default-features`, so it links no GPUI at all. It owns a Win32
//! window and a DX12 swapchain, renders the shared world through `OffscreenRenderer`, and presents
//! the final color with a **zero-copy fullscreen blit** (scene color view -> swapchain, no CPU
//! readback). It is the "without GPUI" half of a fair comparison against `world_bench --mode=sidecar`
//! (GPUI sidecar), using the same scene and camera.
//!
//! ```text
//! cargo run -p gpui-3d --example native_realtime --no-default-features -- \
//!   --instances=20000 --duration-ms=6000 --fps-cap=120 --present-mode=mailbox --json=native.json
//! ```

#![cfg_attr(
    windows,
    expect(
        unsafe_code,
        reason = "the native presenter owns the Win32 window and raw handle boundary"
    )
)]

#[cfg(not(windows))]
fn main() {
    eprintln!("native_realtime is only available on Windows");
}

#[cfg(windows)]
mod app {
    use gfx_core::{
        AddressMode, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
        BindGroupLayoutEntry, BindingResource, BindingType, BlendMode, ClearColor,
        ColorAttachmentDescriptor, ColorTargetState, Device, DeviceDescriptor, FilterMode, Format,
        LoadOp, PipelineDevice, PipelineLayoutDescriptor, PresentFrameDescriptor, PresentMode,
        PresentationDevice, PrimitiveTopology, RenderCommand, RenderPassDescriptor, RenderPassId,
        RenderPipelineDescriptor, RenderPipelineId, ResourceDevice, SamplerBinding, SamplerDescriptor,
        ShaderModuleDescriptor, ShaderStages, SurfaceConfig, SurfaceDescriptor, SurfaceDevice,
        SurfaceId, SwapchainId, TextureBinding, TextureViewId,
    };
    use gfx_dx12::Dx12Device;
    use gpui_3d::{
        BackendKind, Camera, ColorTargetState as _UnusedColorTargetStateAliasGuard, OffscreenRenderer,
        RenderOptions, RenderQuality, Scene, ShaderStage, TextureAsset, TextureAssetId, Vec3,
        Viewport, blit_shader,
    };
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
        Win32WindowHandle, WindowHandle,
    };
    use std::{
        collections::HashMap,
        num::NonZeroIsize,
        sync::Arc,
        time::{Duration, Instant},
    };
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::PAINTSTRUCT,
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, PeekMessageW, PM_REMOVE, PostQuitMessage,
                RegisterClassW, SW_SHOW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE,
                WM_DESTROY, WM_PAINT, WM_QUIT, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
        core::w,
    };

    // Shared world scene/camera, reused verbatim from world_bench (examples-only #[path] module).
    // Paths are relative to this file's directory, which contains this inline `app` module.
    #[path = "../../world_bench/scene.rs"]
    mod scene;
    #[path = "../../world_bench/camera.rs"]
    mod camera;

    const BUDGET_NS: u64 = 8_333_333; // 120 FPS

    struct Args {
        width: u32,
        height: u32,
        instances: u32,
        duration: Duration,
        fps_cap: Option<u32>,
        present_mode: PresentMode,
        json: Option<String>,
        trace: bool,
        yaw: f32,
        pitch: f32,
    }

    impl Default for Args {
        fn default() -> Self {
            Self {
                width: 1280,
                height: 720,
                instances: 20_000,
                duration: Duration::from_millis(6000),
                fps_cap: Some(120),
                present_mode: PresentMode::Mailbox,
                json: None,
                trace: false,
                yaw: 0.62,
                pitch: 0.32,
            }
        }
    }

    fn main() -> Result<(), Box<dyn std::error::Error>> {
        let args = parse_args()?;
        let window = NativeWindow::new(args.width, args.height)?;
        let surface_config = SurfaceConfig::new(args.width, args.height, Format::Bgra8Unorm)?;
        let mut presenter = Presenter::new(&window, surface_config, &args)?;
        let report = run_loop(&mut presenter, &args)?;
        print_report(&report);
        if let Some(path) = &args.json {
            std::fs::write(path, json_report(&report))?;
            println!("wrote {path}");
        }
        Ok(())
    }

    fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
        let mut args = Args::default();
        for argument in std::env::args().skip(1) {
            let (key, value) = argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(k, v)| (k, Some(v)));
            match key {
                "--width" => args.width = value.unwrap().parse()?,
                "--height" => args.height = value.unwrap().parse()?,
                "--instances" => args.instances = value.unwrap().parse()?,
                "--duration-ms" => {
                    args.duration = Duration::from_millis(value.unwrap().parse()?);
                }
                "--fps-cap" => args.fps_cap = Some(value.unwrap().parse()?),
                "--present-mode" => {
                    args.present_mode = match value.unwrap() {
                        "mailbox" => PresentMode::Mailbox,
                        "fifo" => PresentMode::Fifo,
                        "immediate" => PresentMode::Immediate,
                        other => return Err(format!("unknown present mode {other}").into()),
                    };
                }
                "--json" => args.json = Some(value.unwrap().to_string()),
                "--trace" => args.trace = true,
                "--yaw" => args.yaw = value.unwrap().parse()?,
                "--pitch" => args.pitch = value.unwrap().parse()?,
                "--help" => {
                    println!(
                        "native_realtime [--width=1280] [--height=720] [--instances=20000]\n  [--duration-ms=6000] [--fps-cap=120] [--present-mode=mailbox|fifo|immediate]\n  [--json=report.json] [--trace]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}").into()),
            }
        }
        Ok(args)
    }

    struct Presenter {
        device: Dx12Device,
        _surface: SurfaceId,
        swapchain: SwapchainId,
        surface_format: Format,
        blit_pass: RenderPassId,
        blit_pipeline: RenderPipelineId,
        _bind_layout: gfx_core::BindGroupLayoutId,
        _pipeline_layout: gfx_core::PipelineLayoutId,
        sampler: gfx_core::SamplerId,
        blit_bind_group: gfx_core::BindGroupId,
        bound_view: Option<TextureViewId>,
        renderer: OffscreenRenderer,
        scene: Scene,
        textures: HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: RenderOptions,
        current_size: (u32, u32),
    }

    impl Presenter {
        fn new(
            window: &NativeWindow,
            mut surface_config: SurfaceConfig,
            args: &Args,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            surface_config.present_mode = args.present_mode;
            let mut device = Dx12Device::new(&DeviceDescriptor {
                application_name: "gpui-3d native_realtime".to_string(),
                power_preference: gfx_core::PowerPreference::HighPerformance,
                ..DeviceDescriptor::default()
            })?;
            let surface = device.create_surface(window, &SurfaceDescriptor { label: None })?;
            let swapchain = device.create_swapchain(surface, surface_config)?;
            let format = surface_config.format;

            let vs = blit_shader(BackendKind::Dx12, ShaderStage::Vertex)?;
            let fs = blit_shader(BackendKind::Dx12, ShaderStage::Fragment)?;
            let vs_module = device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("blit vs".to_string()),
                binary: vs,
            })?;
            let fs_module = device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("blit fs".to_string()),
                binary: fs,
            })?;

            let bind_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("blit layout".to_string()),
                entries: vec![
                    BindGroupLayoutEntry {
                        binding: 0,
                        binding_type: BindingType::SampledTexture,
                        stages: ShaderStages::FRAGMENT,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        binding_type: BindingType::Sampler,
                        stages: ShaderStages::FRAGMENT,
                    },
                ],
            })?;
            let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("blit pipeline layout".to_string()),
                bind_group_layouts: vec![bind_layout],
            })?;
            let sampler = device.create_sampler(&SamplerDescriptor {
                label: Some("blit sampler".to_string()),
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                mipmap_filter: FilterMode::Nearest,
                address_mode_u: AddressMode::ClampToEdge,
                address_mode_v: AddressMode::ClampToEdge,
                address_mode_w: AddressMode::ClampToEdge,
                ..Default::default()
            })?;
            let blit_pass = device.create_render_pass(&RenderPassDescriptor {
                label: Some("blit pass".to_string()),
                color_attachments: vec![ColorAttachmentDescriptor { format }],
                depth_attachment: None,
                sample_count: 1,
                resolve_target: None,
            })?;
            let blit_pipeline = device.create_render_pipeline(
                &RenderPipelineDescriptor {
                    label: Some("blit pipeline".to_string()),
                    vertex_shader: vs_module,
                    vertex_entry_point: "vs_blit".to_string(),
                    fragment_shader: fs_module,
                    fragment_entry_point: "fs_blit".to_string(),
                    vertex_buffers: Vec::new(),
                    render_pass: blit_pass,
                    pipeline_layout: Some(pipeline_layout),
                    color_targets: vec![ColorTargetState::new(format, BlendMode::Replace)],
                    primitive_topology: PrimitiveTopology::TriangleList,
                    raster: Default::default(),
                    depth_state: None,
                },
                surface_config.size,
            )?;

            let mut world_options = scene::WorldOptions::default();
            world_options.instances = args.instances;
            let world = scene::build(&world_options).map_err(|e| format!("build world: {e}"))?;
            world.scene.set_environment(Some(scene::environment()));
            let camera = scene::starting_camera(&world, args.yaw, args.pitch);
            let renderer = OffscreenRenderer::new(&mut device, args.width, args.height)
                .map_err(|e| format!("create renderer: {e}"))?;
            let options = RenderQuality::High.options();

            println!(
                "world_bench: source = {} | nodes={}",
                world.description,
                world.scene.len()
            );

            Ok(Self {
                device,
                _surface: surface,
                swapchain,
                surface_format: format,
                blit_pass,
                blit_pipeline,
                _bind_layout: bind_layout,
                _pipeline_layout: pipeline_layout,
                sampler,
                blit_bind_group: device.create_bind_group(&BindGroupDescriptor {
                    label: Some("blit bind group".to_string()),
                    layout: bind_layout,
                    entries: vec![], // replaced every frame once color_view is known
                })?,
                bound_view: None,
                renderer,
                scene: world.scene,
                textures: world.textures,
                camera,
                options,
                current_size: (args.width, args.height),
            })
        }

        fn resize(&mut self, width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
            if width == 0 || height == 0 || (width, height) == self.current_size {
                return Ok(());
            }
            self.device.resize_swapchain(self.swapchain, width, height)?;
            self.renderer
                .resize(&mut self.device, width, height)
                .map_err(|e| format!("renderer resize: {e}"))?;
            self.current_size = (width, height);
            self.bound_view = None;
            Ok(())
        }

        fn present_frame(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            // 1. Render the scene into the renderer's internal post_color (GPU only).
            self.renderer
                .render(
                    &mut self.device,
                    &self.scene,
                    &self.textures,
                    self.camera,
                    &self.options,
                )
                .map_err(|e| format!("render: {e}"))?;
            // 2. Consume the render submission so the next frame's ensure_idle() does not stall.
            let _ = self
                .renderer
                .poll_render(&mut self.device)
                .map_err(|e| format!("poll_render: {e}"))?;
            // 3. Bind the produced color view (once, or after resize) and blit to the swapchain.
            let view = self
                .renderer
                .color_view()
                .map_err(|e| format!("color_view: {e}"))?;
            if self.bound_view != Some(view) {
                self.device.destroy_bind_group(self.blit_bind_group);
                self.blit_bind_group = self.device.create_bind_group(&BindGroupDescriptor {
                    label: Some("blit bind group".to_string()),
                    layout: self._bind_layout,
                    entries: vec![
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Texture(TextureBinding {
                                texture_view: view,
                            }),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(SamplerBinding {
                                sampler: self.sampler,
                            }),
                        },
                    ],
                })?;
                self.bound_view = Some(view);
            }
            let cmd = RenderCommand::Draw(gfx_core::DrawCommand {
                pipeline: self.blit_pipeline,
                bind_groups: gfx_core::bind_group_list([self.blit_bind_group]),
                vertex_count: 3,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            });
            let frame = PresentFrameDescriptor {
                swapchain: self.swapchain,
                render_pass: self.blit_pass,
                commands: std::slice::from_ref(&cmd),
                color_load_op: LoadOp::Clear(ClearColor {
                    red: 0.0,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 1.0,
                }),
                depth_attachment: None,
                present_region: None,
            };
            self.device.render_and_present(&frame)?;
            Ok(())
        }
    }

    struct Report {
        presents: usize,
        intervals_ns: Vec<u64>,
        host_total_ns: Vec<u64>,
        instances: u32,
        width: u32,
        height: u32,
        fps_cap: Option<u32>,
        present_mode: &'static str,
    }

    fn run_loop(
        presenter: &mut Presenter,
        args: &Args,
    ) -> Result<Report, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let frame_interval = args
            .fps_cap
            .map(|fps| Duration::from_nanos(1_000_000_000u64 / fps.max(1) as u64));
        let mut next_deadline = started;
        let mut last_present: Option<Instant> = None;
        let mut intervals_ns = Vec::new();
        let mut host_total_ns = Vec::new();
        let mut presents = 0usize;
        let mut message = std::mem::zeroed::<windows::Win32::UI::WindowsAndMessaging::MSG>();
        while started.elapsed() < args.duration {
            // Non-blocking message pump.
            unsafe {
                while PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        return Ok(Report {
                            presents,
                            intervals_ns,
                            host_total_ns,
                            instances: args.instances,
                            width: args.width,
                            height: args.height,
                            fps_cap: args.fps_cap,
                            present_mode: present_mode_name(args.present_mode),
                        });
                    }
                    if message.message == WM_SIZE {
                        let (w, h) = (low_word(message.lParam), high_word(message.lParam));
                        presenter.resize(w, h)?;
                    }
                    let _ = TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
            // Gate on swapchain readiness (DX12 frame-latency waitable object).
            if !presenter
                .device
                .poll_present_readiness(presenter.swapchain)?
            {
                std::thread::yield_now();
                continue;
            }
            // Own deadline pacing for a hard 120 FPS cap independent of monitor refresh.
            if let Some(interval) = frame_interval {
                let now = Instant::now();
                if now < next_deadline {
                    std::thread::sleep(next_deadline - now);
                }
                next_deadline = next_deadline.max(now) + interval;
            }
            // Script the camera forward so each frame is a real re-render (no static reuse).
            presenter.camera.eye = presenter.camera.eye + Vec3::new(0.0, 0.0, 0.02);
            presenter.camera.target = presenter.camera.target + Vec3::new(0.0, 0.0, 0.02);

            let frame_started = Instant::now();
            presenter.present_frame()?;
            let host_elapsed = frame_started.elapsed();
            if let Some(previous) = last_present.replace(Instant::now()) {
                let delta = previous.elapsed();
                if delta >= Duration::from_micros(250) && delta <= Duration::from_secs(1) {
                    intervals_ns.push(delta.as_nanos() as u64);
                }
            }
            host_total_ns.push(host_elapsed.as_nanos() as u64);
            presents += 1;
        }
        // Clean shutdown for the device's tracked resources.
        let _ = presenter
            .renderer
            .destroy(&mut presenter.device)
            .map_err(|e| eprintln!("destroy renderer: {e}"));
        Ok(Report {
            presents,
            intervals_ns,
            host_total_ns,
            instances: args.instances,
            width: args.width,
            height: args.height,
            fps_cap: args.fps_cap,
            present_mode: present_mode_name(args.present_mode),
        })
    }

    fn percentile(sorted: &[u64], percent: usize) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        sorted[((sorted.len() - 1) * percent) / 100]
    }

    fn print_report(report: &Report) {
        let mut intervals = report.intervals_ns.clone();
        let mut host = report.host_total_ns.clone();
        intervals.sort_unstable();
        host.sort_unstable();
        let p50 = percentile(&intervals, 50);
        let p95 = percentile(&intervals, 95);
        let p99 = percentile(&intervals, 99);
        println!(
            "path=native backend=dx12 size={}x{} instances={} fps_cap={:?} present_mode={} presents={}",
            report.width,
            report.height,
            report.instances,
            report.fps_cap,
            report.present_mode,
            report.presents,
        );
        println!(
            "120fps_check: present_p50={:.3}ms p95={:.3}ms p99={:.3}ms (budget=8.333ms) {}",
            p50 as f64 / 1e6,
            p95 as f64 / 1e6,
            p99 as f64 / 1e6,
            if p95 <= BUDGET_NS { "PASS" } else { "FAIL" },
        );
        println!(
            "host_frame: p50={:.3}ms p95={:.3}ms (render+blit+present, CPU-side)",
            percentile(&host, 50) as f64 / 1e6,
            percentile(&host, 95) as f64 / 1e6,
        );
        println!("sky: environment_map (direct scene sky + shadow pass)");
    }

    fn json_report(report: &Report) -> String {
        let mut intervals = report.intervals_ns.clone();
        intervals.sort_unstable();
        format!(
            concat!(
                "{{\n",
                "  \"path\": \"native\",\n",
                "  \"backend\": \"dx12\",\n",
                "  \"width\": {},\n",
                "  \"height\": {},\n",
                "  \"instances\": {},\n",
                "  \"fps_cap\": {},\n",
                "  \"present_mode\": \"{}\",\n",
                "  \"presents\": {},\n",
                "  \"present_interval_p50_ns\": {},\n",
                "  \"present_interval_p95_ns\": {},\n",
                "  \"present_interval_p99_ns\": {},\n",
                "  \"budget_120fps_ns\": {},\n",
                "  \"pass_120fps\": {}\n",
                "}}\n"
            ),
            report.width,
            report.height,
            report.instances,
            report.fps_cap.unwrap_or(0),
            report.present_mode,
            report.presents,
            percentile(&intervals, 50),
            percentile(&intervals, 95),
            percentile(&intervals, 99),
            BUDGET_NS,
            if percentile(&intervals, 95) <= BUDGET_NS {
                "true"
            } else {
                "false"
            },
        )
    }

    fn present_mode_name(mode: PresentMode) -> &'static str {
        match mode {
            PresentMode::Fifo => "fifo",
            PresentMode::Mailbox => "mailbox",
            PresentMode::Immediate => "immediate",
        }
    }

    struct NativeWindow {
        hwnd: HWND,
        hinstance: isize,
        width: u32,
        height: u32,
    }

    impl NativeWindow {
        fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
            let module = unsafe { GetModuleHandleW(None) }?;
            let hinstance = module.0 as isize;
            let class_name = w!("Gpui3dNativeRealtimeWindow");
            let window_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&raw const window_class) };
            if atom == 0 {
                return Err(Box::new(windows::core::Error::from_thread()));
            }
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class_name,
                    w!("gpui-3d native realtime (no GPUI)"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    i32::try_from(width)?,
                    i32::try_from(height)?,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
            }?;
            let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
            Ok(Self {
                hwnd,
                hinstance,
                width,
                height,
            })
        }
    }

    impl HasWindowHandle for NativeWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let hwnd = NonZeroIsize::new(self.hwnd.0 as isize).ok_or(HandleError::Unavailable)?;
            let mut handle = Win32WindowHandle::new(hwnd);
            handle.hinstance = NonZeroIsize::new(self.hinstance);
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
        }
    }

    impl HasDisplayHandle for NativeWindow {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }

    extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &raw mut paint);
                    let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &raw const paint);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn low_word(value: LPARAM) -> u32 {
        let bytes = value.0.to_ne_bytes();
        u32::from(u16::from_ne_bytes([bytes[0], bytes[1]]))
    }
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::main()
}
