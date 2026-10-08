//! Hardware gates: run explicitly with `cargo test -p gfx-opengl -- --ignored`.

use crate::OpenGlDevice;
use gfx_core::{
    BindingResource, BufferBinding, BufferDescriptor, BufferUsage, ClearColor,
    ColorAttachmentDescriptor, CommandEncoderDescriptor, CompareFunction, CompositeAlphaMode,
    DepthAttachmentDescriptor, DepthState, DeviceDescriptor, DrawIndexedStepDescriptor,
    DrawStepDescriptor, Extent2d, Format, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation,
    Origin2d, PipelineLayoutDescriptor, PresentMode, PrimitiveTopology, RenderPassDepthAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, RenderStepDescriptor, RenderStepList,
    ResourceBinding, ResourceBindingType, ResourceSetDescriptor, ResourceSetLayoutDescriptor,
    ResourceSetLayoutEntry, SamplerBinding, SamplerDescriptor, ShaderModuleDescriptor, ShaderStage,
    ShaderStages, TextureBinding, TextureDataLayout, TextureDescriptor, TextureDimension,
    TextureUsage, TextureViewDescriptor, TextureWriteDescriptor, resource_set_list,
};
use gfx_core::{
    PipelineDevice as _, PresentationDevice as _, ResourceDevice as _, SubmissionDevice as _,
    SurfaceDevice as _, TextureTransferDevice as _,
};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

const CLEAR: ClearColor = ClearColor {
    red: 0.0,
    green: 0.0,
    blue: 0.0,
    alpha: 0.0,
};
const WGSL: &str = r#"
struct Params { color: vec4<f32>, depth: vec4<f32> };
@group(0) @binding(119) var<storage, read> vertices: array<vec4<f32>>;
@group(0) @binding(120) var<uniform> params: Params;
@vertex fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> @builtin(position) vec4<f32> {
    if i != 2u { return vec4<f32>(4.0, 4.0, 0.0, 1.0); }
    return vec4<f32>(vertices[v - 3u].xy, params.depth.x, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return params.color; }
"#;

fn shader(
    device: &mut OpenGlDevice,
    source: &str,
    stage: ShaderStage,
    entry: &str,
) -> gfx_core::ShaderModuleId {
    let binary = gfx_shader::WgslModule::parse(source)
        .expect("test WGSL")
        .compile_glsl(stage, entry)
        .expect("GLSL 4.50 translation");
    device
        .create_shader_module(&ShaderModuleDescriptor {
            label: None,
            binary,
        })
        .expect("native GLSL driver compiler")
}

fn open_device() -> OpenGlDevice {
    let bootstrap = TestWindow::new();
    // SAFETY: bootstrap is live during initialization; WGL owns a separate lifetime HWND.
    unsafe {
        OpenGlDevice::new(
            &DeviceDescriptor::default(),
            raw_window_handle::RawDisplayHandle::Windows(
                raw_window_handle::WindowsDisplayHandle::new(),
            ),
            bootstrap.window_handle().expect("HWND").as_raw(),
        )
    }
    .expect("native OpenGL 4.5 device")
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn buffer(device: &mut OpenGlDevice, size: u64, usage: BufferUsage) -> gfx_core::BufferId {
    device
        .create_buffer(&BufferDescriptor {
            label: None,
            size,
            usage,
            memory_location: MemoryLocation::CpuToGpu,
        })
        .expect("buffer")
}
fn texture(
    device: &mut OpenGlDevice,
    format: Format,
    usage: TextureUsage,
    levels: u32,
) -> (gfx_core::TextureId, gfx_core::TextureViewId) {
    let texture = device
        .create_texture(&TextureDescriptor {
            label: None,
            size: Extent2d::new(16, 16).expect("extent"),
            mip_level_count: levels,
            format,
            usage,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("texture");
    let view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture,
            format,
            base_mip_level: 0,
            mip_level_count: 1,
        })
        .expect("view");
    (texture, view)
}
fn pass(device: &mut OpenGlDevice, depth: bool) -> gfx_core::RenderPassId {
    device
        .create_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachment: ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            },
            depth_attachment: depth.then_some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
            }),
        })
        .expect("pass")
}
fn pipeline(
    device: &mut OpenGlDevice,
    source: &str,
    pass: gfx_core::RenderPassId,
    layout: gfx_core::PipelineLayoutId,
    blend: gfx_core::BlendMode,
    depth: bool,
) -> gfx_core::RenderPipelineId {
    let vs = shader(device, source, ShaderStage::Vertex, "vs");
    let fs = shader(device, source, ShaderStage::Fragment, "fs");
    device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: None,
                vertex_shader: vs,
                vertex_entry_point: "vs".into(),
                fragment_shader: fs,
                fragment_entry_point: "fs".into(),
                vertex_buffers: Vec::new(),
                render_pass: pass,
                pipeline_layout: Some(layout),
                color_format: Format::Rgba8Unorm,
                blend_mode: blend,
                primitive_topology: PrimitiveTopology::TriangleList,
                depth_state: depth.then_some(DepthState {
                    compare: CompareFunction::Less,
                    write_enabled: true,
                }),
            },
            Extent2d::new(16, 16).expect("extent"),
        )
        .expect("pipeline")
}

struct TestWindow(HWND);
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    // SAFETY: forward the unchanged native window message to the standard procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
impl TestWindow {
    fn new() -> Self {
        // SAFETY: native test window uses the standard procedure and owns its HWND until Drop.
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).expect("module").into();
            let class = WNDCLASSW {
                hInstance: instance,
                lpfnWndProc: Some(window_proc),
                lpszClassName: w!("NovaOpenGlHardwareGate"),
                ..Default::default()
            };
            RegisterClassW(&class);
            Self(
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP,
                    class.lpszClassName,
                    w!("Nova OpenGL gate"),
                    WS_POPUP,
                    0,
                    0,
                    64,
                    64,
                    None,
                    None,
                    Some(instance),
                    None,
                )
                .expect("test HWND"),
            )
        }
    }
}
impl HasWindowHandle for TestWindow {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(
            std::num::NonZeroIsize::new(self.0.0 as isize).expect("live HWND"),
        );
        // SAFETY: window is owned by self and outlives this borrowed handle.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}
impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: this test owns the window, and surface resources have already dropped.
        unsafe {
            DestroyWindow(self.0).expect("destroy test HWND");
        }
    }
}

mod draw;
mod surface;
mod texture;
