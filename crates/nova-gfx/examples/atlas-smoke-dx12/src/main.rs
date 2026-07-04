#![cfg_attr(
    windows,
    expect(
        unsafe_code,
        reason = "the Windows DX12 atlas example owns the native window and raw handle boundary"
    )
)]

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_example::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("nova-atlas-smoke-dx12 is only available on Windows");
}

#[cfg(windows)]
mod windows_example {
    use std::{num::NonZeroIsize, time::Instant};

    use gfx_core::{
        BlendMode, BufferBinding, BufferDesc, BufferUsage, ClearColor, ColorAttachmentDesc,
        DeviceDesc, Format, GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice,
        GfxSurfaceDevice, MemoryLocation, Origin2d, PipelineLayoutDesc, PresentMode,
        RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId, ResourceBinding,
        ResourceBindingResource, ResourceBindingType, ResourceSetDesc, ResourceSetId,
        ResourceSetLayoutDesc, ResourceSetLayoutEntry, SamplerBinding, SamplerDesc,
        ShaderModuleDesc, ShaderStage, ShaderStages, SurfaceConfig, SurfaceDesc, SurfaceId,
        TextureBinding, TextureDataLayout, TextureDesc, TextureDimension, TextureUsage,
        TextureViewDesc, TextureWriteDesc,
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

    const ATLAS_SIZE: u32 = 64;
    const ATLAS_WGSL: &str = r"
struct Uniforms {
    tint: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var atlas_texture: texture_2d<f32>;

@group(0) @binding(2)
var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-0.62, -0.62),
        vec2<f32>( 0.62, -0.62),
        vec2<f32>( 0.62,  0.62),
        vec2<f32>(-0.62, -0.62),
        vec2<f32>( 0.62,  0.62),
        vec2<f32>(-0.62,  0.62),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(atlas_texture, atlas_sampler, in.uv) * uniforms.tint;
}
";

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let started_at = Instant::now();
        let window = NativeWindow::new(960, 540)?;
        let size = window.size();
        let vertex_shader = compile_wgsl_to_hlsl(ATLAS_WGSL, ShaderStage::Vertex, "vs_main")?;
        let fragment_shader = compile_wgsl_to_hlsl(ATLAS_WGSL, ShaderStage::Fragment, "fs_main")?;
        let mut surface_config =
            SurfaceConfig::new(size.width.max(1), size.height.max(1), Format::Bgra8Unorm)?;
        surface_config.present_mode = PresentMode::Fifo;
        let mut renderer =
            AtlasRenderer::new(&window, surface_config, vertex_shader, fragment_shader)?;
        println!(
            "startup_time_ms={}",
            renderer.metrics.startup_time.as_millis()
        );
        renderer.draw_atlas()?;
        let mut system = System::new();
        print_first_frame_metrics(&renderer.metrics, started_at, &mut system);
        run_message_loop(&mut renderer)
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
            let class_name = w!("NovaGfxAtlasSmokeDx12Window");
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
                    w!("nova-gfx atlas smoke dx12"),
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

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct WindowSize {
        width: u32,
        height: u32,
    }

    struct Metrics {
        startup_time: std::time::Duration,
        first_frame_time: Option<std::time::Duration>,
        submitted_frames: u64,
    }

    struct AtlasRenderer {
        device: Dx12Device,
        surface: SurfaceId,
        swapchain: gfx_core::SwapchainId,
        render_pass: RenderPassId,
        pipeline: RenderPipelineId,
        resource_set: ResourceSetId,
        current_size: WindowSize,
        metrics_started_at: Instant,
        metrics: Metrics,
    }

    impl AtlasRenderer {
        #[expect(
            clippy::too_many_lines,
            reason = "atlas smoke keeps one visible resource creation sequence for validation"
        )]
        fn new(
            window: &NativeWindow,
            surface_config: SurfaceConfig,
            vertex_shader: gfx_core::ShaderBinary,
            fragment_shader: gfx_core::ShaderBinary,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let metrics_started_at = Instant::now();
            let mut device = Dx12Device::new(&DeviceDesc {
                application_name: "nova-gfx atlas smoke dx12".to_string(),
                ..DeviceDesc::default()
            })?;
            let surface = device.create_surface(window, &SurfaceDesc { label: None })?;
            let current_size = WindowSize {
                width: surface_config.size.width(),
                height: surface_config.size.height(),
            };
            let swapchain = device.create_swapchain(surface, surface_config)?;
            let vertex_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("atlas dx12 vertex shader".to_string()),
                binary: vertex_shader,
            })?;
            let fragment_shader = device.create_shader_module(&ShaderModuleDesc {
                label: Some("atlas dx12 fragment shader".to_string()),
                binary: fragment_shader,
            })?;
            let render_pass = device.create_render_pass(&RenderPassDesc {
                label: Some("atlas dx12 render pass".to_string()),
                color_attachment: ColorAttachmentDesc {
                    format: surface_config.format,
                },
                depth_attachment: None,
            })?;
            let layout = device.create_resource_set_layout(&atlas_layout_desc())?;
            let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
                label: Some("atlas dx12 pipeline layout".to_string()),
                resource_set_layouts: vec![layout],
            })?;
            let uniform = device.create_buffer(&BufferDesc {
                label: Some("atlas dx12 uniform".to_string()),
                size: 256,
                usage: BufferUsage::UNIFORM,
                memory_location: MemoryLocation::CpuToGpu,
            })?;
            device.write_buffer(uniform, 0, &uniform_bytes())?;
            let texture = device.create_texture(&TextureDesc {
                label: Some("atlas dx12 texture".to_string()),
                size: gfx_core::Extent2d::new(ATLAS_SIZE, ATLAS_SIZE)?,
                format: Format::Rgba8Unorm,
                usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
            })?;
            let atlas = atlas_pixels();
            device.write_texture(
                TextureWriteDesc {
                    texture,
                    layout: TextureDataLayout::new(0, ATLAS_SIZE * 4, ATLAS_SIZE)?,
                    origin: Origin2d::ZERO,
                    size: gfx_core::Extent2d::new(ATLAS_SIZE, ATLAS_SIZE)?,
                },
                &atlas,
            )?;
            let texture_view = device.create_texture_view(&TextureViewDesc {
                label: Some("atlas dx12 texture view".to_string()),
                texture,
                format: Format::Rgba8Unorm,
            })?;
            let sampler = device.create_sampler(&SamplerDesc::default())?;
            let resource_set = device.create_resource_set(&ResourceSetDesc {
                label: Some("atlas dx12 resource set".to_string()),
                layout,
                bindings: vec![
                    ResourceBinding {
                        binding: 0,
                        resource: ResourceBindingResource::Buffer(BufferBinding {
                            buffer: uniform,
                            offset: 0,
                            size: 16,
                            stride: None,
                        }),
                    },
                    ResourceBinding {
                        binding: 1,
                        resource: ResourceBindingResource::Texture(TextureBinding { texture_view }),
                    },
                    ResourceBinding {
                        binding: 2,
                        resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
                    },
                ],
            })?;
            let pipeline = device.create_render_pipeline(
                &RenderPipelineDesc {
                    label: Some("atlas dx12 pipeline".to_string()),
                    vertex_shader,
                    vertex_entry_point: "vs_main".to_string(),
                    fragment_shader,
                    fragment_entry_point: "fs_main".to_string(),
                    vertex_buffers: Vec::new(),
                    render_pass,
                    pipeline_layout: Some(pipeline_layout),
                    color_format: surface_config.format,
                    blend_mode: BlendMode::Replace,
                    primitive_topology: gfx_core::PrimitiveTopology::TriangleList,
                    depth_state: None,
                },
                surface_config.size,
            )?;
            Ok(Self {
                device,
                surface,
                swapchain,
                render_pass,
                pipeline,
                resource_set,
                current_size,
                metrics_started_at,
                metrics: Metrics {
                    startup_time: metrics_started_at.elapsed(),
                    first_frame_time: None,
                    submitted_frames: 0,
                },
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

        fn draw_atlas(&mut self) -> gfx_core::Result<()> {
            self.device.draw_resources_and_present(
                self.swapchain,
                self.render_pass,
                self.pipeline,
                &[self.resource_set],
                clear_color(),
                6,
            )?;
            self.metrics.submitted_frames = self.metrics.submitted_frames.saturating_add(1);
            if self.metrics.first_frame_time.is_none() {
                self.metrics.first_frame_time = Some(self.metrics_started_at.elapsed());
            }
            let _ = self.surface;
            Ok(())
        }
    }

    fn atlas_layout_desc() -> ResourceSetLayoutDesc {
        ResourceSetLayoutDesc {
            label: Some("atlas dx12 resource set layout".to_string()),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 1,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 2,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        }
    }

    fn atlas_pixels() -> Vec<u8> {
        let mut pixels = Vec::with_capacity((ATLAS_SIZE * ATLAS_SIZE * 4) as usize);
        for y in 0..ATLAS_SIZE {
            for x in 0..ATLAS_SIZE {
                let block = ((x / 8) + (y / 8)) % 2 == 0;
                let (red, green, blue) = if block {
                    (245, 245, 245)
                } else {
                    (220, 90, 40)
                };
                pixels.extend_from_slice(&[red, green, blue, 255]);
            }
        }
        pixels
    }

    fn uniform_bytes() -> [u8; 16] {
        let mut bytes = [0; 16];
        for (index, value) in [1.0_f32, 1.0, 1.0, 1.0].into_iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }

    fn clear_color() -> ClearColor {
        ClearColor {
            red: 0.02,
            green: 0.025,
            blue: 0.035,
            alpha: 1.0,
        }
    }

    fn run_message_loop(renderer: &mut AtlasRenderer) -> Result<(), Box<dyn std::error::Error>> {
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
                    renderer.draw_atlas()?;
                }
            } else if should_draw_after_dispatch {
                renderer.draw_atlas()?;
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

    fn print_first_frame_metrics(metrics: &Metrics, started_at: Instant, system: &mut System) {
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
