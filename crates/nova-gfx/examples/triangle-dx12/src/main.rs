#![cfg_attr(
    windows,
    expect(
        unsafe_code,
        reason = "the Windows DX12 triangle example owns the native window and raw handle boundary"
    )
)]

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_example::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("nova-triangle-dx12 is only available on Windows");
}

#[cfg(windows)]
mod windows_example {
    use std::{num::NonZeroIsize, time::Duration, time::Instant};

    use gfx_core::{
        BlendMode, ClearColor, ColorAttachmentDesc, DeviceDesc, Format, GfxPipelineDevice,
        GfxPresentationDevice, GfxSurfaceDevice, PresentMode, RenderPassDesc, RenderPassId,
        RenderPipelineDesc, RenderPipelineId, ShaderBinary, ShaderModuleDesc, ShaderStage,
        SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId,
    };
    use gfx_dx12::Dx12Device;
    use gfx_shader::compile_wgsl_to_hlsl;
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
        Win32WindowHandle, WindowHandle,
    };
    use sysinfo::{Pid, ProcessesToUpdate, System};
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW, MSG,
                PostQuitMessage, RegisterClassW, SW_SHOW, ShowWindow, TranslateMessage,
                WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_PAINT, WM_SIZE, WNDCLASSW,
                WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
        core::w,
    };

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
        let window = NativeWindow::new(960, 540)?;
        let size = window.size();
        let vertex_shader = compile_wgsl_to_hlsl(TRIANGLE_WGSL, ShaderStage::Vertex, "vs_main")?;
        let fragment_shader =
            compile_wgsl_to_hlsl(TRIANGLE_WGSL, ShaderStage::Fragment, "fs_main")?;
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
        run_message_loop(&mut renderer)
    }

    #[derive(Default)]
    struct BaselineMetrics {
        startup_time: Duration,
        first_frame_time: Option<Duration>,
        submitted_frames: u64,
    }

    struct NativeWindow {
        hwnd: HWND,
        hinstance: isize,
        width: u32,
        height: u32,
    }

    impl NativeWindow {
        fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
            // SAFETY: Passing None requests the current process module handle.
            let module = unsafe { GetModuleHandleW(None) }?;
            let hinstance = module.0 as isize;
            let class_name = w!("NovaGfxTriangleDx12Window");
            let window_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: class_name,
                // SAFETY: Loading a predefined cursor does not borrow application memory.
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
                ..Default::default()
            };
            // SAFETY: WNDCLASSW points to static class name data and a valid window proc.
            let atom = unsafe { RegisterClassW(&raw const window_class) };
            if atom == 0 {
                return Err(Box::new(windows::core::Error::from_thread()));
            }

            // SAFETY: Class was registered above, title is static, and parent/menu are null.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class_name,
                    w!("nova-gfx triangle dx12"),
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
            // SAFETY: HWND was returned by CreateWindowExW and is valid.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
            Ok(Self {
                hwnd,
                hinstance,
                width,
                height,
            })
        }

        fn size(&self) -> WindowSize {
            WindowSize {
                width: self.width,
                height: self.height,
            }
        }
    }

    impl HasWindowHandle for NativeWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let hwnd = NonZeroIsize::new(self.hwnd.0 as isize).ok_or(HandleError::Unavailable)?;
            let mut handle = Win32WindowHandle::new(hwnd);
            handle.hinstance = NonZeroIsize::new(self.hinstance);
            // SAFETY: The borrowed raw handle is tied to self and self owns a live HWND.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
        }
    }

    impl HasDisplayHandle for NativeWindow {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct WindowSize {
        width: u32,
        height: u32,
    }

    struct TriangleRenderer {
        device: Dx12Device,
        surface: SurfaceId,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        pipeline: RenderPipelineId,
        current_size: WindowSize,
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
            let mut device = Dx12Device::new(&DeviceDesc {
                application_name: "nova-gfx triangle dx12".to_string(),
            })?;
            let surface = device.create_surface(window, &SurfaceDesc { label: None })?;
            let current_size = WindowSize {
                width: surface_config.size.width(),
                height: surface_config.size.height(),
            };
            let swapchain = device.create_swapchain(surface, surface_config)?;
            let vertex_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("triangle dx12 vertex shader".to_string()),
                binary: vertex_shader,
            })?;
            let fragment_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("triangle dx12 fragment shader".to_string()),
                binary: fragment_shader,
            })?;
            let render_pass = device.create_render_pass(&RenderPassDesc {
                label: Some("triangle dx12 render pass".to_string()),
                color_attachment: ColorAttachmentDesc {
                    format: surface_config.format,
                },
            })?;
            let pipeline = device.create_render_pipeline(
                &RenderPipelineDesc {
                    label: Some("triangle dx12 pipeline".to_string()),
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
                current_size,
                metrics_started_at,
                metrics,
            })
        }

        fn resize(&mut self, width: u32, height: u32) -> gfx_core::Result<bool> {
            let next_size = WindowSize { width, height };
            if width == 0 || height == 0 || next_size == self.current_size {
                return Ok(false);
            }
            self.device
                .resize_swapchain(self.swapchain, width, height)?;
            self.current_size = next_size;
            Ok(true)
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

    fn run_message_loop(renderer: &mut TriangleRenderer) -> Result<(), Box<dyn std::error::Error>> {
        let mut message = MSG::default();
        loop {
            // SAFETY: Message pointer is valid for writes during this call.
            let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
            if result.0 == -1 {
                return Err(Box::new(windows::core::Error::from_thread()));
            }
            if result.0 == 0 {
                return Ok(());
            }
            let resize = (message.message == WM_SIZE)
                .then(|| (low_word(message.lParam), high_word(message.lParam)));
            let should_draw_after_dispatch = message.message == WM_PAINT;
            // SAFETY: Message was produced by GetMessageW.
            unsafe {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
            if let Some((width, height)) = resize {
                if renderer.resize(width, height)? {
                    renderer.draw_triangle()?;
                }
            } else if should_draw_after_dispatch {
                renderer.draw_triangle()?;
            }
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
                // SAFETY: hwnd is supplied by the system for this callback.
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: Posting quit is process-local and takes no borrowed data.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: hwnd is valid for this paint callback and paint storage is local.
                unsafe {
                    BeginPaint(hwnd, &raw mut paint);
                    let _ = EndPaint(hwnd, &raw const paint);
                }
                LRESULT(0)
            }
            _ => {
                // SAFETY: Default window procedure is called with the original callback values.
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
    }

    fn low_word(value: LPARAM) -> u32 {
        let bytes = value.0.to_ne_bytes();
        u32::from(u16::from_ne_bytes([bytes[0], bytes[1]]))
    }

    fn high_word(value: LPARAM) -> u32 {
        let bytes = value.0.to_ne_bytes();
        u32::from(u16::from_ne_bytes([bytes[2], bytes[3]]))
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
