//! Native Nova device selection for the offscreen scene lab.
//!
//! Each backend is created with the same [`gfx_core::DeviceDescriptor`]. The renderer itself
//! only needs gfx-core traits, so the lab is the only place that names concrete backend types.

#![allow(
    unsafe_code,
    reason = "Creating a WGL OpenGL device requires a hidden Win32 window and audited win32 FFI; this code is example-only and never part of the gpui-3d library."
)]
#![allow(
    clippy::borrow_as_ptr,
    clippy::no_effect_underscore_binding,
    clippy::similar_names,
    reason = "Win32 class registration borrows a stack value as a pointer and the hidden window binding documents its lifetime; the rendered/scene names describe distinct GPU frame stages."
)]

use gpui_3d::{
    Camera, CapturedImage, FrameStatistics, OffscreenRenderer, RenderOptions, RendererStatistics,
    Scene, TextureAsset, TextureAssetId, TransformTrack,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) struct RenderRequest<'a> {
    pub scene: &'a Scene,
    pub textures: &'a HashMap<TextureAssetId, Arc<TextureAsset>>,
    pub camera: Camera,
    pub options: &'a RenderOptions,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Optional sampled animation clip and absolute clip time.
    pub animation: Option<(&'a [TransformTrack], Duration)>,
}

pub(crate) struct BackendReport {
    pub backend: String,
    pub image: CapturedImage,
    pub first_frame: FrameStatistics,
    pub steady_frame: FrameStatistics,
    pub renderer: RendererStatistics,
    pub render_nanos: u128,
    pub capture_nanos: u128,
    pub frames: u32,
}

pub(crate) fn available_backends() -> Vec<&'static str> {
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

pub(crate) fn render(backend: &str, request: &RenderRequest<'_>) -> Result<BackendReport, String> {
    match backend {
        #[cfg(windows)]
        "dx12" => run_dx12(request),
        #[cfg(windows)]
        "dx11" => run_dx11(request),
        #[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
        "vulkan" => run_vulkan(request),
        #[cfg(windows)]
        "opengl" => run_opengl(request),
        other => Err(format!(
            "backend '{other}' is not available in this build (available: {})",
            available_backends().join(", ")
        )),
    }
}

fn descriptor() -> gfx_core::DeviceDescriptor {
    gfx_core::DeviceDescriptor {
        application_name: "gpui-3d scene lab".to_string(),
        adapter_name: None,
        power_preference: gfx_core::PowerPreference::HighPerformance,
        pipeline_cache_dir: None,
    }
}

fn run_generic<D>(
    device: &mut D,
    name: &str,
    request: &RenderRequest<'_>,
) -> Result<BackendReport, String>
where
    D: gfx_core::Device + gfx_core::TextureTransferDevice + ?Sized,
{
    let mut renderer = OffscreenRenderer::new(device, request.width, request.height)
        .map_err(|error| format!("create offscreen renderer: {error}"))?;
    let mut first_frame = FrameStatistics::default();
    let mut steady_frame = FrameStatistics::default();
    let mut image = None;
    let mut render_nanos = 0_u128;
    let mut capture_nanos = 0_u128;
    let frames = request.frames.max(1);
    for frame in 0..frames {
        let start = Instant::now();
        let rendered = match request.animation {
            Some((tracks, time)) => renderer.render_animated(
                device,
                request.scene,
                tracks,
                time,
                request.textures,
                request.camera,
                request.options,
            ),
            None => renderer.render(
                device,
                request.scene,
                request.textures,
                request.camera,
                request.options,
            ),
        };
        let statistics = rendered.map_err(|error| format!("render frame {frame}: {error}"))?;
        render_nanos += start.elapsed().as_nanos();
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
        capture_nanos += capture_start.elapsed().as_nanos();
    }
    let renderer_statistics = renderer.statistics();
    renderer
        .destroy(device)
        .map_err(|error| format!("destroy renderer: {error}"))?;
    Ok(BackendReport {
        backend: name.to_string(),
        image: image.ok_or_else(|| "renderer produced no frame".to_string())?,
        first_frame,
        steady_frame,
        renderer: renderer_statistics,
        render_nanos,
        capture_nanos,
        frames,
    })
}

#[cfg(windows)]
fn run_dx12(request: &RenderRequest<'_>) -> Result<BackendReport, String> {
    let mut device = gfx_dx12::Dx12Device::new(&descriptor())
        .map_err(|error| format!("create DX12 device: {error}"))?;
    run_generic(&mut device, "dx12", request)
}

#[cfg(windows)]
fn run_dx11(request: &RenderRequest<'_>) -> Result<BackendReport, String> {
    let mut device = gfx_dx11::Dx11Device::new(&descriptor())
        .map_err(|error| format!("create DX11 device: {error}"))?;
    run_generic(&mut device, "dx11", request)
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
fn run_vulkan(request: &RenderRequest<'_>) -> Result<BackendReport, String> {
    let mut device = gfx_vulkan::VulkanDevice::new(&descriptor())
        .map_err(|error| format!("create Vulkan device: {error}"))?;
    run_generic(&mut device, "vulkan", request)
}

#[cfg(windows)]
fn run_opengl(request: &RenderRequest<'_>) -> Result<BackendReport, String> {
    let window = HiddenWindow::new()?;
    let display_handle = raw_window_handle::RawDisplayHandle::Windows(
        raw_window_handle::WindowsDisplayHandle::new(),
    );
    // SAFETY: the hidden window stays alive for the device lifetime and its native handles come
    // from this process.
    let device = unsafe {
        gfx_opengl::OpenGlDevice::new(&descriptor(), display_handle, window.window_handle()?)
    }
    .map_err(|error| format!("create OpenGL device: {error}"))?;
    let mut device = device;
    let report = run_generic(&mut device, "opengl", request);
    // The device is dropped before the hidden window because it is declared later; WGL keeps
    // the native window alive while the context exists.
    let _window_lifetime = window;
    report
}

#[cfg(windows)]
unsafe extern "system" fn hidden_window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    // SAFETY: DefWindowProcW handles every message this hidden bootstrap window can receive.
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

/// Minimal hidden Win32 window used only to create a WGL context.
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
        // SAFETY: the window class and window are created on this thread and the process module
        // handle stays valid for the process lifetime.
        unsafe {
            let instance =
                GetModuleHandleW(None).map_err(|error| format!("GetModuleHandleW: {error}"))?;
            let class_name = w!("Gpui3dSceneLab");
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
                w!("gpui-3d scene lab"),
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
