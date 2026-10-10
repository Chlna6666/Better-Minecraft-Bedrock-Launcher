use super::*;
use gfx_core::{DeviceDescriptor, ShaderBinary, ShaderCode, ShaderStage};
use serde_json::Value;

pub(crate) fn run_backend(options: &Options) -> Result<Value> {
    let descriptor = DeviceDescriptor {
        adapter_name: options.adapter.clone(),
        ..DeviceDescriptor::default()
    };
    match options.backend {
        #[cfg(all(windows, feature = "nova-gfx-dx11"))]
        BackendKind::Dx11 => {
            let mut device = gfx_dx11::Dx11Device::new(&descriptor)?;
            let name = device.adapter_name().to_owned();
            workload::run(&mut device, &name, options)
        }
        #[cfg(all(windows, feature = "nova-gfx-dx12"))]
        BackendKind::Dx12 => {
            let mut device = gfx_dx12::Dx12Device::new(&descriptor)?;
            let name = device.adapter_name().to_owned();
            workload::run(&mut device, &name, options)
        }
        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(windows, target_os = "linux", target_os = "freebsd")
        ))]
        BackendKind::Vulkan => {
            let mut device = gfx_vulkan::VulkanDevice::new(&descriptor)?;
            let name = device.adapter_name().to_owned();
            workload::run(&mut device, &name, options)
        }
        #[cfg(all(feature = "nova-gfx-opengl", any(windows, target_os = "linux")))]
        BackendKind::OpenGl => {
            use winit::raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};
            let event_loop = winit::event_loop::EventLoop::new()?;
            // This hidden bootstrap supplies a native GL context; no UI/event loop is measured.
            #[allow(deprecated)]
            let window = event_loop
                .create_window(winit::window::Window::default_attributes().with_visible(false))?;
            // SAFETY: the bootstrap window and display outlive the device and all owner-thread work.
            let mut device = unsafe {
                gfx_opengl::OpenGlDevice::new(
                    &descriptor,
                    window.display_handle()?.as_raw(),
                    window.window_handle()?.as_raw(),
                )
            }?;
            let name = device.adapter_name().to_owned();
            workload::run(&mut device, &name, options)
        }
        #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
        BackendKind::Metal => {
            use gfx_core::DiagnosticsDevice as _;
            let device = gfx_metal::MetalDevice::new(&descriptor)?;
            Ok(
                serde_json::json!({"schema": 1, "backend": "Metal", "status": "unsupported",
                "memory_architecture": format!("{:?}", device.memory_architecture()?),
                "reason": "Nova Metal native draw and Private Buffer upload are not implemented; no timing is reported"}),
            )
        }
        _ => bail!("requested backend is not enabled on this platform"),
    }
}

pub(super) fn shader(
    source: &str,
    backend: BackendKind,
    stage: ShaderStage,
    entry: &str,
) -> Result<ShaderBinary> {
    let mut binary = gfx_shader::compile_wgsl_for_backend(source, backend, stage, entry)?;
    #[cfg(windows)]
    if let ShaderCode::Hlsl(source) = &binary.code {
        binary.code = ShaderCode::DxBytecode(compile_fxc(source, stage, entry, backend)?);
    }
    Ok(binary)
}

#[cfg(windows)]
fn compile_fxc(
    source: &str,
    stage: ShaderStage,
    entry: &str,
    backend: BackendKind,
) -> Result<Vec<u8>> {
    use windows::{
        Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCompile},
        core::PCSTR,
    };
    let entry = std::ffi::CString::new(entry)?;
    let profile = match (stage, backend) {
        (ShaderStage::Vertex, BackendKind::Dx11) => b"vs_5_0\0",
        (ShaderStage::Fragment, BackendKind::Dx11) => b"ps_5_0\0",
        (ShaderStage::Vertex, _) => b"vs_5_1\0",
        (ShaderStage::Fragment, _) => b"ps_5_1\0",
    };
    let mut code = None;
    let mut errors = None;
    // SAFETY: source, entry and profile storage stay live throughout the synchronous compiler call.
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(profile.as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        let message = errors.as_ref().map(|blob| {
            // SAFETY: the live compiler blob owns the immutable diagnostic bytes.
            unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    blob.GetBufferPointer().cast(),
                    blob.GetBufferSize(),
                ))
                .into_owned()
            }
        });
        bail!("FXC: {error}: {message:?}");
    }
    let code = code.ok_or_else(|| anyhow::anyhow!("FXC returned no bytecode"))?;
    // SAFETY: copy bytes while their native compiler blob is still live.
    Ok(unsafe {
        std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()).to_vec()
    })
}
