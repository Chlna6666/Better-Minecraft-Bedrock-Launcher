//! Hardware gates: run explicitly with `cargo test -p gfx-opengl -- --ignored`.

use crate::OpenGlDevice;
use gfx_core::{
    BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
    BindingResource, BindingType, BufferBinding, BufferDescriptor, BufferUsage, ClearColor,
    ColorAttachmentDescriptor, ColorTargetState, CommandEncoderDescriptor, CompareFunction,
    CompositeAlphaMode, DepthAttachment, DepthAttachmentDescriptor, DepthState, DeviceDescriptor,
    DrawCommand, DrawIndexedCommand, Extent2d, Format, IndexBufferBinding, IndexFormat, LoadOp,
    MemoryLocation, Origin2d, PipelineLayoutDescriptor, PresentFrameDescriptor, PresentMode,
    PrimitiveTopology, RenderCommand, RenderPassCommands, RenderPassDescriptor,
    RenderPipelineDescriptor, SamplerBinding, SamplerDescriptor, ShaderModuleDescriptor,
    ShaderStage, ShaderStages, TextureBinding, TextureDataLayout, TextureDescriptor,
    TextureDimension, TextureUsage, TextureViewDescriptor, TextureWriteDescriptor, bind_group_list,
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
            sample_count: 1,
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
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: depth.then_some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,

                stencil_format: None,
            }),

            sample_count: 1,
            resolve_target: None,
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
                color_targets: vec![ColorTargetState::new(Format::Rgba8Unorm, blend)],
                primitive_topology: PrimitiveTopology::TriangleList,
                raster: Default::default(),
                depth_state: depth.then_some(DepthState {
                    compare: CompareFunction::Less,
                    write_enabled: true,

                    stencil: None,
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

#[test]
#[ignore = "requires OpenGL 4.5 hardware"]
fn conformance_msaa_resolve() {
    let mut device = open_device();
    gfx_core::conformance::msaa_resolve_roundtrip(&mut device).expect("MSAA resolve roundtrip");
}

#[test]
#[ignore = "requires OpenGL 4.5 hardware"]
fn conformance_format_allocation() {
    let mut device = open_device();
    gfx_core::conformance::format_allocation_roundtrip(&mut device)
        .expect("format allocation roundtrip");
}

#[test]
#[ignore = "requires OpenGL 4.5 hardware"]
fn conformance_texture_buffer() {
    let mut device = open_device();
    gfx_core::conformance::texture_buffer_roundtrip(&mut device).expect("texture buffer roundtrip");
}

#[test]
#[ignore = "requires OpenGL 4.5 hardware"]
fn storage_image_writes_from_fragment_shader() {
    let mut device = open_device();
    let (storage, storage_view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::STORAGE | TextureUsage::COPY_SRC,
        1,
    );
    let (color, color_view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        1,
    );
    let pass = pass(&mut device, false);
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageTexture {
                    format: Format::Rgba8Unorm,
                    read_only: false,
                },
                stages: ShaderStages::FRAGMENT,
            }],
        })
        .expect("layout");
    let pipeline_layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: vec![layout],
        })
        .expect("pipeline layout");
    let source = "@group(0) @binding(0) var dst: texture_storage_2d<rgba8unorm, write>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
@fragment fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    textureStore(dst, vec2<i32>(at.xy), vec4<f32>(0.25, 0.5, 0.75, 1.0));
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}";
    let pipeline = pipeline(
        &mut device,
        source,
        pass,
        pipeline_layout,
        gfx_core::BlendMode::Replace,
        false,
    );
    let bind_group = device
        .create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: storage_view,
                }),
            }],
        })
        .expect("bind group");
    let commands = [RenderCommand::Draw(DrawCommand {
        pipeline,
        bind_groups: bind_group_list([bind_group]),
        vertex_count: 3,
        first_vertex: 0,
        instance_count: 1,
        first_instance: 0,
        scissor: None,
    })];
    device
        .render_to_textures(&[RenderPassCommands {
            color_attachments: std::slice::from_ref(&color_view),
            render_pass: pass,
            commands: &commands,
            color_load_op: LoadOp::Clear(ClearColor::default()),
            clear_region: None,
            depth_attachment: None,
            viewport: None,
            resolve_target: None,
        }])
        .expect("storage image pass");
    let pixels = device.read_texture(storage).expect("readback");
    let at = (4 * 16 + 4) * 4;
    assert_eq!(
        &pixels.bytes[at..at + 4],
        &[64, 128, 191, 255],
        "fragment shader must write through the image unit"
    );
    let _ = color;
}

#[test]
#[ignore = "requires OpenGL 4.5 hardware"]
fn compute_writes_storage_image_through_bind_group() {
    let mut device = open_device();
    let (storage, storage_view) = texture(
        &mut device,
        Format::Rgba8Unorm,
        TextureUsage::STORAGE | TextureUsage::COPY_SRC,
        1,
    );
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::StorageTexture {
                    format: Format::Rgba8Unorm,
                    read_only: false,
                },
                stages: ShaderStages::COMPUTE,
            }],
        })
        .expect("layout");
    let source = "@group(0) @binding(0) var dst: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(1) fn cs(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(0.25, 0.5, 0.75, 1.0));
}";
    let shader = shader(&mut device, source, ShaderStage::Compute, "cs");
    let bind_group = device
        .create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::Texture(TextureBinding {
                    texture_view: storage_view,
                }),
            }],
        })
        .expect("bind group");
    device
        .dispatch_compute_bind_group(&gfx_core::ComputeBindGroupDispatch {
            shader,
            bind_group,
            workgroups: [16, 16, 1],
        })
        .expect("compute dispatch");
    let pixels = device.read_texture(storage).expect("readback");
    let at = (8 * 16 + 8) * 4;
    assert_eq!(
        &pixels.bytes[at..at + 4],
        &[64, 128, 191, 255],
        "compute shader must write through the storage image binding"
    );
}

#[test]
#[ignore = "requires desktop OpenGL 4.5 compute-capable driver"]
fn compute_uav_to_srv_to_storage_image_roundtrip() {
    let mut device = open_device();
    let writer = gfx_shader::WgslModule::parse(
        gfx_core::conformance::COMPUTE_RESOURCE_SET_WRITER_WGSL,
    )
    .expect("writer WGSL")
    .compile_glsl(ShaderStage::Compute, "write_data")
    .expect("writer GLSL");
    let reader = gfx_shader::WgslModule::parse(
        gfx_core::conformance::COMPUTE_RESOURCE_SET_READER_WGSL,
    )
    .expect("reader WGSL")
    .compile_glsl(ShaderStage::Compute, "write_image")
    .expect("reader GLSL");
    gfx_core::conformance::compute_resource_set_roundtrip(&mut device, writer, reader)
        .expect("native GL shader-storage/image visibility and readback");
}
