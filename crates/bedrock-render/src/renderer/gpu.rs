#![allow(
    clippy::borrow_as_ptr,
    clippy::map_unwrap_or,
    clippy::needless_raw_string_hashes,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

use super::pipeline::{RenderGpuBackend, RenderGpuDiagnostics, RenderGpuFallbackPolicy};
use crate::error::{BedrockRenderError, Result};

#[cfg(feature = "gpu-core")]
mod imp {
    use super::*;
    use std::sync::Arc;

    #[cfg(feature = "gpu-dx11")]
    #[allow(unsafe_code)]
    mod dx11 {
        use super::*;
        use std::sync::Mutex;
        use std::time::Instant;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCompile};
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D11_SRV_DIMENSION_BUFFER,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS, D3D11_BUFFER_DESC,
            D3D11_BUFFER_SRV, D3D11_BUFFER_SRV_0, D3D11_BUFFER_SRV_1, D3D11_CPU_ACCESS_READ,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_RESOURCE_MISC_BUFFER_STRUCTURED, D3D11_SDK_VERSION,
            D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SUBRESOURCE_DATA, D3D11_UAV_DIMENSION_BUFFER,
            D3D11_UNORDERED_ACCESS_VIEW_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
            D3D11CreateDevice, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device,
            ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11UnorderedAccessView,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
        use windows::Win32::Graphics::Dxgi::{DXGI_ADAPTER_DESC, IDXGIAdapter, IDXGIDevice};
        use windows::core::{Interface as _, PCSTR};

        const COPY_SHADER: &[u8] = br#"
RWStructuredBuffer<uint> output_pixels : register(u0);
StructuredBuffer<uint> input_pixels : register(t0);

[numthreads(256, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint count;
    uint stride;
    output_pixels.GetDimensions(count, stride);
    if (id.x >= count) {
        return;
    }
    output_pixels[id.x] = input_pixels[id.x];
}
"#;

        pub struct Dx11RenderContext {
            device: ID3D11Device,
            context: ID3D11DeviceContext,
            shader: ID3D11ComputeShader,
            adapter_name: String,
            lock: Mutex<()>,
        }

        impl Dx11RenderContext {
            pub fn new() -> Result<Self> {
                let mut device = None;
                let mut context = None;
                let mut feature_level = D3D_FEATURE_LEVEL_11_0;
                // SAFETY: All out-pointers are valid for the duration of the call and
                // are initialized by D3D11CreateDevice on success.
                unsafe {
                    D3D11CreateDevice(
                        None,
                        D3D_DRIVER_TYPE_HARDWARE,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        Some(&[D3D_FEATURE_LEVEL_11_0]),
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        Some(&mut feature_level),
                        Some(&mut context),
                    )
                }
                .map_err(|error| {
                    BedrockRenderError::Validation(format!("DX11 device unavailable: {error}"))
                })?;
                let device = device.ok_or_else(|| {
                    BedrockRenderError::Validation("DX11 device was not created".to_string())
                })?;
                let context = context.ok_or_else(|| {
                    BedrockRenderError::Validation("DX11 context was not created".to_string())
                })?;
                let adapter_name =
                    dx11_adapter_name(&device).unwrap_or_else(|| "Direct3D 11".to_string());
                let shader_blob = compile_copy_shader()?;
                let mut shader = None;
                let shader_bytecode = unsafe {
                    // SAFETY: shader_blob is live, and the compiler reports a byte
                    // pointer/length pair that remains valid for the blob lifetime.
                    std::slice::from_raw_parts(
                        shader_blob.GetBufferPointer().cast::<u8>(),
                        shader_blob.GetBufferSize(),
                    )
                };
                // SAFETY: shader bytecode slice comes from a live ID3DBlob, and the
                // output pointer is valid.
                unsafe { device.CreateComputeShader(shader_bytecode, None, Some(&mut shader)) }
                    .map_err(|error| {
                        BedrockRenderError::Validation(format!(
                            "DX11 compute shader failed: {error}"
                        ))
                    })?;
                let shader = shader.ok_or_else(|| {
                    BedrockRenderError::Validation(
                        "DX11 compute shader was not created".to_string(),
                    )
                })?;
                log::info!(
                    target: "bedrock_render::gpu",
                    "GPU backend initialized backend=dx11 adapter={adapter_name}"
                );
                Ok(Self {
                    device,
                    context,
                    shader,
                    adapter_name,
                    lock: Mutex::new(()),
                })
            }

            pub fn process_rgba(
                &self,
                rgba: &[u8],
                requested_backend: RenderGpuBackend,
            ) -> Result<GpuProcessResult> {
                let _guard = self.lock.lock().map_err(|_| {
                    BedrockRenderError::Validation("DX11 context lock poisoned".to_string())
                })?;
                let pixel_count = validate_rgba_pixels(rgba)?;
                let started = Instant::now();
                let input = self.create_upload_buffer(rgba, pixel_count)?;
                let srv = self.create_srv(&input, pixel_count)?;
                let output = self.create_storage_buffer(rgba.len(), pixel_count)?;
                let readback = self.create_readback_buffer(rgba.len(), pixel_count)?;
                let uav = self.create_uav(&output, pixel_count)?;
                let upload_ms = started.elapsed().as_millis();
                let dispatch_started = Instant::now();
                let srvs = [Some(srv)];
                let uavs = [Some(uav)];
                let empty_srvs: [Option<ID3D11ShaderResourceView>; 1] = [None];
                let empty_uavs: [Option<ID3D11UnorderedAccessView>; 1] = [None];
                // SAFETY: All COM objects are live for the duration of dispatch, and
                // buffer/SRV/UAV bindings match the shader resource declarations.
                unsafe {
                    self.context.CSSetShader(Some(&self.shader), None);
                    self.context.CSSetShaderResources(0, Some(&srvs));
                    self.context
                        .CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
                    self.context.Dispatch(
                        u32::try_from(pixel_count.div_ceil(256)).map_err(|_| {
                            BedrockRenderError::Validation("DX11 dispatch overflow".to_string())
                        })?,
                        1,
                        1,
                    );
                    self.context.CSSetShader(None, None);
                    self.context.CSSetShaderResources(0, Some(&empty_srvs));
                    self.context
                        .CSSetUnorderedAccessViews(0, 1, Some(empty_uavs.as_ptr()), None);
                    self.context.CopyResource(&readback, &output);
                }
                let dispatch_ms = dispatch_started.elapsed().as_millis();
                let readback_started = Instant::now();
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                // SAFETY: readback is a staging buffer created with CPU read access,
                // and mapped is a valid out-parameter. The buffer is unmapped exactly once.
                unsafe {
                    self.context
                        .Map(&readback, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                }
                .map_err(|error| {
                    BedrockRenderError::Validation(format!("DX11 readback map failed: {error}"))
                })?;
                let bytes = unsafe {
                    // SAFETY: D3D11 Map returned a valid pointer to at least rgba.len()
                    // bytes because readback was created with that exact ByteWidth.
                    std::slice::from_raw_parts(mapped.pData.cast::<u8>(), rgba.len())
                }
                .to_vec();
                // SAFETY: readback is currently mapped by this context.
                unsafe { self.context.Unmap(&readback, 0) };
                Ok(GpuProcessResult {
                    rgba: bytes,
                    diagnostics: RenderGpuDiagnostics {
                        requested_backend,
                        actual_backend: RenderGpuBackend::Dx11,
                        adapter_name: Some(self.adapter_name.clone()),
                        device_name: Some("Direct3D 11 compute".to_string()),
                        tiles: 1,
                        upload_ms,
                        dispatch_ms,
                        readback_ms: readback_started.elapsed().as_millis(),
                        uploaded_bytes: rgba.len(),
                        readback_bytes: rgba.len(),
                        peak_in_flight: 1,
                        ..RenderGpuDiagnostics::default()
                    },
                })
            }

            fn create_upload_buffer(
                &self,
                rgba: &[u8],
                pixel_count: usize,
            ) -> Result<ID3D11Buffer> {
                let byte_width = u32::try_from(rgba.len()).map_err(|_| {
                    BedrockRenderError::Validation("DX11 upload buffer too large".to_string())
                })?;
                let desc = structured_buffer_desc(
                    byte_width,
                    pixel_count,
                    D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    D3D11_USAGE_DEFAULT,
                    0,
                )?;
                let data = D3D11_SUBRESOURCE_DATA {
                    pSysMem: rgba.as_ptr().cast(),
                    SysMemPitch: 0,
                    SysMemSlicePitch: 0,
                };
                let mut buffer = None;
                // SAFETY: desc and data point to initialized values for the call.
                unsafe {
                    self.device
                        .CreateBuffer(&desc, Some(&data), Some(&mut buffer))
                }
                .map_err(|error| {
                    BedrockRenderError::Validation(format!("DX11 upload buffer failed: {error}"))
                })?;
                buffer.ok_or_else(|| {
                    BedrockRenderError::Validation("DX11 upload buffer was not created".to_string())
                })
            }

            fn create_storage_buffer(
                &self,
                byte_len: usize,
                pixel_count: usize,
            ) -> Result<ID3D11Buffer> {
                let byte_width = u32::try_from(byte_len).map_err(|_| {
                    BedrockRenderError::Validation("DX11 storage buffer too large".to_string())
                })?;
                let desc = structured_buffer_desc(
                    byte_width,
                    pixel_count,
                    D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                    D3D11_USAGE_DEFAULT,
                    0,
                )?;
                let mut buffer = None;
                // SAFETY: desc is initialized and output pointer is valid.
                unsafe { self.device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(
                    |error| {
                        BedrockRenderError::Validation(format!(
                            "DX11 storage buffer failed: {error}"
                        ))
                    },
                )?;
                buffer.ok_or_else(|| {
                    BedrockRenderError::Validation(
                        "DX11 storage buffer was not created".to_string(),
                    )
                })
            }

            fn create_readback_buffer(
                &self,
                byte_len: usize,
                pixel_count: usize,
            ) -> Result<ID3D11Buffer> {
                let byte_width = u32::try_from(byte_len).map_err(|_| {
                    BedrockRenderError::Validation("DX11 readback buffer too large".to_string())
                })?;
                let desc =
                    structured_buffer_desc(byte_width, pixel_count, 0, D3D11_USAGE_STAGING, 0)
                        .map(|mut desc| {
                            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                            desc.MiscFlags = 0;
                            desc
                        })?;
                let mut buffer = None;
                // SAFETY: desc is initialized and output pointer is valid.
                unsafe { self.device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(
                    |error| {
                        BedrockRenderError::Validation(format!(
                            "DX11 readback buffer failed: {error}"
                        ))
                    },
                )?;
                buffer.ok_or_else(|| {
                    BedrockRenderError::Validation(
                        "DX11 readback buffer was not created".to_string(),
                    )
                })
            }

            fn create_uav(
                &self,
                buffer: &ID3D11Buffer,
                pixel_count: usize,
            ) -> Result<ID3D11UnorderedAccessView> {
                let desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_UNKNOWN,
                    ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
                    Anonymous:
                        windows::Win32::Graphics::Direct3D11::D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                            Buffer: windows::Win32::Graphics::Direct3D11::D3D11_BUFFER_UAV {
                                FirstElement: 0,
                                NumElements: u32::try_from(pixel_count).map_err(|_| {
                                    BedrockRenderError::Validation(
                                        "DX11 UAV element count overflow".to_string(),
                                    )
                                })?,
                                Flags: 0,
                            },
                        },
                };
                let mut uav = None;
                // SAFETY: buffer and descriptor are valid, and output pointer is valid.
                unsafe {
                    self.device
                        .CreateUnorderedAccessView(buffer, Some(&desc), Some(&mut uav))
                }
                .map_err(|error| {
                    BedrockRenderError::Validation(format!("DX11 UAV failed: {error}"))
                })?;
                uav.ok_or_else(|| {
                    BedrockRenderError::Validation("DX11 UAV was not created".to_string())
                })
            }

            fn create_srv(
                &self,
                buffer: &ID3D11Buffer,
                pixel_count: usize,
            ) -> Result<ID3D11ShaderResourceView> {
                let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_UNKNOWN,
                    ViewDimension: D3D11_SRV_DIMENSION_BUFFER,
                    Anonymous:
                        windows::Win32::Graphics::Direct3D11::D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                            Buffer: D3D11_BUFFER_SRV {
                                Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 },
                                Anonymous2: D3D11_BUFFER_SRV_1 {
                                    NumElements: u32::try_from(pixel_count).map_err(|_| {
                                        BedrockRenderError::Validation(
                                            "DX11 SRV element count overflow".to_string(),
                                        )
                                    })?,
                                },
                            },
                        },
                };
                let mut srv = None;
                // SAFETY: buffer and descriptor are valid, and output pointer is valid.
                unsafe {
                    self.device
                        .CreateShaderResourceView(buffer, Some(&desc), Some(&mut srv))
                }
                .map_err(|error| {
                    BedrockRenderError::Validation(format!("DX11 SRV failed: {error}"))
                })?;
                srv.ok_or_else(|| {
                    BedrockRenderError::Validation("DX11 SRV was not created".to_string())
                })
            }
        }

        fn dx11_adapter_name(device: &ID3D11Device) -> Option<String> {
            // SAFETY: The D3D11 device is live, COM casts are checked by QueryInterface,
            // and GetDesc returns an initialized adapter descriptor on success.
            let dxgi_device: IDXGIDevice = device.cast().ok()?;
            let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter().ok()? };
            let desc: DXGI_ADAPTER_DESC = unsafe { adapter.GetDesc().ok()? };
            let len = desc
                .Description
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len])
                .trim()
                .to_string();
            (!name.is_empty()).then_some(name)
        }

        fn structured_buffer_desc(
            byte_width: u32,
            pixel_count: usize,
            bind_flags: u32,
            usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
            cpu_access_flags: u32,
        ) -> Result<D3D11_BUFFER_DESC> {
            if pixel_count > u32::MAX as usize {
                return Err(BedrockRenderError::Validation(
                    "DX11 structured buffer element count overflow".to_string(),
                ));
            }
            Ok(D3D11_BUFFER_DESC {
                ByteWidth: byte_width,
                Usage: usage,
                BindFlags: bind_flags,
                CPUAccessFlags: cpu_access_flags,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: 4,
            })
        }

        fn compile_copy_shader() -> Result<windows::Win32::Graphics::Direct3D::ID3DBlob> {
            let mut shader = None;
            let mut errors = None;
            let entry = b"main\0";
            let target = b"cs_5_0\0";
            let source_name = b"bedrock-render-dx11-copy.hlsl\0";
            // SAFETY: pointers reference static byte strings for the duration of the
            // call; output blob pointers are valid and initialized by D3DCompile.
            let result = unsafe {
                D3DCompile(
                    COPY_SHADER.as_ptr().cast(),
                    COPY_SHADER.len(),
                    PCSTR(source_name.as_ptr()),
                    None,
                    None,
                    PCSTR(entry.as_ptr()),
                    PCSTR(target.as_ptr()),
                    D3DCOMPILE_ENABLE_STRICTNESS,
                    0,
                    &mut shader,
                    Some(&mut errors),
                )
            };
            if let Err(error) = result {
                let message = errors
                    .as_ref()
                    .map(|blob| unsafe {
                        // SAFETY: compiler error blob is valid UTF-8-ish bytes owned
                        // by the blob; lossy conversion handles non-UTF8 diagnostics.
                        let bytes = std::slice::from_raw_parts(
                            blob.GetBufferPointer().cast::<u8>(),
                            blob.GetBufferSize(),
                        );
                        String::from_utf8_lossy(bytes).into_owned()
                    })
                    .unwrap_or_else(|| error.to_string());
                return Err(BedrockRenderError::Validation(format!(
                    "DX11 shader compile failed: {message}"
                )));
            }
            shader.ok_or_else(|| {
                BedrockRenderError::Validation("DX11 shader compiler returned no blob".to_string())
            })
        }
    }

    #[cfg(feature = "gpu-vulkan")]
    mod wgpu_vk {
        use super::*;
        use std::sync::Mutex;
        use std::time::Instant;
        use wgpu::util::DeviceExt as _;

        const COPY_SHADER: &str = r#"
@group(0) @binding(0)
var<storage, read> input_pixels: array<u32>;

@group(0) @binding(1)
var<storage, read_write> output_pixels: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= arrayLength(&input_pixels)) {
        return;
    }
    output_pixels[index] = input_pixels[index];
}
"#;

        pub struct WgpuVulkanContext {
            device: wgpu::Device,
            queue: wgpu::Queue,
            adapter_info: wgpu::AdapterInfo,
            bind_group_layout: wgpu::BindGroupLayout,
            pipeline: wgpu::ComputePipeline,
            lock: Mutex<()>,
        }

        impl WgpuVulkanContext {
            pub fn new() -> Result<Self> {
                let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
                descriptor.backends = wgpu::Backends::VULKAN;
                let instance = wgpu::Instance::new(descriptor);
                let adapter =
                    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: None,
                        force_fallback_adapter: false,
                    }))
                    .map_err(|error| {
                        BedrockRenderError::Validation(format!(
                            "Vulkan adapter unavailable: {error}"
                        ))
                    })?;
                let adapter_info = adapter.get_info();
                let (device, queue) =
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some("bedrock-render vulkan device"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_defaults(),
                        experimental_features: wgpu::ExperimentalFeatures::disabled(),
                        memory_hints: wgpu::MemoryHints::Performance,
                        trace: wgpu::Trace::Off,
                    }))
                    .map_err(|error| {
                        BedrockRenderError::Validation(format!(
                            "Vulkan device unavailable: {error}"
                        ))
                    })?;
                let bind_group_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("bedrock-render vulkan copy bind group layout"),
                        entries: &[
                            wgpu::BindGroupLayoutEntry {
                                binding: 0,
                                visibility: wgpu::ShaderStages::COMPUTE,
                                ty: wgpu::BindingType::Buffer {
                                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                                    has_dynamic_offset: false,
                                    min_binding_size: None,
                                },
                                count: None,
                            },
                            wgpu::BindGroupLayoutEntry {
                                binding: 1,
                                visibility: wgpu::ShaderStages::COMPUTE,
                                ty: wgpu::BindingType::Buffer {
                                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                                    has_dynamic_offset: false,
                                    min_binding_size: None,
                                },
                                count: None,
                            },
                        ],
                    });
                let pipeline_layout =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("bedrock-render vulkan copy pipeline layout"),
                        bind_group_layouts: &[Some(&bind_group_layout)],
                        immediate_size: 0,
                    });
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("bedrock-render vulkan copy shader"),
                    source: wgpu::ShaderSource::Wgsl(COPY_SHADER.into()),
                });
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("bedrock-render vulkan copy pipeline"),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });
                log::info!(
                    target: "bedrock_render::gpu",
                    "GPU backend initialized backend=vulkan adapter={} device_type={:?}",
                    adapter_info.name,
                    adapter_info.device_type
                );
                Ok(Self {
                    device,
                    queue,
                    adapter_info,
                    bind_group_layout,
                    pipeline,
                    lock: Mutex::new(()),
                })
            }

            pub fn process_rgba(
                &self,
                rgba: &[u8],
                requested_backend: RenderGpuBackend,
            ) -> Result<GpuProcessResult> {
                let pixel_count = validate_rgba_pixels(rgba)?;
                let _guard = self.lock.lock().map_err(|_| {
                    BedrockRenderError::Validation("Vulkan context lock poisoned".to_string())
                })?;
                let upload_started = Instant::now();
                let byte_len = u64::try_from(rgba.len()).map_err(|_| {
                    BedrockRenderError::Validation("RGBA buffer is too large".to_string())
                })?;
                let input = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("bedrock-render vulkan copy input"),
                        contents: rgba,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
                let output = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("bedrock-render vulkan copy output"),
                    size: byte_len,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("bedrock-render vulkan copy readback"),
                    size: byte_len,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bedrock-render vulkan copy bind group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: input.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: output.as_entire_binding(),
                        },
                    ],
                });
                let upload_ms = upload_started.elapsed().as_millis();
                let dispatch_started = Instant::now();
                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("bedrock-render vulkan copy encoder"),
                        });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("bedrock-render vulkan copy pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    let workgroups = u32::try_from(pixel_count.div_ceil(256)).map_err(|_| {
                        BedrockRenderError::Validation(
                            "Vulkan workgroup count overflow".to_string(),
                        )
                    })?;
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
                encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, byte_len);
                self.queue.submit(Some(encoder.finish()));
                let dispatch_ms = dispatch_started.elapsed().as_millis();
                let readback_started = Instant::now();
                let slice = readback.slice(..);
                let (sender, receiver) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
                let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
                receiver
                    .recv()
                    .map_err(|_| {
                        BedrockRenderError::Validation("Vulkan readback channel closed".to_string())
                    })?
                    .map_err(|error| {
                        BedrockRenderError::Validation(format!("Vulkan readback failed: {error}"))
                    })?;
                let mapped = slice.get_mapped_range();
                let result = mapped.to_vec();
                drop(mapped);
                readback.unmap();
                Ok(GpuProcessResult {
                    rgba: result,
                    diagnostics: RenderGpuDiagnostics {
                        requested_backend,
                        actual_backend: RenderGpuBackend::Vulkan,
                        adapter_name: Some(self.adapter_info.name.clone()),
                        device_name: Some(self.adapter_info.name.clone()),
                        tiles: 1,
                        upload_ms,
                        dispatch_ms,
                        readback_ms: readback_started.elapsed().as_millis(),
                        uploaded_bytes: rgba.len(),
                        readback_bytes: rgba.len(),
                        peak_in_flight: 1,
                        ..RenderGpuDiagnostics::default()
                    },
                })
            }
        }
    }

    pub struct GpuRenderContext {
        #[cfg(feature = "gpu-dx11")]
        dx11: Option<Arc<dx11::Dx11RenderContext>>,
        #[cfg(feature = "gpu-vulkan")]
        vulkan: Option<Arc<wgpu_vk::WgpuVulkanContext>>,
    }

    impl Clone for GpuRenderContext {
        fn clone(&self) -> Self {
            Self {
                #[cfg(feature = "gpu-dx11")]
                dx11: self.dx11.clone(),
                #[cfg(feature = "gpu-vulkan")]
                vulkan: self.vulkan.clone(),
            }
        }
    }

    impl GpuRenderContext {
        pub fn new(backend: RenderGpuBackend) -> Result<Self> {
            let mut last_error = None;
            #[cfg(feature = "gpu-dx11")]
            let dx11 = if matches!(backend, RenderGpuBackend::Auto | RenderGpuBackend::Dx11) {
                match dx11::Dx11RenderContext::new() {
                    Ok(context) => Some(Arc::new(context)),
                    Err(error) => {
                        last_error = Some(error);
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(not(feature = "gpu-dx11"))]
            let _dx11 = ();

            #[cfg(feature = "gpu-vulkan")]
            let vulkan = if matches!(backend, RenderGpuBackend::Auto | RenderGpuBackend::Vulkan)
                && (backend != RenderGpuBackend::Auto || last_error.is_some())
            {
                match wgpu_vk::WgpuVulkanContext::new() {
                    Ok(context) => Some(Arc::new(context)),
                    Err(error) => {
                        last_error = Some(error);
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(not(feature = "gpu-vulkan"))]
            let _vulkan = ();

            #[cfg(feature = "gpu-dx11")]
            if dx11.is_some() {
                log::info!(
                    target: "bedrock_render::gpu",
                    "GPU render context selected requested_backend={backend:?} actual_backend=dx11 vulkan_fallback_available={}",
                    cfg!(feature = "gpu-vulkan")
                );
                return Ok(Self {
                    dx11,
                    #[cfg(feature = "gpu-vulkan")]
                    vulkan,
                });
            }
            #[cfg(feature = "gpu-vulkan")]
            if vulkan.is_some() {
                log::info!(
                    target: "bedrock_render::gpu",
                    "GPU render context selected requested_backend={backend:?} actual_backend=vulkan"
                );
                return Ok(Self {
                    #[cfg(feature = "gpu-dx11")]
                    dx11,
                    vulkan,
                });
            }
            Err(last_error.unwrap_or_else(|| {
                BedrockRenderError::Validation(
                    "no requested GPU backend was compiled or available".to_string(),
                )
            }))
        }

        pub fn process_rgba(
            &self,
            rgba: &[u8],
            backend: RenderGpuBackend,
            fallback_policy: RenderGpuFallbackPolicy,
        ) -> Result<GpuProcessResult> {
            if rgba.is_empty() {
                return Ok(GpuProcessResult {
                    rgba: Vec::new(),
                    diagnostics: RenderGpuDiagnostics {
                        requested_backend: backend,
                        ..RenderGpuDiagnostics::default()
                    },
                });
            }
            let result = match backend {
                RenderGpuBackend::Auto | RenderGpuBackend::Dx11 => {
                    #[cfg(feature = "gpu-dx11")]
                    if let Some(dx11) = &self.dx11 {
                        dx11.process_rgba(rgba, backend)
                    } else {
                        try_vulkan(self, rgba, backend)
                    }
                    #[cfg(not(feature = "gpu-dx11"))]
                    {
                        try_vulkan(self, rgba, backend)
                    }
                }
                RenderGpuBackend::Vulkan => try_vulkan(self, rgba, backend),
                RenderGpuBackend::Dx12 => Err(BedrockRenderError::Validation(
                    "DX12 is not enabled in this build; use DX11 or Vulkan".to_string(),
                )),
            };
            match result {
                Ok(result) => Ok(result),
                Err(error) if matches!(fallback_policy, RenderGpuFallbackPolicy::AllowCpu) => {
                    log::warn!(
                        target: "bedrock_render::gpu",
                        "GPU processing failed; falling back to CPU requested_backend={backend:?} error={error}"
                    );
                    Ok(GpuProcessResult {
                        rgba: rgba.to_vec(),
                        diagnostics: RenderGpuDiagnostics {
                            requested_backend: backend,
                            fallback_reason: Some(error.to_string()),
                            ..RenderGpuDiagnostics::default()
                        },
                    })
                }
                Err(error) => Err(error),
            }
        }
    }

    #[cfg(feature = "gpu-vulkan")]
    fn try_vulkan(
        context: &GpuRenderContext,
        rgba: &[u8],
        backend: RenderGpuBackend,
    ) -> Result<GpuProcessResult> {
        context.vulkan.as_ref().map_or_else(
            || {
                Err(BedrockRenderError::Validation(
                    "Vulkan backend unavailable".to_string(),
                ))
            },
            |vulkan| vulkan.process_rgba(rgba, backend),
        )
    }

    #[cfg(not(feature = "gpu-vulkan"))]
    fn try_vulkan(
        _context: &GpuRenderContext,
        _rgba: &[u8],
        _backend: RenderGpuBackend,
    ) -> Result<GpuProcessResult> {
        Err(BedrockRenderError::Validation(
            "Vulkan backend is not compiled".to_string(),
        ))
    }

    fn validate_rgba_pixels(rgba: &[u8]) -> Result<usize> {
        let Some(pixel_count) = rgba.len().checked_div(4) else {
            return Err(BedrockRenderError::Validation(
                "RGBA byte length overflow".to_string(),
            ));
        };
        if pixel_count.saturating_mul(4) != rgba.len() {
            return Err(BedrockRenderError::Validation(
                "RGBA byte length must be a multiple of four".to_string(),
            ));
        }
        Ok(pixel_count)
    }
}

#[cfg(not(feature = "gpu-core"))]
mod imp {
    use super::*;

    #[derive(Clone)]
    pub struct GpuRenderContext;

    impl GpuRenderContext {
        pub fn new(_backend: RenderGpuBackend) -> Result<Self> {
            Err(BedrockRenderError::Validation(
                "bedrock-render was built without GPU features".to_string(),
            ))
        }

        pub fn process_rgba(
            &self,
            rgba: &[u8],
            backend: RenderGpuBackend,
            fallback_policy: RenderGpuFallbackPolicy,
        ) -> Result<GpuProcessResult> {
            let _ = self;
            if matches!(fallback_policy, RenderGpuFallbackPolicy::Required) {
                return Err(BedrockRenderError::Validation(
                    "bedrock-render was built without GPU features".to_string(),
                ));
            }
            Ok(GpuProcessResult {
                rgba: rgba.to_vec(),
                diagnostics: RenderGpuDiagnostics {
                    requested_backend: backend,
                    fallback_reason: Some(
                        "bedrock-render was built without GPU features".to_string(),
                    ),
                    ..RenderGpuDiagnostics::default()
                },
            })
        }
    }
}

pub use imp::GpuRenderContext;

pub struct GpuProcessResult {
    pub rgba: Vec<u8>,
    pub diagnostics: RenderGpuDiagnostics,
}
