//! Direct3D 12 backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Direct3D 12 on
//! Windows. Non-Windows builds expose a minimal stub that returns
//! `GfxError::Unavailable`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

#![cfg_attr(
    windows,
    expect(
        unsafe_code,
        reason = "D3D12 FFI requires unsafe calls; each unsafe block documents its safety invariant"
    )
)]
#![cfg_attr(
    windows,
    expect(
        dead_code,
        reason = "backend skeleton stores native resource slots before the full present path is wired"
    )
)]
use gfx_core::{GfxError, Result};

#[cfg(windows)]
mod platform {
    use std::{
        ptr::{self, NonNull},
        str,
        sync::Arc,
        time::{Duration, Instant},
    };

    use super::{GfxError, Result};
    use crate::error::Dx12Error;
    use crate::registry::ResourceRegistry;
    use gfx_core::{
        AdapterInfo, AddressMode, BackendCapabilities, BackendKind, BlendMode, BufferBinding,
        BufferDesc, BufferId, BufferUsage, ClearColor, CommandEncoderDesc, CommandEncoderId,
        CompositeAlphaMode, DeviceDesc, DrawDesc, DrawStepDesc, FilterMode, Format, GfxBackend,
        GfxCommandDevice, GfxDiagnosticsDevice, GfxPipelineDevice, GfxPresentationDevice,
        GfxResourceDevice, GfxSubmissionDevice, GfxSurfaceDevice, GfxTextureTransferDevice,
        GfxThreadingMode, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation,
        PipelineLayoutDesc, PipelineLayoutId, PowerPreference, PresentMode, PrimitiveTopology,
        RenderPassDepthAttachment, RenderPassDesc, RenderPassId, RenderPipelineDesc,
        RenderPipelineId, RenderStepDescriptor, RenderStepList, RenderStepRef, RenderTarget,
        ResourceBindingResource, ResourceBindingType, ResourceSetDesc, ResourceSetId,
        ResourceSetLayoutDesc, ResourceSetLayoutId, ResourceStats, SamplerDesc, SamplerId,
        ScissorRect, ShaderCode, ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages,
        SubmissionId, SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId,
        TextureDesc, TextureDimension, TextureId, TextureReadback, TextureUsage, TextureViewDesc,
        TextureViewId, TextureWrite, TextureWriteDesc, resource_set_list,
    };
    use gfx_memory::{
        DeferredFreeQueue, UploadAllocation, UploadRingAllocator, UploadRingAllocatorDesc,
    };
    use log::log;
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
    use windows::{
        Win32::Graphics::{
            Direct3D::Fxc::D3DCompile,
            Direct3D::{
                D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY,
                D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
                ID3DBlob,
            },
            Direct3D12::{
                D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA,
                D3D12_BLEND_INV_SRC1_COLOR, D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD,
                D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_SRC1_COLOR, D3D12_BLEND_ZERO,
                D3D12_CACHED_PIPELINE_STATE, D3D12_CLEAR_FLAG_DEPTH, D3D12_COLOR_WRITE_ENABLE_ALL,
                D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
                D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_COMPARISON_FUNC,
                D3D12_COMPARISON_FUNC_LESS_EQUAL, D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
                D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CULL_MODE_NONE, D3D12_DEFAULT_DEPTH_BIAS,
                D3D12_DEFAULT_DEPTH_BIAS_CLAMP, D3D12_DEFAULT_SLOPE_SCALED_DEPTH_BIAS,
                D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_WRITE_MASK_ALL, D3D12_DESCRIPTOR_HEAP_DESC,
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
                D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_TYPE_CBV,
                D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER, D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                D3D12_FENCE_FLAG_NONE, D3D12_FILL_MODE_SOLID, D3D12_FILTER_MIN_MAG_MIP_LINEAR,
                D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_GPU_DESCRIPTOR_HANDLE,
                D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
                D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK, D3D12_HEAP_TYPE_UPLOAD,
                D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED, D3D12_INDEX_BUFFER_VIEW,
                D3D12_INPUT_LAYOUT_DESC, D3D12_LOGIC_OP_NOOP, D3D12_MESSAGE,
                D3D12_PIPELINE_STATE_FLAG_NONE, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                D3D12_QUERY_HEAP_DESC, D3D12_QUERY_HEAP_TYPE_TIMESTAMP, D3D12_QUERY_TYPE_TIMESTAMP,
                D3D12_RANGE, D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC,
                D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
                D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
                D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC,
                D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_DEPTH_WRITE,
                D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_ROOT_CONSTANTS,
                D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0,
                D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC,
                D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SAMPLER_DESC,
                D3D12_SHADER_BYTECODE, D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_VISIBILITY,
                D3D12_SHADER_VISIBILITY_ALL, D3D12_STREAM_OUTPUT_DESC, D3D12_TEXTURE_COPY_LOCATION,
                D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12_VIEWPORT, D3D12CreateDevice,
                D3D12GetDebugInterface, D3D12SerializeRootSignature, ID3D12CommandAllocator,
                ID3D12CommandList, ID3D12CommandQueue, ID3D12Debug, ID3D12DescriptorHeap,
                ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12InfoQueue,
                ID3D12PipelineState, ID3D12QueryHeap, ID3D12Resource, ID3D12RootSignature,
            },
            DirectComposition::{
                DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR, DCompositionCreateDevice2,
                IDCompositionDesktopDevice, IDCompositionMatrixTransform, IDCompositionTarget,
                IDCompositionVisual,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED,
                    DXGI_ALPHA_MODE_STRAIGHT, DXGI_ALPHA_MODE_UNSPECIFIED,
                    DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
                },
                CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS,
                DXGI_ERROR_NOT_FOUND, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_GPU_PREFERENCE_MINIMUM_POWER,
                DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_PRESENT_PARAMETERS,
                DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
                DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter1,
                IDXGIFactory4, IDXGIFactory5, IDXGIFactory6, IDXGIOutput, IDXGISwapChain1,
                IDXGISwapChain3,
            },
        },
        Win32::{
            Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{CreateEventW, WaitForSingleObject},
        },
        core::{BOOL, Error as WindowsError, Interface, PCSTR, PCWSTR},
    };
    use windows_numerics::Matrix3x2;

    const BACK_BUFFER_COUNT: u32 = 3;
    const DX12_MAX_FRAME_LATENCY: u32 = 1;
    const DX12_FRAME_LATENCY_WAIT_MILLIS: u32 = 1000;
    const DX12_UPLOAD_COMMAND_POOL_CAPACITY: usize = 4;
    const DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT: u64 = 512;
    const DX12_TEXTURE_DATA_PITCH_ALIGNMENT: u64 = 256;
    const DX12_RESOURCE_DESCRIPTOR_HEAP_CAPACITY: u32 = 4096;
    const DX12_SAMPLER_DESCRIPTOR_HEAP_CAPACITY: u32 = NAGA_HLSL_SAMPLER_HEAP_SIZE;
    const NAGA_HLSL_SAMPLER_HEAP_SIZE: u32 = 2048;
    const NAGA_HLSL_SAMPLER_INDEX_SPACE: u32 = 255;
    const PHASE1_RESOURCE_SET_SPACE: u32 = 0;
    const NAGA_HLSL_SPECIAL_CONSTANTS_REGISTER: u32 = 0;
    const NAGA_HLSL_SPECIAL_CONSTANTS_SPACE: u32 = 254;
    const DX12_FENCE_WAIT_POLL_MILLIS: u32 = 50;
    const DX12_FENCE_WAIT_WARNING_TIMEOUT: Duration = Duration::from_secs(2);

    /// Native presentation target accepted by the Direct3D 12 backend.
    pub trait Dx12SurfaceTarget: HasDisplayHandle + HasWindowHandle {}

    impl<T> Dx12SurfaceTarget for T where T: HasDisplayHandle + HasWindowHandle + ?Sized {}

    /// Generic Direct3D 12 device and resource owner.
    pub struct Dx12Device {
        factory: IDXGIFactory4,
        _adapter: IDXGIAdapter1,
        adapter_name: String,
        device: ID3D12Device,
        graphics_queue: ID3D12CommandQueue,
        fence: ID3D12Fence,
        fence_event: FenceEvent,
        next_fence_value: u64,
        buffers: ResourceRegistry<Dx12Buffer>,
        textures: ResourceRegistry<Dx12Texture>,
        texture_views: ResourceRegistry<Dx12TextureView>,
        samplers: ResourceRegistry<Dx12Sampler>,
        resource_set_layouts: ResourceRegistry<Dx12ResourceSetLayout>,
        resource_sets: ResourceRegistry<Dx12ResourceSet>,
        pipeline_layouts: ResourceRegistry<Dx12PipelineLayout>,
        shader_modules: ResourceRegistry<Dx12ShaderModule>,
        render_passes: ResourceRegistry<Dx12RenderPass>,
        render_pipelines: ResourceRegistry<Dx12RenderPipeline>,
        command_encoders: ResourceRegistry<Dx12CommandEncoder>,
        submissions: ResourceRegistry<Dx12Submission>,
        surfaces: ResourceRegistry<Dx12Surface>,
        swapchains: ResourceRegistry<Dx12Swapchain>,
        resource_heap: DescriptorHeapAllocator,
        sampler_heap: DescriptorHeapAllocator,
        rtv_heap: DescriptorHeapAllocator,
        dsv_heap: DescriptorHeapAllocator,
        upload_ring: UploadRingAllocator,
        upload_pages: Vec<Option<Dx12UploadPage>>,
        deferred_command_encoders: Vec<DeferredDx12CommandEncoder>,
        pending_texture_uploads: DeferredFreeQueue<Dx12SubmittedCommandList>,
        upload_command_pool: Vec<Dx12UploadCommands>,
        timestamp_frequency: Option<u64>,
        last_texture_transfer_time: Option<Duration>,
        deferred_releases: DeferredFreeQueue<DeferredDx12Release>,
        allow_tearing: bool,
        submitted_frames: u64,
    }

    impl Dx12Device {
        /// Creates a Direct3D 12 device.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] if Direct3D 12 initialization fails.
        pub fn new(desc: &DeviceDesc) -> Result<Self> {
            enable_debug_layer_if_requested();
            let factory = create_factory()?;
            let allow_tearing = factory_supports_tearing(&factory);
            let adapter = pick_adapter(&factory, desc)?;
            let adapter_description = adapter_description(&adapter)?;
            log::info!(
                "nova-gfx DX12 selected adapter: name=\"{}\" preference={:?}",
                adapter_description.name,
                desc.power_preference
            );
            let device = create_device(&adapter)?;
            let graphics_queue = create_command_queue(&device)?;
            // SAFETY: The queue is live and returns its timestamp tick frequency by value.
            let timestamp_frequency = unsafe { graphics_queue.GetTimestampFrequency() }
                .ok()
                .filter(|frequency| *frequency > 0);
            let resource_heap = DescriptorHeapAllocator::new(
                &device,
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                DX12_RESOURCE_DESCRIPTOR_HEAP_CAPACITY,
                true,
            )?;
            let sampler_heap = DescriptorHeapAllocator::new(
                &device,
                D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
                DX12_SAMPLER_DESCRIPTOR_HEAP_CAPACITY,
                true,
            )?;
            let rtv_heap =
                DescriptorHeapAllocator::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 256, false)?;
            let dsv_heap =
                DescriptorHeapAllocator::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 256, false)?;
            let upload_ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
                alignment: DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT,
                ..UploadRingAllocatorDesc::default()
            })?;
            Ok(Self {
                fence: create_fence(&device)?,
                fence_event: FenceEvent::new()?,
                next_fence_value: 1,
                factory,
                _adapter: adapter,
                adapter_name: adapter_description.name,
                device,
                graphics_queue,
                buffers: ResourceRegistry::new("buffer"),
                textures: ResourceRegistry::new("texture"),
                texture_views: ResourceRegistry::new("texture view"),
                samplers: ResourceRegistry::new("sampler"),
                resource_set_layouts: ResourceRegistry::new("resource set layout"),
                resource_sets: ResourceRegistry::new("resource set"),
                pipeline_layouts: ResourceRegistry::new("pipeline layout"),
                shader_modules: ResourceRegistry::new("shader module"),
                render_passes: ResourceRegistry::new("render pass"),
                render_pipelines: ResourceRegistry::new("render pipeline"),
                command_encoders: ResourceRegistry::new("command encoder"),
                submissions: ResourceRegistry::new("submission"),
                surfaces: ResourceRegistry::new("surface"),
                swapchains: ResourceRegistry::new("swapchain"),
                resource_heap,
                sampler_heap,
                rtv_heap,
                dsv_heap,
                upload_ring,
                upload_pages: Vec::new(),
                deferred_command_encoders: Vec::new(),
                pending_texture_uploads: DeferredFreeQueue::new(),
                upload_command_pool: Vec::new(),
                timestamp_frequency,
                last_texture_transfer_time: None,
                deferred_releases: DeferredFreeQueue::new(),
                allow_tearing,
                submitted_frames: 0,
            })
        }

        /// Returns the adapter selected when this logical device was created.
        #[must_use]
        pub fn adapter_name(&self) -> &str {
            &self.adapter_name
        }

        /// Creates a native D3D12 surface from raw-window-handle traits.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when native handles are invalid.
        fn create_surface<W>(&mut self, window: &W, desc: &SurfaceDesc) -> Result<SurfaceId>
        where
            W: HasDisplayHandle + HasWindowHandle + ?Sized,
        {
            let _display = window
                .display_handle()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let window = window
                .window_handle()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let hwnd = match window.as_raw() {
                RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
                other => {
                    return Err(GfxError::InvalidInput(format!(
                        "DX12 surface requires RawWindowHandle::Win32, got {other:?}"
                    )));
                }
            };
            Ok(self.surfaces.insert(Dx12Surface {
                label: desc.label.clone(),
                hwnd,
            }))
        }

        /// Configures a surface swapchain.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when swapchain creation fails.
        pub fn configure_surface(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SurfaceId> {
            let _ = self.create_swapchain(surface, config)?;
            Ok(surface)
        }

        /// Creates or replaces a swapchain for a surface.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the surface handle is invalid.
        fn create_swapchain(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            let surface_record = self.surfaces.get(surface)?;
            let swapchain = self.build_swapchain(surface, surface_record.hwnd, config)?;
            Ok(self.swapchains.insert(swapchain))
        }

        fn build_swapchain(
            &self,
            surface: SurfaceId,
            hwnd: HWND,
            config: SurfaceConfig,
        ) -> Result<Dx12Swapchain> {
            let uses_composition = matches!(
                config.alpha_mode,
                CompositeAlphaMode::Premultiplied | CompositeAlphaMode::Postmultiplied
            );
            // Frame-latency waitable objects let the frame loop block before recording
            // instead of stalling inside Present once the queue is full.
            let mut creation_flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
            if self.allow_tearing && !uses_composition {
                creation_flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32;
            }
            let swapchain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: config.size.width(),
                Height: config.size.height(),
                Format: format_to_dxgi(config.format),
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: BACK_BUFFER_COUNT,
                // GPUI sizes the swapchain in physical pixels. During a native resize,
                // the HWND can briefly be larger than the most recently presented
                // buffer while the next frame rebuilds size-dependent targets. Stretching
                // the composition content keeps the previous frame covering the client
                // area during that interval; at a stable size no resampling occurs.
                Scaling: if uses_composition {
                    DXGI_SCALING_STRETCH
                } else {
                    DXGI_SCALING_NONE
                },
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: composite_alpha_to_dxgi(config.alpha_mode),
                Flags: creation_flags,
            };
            let hwnd_creation_flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32
                | if self.allow_tearing {
                    DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32
                } else {
                    0
                };
            let (swapchain1, composition, creation_flags) = if uses_composition {
                match self.build_composition_swapchain(hwnd, &swapchain_desc) {
                    Ok((swapchain, composition)) => (swapchain, composition, creation_flags),
                    Err(error) => {
                        log!(
                            log::Level::Warn,
                            "DX12 DirectComposition swapchain unavailable; falling back to HWND swapchain: {error:?}"
                        );
                        (
                            self.build_hwnd_swapchain(hwnd, config, hwnd_creation_flags)?,
                            None,
                            hwnd_creation_flags,
                        )
                    }
                }
            } else {
                (
                    self.build_hwnd_swapchain(hwnd, config, hwnd_creation_flags)?,
                    None,
                    hwnd_creation_flags,
                )
            };
            let swapchain: IDXGISwapChain3 = swapchain1
                .cast()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let frame_latency_waitable = if creation_flags
                & (DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32)
                != 0
            {
                // SAFETY: Swapchain was created with the frame-latency waitable flag.
                if let Err(error) =
                    unsafe { swapchain.SetMaximumFrameLatency(DX12_MAX_FRAME_LATENCY) }
                {
                    log!(
                        log::Level::Warn,
                        "DX12 SetMaximumFrameLatency failed; frame pacing falls back to Present: {error:?}"
                    );
                    None
                } else {
                    // SAFETY: Swapchain is valid; the returned handle is owned by this record.
                    let handle = unsafe { swapchain.GetFrameLatencyWaitableObject() };
                    (!handle.is_invalid()).then_some(handle)
                }
            } else {
                None
            };
            let partial_presentation = true;
            let mut swapchain = Dx12Swapchain {
                surface,
                config,
                swapchain,
                composition,
                rtv_heap: None,
                render_targets: Vec::new(),
                rtv_descriptor_size: 0,
                frame_index: 0,
                creation_flags,
                frame_latency_waitable,
                partial_presentation,
                pending_damage: vec![Dx12BackBufferDamage::Full; BACK_BUFFER_COUNT as usize],
            };
            Self::rebuild_render_targets(&self.device, &mut swapchain)?;
            Ok(swapchain)
        }

        fn build_composition_swapchain(
            &self,
            hwnd: HWND,
            swapchain_desc: &DXGI_SWAP_CHAIN_DESC1,
        ) -> Result<(IDXGISwapChain1, Option<Dx12Composition>)> {
            // SAFETY: Factory, queue, and descriptor are valid for the duration of the call.
            let swapchain = unsafe {
                self.factory.CreateSwapChainForComposition(
                    &self.graphics_queue,
                    swapchain_desc,
                    Option::<&IDXGIOutput>::None,
                )
            }
            .map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: This visual only hosts an external swapchain. Passing no rendering device
            // avoids treating ID3D12Device as the IDXGIDevice required for DComp-owned surfaces.
            let composition_device: IDCompositionDesktopDevice =
                unsafe { DCompositionCreateDevice2(None::<&windows::core::IUnknown>) }.map_err(
                    |error| GfxError::Backend(format!("DCompositionCreateDevice2 failed: {error}")),
                )?;
            // SAFETY: The HWND belongs to the surface and remains live while the swapchain exists.
            let composition_target = unsafe { composition_device.CreateTargetForHwnd(hwnd, true) }
                .map_err(|error| {
                    GfxError::Backend(format!(
                        "IDCompositionDesktopDevice::CreateTargetForHwnd failed: {error}"
                    ))
                })?;
            // SAFETY: The composition device is valid and owns the visual it creates.
            let composition_visual =
                unsafe { composition_device.CreateVisual() }.map_err(|error| {
                    GfxError::Backend(format!("IDCompositionDevice::CreateVisual failed: {error}"))
                })?;
            // SAFETY: The composition device is valid and owns the transform it creates.
            let composition_transform: IDCompositionMatrixTransform =
                unsafe { composition_device.CreateMatrixTransform() }.map_err(|error| {
                    GfxError::Backend(format!(
                        "IDCompositionDevice::CreateMatrixTransform failed: {error}"
                    ))
                })?;
            // SAFETY: The visual, transform, target, and swapchain are live; Commit applies the root tree.
            let identity = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            };
            unsafe {
                // Set the initial matrix explicitly instead of relying on the DComp
                // default so the visual can never compose with a collapsed transform.
                composition_transform
                    .SetMatrix(&raw const identity)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                composition_visual
                    .SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                composition_visual
                    .SetTransform(&composition_transform)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                composition_visual.SetContent(&swapchain).map_err(|error| {
                    GfxError::Backend(format!("IDCompositionVisual::SetContent failed: {error}"))
                })?;
                composition_target
                    .SetRoot(&composition_visual)
                    .map_err(|error| {
                        GfxError::Backend(format!("IDCompositionTarget::SetRoot failed: {error}"))
                    })?;
                composition_device.Commit().map_err(|error| {
                    GfxError::Backend(format!("IDCompositionDevice::Commit failed: {error}"))
                })?;
            }
            Ok((
                swapchain,
                Some(Dx12Composition {
                    _device: composition_device,
                    _target: composition_target,
                    visual: composition_visual.into(),
                    transform: composition_transform,
                }),
            ))
        }

        fn commit_composition(composition: &Dx12Composition) -> Result<()> {
            // SAFETY: The composition device remains alive through Dx12Composition.
            unsafe { composition._device.Commit() }
                .map_err(|error| GfxError::Backend(error.to_string()))
        }

        /// Resets any live-resize stretch on a composition visual to identity.
        ///
        /// Runs right after `ResizeBuffers`. The new back buffers hold undefined
        /// content, so this commit must only publish the transform reset; the next
        /// `Present` publishes the freshly rendered frame. Resetting before that
        /// frame also keeps DWM from scaling the new buffer by the old stretch ratio.
        fn reset_composition_stretch(composition: &Dx12Composition) -> Result<()> {
            let identity = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            };
            // SAFETY: The composition device, visual, and transform are live members
            // of the swapchain record and outlive this call.
            unsafe {
                composition
                    .transform
                    .SetMatrix(&raw const identity)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                composition
                    .visual
                    .SetTransform(&composition.transform)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                Self::commit_composition(composition)
            }
        }

        fn build_hwnd_swapchain(
            &self,
            hwnd: HWND,
            config: SurfaceConfig,
            creation_flags: u32,
        ) -> Result<IDXGISwapChain1> {
            let swapchain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: config.size.width(),
                Height: config.size.height(),
                Format: format_to_dxgi(config.format),
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: BACK_BUFFER_COUNT,
                // Keep the last presented buffer covering the client area while a
                // coalesced resize is pending, just like the composition swapchain.
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: match config.alpha_mode {
                    CompositeAlphaMode::Opaque => DXGI_ALPHA_MODE_IGNORE,
                    CompositeAlphaMode::Auto
                    | CompositeAlphaMode::Premultiplied
                    | CompositeAlphaMode::Postmultiplied
                    | CompositeAlphaMode::Inherit => DXGI_ALPHA_MODE_UNSPECIFIED,
                },
                Flags: creation_flags,
            };

            // SAFETY: Factory, queue, HWND, and descriptor are valid for the duration of the call.
            unsafe {
                self.factory.CreateSwapChainForHwnd(
                    &self.graphics_queue,
                    hwnd,
                    &raw const swapchain_desc,
                    None,
                    Option::<&IDXGIOutput>::None,
                )
            }
            .map_err(|error| GfxError::Backend(error.to_string()))
        }

        /// Recreates an existing swapchain.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the swapchain handle is invalid.
        pub fn resize_swapchain(
            &mut self,
            swapchain: SwapchainId,
            width: u32,
            height: u32,
        ) -> Result<()> {
            if width == 0 || height == 0 {
                return Ok(());
            }
            let mut config = self.swapchains.get(swapchain)?.config;
            config.size = gfx_core::Extent2d::new(width, height)?;
            self.reconfigure_swapchain_in_place(swapchain, config)
        }

        /// Recreates an existing swapchain with a full surface configuration.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the swapchain handle is invalid or recreation fails.
        pub fn reconfigure_swapchain(
            &mut self,
            swapchain: SwapchainId,
            config: SurfaceConfig,
        ) -> Result<()> {
            self.reconfigure_swapchain_in_place(swapchain, config)
        }

        /// Recreates an existing swapchain and returns the still-live handle.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the swapchain handle is invalid or recreation fails.
        pub fn recreate_swapchain(
            &mut self,
            swapchain: SwapchainId,
            config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            self.recreate_swapchain_in_place(swapchain, config)?;
            Ok(swapchain)
        }

        fn recreate_swapchain_in_place(
            &mut self,
            swapchain: SwapchainId,
            config: SurfaceConfig,
        ) -> Result<()> {
            self.wait_for_pending_work()?;
            let (surface, hwnd) = {
                let swapchain_record = self.swapchains.get(swapchain)?;
                let surface = swapchain_record.surface;
                let hwnd = self.surfaces.get(surface)?.hwnd;
                (surface, hwnd)
            };

            let next_swapchain = self.build_swapchain(surface, hwnd, config)?;
            let old_swapchain = self.swapchains.replace_live(swapchain, next_swapchain)?;
            // The GPU idled in wait_for_pending_work above; this releases the
            // old backbuffers and closes its frame-latency waitable handle.
            self.release_deferred_payload(DeferredDx12Release::Swapchain(old_swapchain));
            Ok(())
        }

        fn reconfigure_swapchain_in_place(
            &mut self,
            swapchain: SwapchainId,
            config: SurfaceConfig,
        ) -> Result<()> {
            self.wait_for_pending_work()?;
            let device = self.device.clone();
            let previous_config = self.swapchains.get(swapchain)?.config;
            if previous_config.alpha_mode != config.alpha_mode {
                return Err(GfxError::InvalidInput(format!(
                    "DX12 swapchain alpha mode cannot be changed after swapchain creation; \
                     destroy and recreate the swapchain instead: old={:?} new={:?}",
                    previous_config.alpha_mode, config.alpha_mode
                )));
            }

            let swapchain = self.swapchains.get_mut(swapchain)?;

            swapchain.render_targets.clear();
            swapchain.rtv_heap = None;
            swapchain.config = config;

            // SAFETY: Swapchain is valid and all references to previous backbuffers were dropped.
            if let Err(error) = unsafe {
                swapchain.swapchain.ResizeBuffers(
                    BACK_BUFFER_COUNT,
                    config.size.width(),
                    config.size.height(),
                    format_to_dxgi(config.format),
                    DXGI_SWAP_CHAIN_FLAG(swapchain.creation_flags as i32),
                )
            } {
                swapchain.config = previous_config;
                Self::rebuild_render_targets(&device, swapchain).map_err(|rollback_error| {
                    GfxError::Backend(format!(
                        "DX12 ResizeBuffers failed: {error}; rebuilding previous render targets failed: {rollback_error}"
                    ))
                })?;
                return Err(GfxError::Backend(error.to_string()));
            }

            if let Err(error) = Self::rebuild_render_targets(&device, swapchain) {
                swapchain.config = previous_config;
                // SAFETY: Swapchain is valid and the failed render-target rebuild left no live
                // backbuffer references in this registry entry.
                let rollback = unsafe {
                    swapchain.swapchain.ResizeBuffers(
                        BACK_BUFFER_COUNT,
                        previous_config.size.width(),
                        previous_config.size.height(),
                        format_to_dxgi(previous_config.format),
                        DXGI_SWAP_CHAIN_FLAG(swapchain.creation_flags as i32),
                    )
                };
                if let Err(rollback_error) = rollback {
                    return Err(GfxError::Backend(format!(
                        "DX12 render target rebuild failed after ResizeBuffers: {error}; rollback ResizeBuffers failed: {rollback_error}"
                    )));
                }
                Self::rebuild_render_targets(&device, swapchain).map_err(|rollback_error| {
                    GfxError::Backend(format!(
                        "DX12 render target rebuild failed after ResizeBuffers: {error}; rollback render target rebuild failed: {rollback_error}"
                    ))
                })?;
                return Err(error);
            }

            if let Some(composition) = swapchain.composition.as_ref() {
                Self::reset_composition_stretch(composition)?;
            }

            Ok(())
        }

        fn release_resource_set_descriptors(&mut self, resource_set: &Dx12ResourceSet) {
            for table in &resource_set.resource_tables {
                self.resource_heap.free_index(table.descriptor_index);
            }
            if let Some(table) = resource_set.sampler_index_table {
                self.resource_heap.free_index(table.descriptor_index);
            }
            for table in &resource_set.sampler_tables {
                self.sampler_heap.free_index(table.descriptor_index);
            }
        }

        /// Creates a buffer record.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when validation fails.
        fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
            desc.validate()?;
            let resource = create_buffer_resource(&self.device, desc)?;
            Ok(self.buffers.insert(Dx12Buffer {
                desc: desc.clone(),
                resource: Some(resource),
            }))
        }

        /// Writes data into a CPU-visible buffer record.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the handle or write range is invalid.
        fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
            let buffer = self.buffers.get_mut(buffer)?;
            if buffer.desc.memory_location != MemoryLocation::CpuToGpu {
                return Err(GfxError::Unavailable(
                    "DX12 GPU-only staging upload is not enabled in this build".to_string(),
                ));
            }
            let offset = usize::try_from(offset)
                .map_err(|error| GfxError::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| GfxError::InvalidInput("buffer write range overflow".to_string()))?;
            let size = usize::try_from(buffer.desc.size).map_err(|error| {
                GfxError::InvalidInput(format!("buffer size overflow: {error}"))
            })?;
            if end > size {
                return Err(GfxError::InvalidInput(
                    "buffer write range is out of bounds".to_string(),
                ));
            }
            let resource = buffer.resource.as_ref().ok_or_else(|| {
                GfxError::Backend("DX12 buffer has no native resource".to_string())
            })?;
            upload_to_mapped_buffer(resource, offset, data)?;
            Ok(())
        }

        /// Creates a 2D texture record.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when validation fails.
        fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
            desc.validate()?;
            if desc.dimension != TextureDimension::D2 {
                return Err(GfxError::InvalidInput(
                    "only 2D textures are supported".to_string(),
                ));
            }
            let resource = create_texture_resource(&self.device, desc)?;
            Ok(self.textures.insert(Dx12Texture {
                desc: desc.clone(),
                resource: Some(resource),
                state: initial_texture_state(desc),
            }))
        }

        /// Writes data into a texture.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when upload resources or command recording fail.
        fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
            let copy = self.prepare_texture_copy(desc, data)?;
            let fence_value = self.upload_textures_2d(std::slice::from_ref(&copy))?;
            self.upload_ring.retire_used_pages(fence_value);
            self.textures.get_mut(desc.texture)?.state = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
            Ok(())
        }

        fn write_texture_batch<'a>(
            &mut self,
            writes: impl IntoIterator<Item = TextureWrite<'a>>,
        ) -> Result<()> {
            let writes = writes.into_iter();
            let mut plans = Vec::with_capacity(writes.size_hint().0);
            for write in writes {
                plans.push(self.prepare_texture_copy_plan(write)?);
            }
            if plans.is_empty() {
                return Ok(());
            }
            let upload_sizes = plans
                .iter()
                .map(|plan| plan.upload_layout.upload_size)
                .collect::<Vec<_>>();
            let allocations = self.upload_ring.allocate_batch(&upload_sizes)?;
            let mut copies = Vec::with_capacity(plans.len());
            for (plan, allocation) in plans.into_iter().zip(allocations) {
                copies.push(self.stage_texture_copy(plan, allocation)?);
            }
            let fence_value = self.upload_textures_2d(&copies)?;
            self.upload_ring.retire_used_pages(fence_value);
            for copy in &copies {
                self.textures.get_mut(copy.texture_id)?.state =
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
            }
            Ok(())
        }

        fn read_texture_pixels(&mut self, texture_id: TextureId) -> Result<TextureReadback> {
            self.wait_for_pending_work()?;
            let (resource, desc, state) = {
                let texture = self.textures.get(texture_id)?;
                if !texture.desc.usage.contains(TextureUsage::COPY_SRC) {
                    return Err(GfxError::InvalidInput(
                        "texture readback requires COPY_SRC usage".to_string(),
                    ));
                }
                let resource = texture.resource.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 texture has no native resource".to_string())
                })?;
                (resource, texture.desc.clone(), texture.state)
            };
            let bytes_per_row = desc
                .size
                .width()
                .checked_mul(desc.format.bytes_per_pixel())
                .ok_or_else(|| GfxError::InvalidInput("texture row size overflow".to_string()))?;
            let row_pitch =
                align_to_u32(u64::from(bytes_per_row), DX12_TEXTURE_DATA_PITCH_ALIGNMENT)?;
            let buffer_size = u64::from(row_pitch)
                .checked_mul(u64::from(desc.size.height()))
                .ok_or_else(|| GfxError::InvalidInput("texture readback size overflow".into()))?;
            let readback_desc = BufferDesc {
                label: Some("nova-gfx DX12 texture readback".to_string()),
                size: buffer_size,
                usage: BufferUsage::COPY_DST,
                memory_location: MemoryLocation::GpuToCpu,
            };
            let readback = create_buffer_resource(&self.device, &readback_desc)?;
            self.copy_texture_to_readback(&resource, state, &desc, &readback, row_pitch)?;

            let buffer_size = usize::try_from(buffer_size).map_err(|error| {
                GfxError::InvalidInput(format!("texture readback size overflow: {error}"))
            })?;
            let row_bytes = usize::try_from(bytes_per_row).map_err(|error| {
                GfxError::InvalidInput(format!("texture row size overflow: {error}"))
            })?;
            let row_pitch = usize::try_from(row_pitch).map_err(|error| {
                GfxError::InvalidInput(format!("texture row pitch overflow: {error}"))
            })?;
            let height = usize::try_from(desc.size.height()).map_err(|error| {
                GfxError::InvalidInput(format!("texture height overflow: {error}"))
            })?;
            let tight_len = row_bytes
                .checked_mul(height)
                .ok_or_else(|| GfxError::InvalidInput("texture readback length overflow".into()))?;
            let mut mapped = ptr::null_mut();
            let read_range = D3D12_RANGE {
                Begin: 0,
                End: buffer_size,
            };
            // SAFETY: The resource is a readback-heap buffer and the range covers its allocation.
            unsafe { readback.Map(0, Some(&raw const read_range), Some(&raw mut mapped)) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let Some(mapped) = NonNull::new(mapped.cast::<u8>()) else {
                let written_range = D3D12_RANGE { Begin: 0, End: 0 };
                // SAFETY: Map succeeded above and must be balanced before returning.
                unsafe { readback.Unmap(0, Some(&raw const written_range)) };
                return Err(GfxError::Backend(
                    "DX12 readback mapping returned a null pointer".to_string(),
                ));
            };
            let mut bytes = Vec::with_capacity(tight_len);
            for row in 0..height {
                // SAFETY: Each source row is contained by the mapped readback allocation.
                let source = unsafe {
                    std::slice::from_raw_parts(mapped.as_ptr().add(row * row_pitch), row_bytes)
                };
                bytes.extend_from_slice(source);
            }
            let written_range = D3D12_RANGE { Begin: 0, End: 0 };
            // SAFETY: This map was used for reading only and is balanced exactly once.
            unsafe { readback.Unmap(0, Some(&raw const written_range)) };
            Ok(TextureReadback {
                format: desc.format,
                size: desc.size,
                bytes_per_row,
                bytes,
            })
        }

        fn copy_texture_to_readback(
            &mut self,
            texture: &ID3D12Resource,
            old_state: D3D12_RESOURCE_STATES,
            desc: &TextureDesc,
            readback: &ID3D12Resource,
            row_pitch: u32,
        ) -> Result<()> {
            let allocator = create_command_allocator(&self.device)?;
            let command_list = create_command_list(&self.device, &allocator)?;
            if old_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
                record_transition_barrier(
                    &command_list,
                    texture,
                    old_state,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                );
            }
            let footprint =
                windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: windows::Win32::Graphics::Direct3D12::D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format_to_dxgi(desc.format),
                        Width: desc.size.width(),
                        Height: desc.size.height(),
                        Depth: 1,
                        RowPitch: row_pitch,
                    },
                };
            let mut destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: core::mem::ManuallyDrop::new(Some(readback.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: footprint,
                },
            };
            let mut source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: core::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            // SAFETY: Both resources are live and in copy-compatible states.
            unsafe {
                command_list.CopyTextureRegion(
                    &raw const destination,
                    0,
                    0,
                    0,
                    &raw const source,
                    None,
                );
            }
            release_texture_copy_location_resource(&mut destination);
            release_texture_copy_location_resource(&mut source);
            if old_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
                record_transition_barrier(
                    &command_list,
                    texture,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    old_state,
                );
            }
            // SAFETY: The command list is open and ready to close.
            unsafe { command_list.Close() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let executable: ID3D12CommandList = command_list
                .cast()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: The command list is closed and belongs to this queue's device.
            unsafe { self.graphics_queue.ExecuteCommandLists(&[Some(executable)]) };
            self.wait_for_gpu()
        }

        fn prepare_texture_copy(
            &mut self,
            desc: TextureWriteDesc,
            data: &[u8],
        ) -> Result<Dx12TextureCopyOwned> {
            let plan = self.prepare_texture_copy_plan(TextureWrite {
                descriptor: desc,
                data,
            })?;
            let allocation = self.upload_ring.allocate(plan.upload_layout.upload_size)?;
            self.stage_texture_copy(plan, allocation)
        }

        fn prepare_texture_copy_plan<'a>(
            &self,
            write: TextureWrite<'a>,
        ) -> Result<Dx12TextureCopyPlan<'a>> {
            let desc = write.descriptor;
            let texture = self.textures.get(desc.texture)?;
            desc.validate_against(&texture.desc, write.data.len())?;
            let texture_resource = texture.resource.clone().ok_or_else(|| {
                GfxError::Backend("DX12 texture has no native resource".to_string())
            })?;
            let format = texture.desc.format;
            let upload_layout = texture_upload_layout(desc, format, write.data.len())?;
            Ok(Dx12TextureCopyPlan {
                write,
                texture: texture_resource,
                old_state: texture.state,
                format,
                upload_layout,
            })
        }

        fn stage_texture_copy(
            &mut self,
            plan: Dx12TextureCopyPlan<'_>,
            allocation: UploadAllocation,
        ) -> Result<Dx12TextureCopyOwned> {
            let desc = plan.write.descriptor;
            let upload =
                self.write_texture_upload_data(plan.write.data, plan.upload_layout, allocation)?;
            let upload_resource = self.upload_page_resource(upload.allocation.page_index)?;
            Ok(Dx12TextureCopyOwned {
                texture_id: desc.texture,
                upload: upload_resource,
                texture: plan.texture,
                old_state: plan.old_state,
                desc,
                format: plan.format,
                upload_offset: upload.allocation.offset,
                row_pitch: upload.row_pitch,
            })
        }

        /// Creates a texture view record.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the texture handle is invalid.
        fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
            let (usage, resource) = {
                let texture = self.textures.get(desc.texture)?;
                let resource = texture.resource.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 texture has no native resource".to_string())
                })?;
                (texture.desc.usage, resource)
            };

            let rtv_slot = if usage.contains(TextureUsage::COLOR_ATTACHMENT) {
                let slot = self.rtv_heap.allocate()?;
                // SAFETY: Texture was created by this device with color attachment usage and RTV handle is valid.
                unsafe {
                    self.device
                        .CreateRenderTargetView(&resource, None, slot.cpu_handle);
                }
                Some(slot)
            } else {
                None
            };

            let dsv_slot = if usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
                let slot = match self.dsv_heap.allocate() {
                    Ok(slot) => slot,
                    Err(error) => {
                        if let Some(slot) = rtv_slot {
                            self.rtv_heap.free(slot);
                        }
                        return Err(error);
                    }
                };
                // SAFETY: Texture was created by this device with depth usage and DSV handle is valid.
                unsafe {
                    self.device
                        .CreateDepthStencilView(&resource, None, slot.cpu_handle);
                }
                Some(slot)
            } else {
                None
            };

            Ok(self.texture_views.insert(Dx12TextureView {
                texture: desc.texture,
                format: desc.format,
                rtv_slot,
                dsv_slot,
            }))
        }

        /// Creates a sampler record.
        #[expect(
            clippy::unnecessary_wraps,
            reason = "inherent helper mirrors the fallible GfxResourceDevice trait method"
        )]
        fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
            Ok(self.samplers.insert(Dx12Sampler {
                mag_filter: desc.mag_filter,
                min_filter: desc.min_filter,
                address_mode_u: desc.address_mode_u,
                address_mode_v: desc.address_mode_v,
            }))
        }

        /// Creates a resource set layout.
        fn create_resource_set_layout(
            &mut self,
            desc: &ResourceSetLayoutDesc,
        ) -> Result<ResourceSetLayoutId> {
            desc.validate()?;
            Ok(self
                .resource_set_layouts
                .insert(Dx12ResourceSetLayout { desc: desc.clone() }))
        }

        /// Creates a pipeline layout backed by a D3D12 root signature.
        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
            desc.validate()?;
            let layouts = desc
                .resource_set_layouts
                .iter()
                .copied()
                .map(|layout| Ok(self.resource_set_layouts.get(layout)?.desc.clone()))
                .collect::<Result<Vec<_>>>()?;
            let root_signature = create_root_signature(&self.device, &layouts)?;
            let draw_step_constants_root_index = draw_step_constants_root_index(&layouts)?;
            Ok(self.pipeline_layouts.insert(Dx12PipelineLayout {
                root_signature,
                resource_set_layouts: Arc::from(desc.resource_set_layouts.as_slice()),
                draw_step_constants_root_index,
            }))
        }

        /// Creates a resource set and writes D3D12 descriptors.
        #[expect(
            clippy::too_many_lines,
            reason = "D3D12 descriptor writes stay together at the resource set FFI boundary"
        )]
        fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
            let layout = self.resource_set_layouts.get(desc.layout)?.desc.clone();
            desc.validate_against(&layout)?;
            let mut resource_tables = Vec::new();
            let mut sampler_tables = Vec::new();

            for binding in &desc.bindings {
                match binding.resource {
                    ResourceBindingResource::Buffer(buffer_binding) => {
                        let buffer = self.buffers.get(buffer_binding.buffer)?;
                        buffer_binding.validate_against(buffer.desc.size)?;
                        let entry = layout
                            .entries
                            .iter()
                            .find(|entry| entry.binding == binding.binding)
                            .ok_or_else(|| {
                                GfxError::InvalidInput(format!(
                                    "resource set layout is missing binding {}",
                                    binding.binding
                                ))
                            })?;
                        let resource = buffer.resource.as_ref().ok_or_else(|| {
                            GfxError::Backend("DX12 buffer has no native resource".to_string())
                        })?;
                        match entry.binding_type {
                            ResourceBindingType::UniformBuffer => {
                                validate_uniform_buffer_binding(buffer_binding, buffer.desc.size)?;
                            }
                            ResourceBindingType::StorageBuffer => {
                                validate_storage_buffer_binding(buffer_binding)?;
                            }
                            ResourceBindingType::SampledTexture | ResourceBindingType::Sampler => {
                                return Err(GfxError::InvalidInput(format!(
                                    "unexpected buffer binding type {:?}",
                                    entry.binding_type
                                )));
                            }
                        }
                        let slot = self.resource_heap.allocate()?;
                        match entry.binding_type {
                            ResourceBindingType::UniformBuffer => {
                                let size = align_to_u32(buffer_binding.size, 256)?;
                                let desc =
                                windows::Win32::Graphics::Direct3D12::D3D12_CONSTANT_BUFFER_VIEW_DESC {
                                    BufferLocation: unsafe { resource.GetGPUVirtualAddress() }
                                        + buffer_binding.offset,
                                    SizeInBytes: size,
                                };
                                // SAFETY: Resource heap CPU slot is valid and CBV desc references a live upload buffer.
                                unsafe {
                                    self.device.CreateConstantBufferView(
                                        Some(&raw const desc),
                                        slot.cpu_handle,
                                    );
                                }
                            }
                            ResourceBindingType::StorageBuffer => {
                                let byte_offset = buffer_binding.offset;
                                let byte_size = buffer_binding.size;
                                let num_elements =
                                    u32::try_from(byte_size / 4).map_err(|error| {
                                        GfxError::InvalidInput(format!(
                                            "raw storage buffer element count overflow: {error}"
                                        ))
                                    })?;
                                // Naga lowers WGSL storage buffers to HLSL ByteAddressBuffer, so
                                // the SRV must be raw even when gfx-core carries a logical stride.
                                let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                                Format: DXGI_FORMAT_R32_TYPELESS,
                                ViewDimension:
                                    windows::Win32::Graphics::Direct3D12::D3D12_SRV_DIMENSION_BUFFER,
                                Shader4ComponentMapping: windows::Win32::Graphics::Direct3D12::D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                                Anonymous: windows::Win32::Graphics::Direct3D12::D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                                    Buffer: windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV {
                                        FirstElement: byte_offset / 4,
                                        NumElements: num_elements,
                                        StructureByteStride: 0,
                                        Flags: windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV_FLAG_RAW,
                                    },
                                },
                            };
                                // SAFETY: SRV descriptor references a live buffer resource and heap slot.
                                unsafe {
                                    self.device.CreateShaderResourceView(
                                        Some(resource),
                                        Some(&raw const desc),
                                        slot.cpu_handle,
                                    );
                                }
                            }
                            ResourceBindingType::SampledTexture | ResourceBindingType::Sampler => {
                                unreachable!(
                                    "unexpected buffer binding type was validated earlier"
                                );
                            }
                        }
                        resource_tables.push(Dx12DescriptorTable {
                            binding: binding.binding,
                            gpu_handle: slot.gpu_handle,
                            descriptor_index: slot.index,
                        });
                    }
                    ResourceBindingResource::Texture(texture_binding) => {
                        let view = self.texture_views.get(texture_binding.texture_view)?;
                        let texture = self.textures.get(view.texture)?;
                        let resource = texture.resource.as_ref().ok_or_else(|| {
                            GfxError::Backend("DX12 texture has no native resource".to_string())
                        })?;
                        let slot = self.resource_heap.allocate()?;
                        let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                            Format: format_to_dxgi(view.format),
                            ViewDimension: windows::Win32::Graphics::Direct3D12::D3D12_SRV_DIMENSION_TEXTURE2D,
                            Shader4ComponentMapping: windows::Win32::Graphics::Direct3D12::D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                            Anonymous: windows::Win32::Graphics::Direct3D12::D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2D: windows::Win32::Graphics::Direct3D12::D3D12_TEX2D_SRV {
                                    MostDetailedMip: 0,
                                    MipLevels: 1,
                                    PlaneSlice: 0,
                                    ResourceMinLODClamp: 0.0,
                                },
                            },
                        };
                        // SAFETY: SRV descriptor references a live texture resource and heap slot.
                        unsafe {
                            self.device.CreateShaderResourceView(
                                Some(resource),
                                Some(&raw const srv_desc),
                                slot.cpu_handle,
                            );
                        }
                        resource_tables.push(Dx12DescriptorTable {
                            binding: binding.binding,
                            gpu_handle: slot.gpu_handle,
                            descriptor_index: slot.index,
                        });
                    }
                    ResourceBindingResource::Sampler(sampler_binding) => {
                        let sampler = self.samplers.get(sampler_binding.sampler)?;
                        let slot = self.sampler_heap.allocate()?;
                        let sampler_desc = sampler_desc_to_dx12(*sampler);
                        // SAFETY: Sampler heap CPU slot is valid and descriptor is self-contained.
                        unsafe {
                            self.device
                                .CreateSampler(&raw const sampler_desc, slot.cpu_handle);
                        }
                        sampler_tables.push(Dx12DescriptorTable {
                            binding: binding.binding,
                            gpu_handle: self.sampler_heap.gpu_start(),
                            descriptor_index: slot.index,
                        });
                    }
                }
            }
            let mut owned_buffers = Vec::new();
            let sampler_index_table = if sampler_tables.is_empty() {
                None
            } else {
                let index_buffer =
                    create_sampler_index_buffer(&self.device, &sampler_tables, &layout)?;
                let slot = self.resource_heap.allocate()?;
                let srv_desc = windows::Win32::Graphics::Direct3D12::D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_UNKNOWN,
                    ViewDimension: windows::Win32::Graphics::Direct3D12::D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: windows::Win32::Graphics::Direct3D12::D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: windows::Win32::Graphics::Direct3D12::D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV {
                            FirstElement: 0,
                            NumElements: layout_sampler_index_count(&layout)?,
                            StructureByteStride: u32::try_from(std::mem::size_of::<u32>())
                                .map_err(|error| {
                                    GfxError::InvalidInput(format!(
                                        "sampler index stride overflow: {error}"
                                    ))
                                })?,
                            Flags: windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV_FLAG_NONE,
                        },
                    },
                };
                // SAFETY: SRV descriptor references a live upload buffer and a valid descriptor slot.
                unsafe {
                    self.device.CreateShaderResourceView(
                        Some(&index_buffer),
                        Some(&raw const srv_desc),
                        slot.cpu_handle,
                    );
                }
                owned_buffers.push(index_buffer);
                Some(Dx12DescriptorTable {
                    binding: NAGA_HLSL_SAMPLER_INDEX_SPACE,
                    gpu_handle: slot.gpu_handle,
                    descriptor_index: slot.index,
                })
            };
            Ok(self.resource_sets.insert(Dx12ResourceSet {
                layout: desc.layout,
                resource_tables,
                sampler_tables,
                sampler_index_table,
                owned_buffers,
            }))
        }

        /// Creates and validates a shader module.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when validation or HLSL compilation fails.
        fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
            desc.validate()?;
            let bytecode = match &desc.binary.code {
                ShaderCode::Hlsl(source) => compile_hlsl_to_dx_bytecode(
                    source,
                    &desc.binary.entry_point,
                    desc.binary.stage,
                )?,
                ShaderCode::DxBytecode(bytecode) => bytecode.clone(),
                ShaderCode::Spirv(_) | ShaderCode::Msl(_) => {
                    return Err(GfxError::Shader(
                        "DX12 shader module requires HLSL or D3D bytecode".to_string(),
                    ));
                }
            };
            Ok(self.shader_modules.insert(Dx12ShaderModule {
                stage: desc.binary.stage,
                entry_point: desc.binary.entry_point.clone(),
                bytecode,
            }))
        }

        /// Creates a render pass record.
        #[expect(
            clippy::unnecessary_wraps,
            reason = "inherent helper mirrors the fallible GfxPipelineDevice trait method"
        )]
        fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
            Ok(self.render_passes.insert(Dx12RenderPass {
                color_format: desc.color_attachment.format,
                depth_format: desc
                    .depth_attachment
                    .as_ref()
                    .map(|attachment| attachment.format),
            }))
        }

        /// Creates a graphics render pipeline record.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when validation or shader handles fail.
        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDesc,
            _viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            desc.validate()?;
            let vertex_shader = self.shader_modules.get(desc.vertex_shader)?;
            let fragment_shader = self.shader_modules.get(desc.fragment_shader)?;
            if vertex_shader.stage != ShaderStage::Vertex {
                return Err(GfxError::InvalidInput(
                    "vertex shader module must use ShaderStage::Vertex".to_string(),
                ));
            }
            if fragment_shader.stage != ShaderStage::Fragment {
                return Err(GfxError::InvalidInput(
                    "fragment shader module must use ShaderStage::Fragment".to_string(),
                ));
            }
            let render_pass = self.render_passes.get(desc.render_pass)?;
            if desc.depth_state.is_some() && render_pass.depth_format.is_none() {
                return Err(GfxError::InvalidInput(
                    "DX12 depth pipeline requires a render pass depth attachment".to_string(),
                ));
            }
            let (root_signature, draw_step_constants_root_index, resource_set_layouts) =
                if let Some(pipeline_layout) = desc.pipeline_layout {
                    let pipeline_layout = self.pipeline_layouts.get(pipeline_layout)?;
                    (
                        pipeline_layout.root_signature.clone(),
                        pipeline_layout.draw_step_constants_root_index,
                        pipeline_layout.resource_set_layouts.clone(),
                    )
                } else {
                    (
                        create_empty_root_signature(&self.device)?,
                        0,
                        Arc::from(Vec::new()),
                    )
                };
            let pipeline_state = create_pipeline_state(
                &self.device,
                &root_signature,
                vertex_shader,
                fragment_shader,
                desc.color_format,
                desc.blend_mode,
                render_pass.depth_format,
                desc.depth_state.is_some(),
            )?;
            Ok(self.render_pipelines.insert(Dx12RenderPipeline {
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: desc.primitive_topology,
                pipeline_state: Some(pipeline_state),
                root_signature: Some(root_signature),
                resource_set_layouts,
                draw_step_constants_root_index,
            }))
        }

        /// Creates a command encoder record.
        fn create_command_encoder(
            &mut self,
            _desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
            let allocator = create_command_allocator(&self.device)?;
            let command_list = create_command_list(&self.device, &allocator)?;
            // SAFETY: Newly created command lists start open; close it so frame recording can reset it.
            unsafe { command_list.Close() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            Ok(self.command_encoders.insert(Dx12CommandEncoder {
                allocator: Some(allocator),
                command_list: Some(command_list),
            }))
        }

        /// Records a draw call with optional resource sets.
        fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: &DrawDesc) -> Result<()> {
            let _encoder = self.command_encoders.get(encoder)?;
            let RenderTarget::Swapchain { swapchain, .. } = draw.pass.target else {
                return Err(GfxError::Unavailable(
                    "DX12 offscreen render target is not implemented yet".to_string(),
                ));
            };
            let _swapchain = self.swapchains.get(swapchain)?;
            let _render_pass = self.render_passes.get(draw.pass.render_pass)?;
            let _pipeline = self.render_pipelines.get(draw.pipeline)?;
            for resource_set in &draw.resource_sets {
                let _ = self.resource_sets.get(*resource_set)?;
            }
            Ok(())
        }

        /// Submits a command encoder.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the encoder is invalid.
        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            let fence_value = self.submit_without_wait(encoder)?;
            self.wait_for_fence_value(fence_value)?;
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            self.poll_cleanup();
            Ok(())
        }

        fn submit_without_wait(&mut self, encoder: CommandEncoderId) -> Result<u64> {
            self.check_device_removed("DX12 submit preflight")?;
            let command_list = {
                let encoder = self.command_encoders.get(encoder)?;
                encoder.command_list.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no command list".to_string())
                })?
            };
            let command_list: ID3D12CommandList = command_list
                .cast()
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: Command list is closed and ready to execute on this queue.
            unsafe {
                self.graphics_queue
                    .ExecuteCommandLists(&[Some(command_list)]);
            };
            let fence_value = self.signal_frame()?;
            self.check_device_removed("DX12 submit")?;
            Ok(fence_value)
        }

        fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
            let fence_value = self.submit_without_wait(encoder)?;
            let encoder = self.command_encoders.take(encoder)?;
            self.deferred_command_encoders
                .push(DeferredDx12CommandEncoder {
                    fence_value,
                    _encoder: encoder,
                });
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            self.poll_cleanup();
            Ok(self.submissions.insert(Dx12Submission { fence_value }))
        }

        fn poll_submission(&mut self, submission_id: SubmissionId) -> Result<SubmissionStatus> {
            self.poll_cleanup();
            let submission = self.submissions.get(submission_id)?;
            let completed = match self.completed_fence_value("ID3D12Fence::GetCompletedValue") {
                Ok(completed) => completed,
                Err(error) => {
                    let _completed = self.submissions.take(submission_id)?;
                    return Ok(SubmissionStatus::Failed(error.to_string()));
                }
            };
            if completed >= submission.fence_value {
                let _completed = self.submissions.take(submission_id)?;
                Ok(SubmissionStatus::Complete)
            } else if let Err(error) = self.check_device_removed("DX12 submission poll") {
                let _failed = self.submissions.take(submission_id)?;
                Ok(SubmissionStatus::Failed(error.to_string()))
            } else {
                Ok(SubmissionStatus::Pending)
            }
        }

        fn wait_submission(&mut self, submission_id: SubmissionId) -> Result<()> {
            let fence_value = self.submissions.get(submission_id)?.fence_value;
            match self.wait_for_fence_value(fence_value) {
                Ok(()) => {
                    let _completed = self.submissions.take(submission_id)?;
                    self.poll_cleanup();
                    Ok(())
                }
                Err(error) => {
                    let _failed = self.submissions.take(submission_id)?;
                    self.deferred_command_encoders.clear();
                    Err(error)
                }
            }
        }

        /// Draws and presents one frame with multiple draw steps.
        fn draw_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: ClearColor,
        ) -> Result<()> {
            self.wait_swapchain_frame_latency(swapchain);
            self.prepare_swapchain_damage(swapchain, None)?;
            let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
            let result = self
                .record_resource_steps_frame(encoder, swapchain, render_pass, steps, clear_color)
                .and_then(|()| self.submit(encoder));
            self.finish_temporary_command_encoder(encoder, result)?;
            self.present(swapchain, 0)
        }

        fn render_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
        ) -> Result<()> {
            self.render_steps_and_present_with_depth(
                swapchain,
                render_pass,
                steps,
                clear_color,
                None,
            )
        }

        fn render_steps_and_present_with_depth(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            self.render_step_list_and_present_with_depth(
                swapchain,
                render_pass,
                RenderStepList::from_render_steps(steps),
                clear_color,
                depth_attachment,
            )
        }

        fn render_step_list_and_present_with_depth(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            self.render_step_list_and_present_with_depth_and_damage(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                None,
            )
        }

        fn render_step_list_and_present_with_depth_and_damage(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
            damage: Option<ScissorRect>,
        ) -> Result<()> {
            self.wait_swapchain_frame_latency(swapchain);
            let render_damage = self.prepare_swapchain_damage(swapchain, damage)?;
            let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
            let result = self
                .record_render_step_list_frame(
                    encoder,
                    swapchain,
                    render_pass,
                    steps,
                    clear_color,
                    depth_attachment,
                    render_damage,
                )
                .and_then(|()| self.submit(encoder));
            self.finish_temporary_command_encoder(encoder, result)?;
            self.present_with_damage(swapchain, 0, render_damage)
        }

        /// Records and submits draw steps into a regular texture view.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError`] when the texture view is not renderable or command recording fails.
        fn draw_steps_to_texture(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
            let result = self
                .record_resource_steps_texture(
                    encoder,
                    texture_view,
                    render_pass,
                    steps,
                    color_load_op,
                )
                .and_then(|()| self.submit_temporary_command_encoder_deferred(encoder));
            self.finish_temporary_command_encoder_after_result(encoder, result)
        }

        fn render_steps_to_texture(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            self.render_steps_to_texture_with_depth(
                texture_view,
                render_pass,
                steps,
                color_load_op,
                None,
            )
        }

        fn render_steps_to_texture_with_depth(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            self.render_step_list_to_texture_with_depth(
                texture_view,
                render_pass,
                RenderStepList::from_render_steps(steps),
                color_load_op,
                depth_attachment,
            )
        }

        fn render_step_list_to_texture_with_depth(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
            let result = self
                .record_render_step_list_texture(
                    encoder,
                    texture_view,
                    render_pass,
                    steps,
                    color_load_op,
                    depth_attachment,
                )
                .and_then(|()| self.submit_temporary_command_encoder_deferred(encoder));
            self.finish_temporary_command_encoder_after_result(encoder, result)
        }

        fn render_steps_and_present_deferred(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
        ) -> Result<SubmissionId> {
            self.render_step_list_and_present_deferred(
                swapchain,
                render_pass,
                RenderStepList::from_render_steps(steps),
                clear_color,
                None,
            )
        }

        fn render_step_list_and_present_deferred(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<SubmissionId> {
            self.render_step_list_and_present_deferred_with_damage(
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                None,
            )
        }

        fn render_step_list_and_present_deferred_with_damage(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
            damage: Option<ScissorRect>,
        ) -> Result<SubmissionId> {
            self.wait_swapchain_frame_latency(swapchain);
            let render_damage = self.prepare_swapchain_damage(swapchain, damage)?;
            let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
            let result = self
                .record_render_step_list_frame(
                    encoder,
                    swapchain,
                    render_pass,
                    steps,
                    clear_color,
                    depth_attachment,
                    render_damage,
                )
                .and_then(|()| Self::submit_deferred(self, encoder));
            let submission = match result {
                Ok(submission) => submission,
                Err(error) => {
                    let _destroy_result = self.destroy_temporary_command_encoder_now(encoder);
                    return Err(error);
                }
            };
            self.present_with_damage(swapchain, 0, render_damage)?;
            Ok(submission)
        }

        fn submit_temporary_command_encoder_deferred(
            &mut self,
            encoder: CommandEncoderId,
        ) -> Result<()> {
            let fence_value = self.submit_without_wait(encoder)?;
            let encoder = self.command_encoders.take(encoder)?;
            self.deferred_command_encoders
                .push(DeferredDx12CommandEncoder {
                    fence_value,
                    _encoder: encoder,
                });
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            self.poll_cleanup();
            Ok(())
        }

        fn finish_temporary_command_encoder_after_result(
            &mut self,
            encoder: CommandEncoderId,
            result: Result<()>,
        ) -> Result<()> {
            match result {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _destroy_result = self.destroy_temporary_command_encoder_now(encoder);
                    Err(error)
                }
            }
        }

        fn finish_temporary_command_encoder(
            &mut self,
            encoder: CommandEncoderId,
            result: Result<()>,
        ) -> Result<()> {
            let destroy_result = self.destroy_temporary_command_encoder_now(encoder);
            match (result, destroy_result) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(error), _) | (Ok(()), Err(error)) => Err(error),
            }
        }

        fn destroy_temporary_command_encoder_now(
            &mut self,
            encoder: CommandEncoderId,
        ) -> Result<()> {
            let _encoder = self.command_encoders.take(encoder)?;
            Ok(())
        }

        /// Presents a swapchain image.
        ///
        /// # Errors
        ///
        /// Returns [`GfxError::Unavailable`] until the DXGI present path is enabled.
        /// Blocks until the swapchain can accept a new frame, bounded by
        /// [`DX12_FRAME_LATENCY_WAIT_MILLIS`]. Waiting here (before recording)
        /// keeps `Present` from stalling the UI thread mid-frame and caps
        /// queued-frame latency at [`DX12_MAX_FRAME_LATENCY`].
        fn wait_swapchain_frame_latency(&self, swapchain: SwapchainId) {
            let Ok(record) = self.swapchains.get(swapchain) else {
                return;
            };
            if let Some(handle) = record.frame_latency_waitable {
                // SAFETY: Handle is a live waitable object owned by the swapchain
                // record; a timeout simply lets the frame proceed.
                let _ = unsafe { WaitForSingleObject(handle, DX12_FRAME_LATENCY_WAIT_MILLIS) };
            }
        }

        fn present(&mut self, swapchain: SwapchainId, _image_index: u32) -> Result<()> {
            self.present_with_damage(swapchain, _image_index, None)
        }

        fn present_with_damage(
            &mut self,
            swapchain: SwapchainId,
            _image_index: u32,
            damage: Option<ScissorRect>,
        ) -> Result<()> {
            self.check_device_removed("DX12 present preflight")?;
            let result = {
                let swapchain = self.swapchains.get_mut(swapchain)?;
                let supports_tearing =
                    swapchain.creation_flags & (DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32) != 0;
                let (sync_interval, flags) =
                    present_mode_to_dxgi(swapchain.config.present_mode, supports_tearing);
                let result = if let Some(damage) = damage.filter(|_| swapchain.partial_presentation)
                {
                    let mut rect = Self::damage_to_dxgi_rect(damage)?;
                    let parameters = DXGI_PRESENT_PARAMETERS {
                        DirtyRectsCount: 1,
                        pDirtyRects: &raw mut rect,
                        pScrollRect: ptr::null_mut(),
                        pScrollOffset: ptr::null_mut(),
                    };
                    let swapchain1: IDXGISwapChain1 = swapchain
                        .swapchain
                        .cast()
                        .map_err(|error| GfxError::Backend(error.to_string()))?;
                    // SAFETY: The swapchain and damage rectangle remain valid for the call.
                    unsafe { swapchain1.Present1(sync_interval, flags, &raw const parameters) }
                } else {
                    // SAFETY: Swapchain is valid and command submission has completed recording.
                    unsafe { swapchain.swapchain.Present(sync_interval, flags) }
                };
                result
            };
            result
                .ok()
                .map_err(|error| self.backend_error_with_device_reason("DXGI Present", &error))?;
            {
                let swapchain = self.swapchains.get_mut(swapchain)?;
                let presented_frame_index =
                    usize::try_from(swapchain.frame_index).map_err(|error| {
                        GfxError::InvalidInput(format!("swapchain frame index overflow: {error}"))
                    })?;
                let pending_damage = swapchain
                    .pending_damage
                    .get_mut(presented_frame_index)
                    .ok_or_else(|| {
                        GfxError::Backend(
                            "DX12 swapchain damage index is out of bounds".to_string(),
                        )
                    })?;
                *pending_damage = Dx12BackBufferDamage::Clean;
            }
            let swapchain = self.swapchains.get_mut(swapchain)?;
            // SAFETY: Swapchain is valid after a successful Present call.
            swapchain.frame_index = unsafe { swapchain.swapchain.GetCurrentBackBufferIndex() };
            Ok(())
        }

        fn prepare_swapchain_damage(
            &mut self,
            swapchain: SwapchainId,
            damage: Option<ScissorRect>,
        ) -> Result<Option<ScissorRect>> {
            let swapchain = self.swapchains.get_mut(swapchain)?;
            if !swapchain.partial_presentation {
                return Ok(None);
            }
            for pending_damage in &mut swapchain.pending_damage {
                pending_damage.accumulate(damage);
            }
            let frame_index = usize::try_from(swapchain.frame_index).map_err(|error| {
                GfxError::InvalidInput(format!("swapchain frame index overflow: {error}"))
            })?;
            let pending_damage = swapchain.pending_damage.get(frame_index).ok_or_else(|| {
                GfxError::Backend("DX12 swapchain damage index is out of bounds".to_string())
            })?;
            Ok(pending_damage.present_damage())
        }

        fn damage_to_dxgi_rect(damage: ScissorRect) -> Result<RECT> {
            Ok(RECT {
                left: i32::try_from(damage.x).map_err(|error| {
                    GfxError::InvalidInput(format!("damage left overflow: {error}"))
                })?,
                top: i32::try_from(damage.y).map_err(|error| {
                    GfxError::InvalidInput(format!("damage top overflow: {error}"))
                })?,
                right: i32::try_from(damage.x.saturating_add(damage.width)).map_err(|error| {
                    GfxError::InvalidInput(format!("damage right overflow: {error}"))
                })?,
                bottom: i32::try_from(damage.y.saturating_add(damage.height)).map_err(|error| {
                    GfxError::InvalidInput(format!("damage bottom overflow: {error}"))
                })?,
            })
        }

        /// Destroys a buffer once in-flight GPU work has retired.
        fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
            let buffer = self.buffers.take(buffer)?;
            self.defer_release(DeferredDx12Release::Buffer(buffer));
            Ok(())
        }

        /// Destroys a texture once in-flight GPU work has retired.
        fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
            let texture = self.textures.take(texture)?;
            self.defer_release(DeferredDx12Release::Texture(texture));
            Ok(())
        }

        /// Destroys a texture view once in-flight GPU work has retired.
        fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
            let view = self.texture_views.take(view)?;
            self.defer_release(DeferredDx12Release::TextureView(view));
            Ok(())
        }

        /// Destroys a sampler.
        fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
            let _sampler = self.samplers.take(sampler)?;
            Ok(())
        }

        /// Destroys a resource set layout.
        fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
            let _layout = self.resource_set_layouts.take(layout)?;
            Ok(())
        }

        /// Destroys a resource set once in-flight GPU work has retired.
        ///
        /// Shader-visible descriptor slots are recycled only after the fence
        /// completes so in-flight command lists never observe rewritten tables.
        fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
            let resource_set = self.resource_sets.take(resource_set)?;
            self.defer_release(DeferredDx12Release::ResourceSet(resource_set));
            Ok(())
        }

        /// Destroys a pipeline layout.
        fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
            let _layout = self.pipeline_layouts.take(layout)?;
            Ok(())
        }

        /// Destroys a shader module.
        fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
            let _shader = self.shader_modules.take(shader)?;
            Ok(())
        }

        /// Destroys a render pass.
        fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
            let _render_pass = self.render_passes.take(render_pass)?;
            Ok(())
        }

        /// Destroys a render pipeline.
        fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
            let _pipeline = self.render_pipelines.take(pipeline)?;
            Ok(())
        }

        /// Destroys a command encoder once in-flight GPU work has retired.
        fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
            let encoder = self.command_encoders.take(encoder)?;
            self.defer_release(DeferredDx12Release::CommandEncoder(encoder));
            Ok(())
        }

        /// Destroys a swapchain.
        ///
        /// Presentation to the swapchain has stopped by contract, but the last
        /// frame may still be executing; the backbuffers are released through
        /// the deferred queue instead of stalling the device.
        fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
            let swapchain = self.swapchains.take(swapchain)?;
            self.defer_release(DeferredDx12Release::Swapchain(swapchain));
            Ok(())
        }

        /// Destroys a surface.
        fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
            let _surface = self.surfaces.take(surface)?;
            Ok(())
        }

        /// Polls and releases deferred resources.
        pub fn poll_cleanup(&mut self) {
            let completed_fence = match self.completed_fence_value("ID3D12Fence::GetCompletedValue")
            {
                Ok(completed_fence) => completed_fence,
                Err(error) => {
                    log!(
                        log::Level::Warn,
                        "DX12 deferred cleanup observed device removal: {error}"
                    );
                    self.deferred_command_encoders.clear();
                    let _dead_uploads = self.pending_texture_uploads.drain_all();
                    let releases = self.deferred_releases.drain_all();
                    for payload in releases {
                        self.release_deferred_payload(payload);
                    }
                    return;
                }
            };
            self.deferred_command_encoders
                .retain(|encoder| encoder.fence_value > completed_fence);
            let completed_uploads = self
                .pending_texture_uploads
                .collect_completed(completed_fence);
            for commands in completed_uploads {
                self.recycle_upload_commands(commands);
            }
            let releases = self.deferred_releases.collect_completed(completed_fence);
            for payload in releases {
                self.release_deferred_payload(payload);
            }
            self.upload_ring.complete_fence(completed_fence);
            let retained_page_count = self.upload_ring.trim_idle_pages();
            self.trim_upload_pages(retained_page_count);
        }

        /// Returns whether the graphics queue still owns work that must retire
        /// before swapchain-dependent resources can be replaced.
        pub fn has_pending_gpu_work(&mut self) -> Result<bool> {
            self.poll_cleanup();
            let completed = self.completed_fence_value("ID3D12Fence::GetCompletedValue")?;
            Ok(completed < self.next_fence_value.saturating_sub(1))
        }

        fn trim_upload_pages(&mut self, retained_page_count: usize) {
            self.upload_pages.truncate(retained_page_count);
        }

        /// Queues `payload` for release once every submission recorded so far has
        /// retired on the GPU, or releases it immediately when the GPU already
        /// caught up. This replaces full-device stalls on the destroy path.
        fn defer_release(&mut self, payload: DeferredDx12Release) {
            let retire_fence = self.next_fence_value.saturating_sub(1);
            match self.completed_fence_value("ID3D12Fence::GetCompletedValue") {
                Ok(completed) if completed >= retire_fence => {
                    self.release_deferred_payload(payload);
                }
                Ok(_) => self.deferred_releases.retire(retire_fence, payload),
                // Device removal: the GPU can no longer touch the resource.
                Err(_) => self.release_deferred_payload(payload),
            }
        }

        fn release_deferred_payload(&mut self, payload: DeferredDx12Release) {
            match payload {
                DeferredDx12Release::TextureView(view) => {
                    if let Some(slot) = view.rtv_slot {
                        self.rtv_heap.free(slot);
                    }
                    if let Some(slot) = view.dsv_slot {
                        self.dsv_heap.free(slot);
                    }
                }
                DeferredDx12Release::ResourceSet(resource_set) => {
                    self.release_resource_set_descriptors(&resource_set);
                }
                DeferredDx12Release::Swapchain(swapchain) => {
                    if let Some(handle) = swapchain.frame_latency_waitable {
                        // SAFETY: The handle was returned by GetFrameLatencyWaitableObject
                        // and is owned exclusively by this swapchain record.
                        unsafe {
                            let _ = CloseHandle(handle);
                        }
                    }
                }
                DeferredDx12Release::Buffer(_)
                | DeferredDx12Release::Texture(_)
                | DeferredDx12Release::CommandEncoder(_) => {}
            }
        }

        /// Returns live resource statistics.
        #[must_use]
        fn resource_stats(&self) -> ResourceStats {
            ResourceStats {
                buffers: self.buffers.live_len(),
                textures: self.textures.live_len(),
                texture_views: self.texture_views.live_len(),
                samplers: self.samplers.live_len(),
                resource_set_layouts: self.resource_set_layouts.live_len(),
                resource_sets: self.resource_sets.live_len(),
                pipeline_layouts: self.pipeline_layouts.live_len(),
                shader_modules: self.shader_modules.live_len(),
                render_passes: self.render_passes.live_len(),
                render_pipelines: self.render_pipelines.live_len(),
                command_encoders: self.command_encoders.live_len(),
                submissions: self.submissions.live_len(),
                surfaces: self.surfaces.live_len(),
                swapchains: self.swapchains.live_len(),
                allocated_bytes: self.upload_ring.stats().used_bytes,
                reserved_bytes: self
                    .upload_pages
                    .iter()
                    .flatten()
                    .map(|page| page.size)
                    .fold(0_u64, u64::saturating_add),
            }
        }

        fn rebuild_render_targets(
            device: &ID3D12Device,
            swapchain: &mut Dx12Swapchain,
        ) -> Result<()> {
            rebuild_render_targets(device, swapchain)
        }

        fn write_texture_upload_data(
            &mut self,
            data: &[u8],
            layout: Dx12TextureUploadLayout,
            allocation: UploadAllocation,
        ) -> Result<Dx12TextureUpload> {
            self.ensure_upload_page(allocation.page_index)?;
            let page = self
                .upload_pages
                .get(allocation.page_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| GfxError::Backend("missing DX12 upload page".to_string()))?;
            let offset = usize::try_from(allocation.offset).map_err(|error| {
                GfxError::InvalidInput(format!("upload offset overflow: {error}"))
            })?;
            for row in 0..layout.height {
                let source_start = layout
                    .source_offset
                    .checked_add(row.checked_mul(layout.source_row_pitch).ok_or_else(|| {
                        GfxError::InvalidInput("source texture row offset overflow".to_string())
                    })?)
                    .ok_or_else(|| {
                        GfxError::InvalidInput("source texture row offset overflow".to_string())
                    })?;
                let source_end = source_start.checked_add(layout.row_bytes).ok_or_else(|| {
                    GfxError::InvalidInput("source texture row range overflow".to_string())
                })?;
                let destination_start = offset
                    .checked_add(row.checked_mul(layout.row_pitch_usize).ok_or_else(|| {
                        GfxError::InvalidInput(
                            "destination texture row offset overflow".to_string(),
                        )
                    })?)
                    .ok_or_else(|| {
                        GfxError::InvalidInput("destination texture row range overflow".to_string())
                    })?;
                let source = data.get(source_start..source_end).ok_or_else(|| {
                    GfxError::InvalidInput("texture upload data is smaller than layout".to_string())
                })?;
                // SAFETY: The page remains mapped for its lifetime and the destination range is
                // inside the suballocation returned by the upload ring.
                unsafe {
                    ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        page.mapped.as_ptr().add(destination_start),
                        source.len(),
                    );
                }
            }
            Ok(Dx12TextureUpload {
                allocation,
                row_pitch: layout.row_pitch,
            })
        }

        fn ensure_upload_page(&mut self, page_index: usize) -> Result<()> {
            let size = self.upload_ring.page_size(page_index).ok_or_else(|| {
                GfxError::Backend(format!("upload ring page {page_index} has no size"))
            })?;
            while self.upload_pages.len() <= page_index {
                self.upload_pages.push(None);
            }
            if let Some(page) = self.upload_pages[page_index].as_ref() {
                if page.size >= size {
                    return Ok(());
                }
            }
            self.upload_pages[page_index] = None;
            let desc = BufferDesc {
                label: Some(format!("nova-gfx dx12 upload page {page_index}")),
                size,
                usage: gfx_core::BufferUsage::COPY_SRC,
                memory_location: MemoryLocation::CpuToGpu,
            };
            let resource = create_buffer_resource(&self.device, &desc)?;
            self.upload_pages[page_index] = Some(Dx12UploadPage::new(resource, size)?);
            Ok(())
        }

        fn upload_page_resource(&self, page_index: usize) -> Result<ID3D12Resource> {
            self.upload_pages
                .get(page_index)
                .and_then(Option::as_ref)
                .map(|page| page.resource.clone())
                .ok_or_else(|| GfxError::Backend(format!("missing DX12 upload page {page_index}")))
        }

        #[expect(
            clippy::too_many_lines,
            reason = "D3D12 frame recording is kept together at the FFI command-list boundary"
        )]
        fn record_triangle_frame(
            &mut self,
            encoder_id: CommandEncoderId,
            swapchain_id: SwapchainId,
            render_pass_id: RenderPassId,
            pipeline_id: RenderPipelineId,
            clear_color: ClearColor,
        ) -> Result<()> {
            let (allocator, command_list) = {
                let encoder = self.command_encoders.get(encoder_id)?;
                let allocator = encoder.allocator.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no allocator".to_string())
                })?;
                let command_list = encoder.command_list.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no command list".to_string())
                })?;
                (allocator, command_list)
            };
            let swapchain = self.swapchains.get(swapchain_id)?;
            let render_pass = self.render_passes.get(render_pass_id)?;
            let (pipeline_state, root_signature, pipeline_color_format, blend_mode) = {
                let pipeline = self.render_pipelines.get(pipeline_id)?;
                let pipeline_state = pipeline.pipeline_state.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native pipeline state".to_string())
                })?;
                let root_signature = pipeline.root_signature.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native root signature".to_string())
                })?;
                (
                    pipeline_state,
                    root_signature,
                    pipeline.color_format,
                    pipeline.blend_mode,
                )
            };
            if render_pass.color_format != pipeline_color_format {
                return Err(GfxError::InvalidInput(
                    "render pass and pipeline color formats differ".to_string(),
                ));
            }
            let frame_index = usize::try_from(swapchain.frame_index).map_err(|error| {
                GfxError::InvalidInput(format!("swapchain frame index overflow: {error}"))
            })?;
            let render_target = swapchain.render_targets.get(frame_index).ok_or_else(|| {
                GfxError::Backend("DX12 swapchain frame index is out of bounds".to_string())
            })?;
            let rtv_heap = swapchain
                .rtv_heap
                .as_ref()
                .ok_or_else(|| GfxError::Backend("DX12 swapchain has no RTV heap".to_string()))?;
            // SAFETY: RTV heap exists while render targets are live.
            let heap_start = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
            let rtv_handle = descriptor_handle_at(
                heap_start,
                swapchain.rtv_descriptor_size,
                swapchain.frame_index,
            )?;
            // SAFETY: Command allocator belongs to this device and is not in use after wait_for_gpu.
            unsafe { allocator.Reset() }.map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: Command list belongs to this device and is reset with a valid allocator/PSO.
            unsafe { command_list.Reset(&allocator, &pipeline_state) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            record_transition_barrier(
                &command_list,
                render_target,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            );
            #[expect(
                clippy::cast_precision_loss,
                reason = "D3D12 viewport dimensions are f32 by API contract"
            )]
            let viewport = D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: swapchain.config.size.width() as f32,
                Height: swapchain.config.size.height() as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            let scissor = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: i32::try_from(swapchain.config.size.width()).map_err(|error| {
                    GfxError::InvalidInput(format!("swapchain width overflow: {error}"))
                })?,
                bottom: i32::try_from(swapchain.config.size.height()).map_err(|error| {
                    GfxError::InvalidInput(format!("swapchain height overflow: {error}"))
                })?,
            };
            let clear = [
                clear_color.red,
                clear_color.green,
                clear_color.blue,
                clear_color.alpha,
            ];
            let rtv_handle_pointer = &raw const rtv_handle;
            // SAFETY: All command arguments reference resources owned by this device.
            unsafe {
                command_list.SetGraphicsRootSignature(&root_signature);
                command_list.RSSetViewports(&[viewport]);
                command_list.RSSetScissorRects(&[scissor]);
                command_list.OMSetRenderTargets(1, Some(rtv_handle_pointer), false, None);
                command_list.ClearRenderTargetView(rtv_handle, &clear, None);
                command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                command_list.DrawInstanced(3, 1, 0, 0);
            }
            record_transition_barrier(
                &command_list,
                render_target,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            // SAFETY: Command list is open and can be closed after recording.
            unsafe { command_list.Close() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let _ = blend_mode;
            Ok(())
        }

        #[expect(
            clippy::too_many_arguments,
            reason = "D3D12 compatibility frame recording mirrors the command-list inputs directly"
        )]
        fn record_resource_frame(
            &mut self,
            encoder_id: CommandEncoderId,
            swapchain_id: SwapchainId,
            render_pass_id: RenderPassId,
            pipeline_id: RenderPipelineId,
            resource_sets: &[ResourceSetId],
            clear_color: ClearColor,
            vertex_count: u32,
        ) -> Result<()> {
            self.record_resource_steps_frame(
                encoder_id,
                swapchain_id,
                render_pass_id,
                &[DrawStepDesc {
                    pipeline: pipeline_id,
                    resource_sets: resource_set_list(resource_sets.iter().copied()),
                    vertex_count,
                    first_vertex: 0,
                    instance_count: 1,
                    first_instance: 0,
                    scissor: None,
                }],
                clear_color,
            )
        }

        fn record_resource_steps_frame(
            &mut self,
            encoder_id: CommandEncoderId,
            swapchain_id: SwapchainId,
            render_pass_id: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: ClearColor,
        ) -> Result<()> {
            self.record_render_step_list_frame(
                encoder_id,
                swapchain_id,
                render_pass_id,
                RenderStepList::from_draw_steps(steps),
                clear_color,
                None,
                None,
            )
        }

        fn record_render_steps_frame(
            &mut self,
            encoder_id: CommandEncoderId,
            swapchain_id: SwapchainId,
            render_pass_id: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            self.record_render_step_list_frame(
                encoder_id,
                swapchain_id,
                render_pass_id,
                RenderStepList::from_render_steps(steps),
                clear_color,
                depth_attachment,
                None,
            )
        }

        #[expect(
            clippy::too_many_lines,
            reason = "D3D12 frame recording stays together at the command-list FFI boundary"
        )]
        fn record_render_step_list_frame(
            &mut self,
            encoder_id: CommandEncoderId,
            swapchain_id: SwapchainId,
            render_pass_id: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
            frame_damage: Option<ScissorRect>,
        ) -> Result<()> {
            let (allocator, command_list) = {
                let encoder = self.command_encoders.get(encoder_id)?;
                let allocator = encoder.allocator.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no allocator".to_string())
                })?;
                let command_list = encoder.command_list.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no command list".to_string())
                })?;
                (allocator, command_list)
            };
            let swapchain = self.swapchains.get(swapchain_id)?;
            let render_pass = self.render_passes.get(render_pass_id)?;
            let depth_handle =
                self.depth_stencil_view_for_attachment(render_pass, depth_attachment)?;
            let first_step = steps.first().ok_or_else(|| {
                GfxError::InvalidInput("DX12 draw step list must not be empty".to_string())
            })?;
            let (pipeline_state, root_signature, pipeline_color_format, primitive_topology) = {
                let pipeline = self.render_pipelines.get(first_step.pipeline())?;
                let pipeline_state = pipeline.pipeline_state.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native pipeline state".to_string())
                })?;
                let root_signature = pipeline.root_signature.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native root signature".to_string())
                })?;
                (
                    pipeline_state,
                    root_signature,
                    pipeline.color_format,
                    pipeline.primitive_topology,
                )
            };
            if render_pass.color_format != pipeline_color_format {
                return Err(GfxError::InvalidInput(
                    "render pass and pipeline color formats differ".to_string(),
                ));
            }
            let frame_index = usize::try_from(swapchain.frame_index).map_err(|error| {
                GfxError::InvalidInput(format!("swapchain frame index overflow: {error}"))
            })?;
            let render_target = swapchain.render_targets.get(frame_index).ok_or_else(|| {
                GfxError::Backend("DX12 swapchain frame index is out of bounds".to_string())
            })?;
            let rtv_heap = swapchain
                .rtv_heap
                .as_ref()
                .ok_or_else(|| GfxError::Backend("DX12 swapchain has no RTV heap".to_string()))?;
            // SAFETY: RTV heap exists while render targets are live.
            let heap_start = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
            let rtv_handle = descriptor_handle_at(
                heap_start,
                swapchain.rtv_descriptor_size,
                swapchain.frame_index,
            )?;
            // SAFETY: Command allocator belongs to this device and is not in use after wait_for_gpu.
            unsafe { allocator.Reset() }.map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: Command list belongs to this device and is reset with a valid allocator/PSO.
            unsafe { command_list.Reset(&allocator, &pipeline_state) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            record_transition_barrier(
                &command_list,
                render_target,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            );
            #[expect(
                clippy::cast_precision_loss,
                reason = "D3D12 viewport dimensions are f32 by API contract"
            )]
            let viewport = D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: swapchain.config.size.width() as f32,
                Height: swapchain.config.size.height() as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            let scissor = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: i32::try_from(swapchain.config.size.width()).map_err(|error| {
                    GfxError::InvalidInput(format!("swapchain width overflow: {error}"))
                })?,
                bottom: i32::try_from(swapchain.config.size.height()).map_err(|error| {
                    GfxError::InvalidInput(format!("swapchain height overflow: {error}"))
                })?,
            };
            let clear_rects = frame_damage
                .or_else(|| {
                    steps
                        .iter()
                        .filter_map(RenderStepRef::scissor)
                        .find(|scissor| !scissor.is_empty())
                })
                .and_then(|scissor| dx12_rect_for_scissor(scissor, swapchain.config.size).ok())
                .map(|rect| [rect]);
            let clear = [
                clear_color.red,
                clear_color.green,
                clear_color.blue,
                clear_color.alpha,
            ];
            let rtv_handle_pointer = &raw const rtv_handle;
            let dsv_handle_pointer = depth_handle
                .as_ref()
                .map(|(handle, _)| handle as *const D3D12_CPU_DESCRIPTOR_HANDLE);
            let heaps = [
                Some(self.resource_heap.heap.clone()),
                Some(self.sampler_heap.heap.clone()),
            ];
            // SAFETY: All command arguments reference resources owned by this device.
            unsafe {
                command_list.SetGraphicsRootSignature(&root_signature);
                command_list.SetDescriptorHeaps(&heaps);
                command_list.RSSetViewports(&[viewport]);
                command_list.RSSetScissorRects(&[scissor]);
                command_list.OMSetRenderTargets(
                    1,
                    Some(rtv_handle_pointer),
                    false,
                    dsv_handle_pointer,
                );
                command_list.ClearRenderTargetView(
                    rtv_handle,
                    &clear,
                    clear_rects.as_ref().map(|rects| rects.as_slice()),
                );
                if let Some((dsv_handle, depth_load_op)) = depth_handle {
                    if let LoadOp::Clear(depth) = depth_load_op {
                        command_list.ClearDepthStencilView(
                            dsv_handle,
                            D3D12_CLEAR_FLAG_DEPTH,
                            depth,
                            0,
                            clear_rects.as_ref().map(|rects| rects.as_slice()),
                        );
                    }
                }
                command_list.IASetPrimitiveTopology(primitive_topology_to_dx12(primitive_topology));
            }
            for step in steps.iter() {
                let step_scissor = match (step.scissor(), frame_damage) {
                    (Some(step_scissor), Some(frame_damage)) => {
                        intersect_scissor_rects(step_scissor, frame_damage)
                    }
                    (Some(step_scissor), None) => step_scissor,
                    (None, Some(frame_damage)) => frame_damage,
                    (None, None) => ScissorRect {
                        x: 0,
                        y: 0,
                        width: swapchain.config.size.width(),
                        height: swapchain.config.size.height(),
                    },
                };
                if step_scissor.is_empty() {
                    continue;
                }
                let step_scissor = dx12_rect_for_scissor(step_scissor, swapchain.config.size)?;
                let (
                    pipeline_state,
                    root_signature,
                    primitive_topology,
                    resource_set_layouts,
                    draw_step_constants_root_index,
                ) = {
                    let pipeline = self.render_pipelines.get(step.pipeline())?;
                    let pipeline_state = pipeline.pipeline_state.clone().ok_or_else(|| {
                        GfxError::Backend("DX12 pipeline has no native pipeline state".to_string())
                    })?;
                    let root_signature = pipeline.root_signature.clone().ok_or_else(|| {
                        GfxError::Backend("DX12 pipeline has no native root signature".to_string())
                    })?;
                    if render_pass.color_format != pipeline.color_format {
                        return Err(GfxError::InvalidInput(
                            "render pass and pipeline color formats differ".to_string(),
                        ));
                    }
                    (
                        pipeline_state,
                        root_signature,
                        pipeline.primitive_topology,
                        pipeline.resource_set_layouts.clone(),
                        pipeline.draw_step_constants_root_index,
                    )
                };
                // SAFETY: Command list is open and the pipeline/root signature are live objects.
                unsafe {
                    command_list.SetPipelineState(&pipeline_state);
                    command_list.SetGraphicsRootSignature(&root_signature);
                    command_list.RSSetScissorRects(&[step_scissor]);
                    command_list
                        .IASetPrimitiveTopology(primitive_topology_to_dx12(primitive_topology));
                }
                bind_resource_sets(
                    &command_list,
                    self,
                    &resource_set_layouts,
                    step.resource_sets(),
                )?;
                self.record_render_step_draw(&command_list, draw_step_constants_root_index, step)?;
            }
            record_transition_barrier(
                &command_list,
                render_target,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            // SAFETY: Command list is open and can be closed after recording.
            unsafe { command_list.Close() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            Ok(())
        }

        fn record_resource_steps_texture(
            &mut self,
            encoder_id: CommandEncoderId,
            texture_view_id: TextureViewId,
            render_pass_id: RenderPassId,
            steps: &[DrawStepDesc],
            color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            self.record_render_step_list_texture(
                encoder_id,
                texture_view_id,
                render_pass_id,
                RenderStepList::from_draw_steps(steps),
                color_load_op,
                None,
            )
        }

        fn record_render_steps_texture(
            &mut self,
            encoder_id: CommandEncoderId,
            texture_view_id: TextureViewId,
            render_pass_id: RenderPassId,
            steps: &[RenderStepDescriptor],
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            self.record_render_step_list_texture(
                encoder_id,
                texture_view_id,
                render_pass_id,
                RenderStepList::from_render_steps(steps),
                color_load_op,
                depth_attachment,
            )
        }

        #[expect(
            clippy::too_many_lines,
            reason = "D3D12 offscreen frame recording stays together at the command-list FFI boundary"
        )]
        fn record_render_step_list_texture(
            &mut self,
            encoder_id: CommandEncoderId,
            texture_view_id: TextureViewId,
            render_pass_id: RenderPassId,
            steps: RenderStepList<'_>,
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            let first_step = steps.first().ok_or_else(|| {
                GfxError::InvalidInput("DX12 draw step list must not be empty".to_string())
            })?;
            let (allocator, command_list) = {
                let encoder = self.command_encoders.get(encoder_id)?;
                let allocator = encoder.allocator.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no allocator".to_string())
                })?;
                let command_list = encoder.command_list.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 command encoder has no command list".to_string())
                })?;
                (allocator, command_list)
            };
            let texture_view = *self.texture_views.get(texture_view_id)?;
            let render_pass = self.render_passes.get(render_pass_id)?;
            let depth_handle =
                self.depth_stencil_view_for_attachment(render_pass, depth_attachment)?;
            let (texture_resource, texture_state, texture_desc) = {
                let texture = self.textures.get(texture_view.texture)?;
                let resource = texture.resource.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 texture has no native resource".to_string())
                })?;
                (resource, texture.state, texture.desc.clone())
            };
            if !texture_desc.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
                return Err(GfxError::InvalidInput(
                    "DX12 offscreen target texture must include COLOR_ATTACHMENT usage".to_string(),
                ));
            }
            if texture_view.format != render_pass.color_format {
                return Err(GfxError::InvalidInput(
                    "texture view and render pass color formats differ".to_string(),
                ));
            }
            let (first_pipeline_state, first_root_signature, pipeline_color_format, first_topology) = {
                let pipeline = self.render_pipelines.get(first_step.pipeline())?;
                let pipeline_state = pipeline.pipeline_state.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native pipeline state".to_string())
                })?;
                let root_signature = pipeline.root_signature.clone().ok_or_else(|| {
                    GfxError::Backend("DX12 pipeline has no native root signature".to_string())
                })?;
                (
                    pipeline_state,
                    root_signature,
                    pipeline.color_format,
                    pipeline.primitive_topology,
                )
            };
            if render_pass.color_format != pipeline_color_format {
                return Err(GfxError::InvalidInput(
                    "render pass and pipeline color formats differ".to_string(),
                ));
            }
            let rtv_handle = texture_view
                .rtv_slot
                .map(|slot| slot.cpu_handle)
                .ok_or_else(|| {
                    GfxError::Backend("DX12 color texture view has no RTV handle".to_string())
                })?;

            // SAFETY: Command allocator belongs to this device and is not in use after wait_for_gpu.
            unsafe { allocator.Reset() }.map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: Command list belongs to this device and is reset with a valid allocator/PSO.
            unsafe { command_list.Reset(&allocator, &first_pipeline_state) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            if texture_state != D3D12_RESOURCE_STATE_RENDER_TARGET {
                record_transition_barrier(
                    &command_list,
                    &texture_resource,
                    texture_state,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "D3D12 viewport dimensions are f32 by API contract"
            )]
            let viewport = D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: texture_desc.size.width() as f32,
                Height: texture_desc.size.height() as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            let scissor = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: i32::try_from(texture_desc.size.width()).map_err(|error| {
                    GfxError::InvalidInput(format!("texture width overflow: {error}"))
                })?,
                bottom: i32::try_from(texture_desc.size.height()).map_err(|error| {
                    GfxError::InvalidInput(format!("texture height overflow: {error}"))
                })?,
            };
            let rtv_handle_pointer = &raw const rtv_handle;
            let dsv_handle_pointer = depth_handle
                .as_ref()
                .map(|(handle, _)| handle as *const D3D12_CPU_DESCRIPTOR_HANDLE);
            let heaps = [
                Some(self.resource_heap.heap.clone()),
                Some(self.sampler_heap.heap.clone()),
            ];
            // SAFETY: All command arguments reference resources owned by this device.
            unsafe {
                command_list.SetGraphicsRootSignature(&first_root_signature);
                command_list.SetDescriptorHeaps(&heaps);
                command_list.RSSetViewports(&[viewport]);
                command_list.RSSetScissorRects(&[scissor]);
                command_list.OMSetRenderTargets(
                    1,
                    Some(rtv_handle_pointer),
                    false,
                    dsv_handle_pointer,
                );
                if let LoadOp::Clear(clear_color) = color_load_op {
                    let clear = [
                        clear_color.red,
                        clear_color.green,
                        clear_color.blue,
                        clear_color.alpha,
                    ];
                    command_list.ClearRenderTargetView(rtv_handle, &clear, None);
                }
                if let Some((dsv_handle, depth_load_op)) = depth_handle {
                    if let LoadOp::Clear(depth) = depth_load_op {
                        command_list.ClearDepthStencilView(
                            dsv_handle,
                            D3D12_CLEAR_FLAG_DEPTH,
                            depth,
                            0,
                            None,
                        );
                    }
                }
                command_list.IASetPrimitiveTopology(primitive_topology_to_dx12(first_topology));
            }
            for step in steps.iter() {
                let (
                    pipeline_state,
                    root_signature,
                    primitive_topology,
                    resource_set_layouts,
                    draw_step_constants_root_index,
                ) = {
                    let pipeline = self.render_pipelines.get(step.pipeline())?;
                    let pipeline_state = pipeline.pipeline_state.clone().ok_or_else(|| {
                        GfxError::Backend("DX12 pipeline has no native pipeline state".to_string())
                    })?;
                    let root_signature = pipeline.root_signature.clone().ok_or_else(|| {
                        GfxError::Backend("DX12 pipeline has no native root signature".to_string())
                    })?;
                    if render_pass.color_format != pipeline.color_format {
                        return Err(GfxError::InvalidInput(
                            "render pass and pipeline color formats differ".to_string(),
                        ));
                    }
                    (
                        pipeline_state,
                        root_signature,
                        pipeline.primitive_topology,
                        pipeline.resource_set_layouts.clone(),
                        pipeline.draw_step_constants_root_index,
                    )
                };
                // SAFETY: Command list is open and the pipeline/root signature are live objects.
                unsafe {
                    command_list.SetPipelineState(&pipeline_state);
                    command_list.SetGraphicsRootSignature(&root_signature);
                    command_list
                        .IASetPrimitiveTopology(primitive_topology_to_dx12(primitive_topology));
                }
                bind_resource_sets(
                    &command_list,
                    self,
                    &resource_set_layouts,
                    step.resource_sets(),
                )?;
                self.record_render_step_draw(&command_list, draw_step_constants_root_index, step)?;
            }
            record_transition_barrier(
                &command_list,
                &texture_resource,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            );
            // SAFETY: Command list is open and can be closed after recording.
            unsafe { command_list.Close() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            self.textures.get_mut(texture_view.texture)?.state =
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
            Ok(())
        }

        fn depth_stencil_view_for_attachment(
            &self,
            render_pass: &Dx12RenderPass,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<Option<(D3D12_CPU_DESCRIPTOR_HANDLE, LoadOp<f32>)>> {
            let Some(depth_attachment) = depth_attachment else {
                if render_pass.depth_format.is_some() {
                    return Err(GfxError::InvalidInput(
                        "DX12 render pass expects a depth attachment".to_string(),
                    ));
                }
                return Ok(None);
            };
            let Some(depth_format) = render_pass.depth_format else {
                return Err(GfxError::InvalidInput(
                    "DX12 depth attachment was provided for a color-only render pass".to_string(),
                ));
            };
            let texture_view = self.texture_views.get(depth_attachment.target)?;
            if texture_view.format != depth_format {
                return Err(GfxError::InvalidInput(
                    "DX12 depth attachment format does not match render pass".to_string(),
                ));
            }
            let texture = self.textures.get(texture_view.texture)?;
            if !texture.desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
                return Err(GfxError::InvalidInput(
                    "DX12 depth attachment texture must include DEPTH_ATTACHMENT usage".to_string(),
                ));
            }
            let dsv_handle = texture_view
                .dsv_slot
                .map(|slot| slot.cpu_handle)
                .ok_or_else(|| {
                    GfxError::Backend("DX12 depth texture view has no DSV handle".to_string())
                })?;
            Ok(Some((dsv_handle, depth_attachment.depth_load_op)))
        }

        fn record_render_step_draw(
            &self,
            command_list: &ID3D12GraphicsCommandList,
            draw_step_constants_root_index: u32,
            step: RenderStepRef<'_>,
        ) -> Result<()> {
            match step {
                RenderStepRef::Draw(step) => {
                    let offsets = non_indexed_draw_offsets(step.first_vertex, step.first_instance);
                    // SAFETY: Command list is open and pipeline/root signature are bound.
                    unsafe {
                        bind_draw_step_constants(
                            command_list,
                            draw_step_constants_root_index,
                            offsets.shader_first_vertex,
                            offsets.shader_first_instance,
                            0,
                        );
                        command_list.DrawInstanced(
                            step.vertex_count,
                            step.instance_count,
                            offsets.start_vertex_location,
                            offsets.start_instance_location,
                        );
                    }
                    Ok(())
                }
                RenderStepRef::DrawIndexed(step) => {
                    let index_buffer_view = self.index_buffer_view(
                        step.index_buffer,
                        step.first_index,
                        step.index_count,
                    )?;
                    let offsets = indexed_draw_offsets(step.base_vertex, step.first_instance)?;
                    // SAFETY: Command list is open, the index buffer view references a live
                    // D3D12 buffer, and pipeline/root signature are bound.
                    unsafe {
                        bind_draw_step_constants(
                            command_list,
                            draw_step_constants_root_index,
                            offsets.shader_first_vertex,
                            offsets.shader_first_instance,
                            0,
                        );
                        command_list.IASetIndexBuffer(Some(&raw const index_buffer_view));
                        command_list.DrawIndexedInstanced(
                            step.index_count,
                            step.instance_count,
                            step.first_index,
                            offsets.base_vertex_location,
                            offsets.start_instance_location,
                        );
                    }
                    Ok(())
                }
            }
        }

        fn index_buffer_view(
            &self,
            binding: IndexBufferBinding,
            first_index: u32,
            index_count: u32,
        ) -> Result<D3D12_INDEX_BUFFER_VIEW> {
            let buffer = self.buffers.get(binding.buffer)?;
            validate_index_buffer_range(
                buffer.desc.usage,
                buffer.desc.size,
                binding,
                first_index,
                index_count,
            )?;
            let resource = buffer.resource.as_ref().ok_or_else(|| {
                GfxError::Backend("DX12 index buffer has no native resource".to_string())
            })?;
            let size_in_bytes = buffer
                .desc
                .size
                .checked_sub(binding.offset)
                .ok_or_else(|| {
                    GfxError::InvalidInput("index buffer offset is out of bounds".to_string())
                })
                .and_then(|size| {
                    u32::try_from(size).map_err(|error| {
                        GfxError::InvalidInput(format!("index buffer view size overflow: {error}"))
                    })
                })?;
            // SAFETY: Resource is a live D3D12 buffer created by this backend.
            let address = unsafe { resource.GetGPUVirtualAddress() }
                .checked_add(binding.offset)
                .ok_or_else(|| {
                    GfxError::InvalidInput("index buffer address overflow".to_string())
                })?;
            Ok(D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: address,
                SizeInBytes: size_in_bytes,
                Format: index_format_to_dxgi(binding.format),
            })
        }

        fn signal_frame(&mut self) -> Result<u64> {
            let fence_value = self.next_fence_value;
            self.next_fence_value = self.next_fence_value.saturating_add(1);
            // SAFETY: Queue and fence are valid and owned by this device.
            unsafe { self.graphics_queue.Signal(&self.fence, fence_value) }.map_err(|error| {
                self.backend_error_with_device_reason("ID3D12CommandQueue::Signal", &error)
            })?;
            Ok(fence_value)
        }

        fn wait_for_gpu(&mut self) -> Result<()> {
            let fence_value = self.signal_frame()?;
            self.wait_for_fence_value(fence_value)
        }

        fn wait_for_pending_work(&mut self) -> Result<()> {
            let fence_value = self.signal_frame()?;
            self.wait_for_fence_value(fence_value)?;
            self.deferred_command_encoders.clear();
            self.recycle_completed_uploads_all();
            let releases = self.deferred_releases.drain_all();
            for payload in releases {
                self.release_deferred_payload(payload);
            }
            self.upload_ring.complete_fence(fence_value);
            let retained_page_count = self.upload_ring.trim_idle_pages();
            self.trim_upload_pages(retained_page_count);
            Ok(())
        }

        fn recycle_completed_uploads_all(&mut self) {
            let completed = self.pending_texture_uploads.drain_all();
            for commands in completed {
                self.recycle_upload_commands(commands);
            }
        }

        fn recycle_upload_commands(&mut self, commands: Dx12SubmittedCommandList) {
            if let (Some(timestamps), Some(frequency)) = (
                commands.commands.timestamps.as_ref(),
                self.timestamp_frequency,
            ) && let Ok(elapsed) = read_dx12_timestamps(&timestamps.readback, frequency)
            {
                self.last_texture_transfer_time = Some(elapsed);
            }
            if self.upload_command_pool.len() < DX12_UPLOAD_COMMAND_POOL_CAPACITY {
                self.upload_command_pool.push(commands.commands);
            }
        }

        fn completed_fence_value(&self, operation: &str) -> Result<u64> {
            // SAFETY: Fence is valid.
            let completed = unsafe { self.fence.GetCompletedValue() };
            if completed == u64::MAX {
                return Err(GfxError::Backend(format!(
                    "{operation} reported device removal; device_removed_reason={}",
                    self.device_removed_reason()
                )));
            }
            Ok(completed)
        }

        fn check_device_removed(&self, operation: &str) -> Result<()> {
            // SAFETY: The D3D12 device is a live COM object owned by this backend.
            match unsafe { self.device.GetDeviceRemovedReason() } {
                Ok(()) => Ok(()),
                Err(error) => Err(self.backend_error_with_device_reason(operation, &error)),
            }
        }

        fn wait_for_fence_value(&self, fence_value: u64) -> Result<()> {
            let completed = self.completed_fence_value("ID3D12Fence::GetCompletedValue")?;
            if completed >= fence_value {
                return Ok(());
            }
            // SAFETY: Fence and event are valid until the wait completes.
            unsafe {
                self.fence
                    .SetEventOnCompletion(fence_value, self.fence_event.0)
            }
            .map_err(|error| {
                self.backend_error_with_device_reason("ID3D12Fence::SetEventOnCompletion", &error)
            })?;
            let started_at = Instant::now();
            let mut warned_slow_wait = false;
            loop {
                let completed = self.completed_fence_value("ID3D12Fence::GetCompletedValue")?;
                if completed >= fence_value {
                    return Ok(());
                }
                self.check_device_removed("DX12 fence wait")?;

                // SAFETY: Event handle is valid and owned by FenceEvent.
                let wait_result =
                    unsafe { WaitForSingleObject(self.fence_event.0, DX12_FENCE_WAIT_POLL_MILLIS) };
                if wait_result == WAIT_OBJECT_0 {
                    continue;
                }
                if wait_result == WAIT_TIMEOUT {
                    if !warned_slow_wait && started_at.elapsed() >= DX12_FENCE_WAIT_WARNING_TIMEOUT
                    {
                        log!(
                            log::Level::Warn,
                            "DX12 fence wait exceeded {DX12_FENCE_WAIT_WARNING_TIMEOUT:?}: \
                             target_fence={fence_value} completed_fence={completed}; \
                             device_removed_reason={}",
                            self.device_removed_reason()
                        );
                        warned_slow_wait = true;
                    }
                    continue;
                }
                return Err(GfxError::Backend(format!(
                    "WaitForSingleObject failed while waiting for DX12 fence {fence_value}: \
                     {wait_result:?}"
                )));
            }
        }

        /// Submits texture uploads on the graphics queue without blocking the CPU.
        ///
        /// The recorded command objects and upload-ring pages stay alive until
        /// [`Self::poll_cleanup`] observes the returned fence value.
        fn upload_textures_2d(&mut self, copies: &[Dx12TextureCopyOwned]) -> Result<u64> {
            let recycled = if copies.is_empty() {
                None
            } else {
                self.upload_command_pool.pop()
            };
            let upload_commands = upload_textures_2d(
                &self.device,
                &self.graphics_queue,
                copies,
                recycled,
                self.timestamp_frequency.is_some(),
            )?;
            let fence_value = self.signal_frame()?;
            if let Some(commands) = upload_commands {
                self.pending_texture_uploads.retire(fence_value, commands);
            }
            Ok(fence_value)
        }

        fn backend_error_with_device_reason(
            &self,
            operation: &str,
            error: &WindowsError,
        ) -> GfxError {
            GfxError::Backend(format!(
                "{operation} failed: {error}; device_removed_reason={}",
                self.device_removed_reason()
            ))
        }

        fn device_removed_reason(&self) -> String {
            // SAFETY: The D3D12 device is a live COM object owned by this backend.
            match unsafe { self.device.GetDeviceRemovedReason() } {
                Ok(()) => "S_OK".to_string(),
                Err(error) => error.to_string(),
            }
        }
    }

    fn create_factory() -> Result<IDXGIFactory4> {
        // SAFETY: Output interface is initialized by DXGI when the call succeeds.
        unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS::default()) }
            .map_err(|error| GfxError::Backend(error.to_string()))
    }

    /// Enumerates Direct3D 12 adapters visible through DXGI.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if the DXGI factory cannot be created or adapter
    /// descriptors cannot be read.
    pub fn enumerate_adapter_info() -> Result<Vec<AdapterInfo>> {
        let factory = create_factory()?;
        let mut adapters = Vec::new();
        let mut adapter_index = 0;
        loop {
            // SAFETY: Factory is valid and adapter_index is advanced monotonically.
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(GfxError::Backend(error.to_string())),
            };
            adapter_index += 1;

            // SAFETY: Adapter is valid and DXGI initializes the descriptor.
            let desc = unsafe { adapter.GetDesc1() }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_end_matches('\0')
                .to_string();
            adapters.push(AdapterInfo {
                backend: BackendKind::Dx12,
                name,
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                capabilities: BackendCapabilities {
                    surface: true,
                    cpu_visible_memory: true,
                    gpu_only_memory: true,
                },
            });
        }
        Ok(adapters)
    }

    struct Dx12AdapterDescription {
        name: String,
        dedicated_video_memory: usize,
        software: bool,
    }

    fn pick_adapter(factory: &IDXGIFactory4, desc: &DeviceDesc) -> Result<IDXGIAdapter1> {
        if let Some(requested_name) = desc.adapter_name.as_deref() {
            let requested_name = requested_name.trim();
            if !requested_name.is_empty() {
                return enumerate_adapters(factory)?
                    .into_iter()
                    .find_map(|(adapter, description)| {
                        description
                            .name
                            .eq_ignore_ascii_case(requested_name)
                            .then_some(adapter)
                    })
                    .ok_or_else(|| {
                        Dx12Error::Unavailable(format!(
                            "requested DX12 adapter is unavailable: {requested_name}"
                        ))
                        .into()
                    });
            }
        }

        if let Ok(factory6) = factory.cast::<IDXGIFactory6>()
            && let Some(adapter) = pick_adapter_by_gpu_preference(&factory6, desc.power_preference)?
        {
            return Ok(adapter);
        }

        let mut adapters = enumerate_adapters(factory)?
            .into_iter()
            .filter(|(_, description)| !description.software)
            .collect::<Vec<_>>();
        adapters.sort_by_key(|(_, description)| {
            fallback_adapter_rank(description.dedicated_video_memory, desc.power_preference)
        });
        if let Some((adapter, _)) = adapters.into_iter().next() {
            return Ok(adapter);
        }

        // SAFETY: The factory is live and initializes the requested WARP adapter interface.
        unsafe { factory.EnumWarpAdapter::<IDXGIAdapter1>() }.map_err(|error| {
            Dx12Error::Unavailable(format!(
                "no hardware DX12 adapter and WARP is unavailable: {error}"
            ))
            .into()
        })
    }

    fn pick_adapter_by_gpu_preference(
        factory: &IDXGIFactory6,
        power_preference: PowerPreference,
    ) -> Result<Option<IDXGIAdapter1>> {
        let preference = match power_preference {
            PowerPreference::LowPower => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
            PowerPreference::HighPerformance => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        };
        let mut adapter_index = 0;
        loop {
            // SAFETY: Factory is live and DXGI validates the monotonically increasing index.
            let adapter = match unsafe {
                factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(adapter_index, preference)
            } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return Ok(None),
                Err(error) => return Err(GfxError::Backend(error.to_string())),
            };
            adapter_index += 1;
            if !adapter_description(&adapter)?.software {
                return Ok(Some(adapter));
            }
        }
    }

    fn enumerate_adapters(
        factory: &IDXGIFactory4,
    ) -> Result<Vec<(IDXGIAdapter1, Dx12AdapterDescription)>> {
        let mut adapters = Vec::new();
        let mut adapter_index = 0;
        loop {
            // SAFETY: Factory is live and DXGI validates the monotonically increasing index.
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(GfxError::Backend(error.to_string())),
            };
            adapter_index += 1;
            let description = adapter_description(&adapter)?;
            adapters.push((adapter, description));
        }
        Ok(adapters)
    }

    fn adapter_description(adapter: &IDXGIAdapter1) -> Result<Dx12AdapterDescription> {
        // SAFETY: Adapter is live and DXGI initializes the returned descriptor.
        let description =
            unsafe { adapter.GetDesc1() }.map_err(|error| GfxError::Backend(error.to_string()))?;
        Ok(Dx12AdapterDescription {
            name: String::from_utf16_lossy(&description.Description)
                .trim_end_matches('\0')
                .to_string(),
            dedicated_video_memory: description.DedicatedVideoMemory,
            software: description.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0,
        })
    }

    fn fallback_adapter_rank(
        dedicated_video_memory: usize,
        power_preference: PowerPreference,
    ) -> usize {
        match power_preference {
            PowerPreference::LowPower => dedicated_video_memory,
            PowerPreference::HighPerformance => usize::MAX.saturating_sub(dedicated_video_memory),
        }
    }

    fn create_device(
        adapter: &IDXGIAdapter1,
    ) -> Result<windows::Win32::Graphics::Direct3D12::ID3D12Device> {
        let mut device = None;
        // SAFETY: Adapter is a valid DXGI adapter and output interface is initialized on success.
        unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &raw mut device) }
            .map_err(|error| GfxError::Backend(error.to_string()))?;
        device.ok_or_else(|| GfxError::Backend("D3D12CreateDevice returned no device".to_string()))
    }

    fn create_command_queue(
        device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    ) -> Result<ID3D12CommandQueue> {
        let desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        // SAFETY: Device is valid and queue descriptor is self-contained.
        unsafe { device.CreateCommandQueue(&raw const desc) }
            .map_err(|error| GfxError::Backend(error.to_string()))
    }

    fn create_fence(
        device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    ) -> Result<ID3D12Fence> {
        // SAFETY: Device is valid and output interface is initialized on success.
        unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }
            .map_err(|error| GfxError::Backend(error.to_string()))
    }

    fn create_command_allocator(device: &ID3D12Device) -> Result<ID3D12CommandAllocator> {
        // SAFETY: Device is valid and allocator type is direct.
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
            .map_err(|error| GfxError::Backend(error.to_string()))
    }

    fn create_command_list(
        device: &ID3D12Device,
        allocator: &ID3D12CommandAllocator,
    ) -> Result<ID3D12GraphicsCommandList> {
        // SAFETY: Device and allocator are valid; no initial pipeline state is bound here.
        unsafe {
            device.CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                allocator,
                Option::<&ID3D12PipelineState>::None,
            )
        }
        .map_err(|error| GfxError::Backend(error.to_string()))
    }

    fn create_empty_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
        let parameter = draw_step_constants_parameter();
        let root_signature_desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: 1,
            pParameters: &raw const parameter,
            NumStaticSamplers: 0,
            pStaticSamplers: ptr::null(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        };
        let mut root_signature_blob = None;
        let mut error_blob = None;
        // SAFETY: Root signature descriptor is self-contained and output blobs are initialized on success.
        match unsafe {
            D3D12SerializeRootSignature(
                &raw const root_signature_desc,
                D3D_ROOT_SIGNATURE_VERSION_1,
                &raw mut root_signature_blob,
                Some(&raw mut error_blob),
            )
        } {
            Ok(()) => {
                let blob = root_signature_blob.ok_or_else(|| {
                    GfxError::Backend("D3D12SerializeRootSignature returned no blob".to_string())
                })?;
                // SAFETY: Blob pointer and size are valid for the duration of this read-only view.
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast::<u8>(),
                        blob.GetBufferSize(),
                    )
                };
                // SAFETY: Serialized root signature bytes are produced by D3D12 itself.
                unsafe { device.CreateRootSignature(0, bytes) }
                    .map_err(|error| GfxError::Backend(error.to_string()))
            }
            Err(error) => {
                let message = error_blob
                    .as_ref()
                    .map_or_else(|| error.to_string(), blob_message);
                Err(GfxError::Backend(message))
            }
        }
    }

    #[expect(
        clippy::field_reassign_with_default,
        reason = "windows-rs D3D12 PSO structs are clearer when filled field-by-field"
    )]
    fn create_pipeline_state(
        device: &ID3D12Device,
        root_signature: &ID3D12RootSignature,
        vertex_shader: &Dx12ShaderModule,
        fragment_shader: &Dx12ShaderModule,
        color_format: Format,
        blend_mode: BlendMode,
        depth_format: Option<Format>,
        depth_enabled: bool,
    ) -> Result<ID3D12PipelineState> {
        let blend_desc = D3D12_BLEND_DESC {
            AlphaToCoverageEnable: false.into(),
            IndependentBlendEnable: false.into(),
            RenderTarget: [render_target_blend_desc(blend_mode); 8],
        };
        let rasterizer_desc = D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            FrontCounterClockwise: false.into(),
            DepthBias: D3D12_DEFAULT_DEPTH_BIAS,
            DepthBiasClamp: D3D12_DEFAULT_DEPTH_BIAS_CLAMP,
            SlopeScaledDepthBias: D3D12_DEFAULT_SLOPE_SCALED_DEPTH_BIAS,
            DepthClipEnable: true.into(),
            MultisampleEnable: false.into(),
            AntialiasedLineEnable: false.into(),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        };
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC::default();
        desc.pRootSignature = core::mem::ManuallyDrop::new(Some(root_signature.clone()));
        desc.VS = shader_bytecode(&vertex_shader.bytecode);
        desc.PS = shader_bytecode(&fragment_shader.bytecode);
        desc.StreamOutput = D3D12_STREAM_OUTPUT_DESC::default();
        desc.BlendState = blend_desc;
        desc.SampleMask = u32::MAX;
        desc.RasterizerState = rasterizer_desc;
        desc.DepthStencilState = if depth_enabled {
            D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
                ..Default::default()
            }
        } else {
            D3D12_DEPTH_STENCIL_DESC::default()
        };
        desc.InputLayout = D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: ptr::null(),
            NumElements: 0,
        };
        desc.IBStripCutValue = D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED;
        desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
        desc.NumRenderTargets = 1;
        desc.RTVFormats[0] = format_to_dxgi(color_format);
        desc.DSVFormat = depth_format.map_or(DXGI_FORMAT_UNKNOWN, format_to_dxgi);
        desc.SampleDesc = DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        };
        desc.NodeMask = 0;
        desc.CachedPSO = D3D12_CACHED_PIPELINE_STATE::default();
        desc.Flags = D3D12_PIPELINE_STATE_FLAG_NONE;
        clear_d3d12_messages(device);
        // SAFETY: Pipeline state description points to live root signature and shader blobs.
        let pipeline_state = unsafe { device.CreateGraphicsPipelineState(&raw const desc) };
        // SAFETY: The descriptor owns one temporary cloned COM reference for the call above.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut desc.pRootSignature);
        }
        pipeline_state.map_err(|error| {
            let messages = d3d12_messages(device);
            let suffix = if messages.is_empty() {
                String::new()
            } else {
                format!("; debug_messages={messages}")
            };
            GfxError::Backend(format!(
                "ID3D12Device::CreateGraphicsPipelineState failed: {error}{suffix}"
            ))
        })
    }

    fn shader_bytecode(bytecode: &[u8]) -> D3D12_SHADER_BYTECODE {
        D3D12_SHADER_BYTECODE {
            pShaderBytecode: bytecode.as_ptr().cast(),
            BytecodeLength: bytecode.len(),
        }
    }

    fn render_target_blend_desc(blend_mode: BlendMode) -> D3D12_RENDER_TARGET_BLEND_DESC {
        let (blend_enable, src_blend, dest_blend, src_blend_alpha, dest_blend_alpha) =
            match blend_mode {
                BlendMode::Replace => (
                    false.into(),
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_ZERO,
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_ZERO,
                ),
                BlendMode::Alpha => (
                    true.into(),
                    D3D12_BLEND_SRC_ALPHA,
                    D3D12_BLEND_INV_SRC_ALPHA,
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_INV_SRC_ALPHA,
                ),
                BlendMode::PremultipliedAlpha => (
                    true.into(),
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_INV_SRC_ALPHA,
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_INV_SRC_ALPHA,
                ),
                BlendMode::AdditiveAlpha => (
                    true.into(),
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_INV_SRC_ALPHA,
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_ONE,
                ),
                BlendMode::SubpixelDualSource => (
                    true.into(),
                    D3D12_BLEND_SRC1_COLOR,
                    D3D12_BLEND_INV_SRC1_COLOR,
                    D3D12_BLEND_ONE,
                    D3D12_BLEND_ZERO,
                ),
            };
        D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: blend_enable,
            LogicOpEnable: false.into(),
            SrcBlend: src_blend,
            DestBlend: dest_blend,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: src_blend_alpha,
            DestBlendAlpha: dest_blend_alpha,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: u8::try_from(D3D12_COLOR_WRITE_ENABLE_ALL.0).unwrap_or(u8::MAX),
        }
    }

    fn primitive_topology_to_dx12(topology: PrimitiveTopology) -> D3D_PRIMITIVE_TOPOLOGY {
        match topology {
            PrimitiveTopology::TriangleList => D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            PrimitiveTopology::TriangleStrip => D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
        }
    }

    fn dx12_rect_for_scissor(
        scissor: gfx_core::ScissorRect,
        extent: gfx_core::Extent2d,
    ) -> Result<windows::Win32::Foundation::RECT> {
        let x = scissor.x.min(extent.width());
        let y = scissor.y.min(extent.height());
        let right = scissor.x.saturating_add(scissor.width).min(extent.width());
        let bottom = scissor
            .y
            .saturating_add(scissor.height)
            .min(extent.height());
        Ok(windows::Win32::Foundation::RECT {
            left: i32::try_from(x)
                .map_err(|error| GfxError::InvalidInput(format!("scissor x overflow: {error}")))?,
            top: i32::try_from(y)
                .map_err(|error| GfxError::InvalidInput(format!("scissor y overflow: {error}")))?,
            right: i32::try_from(right).map_err(|error| {
                GfxError::InvalidInput(format!("scissor right overflow: {error}"))
            })?,
            bottom: i32::try_from(bottom).map_err(|error| {
                GfxError::InvalidInput(format!("scissor bottom overflow: {error}"))
            })?,
        })
    }

    fn union_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
        let x = left.x.min(right.x);
        let y = left.y.min(right.y);
        let right_edge = left
            .x
            .saturating_add(left.width)
            .max(right.x.saturating_add(right.width));
        let bottom_edge = left
            .y
            .saturating_add(left.height)
            .max(right.y.saturating_add(right.height));
        ScissorRect {
            x,
            y,
            width: right_edge.saturating_sub(x),
            height: bottom_edge.saturating_sub(y),
        }
    }

    fn intersect_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
        let x = left.x.max(right.x);
        let y = left.y.max(right.y);
        let right_edge = left
            .x
            .saturating_add(left.width)
            .min(right.x.saturating_add(right.width));
        let bottom_edge = left
            .y
            .saturating_add(left.height)
            .min(right.y.saturating_add(right.height));
        ScissorRect {
            x,
            y,
            width: right_edge.saturating_sub(x),
            height: bottom_edge.saturating_sub(y),
        }
    }

    fn rebuild_render_targets(device: &ID3D12Device, swapchain: &mut Dx12Swapchain) -> Result<()> {
        let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: BACK_BUFFER_COUNT,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        };
        // SAFETY: Device is valid and descriptor heap desc is self-contained.
        let rtv_heap: ID3D12DescriptorHeap =
            unsafe { device.CreateDescriptorHeap(&raw const heap_desc) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
        // SAFETY: Device is valid and returns a static descriptor size for RTV heaps.
        let descriptor_size =
            unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) };
        // SAFETY: Descriptor heap is valid and owns CPU descriptors.
        let heap_start = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        let mut render_targets = Vec::with_capacity(BACK_BUFFER_COUNT as usize);
        for index in 0..BACK_BUFFER_COUNT {
            // SAFETY: Backbuffer index is within BufferCount.
            let resource: ID3D12Resource = unsafe { swapchain.swapchain.GetBuffer(index) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let handle = descriptor_handle_at(heap_start, descriptor_size, index)?;
            // SAFETY: Resource is a swapchain backbuffer and handle points into the RTV heap.
            unsafe {
                device.CreateRenderTargetView(&resource, None, handle);
            }
            render_targets.push(resource);
        }
        // SAFETY: Swapchain is valid.
        swapchain.frame_index = unsafe { swapchain.swapchain.GetCurrentBackBufferIndex() };
        swapchain.rtv_heap = Some(rtv_heap);
        swapchain.render_targets = render_targets;
        swapchain.rtv_descriptor_size = descriptor_size;
        swapchain.pending_damage = vec![Dx12BackBufferDamage::Full; BACK_BUFFER_COUNT as usize];
        Ok(())
    }

    fn descriptor_handle_at(
        start: D3D12_CPU_DESCRIPTOR_HANDLE,
        increment: u32,
        index: u32,
    ) -> Result<D3D12_CPU_DESCRIPTOR_HANDLE> {
        let offset = usize::try_from(u64::from(increment) * u64::from(index)).map_err(|error| {
            GfxError::InvalidInput(format!("descriptor handle offset overflow: {error}"))
        })?;
        let ptr = start.ptr.checked_add(offset).ok_or_else(|| {
            GfxError::InvalidInput("descriptor handle offset overflow".to_string())
        })?;
        Ok(D3D12_CPU_DESCRIPTOR_HANDLE { ptr })
    }

    fn transition_barrier(
        resource: &ID3D12Resource,
        before: D3D12_RESOURCE_STATES,
        after: D3D12_RESOURCE_STATES,
    ) -> D3D12_RESOURCE_BARRIER {
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: core::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: core::mem::ManuallyDrop::new(Some(resource.clone())),
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: before,
                    StateAfter: after,
                }),
            },
        }
    }

    fn record_transition_barrier(
        command_list: &ID3D12GraphicsCommandList,
        resource: &ID3D12Resource,
        before: D3D12_RESOURCE_STATES,
        after: D3D12_RESOURCE_STATES,
    ) {
        let mut barrier = transition_barrier(resource, before, after);
        // SAFETY: Command list is open and the barrier references a live resource for this call.
        unsafe { command_list.ResourceBarrier(std::slice::from_ref(&barrier)) };
        release_transition_barrier_resource(&mut barrier);
    }

    fn release_transition_barrier_resource(barrier: &mut D3D12_RESOURCE_BARRIER) {
        // SAFETY: `transition_barrier` always constructs the Transition variant and owns exactly
        // one cloned COM reference in pResource. Dropping it after ResourceBarrier releases only
        // the temporary reference, not the original resource held by the registry.
        unsafe {
            let transition = &mut *barrier.Anonymous.Transition;
            core::mem::ManuallyDrop::drop(&mut transition.pResource);
        }
    }

    fn release_texture_copy_location_resource(location: &mut D3D12_TEXTURE_COPY_LOCATION) {
        // SAFETY: The copy location owns exactly one cloned COM reference in pResource. The D3D12
        // call has already copied the location data into the command stream.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut location.pResource);
        }
    }

    fn present_mode_to_dxgi(mode: PresentMode, supports_tearing: bool) -> (u32, DXGI_PRESENT) {
        match mode {
            PresentMode::Immediate => (
                0,
                if supports_tearing {
                    DXGI_PRESENT_ALLOW_TEARING
                } else {
                    DXGI_PRESENT::default()
                },
            ),
            // Flip-model present with sync interval 0 and no tearing flag gives
            // latest-frame-wins semantics at the next vblank (true mailbox).
            PresentMode::Mailbox => (0, DXGI_PRESENT::default()),
            PresentMode::Fifo => (1, DXGI_PRESENT::default()),
        }
    }

    fn blob_message(blob: &ID3DBlob) -> String {
        // SAFETY: D3DBlob pointer and size are valid for the lifetime of the blob COM object.
        let bytes = unsafe {
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
        };
        String::from_utf8_lossy(bytes).into_owned()
    }

    struct FenceEvent(HANDLE);

    impl FenceEvent {
        fn new() -> Result<Self> {
            // SAFETY: Requesting an unnamed manual-reset event with initial non-signaled state.
            let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            Ok(Self(handle))
        }
    }

    impl GfxBackend for Dx12Device {
        const BACKEND_KIND: BackendKind = BackendKind::Dx12;
    }

    impl GfxSurfaceDevice for Dx12Device {
        type SurfaceTarget = dyn Dx12SurfaceTarget;

        fn create_surface(
            &mut self,
            target: &Self::SurfaceTarget,
            desc: &SurfaceDesc,
        ) -> Result<SurfaceId> {
            Self::create_surface(self, target, desc)
        }

        fn create_swapchain(
            &mut self,
            surface: SurfaceId,
            config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            Self::create_swapchain(self, surface, config)
        }

        fn destroy_swapchain(&mut self, swapchain: SwapchainId) -> Result<()> {
            Self::destroy_swapchain(self, swapchain)
        }

        fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
            Self::destroy_surface(self, surface)
        }
    }

    impl GfxResourceDevice for Dx12Device {
        fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
            Self::create_buffer(self, desc)
        }

        fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
            Self::write_buffer(self, buffer, offset, data)
        }

        fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
            Self::create_texture(self, desc)
        }

        fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
            Self::write_texture(self, desc, data)
        }

        fn write_texture_batch<'a>(
            &mut self,
            writes: impl IntoIterator<Item = TextureWrite<'a>>,
        ) -> Result<()> {
            Self::write_texture_batch(self, writes)
        }

        fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
            Self::create_texture_view(self, desc)
        }

        fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
            Self::create_sampler(self, desc)
        }

        fn create_resource_set_layout(
            &mut self,
            desc: &ResourceSetLayoutDesc,
        ) -> Result<ResourceSetLayoutId> {
            Self::create_resource_set_layout(self, desc)
        }

        fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
            Self::create_resource_set(self, desc)
        }

        fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
            Self::destroy_buffer(self, buffer)
        }

        fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
            Self::destroy_texture(self, texture)
        }

        fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
            Self::destroy_texture_view(self, view)
        }

        fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
            Self::destroy_sampler(self, sampler)
        }

        fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
            Self::destroy_resource_set_layout(self, layout)
        }

        fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
            Self::destroy_resource_set(self, resource_set)
        }
    }

    impl GfxPipelineDevice for Dx12Device {
        fn create_pipeline_layout(
            &mut self,
            desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
            Self::create_pipeline_layout(self, desc)
        }

        fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
            Self::create_shader_module(self, desc)
        }

        fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
            Self::create_render_pass(self, desc)
        }

        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDesc,
            viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            Self::create_render_pipeline(self, desc, viewport_extent)
        }

        fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
            Self::destroy_pipeline_layout(self, layout)
        }

        fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
            Self::destroy_shader_module(self, shader)
        }

        fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
            Self::destroy_render_pass(self, render_pass)
        }

        fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
            Self::destroy_render_pipeline(self, pipeline)
        }
    }

    impl GfxCommandDevice for Dx12Device {
        fn create_command_encoder(
            &mut self,
            desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
            Self::create_command_encoder(self, desc)
        }

        fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDesc) -> Result<()> {
            Self::record_draw_desc(self, encoder, &draw)
        }

        fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::submit(self, encoder)
        }

        fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
            Self::destroy_command_encoder(self, encoder)
        }
    }

    impl GfxSubmissionDevice for Dx12Device {
        fn async_capabilities(&self) -> gfx_core::GfxAsyncCapabilities {
            gfx_core::GfxAsyncCapabilities {
                threading_mode: GfxThreadingMode::MultiThreadDeviceProxy,
                async_submission: true,
                async_wait: true,
                async_presentation: true,
                partial_presentation: true,
            }
        }

        fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
            Self::submit_deferred(self, encoder)
        }

        fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
            Self::poll_submission(self, submission)
        }

        fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
            Self::wait_submission(self, submission)
        }
    }

    impl GfxPresentationDevice for Dx12Device {
        fn supports_partial_presentation(&self, swapchain: SwapchainId) -> bool {
            self.swapchains
                .get(swapchain)
                .is_ok_and(|swapchain| swapchain.partial_presentation)
        }

        fn set_swapchain_content_stretch(
            &mut self,
            swapchain: SwapchainId,
            scale: Option<[f32; 2]>,
        ) -> Result<()> {
            let record = self.swapchains.get(swapchain)?;
            let Some(composition) = record.composition.as_ref() else {
                return Ok(());
            };
            let [scale_x, scale_y] = scale.unwrap_or([1.0, 1.0]);
            let matrix = Matrix3x2 {
                M11: scale_x,
                M12: 0.0,
                M21: 0.0,
                M22: scale_y,
                M31: 0.0,
                M32: 0.0,
            };
            // SAFETY: The composition device, visual, and transform are live members
            // of the swapchain record and outlive this call.
            unsafe {
                composition
                    .transform
                    .SetMatrix(&raw const matrix)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                composition
                    .visual
                    .SetTransform(&composition.transform)
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                Self::commit_composition(composition)?;
            }
            Ok(())
        }

        fn draw_steps_and_present(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: ClearColor,
        ) -> Result<()> {
            Self::draw_steps_and_present(self, swapchain, render_pass, steps, clear_color)
        }

        fn draw_steps_to_texture(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            Self::draw_steps_to_texture(self, texture_view, render_pass, steps, color_load_op)
        }

        fn draw_steps_and_present_deferred(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[DrawStepDesc],
            clear_color: ClearColor,
        ) -> Result<SubmissionId>
        where
            Self: GfxSubmissionDevice,
        {
            Self::render_step_list_and_present_deferred(
                self,
                swapchain,
                render_pass,
                RenderStepList::from_draw_steps(steps),
                clear_color,
                None,
            )
        }

        fn render_steps_and_present_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Self::render_steps_and_present_with_depth(
                self,
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        }

        fn render_step_list_and_present_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Self::render_step_list_and_present_with_depth(
                self,
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        }

        fn render_step_list_and_present_with_damage_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
            damage: Option<ScissorRect>,
        ) -> Result<()> {
            Self::render_step_list_and_present_with_depth_and_damage(
                self,
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                damage,
            )
        }

        fn render_steps_to_texture_compat(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Self::render_steps_to_texture_with_depth(
                self,
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            )
        }

        fn render_step_list_to_texture_compat(
            &mut self,
            texture_view: TextureViewId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            color_load_op: LoadOp<ClearColor>,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<()> {
            Self::render_step_list_to_texture_with_depth(
                self,
                texture_view,
                render_pass,
                steps,
                color_load_op,
                depth_attachment,
            )
        }

        fn render_steps_and_present_deferred_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: &[RenderStepDescriptor],
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<SubmissionId>
        where
            Self: GfxSubmissionDevice,
        {
            Self::render_step_list_and_present_deferred(
                self,
                swapchain,
                render_pass,
                RenderStepList::from_render_steps(steps),
                clear_color,
                depth_attachment,
            )
        }

        fn render_step_list_and_present_deferred_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
        ) -> Result<SubmissionId>
        where
            Self: GfxSubmissionDevice,
        {
            Self::render_step_list_and_present_deferred(
                self,
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
            )
        }

        fn render_step_list_and_present_deferred_with_damage_compat(
            &mut self,
            swapchain: SwapchainId,
            render_pass: RenderPassId,
            steps: RenderStepList<'_>,
            clear_color: ClearColor,
            depth_attachment: Option<RenderPassDepthAttachment>,
            damage: Option<ScissorRect>,
        ) -> Result<SubmissionId>
        where
            Self: GfxSubmissionDevice,
        {
            Self::render_step_list_and_present_deferred_with_damage(
                self,
                swapchain,
                render_pass,
                steps,
                clear_color,
                depth_attachment,
                damage,
            )
        }
    }

    impl GfxDiagnosticsDevice for Dx12Device {
        fn resource_stats(&self) -> ResourceStats {
            Self::resource_stats(self)
        }
    }

    impl GfxTextureTransferDevice for Dx12Device {
        fn texture_transfer_timestamps_supported(&self) -> bool {
            self.timestamp_frequency.is_some()
        }

        fn wait_texture_transfers(&mut self) -> Result<()> {
            self.wait_for_pending_work()
        }

        fn last_texture_transfer_time(&self) -> Option<Duration> {
            self.last_texture_transfer_time
        }

        fn read_texture(&mut self, texture: TextureId) -> Result<TextureReadback> {
            self.read_texture_pixels(texture)
        }
    }

    impl Drop for FenceEvent {
        fn drop(&mut self) {
            // SAFETY: The event handle is owned by this wrapper and may be closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn compile_hlsl_to_dx_bytecode(
        source: &str,
        entry_point: &str,
        stage: ShaderStage,
    ) -> Result<Vec<u8>> {
        let target = match stage {
            ShaderStage::Vertex => b"vs_5_1\0",
            ShaderStage::Fragment => b"ps_5_1\0",
        };
        let entry_point = std::ffi::CString::new(entry_point)
            .map_err(|error| GfxError::InvalidInput(error.to_string()))?;
        let mut bytecode = None;
        let mut errors = None;
        // SAFETY: Source, entry point, and target pointers remain valid for the call duration.
        unsafe {
            D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry_point.as_ptr().cast()),
                PCSTR(target.as_ptr()),
                0,
                0,
                &raw mut bytecode,
                Some(&raw mut errors),
            )
        }
        .map_err(|error| GfxError::Shader(error.to_string()))?;
        let bytecode = bytecode.ok_or_else(|| {
            GfxError::Shader("D3DCompile did not return shader bytecode".to_string())
        })?;
        // SAFETY: D3DCompile returned a valid blob; the pointer and size are read-only here.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                bytecode.GetBufferPointer().cast::<u8>(),
                bytecode.GetBufferSize(),
            )
        };
        Ok(bytes.to_vec())
    }

    fn create_root_signature(
        device: &ID3D12Device,
        layouts: &[ResourceSetLayoutDesc],
    ) -> Result<ID3D12RootSignature> {
        let mut ranges = Vec::new();
        for (group_index, layout) in layouts.iter().enumerate() {
            for entry in &layout.entries {
                let sampler_index_register = sampler_index_buffer_register(group_index)?;
                let range_type = match entry.binding_type {
                    ResourceBindingType::UniformBuffer => D3D12_DESCRIPTOR_RANGE_TYPE_CBV,
                    ResourceBindingType::StorageBuffer | ResourceBindingType::SampledTexture => {
                        D3D12_DESCRIPTOR_RANGE_TYPE_SRV
                    }
                    ResourceBindingType::Sampler => D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
                };
                ranges.push(D3D12_DESCRIPTOR_RANGE {
                    RangeType: range_type,
                    NumDescriptors: if entry.binding_type == ResourceBindingType::Sampler {
                        NAGA_HLSL_SAMPLER_HEAP_SIZE
                    } else {
                        1
                    },
                    BaseShaderRegister: if entry.binding_type == ResourceBindingType::Sampler {
                        0
                    } else {
                        entry.binding
                    },
                    RegisterSpace: if entry.binding_type == ResourceBindingType::Sampler {
                        0
                    } else {
                        PHASE1_RESOURCE_SET_SPACE
                    },
                    OffsetInDescriptorsFromTableStart: 0,
                });
                if entry.binding_type == ResourceBindingType::Sampler {
                    ranges.push(D3D12_DESCRIPTOR_RANGE {
                        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                        NumDescriptors: 1,
                        BaseShaderRegister: sampler_index_register,
                        RegisterSpace: NAGA_HLSL_SAMPLER_INDEX_SPACE,
                        OffsetInDescriptorsFromTableStart: 0,
                    });
                }
            }
        }
        let mut parameters = Vec::with_capacity(ranges.len().saturating_add(1));
        let mut range_index = 0;
        for layout in layouts {
            for entry in &layout.entries {
                parameters.push(D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        DescriptorTable:
                            windows::Win32::Graphics::Direct3D12::D3D12_ROOT_DESCRIPTOR_TABLE {
                                NumDescriptorRanges: 1,
                                pDescriptorRanges: &raw const ranges[range_index],
                            },
                    },
                    ShaderVisibility: shader_stages_to_dx12(entry.stages),
                });
                range_index += 1;
                if entry.binding_type == ResourceBindingType::Sampler {
                    parameters.push(D3D12_ROOT_PARAMETER {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                        Anonymous: D3D12_ROOT_PARAMETER_0 {
                            DescriptorTable:
                                windows::Win32::Graphics::Direct3D12::D3D12_ROOT_DESCRIPTOR_TABLE {
                                    NumDescriptorRanges: 1,
                                    pDescriptorRanges: &raw const ranges[range_index],
                                },
                        },
                        ShaderVisibility: shader_stages_to_dx12(entry.stages),
                    });
                    range_index += 1;
                }
            }
        }
        parameters.push(draw_step_constants_parameter());
        serialize_root_signature(device, &parameters)
    }

    fn sampler_index_buffer_register(group_index: usize) -> Result<u32> {
        u32::try_from(group_index).map_err(|error| {
            GfxError::InvalidInput(format!("resource set group index overflow: {error}"))
        })
    }

    fn draw_step_constants_root_index(layouts: &[ResourceSetLayoutDesc]) -> Result<u32> {
        let mut root_index = 0_u32;
        for layout in layouts {
            for entry in &layout.entries {
                root_index = root_index.checked_add(1).ok_or_else(|| {
                    GfxError::InvalidInput("root parameter index overflow".to_string())
                })?;
                if entry.binding_type == ResourceBindingType::Sampler {
                    root_index = root_index.checked_add(1).ok_or_else(|| {
                        GfxError::InvalidInput("root parameter index overflow".to_string())
                    })?;
                }
            }
        }
        Ok(root_index)
    }

    fn draw_step_constants_parameter() -> D3D12_ROOT_PARAMETER {
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: NAGA_HLSL_SPECIAL_CONSTANTS_REGISTER,
                    RegisterSpace: NAGA_HLSL_SPECIAL_CONSTANTS_SPACE,
                    Num32BitValues: 3,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        }
    }

    fn serialize_root_signature(
        device: &ID3D12Device,
        parameters: &[D3D12_ROOT_PARAMETER],
    ) -> Result<ID3D12RootSignature> {
        let root_signature_desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: u32::try_from(parameters.len()).map_err(|error| {
                GfxError::InvalidInput(format!("root parameter count overflow: {error}"))
            })?,
            pParameters: parameters.as_ptr(),
            NumStaticSamplers: 0,
            pStaticSamplers: ptr::null(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        };
        let mut root_signature_blob = None;
        let mut error_blob = None;
        // SAFETY: Root signature descriptor references live parameter and range arrays.
        match unsafe {
            D3D12SerializeRootSignature(
                &raw const root_signature_desc,
                D3D_ROOT_SIGNATURE_VERSION_1,
                &raw mut root_signature_blob,
                Some(&raw mut error_blob),
            )
        } {
            Ok(()) => {
                let blob = root_signature_blob.ok_or_else(|| {
                    GfxError::Backend("D3D12SerializeRootSignature returned no blob".to_string())
                })?;
                // SAFETY: Blob pointer and size are valid for the duration of this read-only view.
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast::<u8>(),
                        blob.GetBufferSize(),
                    )
                };
                // SAFETY: Serialized root signature bytes are produced by D3D12 itself.
                unsafe { device.CreateRootSignature(0, bytes) }.map_err(|error| {
                    GfxError::Backend(format!("ID3D12Device::CreateRootSignature failed: {error}"))
                })
            }
            Err(error) => {
                let message = error_blob
                    .as_ref()
                    .map_or_else(|| error.to_string(), blob_message);
                Err(GfxError::Backend(format!(
                    "D3D12SerializeRootSignature failed: {message}"
                )))
            }
        }
    }

    fn shader_stages_to_dx12(_stages: ShaderStages) -> D3D12_SHADER_VISIBILITY {
        D3D12_SHADER_VISIBILITY_ALL
    }

    fn composite_alpha_to_dxgi(alpha_mode: CompositeAlphaMode) -> DXGI_ALPHA_MODE {
        match alpha_mode {
            CompositeAlphaMode::Auto => DXGI_ALPHA_MODE_UNSPECIFIED,
            CompositeAlphaMode::Opaque => DXGI_ALPHA_MODE_IGNORE,
            CompositeAlphaMode::Premultiplied => DXGI_ALPHA_MODE_PREMULTIPLIED,
            CompositeAlphaMode::Postmultiplied => DXGI_ALPHA_MODE_STRAIGHT,
            CompositeAlphaMode::Inherit => DXGI_ALPHA_MODE_UNSPECIFIED,
        }
    }

    fn enable_debug_layer_if_requested() {
        if std::env::var_os("NOVA_GFX_DX12_DEBUG").is_none() {
            return;
        }
        let mut debug = None;
        // SAFETY: D3D12 initializes the output interface on success.
        if unsafe { D3D12GetDebugInterface::<ID3D12Debug>(&raw mut debug) }.is_ok()
            && let Some(debug) = debug
        {
            // SAFETY: The debug interface is valid and may be enabled before device creation.
            unsafe {
                debug.EnableDebugLayer();
            }
        }
    }

    fn clear_d3d12_messages(device: &ID3D12Device) {
        if let Ok(info_queue) = device.cast::<ID3D12InfoQueue>() {
            // SAFETY: Info queue belongs to the live device.
            unsafe {
                info_queue.ClearStoredMessages();
            }
        }
    }

    fn d3d12_messages(device: &ID3D12Device) -> String {
        let Ok(info_queue) = device.cast::<ID3D12InfoQueue>() else {
            return String::new();
        };
        // SAFETY: Info queue belongs to the live device.
        let count = unsafe { info_queue.GetNumStoredMessages() };
        if count == 0 {
            return String::new();
        }
        let start = count.saturating_sub(8);
        let mut messages = Vec::new();
        for index in start..count {
            if let Some(message) = d3d12_message(&info_queue, index) {
                messages.push(message);
            }
        }
        messages.join(" | ")
    }

    fn d3d12_message(info_queue: &ID3D12InfoQueue, index: u64) -> Option<String> {
        let mut byte_length = 0;
        // SAFETY: A null message pointer asks D3D12 for the required size.
        unsafe { info_queue.GetMessage(index, None, &raw mut byte_length) }.ok()?;
        if byte_length == 0 {
            return None;
        }
        let word_count = byte_length.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; word_count];
        let message_pointer = storage.as_mut_ptr().cast::<D3D12_MESSAGE>();
        // SAFETY: Storage is aligned and large enough for the returned message bytes.
        unsafe { info_queue.GetMessage(index, Some(message_pointer), &raw mut byte_length) }
            .ok()?;
        // SAFETY: D3D12 wrote a valid D3D12_MESSAGE structure into message_pointer.
        let message = unsafe { &*message_pointer };
        // SAFETY: Description pointer/length are part of the returned message blob.
        let bytes = unsafe {
            std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength)
        };
        let description = str::from_utf8(bytes).map_or_else(
            |_| String::new(),
            |text| text.trim_end_matches('\0').to_string(),
        );
        Some(format!(
            "severity={:?} id={:?}: {description}",
            message.Severity, message.ID
        ))
    }

    fn create_buffer_resource(device: &ID3D12Device, desc: &BufferDesc) -> Result<ID3D12Resource> {
        let heap_type = match desc.memory_location {
            MemoryLocation::CpuToGpu => D3D12_HEAP_TYPE_UPLOAD,
            MemoryLocation::GpuToCpu => D3D12_HEAP_TYPE_READBACK,
            MemoryLocation::GpuOnly => D3D12_HEAP_TYPE_DEFAULT,
        };
        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: heap_type,
            ..Default::default()
        };
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: align_to(desc.size, 256),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let initial_state = match desc.memory_location {
            MemoryLocation::CpuToGpu => D3D12_RESOURCE_STATE_GENERIC_READ,
            MemoryLocation::GpuToCpu | MemoryLocation::GpuOnly => D3D12_RESOURCE_STATE_COPY_DEST,
        };
        let mut resource = None;
        // SAFETY: Device, heap properties, and resource desc are valid for committed resource creation.
        unsafe {
            device.CreateCommittedResource(
                &raw const heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &raw const resource_desc,
                initial_state,
                None,
                &raw mut resource,
            )
        }
        .map_err(|error| GfxError::Backend(error.to_string()))?;
        resource.ok_or_else(|| {
            GfxError::Backend("CreateCommittedResource returned no buffer".to_string())
        })
    }

    fn create_texture_resource(
        device: &ID3D12Device,
        desc: &TextureDesc,
    ) -> Result<ID3D12Resource> {
        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let mut resource_flags = D3D12_RESOURCE_FLAG_NONE;
        if desc.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
            resource_flags |= D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
        }
        if desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
            resource_flags |= D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
        }
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: u64::from(desc.size.width()),
            Height: desc.size.height(),
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format_to_dxgi(desc.format),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: resource_flags,
        };
        let mut resource = None;
        // SAFETY: Device, heap properties, and texture desc are valid for committed resource creation.
        unsafe {
            device.CreateCommittedResource(
                &raw const heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &raw const resource_desc,
                initial_texture_state(desc),
                None,
                &raw mut resource,
            )
        }
        .map_err(|error| GfxError::Backend(error.to_string()))?;
        resource.ok_or_else(|| {
            GfxError::Backend("CreateCommittedResource returned no texture".to_string())
        })
    }

    fn initial_texture_state(desc: &TextureDesc) -> D3D12_RESOURCE_STATES {
        if desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
            D3D12_RESOURCE_STATE_DEPTH_WRITE
        } else {
            D3D12_RESOURCE_STATE_COPY_DEST
        }
    }

    fn upload_to_mapped_buffer(
        resource: &ID3D12Resource,
        offset: usize,
        data: &[u8],
    ) -> Result<()> {
        let mut mapped = ptr::null_mut();
        // SAFETY: Resource is an upload heap buffer and the mapped range is written immediately.
        unsafe { resource.Map(0, None, Some(&raw mut mapped)) }
            .map_err(|error| GfxError::Backend(error.to_string()))?;
        // SAFETY: Mapped pointer is valid for the buffer allocation and offset/range was checked by caller.
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), mapped.cast::<u8>().add(offset), data.len());
            resource.Unmap(0, None);
        }
        Ok(())
    }

    fn upload_textures_2d(
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        copies: &[Dx12TextureCopyOwned],
        recycled: Option<Dx12UploadCommands>,
        timestamps_enabled: bool,
    ) -> Result<Option<Dx12SubmittedCommandList>> {
        if copies.is_empty() {
            return Ok(None);
        }
        for copy in copies {
            validate_dx12_texture_copy(copy.upload_offset, copy.row_pitch)?;
        }
        let commands = match recycled {
            Some(commands) => {
                // SAFETY: Pooled objects only re-enter circulation after their
                // previous submission's fence completed, so Reset is safe here.
                unsafe { commands.allocator.Reset() }
                    .map_err(|error| GfxError::Backend(error.to_string()))?;
                // SAFETY: The pooled command list was closed after its previous
                // submission; Reset reopens it against the reset allocator.
                unsafe {
                    commands
                        .graphics_command_list
                        .Reset(&commands.allocator, None)
                }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
                commands
            }
            None => create_dx12_upload_commands(device, timestamps_enabled)?,
        };
        let command_list = &commands.graphics_command_list;
        if let Some(timestamps) = &commands.timestamps {
            // SAFETY: The command list is recording and query slot zero is valid.
            unsafe { command_list.EndQuery(&timestamps.heap, D3D12_QUERY_TYPE_TIMESTAMP, 0) };
        }
        record_dx12_texture_copies(command_list, copies);
        if let Some(timestamps) = &commands.timestamps {
            // SAFETY: Both timestamp slots and the 16-byte readback range are valid.
            unsafe {
                command_list.EndQuery(&timestamps.heap, D3D12_QUERY_TYPE_TIMESTAMP, 1);
                command_list.ResolveQueryData(
                    &timestamps.heap,
                    D3D12_QUERY_TYPE_TIMESTAMP,
                    0,
                    2,
                    &timestamps.readback,
                    0,
                );
            }
        }
        // SAFETY: Command list was opened by create_command_list and is ready to close.
        unsafe { command_list.Close() }.map_err(|error| GfxError::Backend(error.to_string()))?;
        let executable: ID3D12CommandList = commands
            .graphics_command_list
            .cast()
            .map_err(|error| GfxError::Backend(error.to_string()))?;
        // SAFETY: Command list is closed and ready to execute.
        unsafe {
            queue.ExecuteCommandLists(&[Some(executable.clone())]);
        }
        Ok(Some(Dx12SubmittedCommandList {
            commands,
            _command_list: executable,
        }))
    }

    fn record_dx12_texture_copies(
        command_list: &ID3D12GraphicsCommandList,
        copies: &[Dx12TextureCopyOwned],
    ) {
        let mut transitioned_textures = Vec::new();
        for copy in copies {
            if !transitioned_textures.contains(&copy.texture_id) {
                if copy.old_state != D3D12_RESOURCE_STATE_COPY_DEST {
                    record_transition_barrier(
                        command_list,
                        &copy.texture,
                        copy.old_state,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    );
                }
                transitioned_textures.push(copy.texture_id);
            }
            let footprint =
                windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: copy.upload_offset,
                    Footprint: windows::Win32::Graphics::Direct3D12::D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format_to_dxgi(copy.format),
                        Width: copy.desc.size.width(),
                        Height: copy.desc.size.height(),
                        Depth: 1,
                        RowPitch: copy.row_pitch,
                    },
                };
            let mut source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: core::mem::ManuallyDrop::new(Some(copy.upload.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: footprint,
                },
            };
            let mut destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: core::mem::ManuallyDrop::new(Some(copy.texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            // SAFETY: Command list is open and copy locations reference live resources.
            unsafe {
                command_list.CopyTextureRegion(
                    &raw const destination,
                    copy.desc.origin.x,
                    copy.desc.origin.y,
                    0,
                    &raw const source,
                    None,
                );
            }
            release_texture_copy_location_resource(&mut source);
            release_texture_copy_location_resource(&mut destination);
        }
        let mut restored_textures = Vec::new();
        for copy in copies {
            if restored_textures.contains(&copy.texture_id) {
                continue;
            }
            record_transition_barrier(
                command_list,
                &copy.texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            );
            restored_textures.push(copy.texture_id);
        }
    }

    fn create_dx12_upload_commands(
        device: &ID3D12Device,
        timestamps_enabled: bool,
    ) -> Result<Dx12UploadCommands> {
        let allocator = create_command_allocator(device)?;
        let graphics_command_list = create_command_list(device, &allocator)?;
        let timestamps = if timestamps_enabled {
            let query_desc = D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                Count: 2,
                NodeMask: 0,
            };
            let mut heap = None;
            // SAFETY: The descriptor and output slot are valid and the device is live.
            unsafe { device.CreateQueryHeap(&raw const query_desc, &raw mut heap) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let heap = heap.ok_or_else(|| {
                GfxError::Backend("DX12 timestamp query heap creation returned null".to_string())
            })?;
            let readback_desc = BufferDesc {
                label: Some("nova-gfx DX12 upload timestamps".to_string()),
                size: 16,
                usage: BufferUsage::COPY_DST,
                memory_location: MemoryLocation::GpuToCpu,
            };
            let readback = create_buffer_resource(device, &readback_desc)?;
            Some(Dx12TimestampQueries { heap, readback })
        } else {
            None
        };
        Ok(Dx12UploadCommands {
            allocator,
            graphics_command_list,
            timestamps,
        })
    }

    fn read_dx12_timestamps(readback: &ID3D12Resource, frequency: u64) -> Result<Duration> {
        let mut mapped = ptr::null_mut();
        let read_range = D3D12_RANGE { Begin: 0, End: 16 };
        // SAFETY: The resource is a readback buffer containing two resolved u64 timestamps.
        unsafe { readback.Map(0, Some(&raw const read_range), Some(&raw mut mapped)) }
            .map_err(|error| GfxError::Backend(error.to_string()))?;
        let Some(mapped) = NonNull::new(mapped.cast::<u64>()) else {
            let written_range = D3D12_RANGE { Begin: 0, End: 0 };
            // SAFETY: Map succeeded above and must be balanced before returning.
            unsafe { readback.Unmap(0, Some(&raw const written_range)) };
            return Err(GfxError::Backend(
                "DX12 timestamp mapping returned a null pointer".to_string(),
            ));
        };
        // SAFETY: The mapped range contains two u64 values written by ResolveQueryData.
        let (start, end) = unsafe {
            (
                ptr::read_unaligned(mapped.as_ptr()),
                ptr::read_unaligned(mapped.as_ptr().add(1)),
            )
        };
        let written_range = D3D12_RANGE { Begin: 0, End: 0 };
        // SAFETY: The mapping was used for reads and is balanced once.
        unsafe { readback.Unmap(0, Some(&raw const written_range)) };
        let elapsed_nanoseconds = u128::from(end.saturating_sub(start))
            .saturating_mul(1_000_000_000)
            .checked_div(u128::from(frequency))
            .unwrap_or(0);
        Ok(Duration::from_nanos(
            u64::try_from(elapsed_nanoseconds).unwrap_or(u64::MAX),
        ))
    }

    fn factory_supports_tearing(factory: &IDXGIFactory4) -> bool {
        let Ok(factory5) = factory.cast::<IDXGIFactory5>() else {
            return false;
        };
        let mut allow_tearing = BOOL::default();
        // SAFETY: Output pointer and size describe a single BOOL as required by
        // DXGI_FEATURE_PRESENT_ALLOW_TEARING.
        let supported = unsafe {
            factory5.CheckFeatureSupport(
                DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                (&raw mut allow_tearing).cast(),
                u32::try_from(size_of::<BOOL>()).unwrap_or(0),
            )
        };
        supported.is_ok() && allow_tearing.as_bool()
    }

    fn validate_dx12_texture_copy(upload_offset: u64, row_pitch: u32) -> Result<()> {
        if upload_offset % DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT != 0 {
            return Err(GfxError::InvalidInput(format!(
                "DX12 texture upload offset {upload_offset} is not aligned to {DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT} bytes"
            )));
        }
        if u64::from(row_pitch) % DX12_TEXTURE_DATA_PITCH_ALIGNMENT != 0 {
            return Err(GfxError::InvalidInput(format!(
                "DX12 texture upload row pitch {row_pitch} is not aligned to {DX12_TEXTURE_DATA_PITCH_ALIGNMENT} bytes"
            )));
        }
        Ok(())
    }

    fn align_to(value: u64, alignment: u64) -> u64 {
        value.div_ceil(alignment) * alignment
    }

    fn align_to_u32(value: u64, alignment: u64) -> Result<u32> {
        u32::try_from(align_to(value, alignment))
            .map_err(|error| GfxError::InvalidInput(format!("aligned size overflow: {error}")))
    }

    fn validate_uniform_buffer_binding(binding: BufferBinding, buffer_size: u64) -> Result<()> {
        const CONSTANT_BUFFER_ALIGNMENT: u64 = 256;
        if binding.offset % CONSTANT_BUFFER_ALIGNMENT != 0 {
            return Err(GfxError::InvalidInput(
                "DX12 constant buffer offset must be aligned to 256 bytes".to_string(),
            ));
        }
        let aligned_size = checked_align_to(
            binding.size,
            CONSTANT_BUFFER_ALIGNMENT,
            "constant buffer size",
        )?;
        let physical_buffer_size = checked_align_to(
            buffer_size,
            CONSTANT_BUFFER_ALIGNMENT,
            "constant buffer resource",
        )?;
        let end = binding.offset.checked_add(aligned_size).ok_or_else(|| {
            GfxError::InvalidInput("DX12 constant buffer binding range overflow".to_string())
        })?;
        if end > physical_buffer_size {
            return Err(GfxError::InvalidInput(format!(
                "DX12 constant buffer aligned range {}..{} exceeds physical buffer size {}",
                binding.offset, end, physical_buffer_size
            )));
        }
        Ok(())
    }

    fn checked_align_to(value: u64, alignment: u64, label: &str) -> Result<u64> {
        value
            .checked_add(alignment.saturating_sub(1))
            .map(|value| value / alignment)
            .and_then(|value| value.checked_mul(alignment))
            .ok_or_else(|| GfxError::InvalidInput(format!("DX12 {label} alignment overflow")))
    }

    fn validate_storage_buffer_binding(binding: BufferBinding) -> Result<()> {
        if binding.offset % 4 != 0 || binding.size % 4 != 0 {
            return Err(GfxError::InvalidInput(
                "DX12 storage buffer offset and size must align to 4 bytes".to_string(),
            ));
        }
        Ok(())
    }

    fn texture_upload_row_bytes(width: u32, format: Format) -> Result<usize> {
        width
            .checked_mul(format_bytes_per_pixel(format))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| GfxError::InvalidInput("texture upload row size overflow".to_string()))
    }

    fn texture_upload_layout(
        desc: TextureWriteDesc,
        format: Format,
        data_len: usize,
    ) -> Result<Dx12TextureUploadLayout> {
        let source_row_pitch = usize::try_from(desc.layout.bytes_per_row.get())
            .map_err(|error| GfxError::InvalidInput(format!("row pitch overflow: {error}")))?;
        let source_offset = usize::try_from(desc.layout.offset).map_err(|error| {
            GfxError::InvalidInput(format!("texture upload offset overflow: {error}"))
        })?;
        let row_bytes = texture_upload_row_bytes(desc.size.width(), format)?;
        if source_row_pitch < row_bytes {
            return Err(GfxError::InvalidInput(format!(
                "texture upload bytes_per_row ({source_row_pitch}) is smaller than row data ({row_bytes})"
            )));
        }
        let row_pitch = align_to_u32(
            u64::from(desc.layout.bytes_per_row.get()),
            DX12_TEXTURE_DATA_PITCH_ALIGNMENT,
        )?;
        let row_pitch_usize = usize::try_from(row_pitch).map_err(|error| {
            GfxError::InvalidInput(format!("aligned row pitch overflow: {error}"))
        })?;
        let height = usize::try_from(desc.size.height())
            .map_err(|error| GfxError::InvalidInput(format!("height overflow: {error}")))?;
        let required_len =
            required_texture_upload_len(source_offset, source_row_pitch, row_bytes, height)?;
        if data_len < required_len {
            return Err(GfxError::InvalidInput(format!(
                "texture upload data is smaller than layout: required {required_len} bytes, got {data_len}"
            )));
        }
        let upload_size = u64::from(row_pitch)
            .checked_mul(u64::from(desc.size.height()))
            .ok_or_else(|| GfxError::InvalidInput("texture upload size overflow".to_string()))?;
        Ok(Dx12TextureUploadLayout {
            source_row_pitch,
            source_offset,
            row_bytes,
            row_pitch,
            row_pitch_usize,
            height,
            upload_size,
        })
    }

    const fn format_bytes_per_pixel(format: Format) -> u32 {
        match format {
            Format::Bgra8Unorm
            | Format::Bgra8UnormSrgb
            | Format::Rgba8Unorm
            | Format::Rgba8UnormSrgb
            | Format::Depth32Float => 4,
        }
    }

    fn required_texture_upload_len(
        offset: usize,
        source_row_pitch: usize,
        row_bytes: usize,
        height: usize,
    ) -> Result<usize> {
        if height == 0 {
            return Ok(offset);
        }
        offset
            .checked_add(
                height
                    .saturating_sub(1)
                    .checked_mul(source_row_pitch)
                    .ok_or_else(|| {
                        GfxError::InvalidInput("texture upload required size overflow".to_string())
                    })?,
            )
            .and_then(|value| value.checked_add(row_bytes))
            .ok_or_else(|| {
                GfxError::InvalidInput("texture upload required size overflow".to_string())
            })
    }

    fn create_sampler_index_buffer(
        device: &ID3D12Device,
        sampler_tables: &[Dx12DescriptorTable],
        layout: &ResourceSetLayoutDesc,
    ) -> Result<ID3D12Resource> {
        let count = layout_sampler_index_count(layout)?;
        let mut indices = vec![
            0u32;
            usize::try_from(count).map_err(|error| {
                GfxError::InvalidInput(format!("sampler index count overflow: {error}"))
            })?
        ];
        for table in sampler_tables {
            let index = usize::try_from(table.binding).map_err(|error| {
                GfxError::InvalidInput(format!("sampler binding index overflow: {error}"))
            })?;
            let slot = indices.get_mut(index).ok_or_else(|| {
                GfxError::InvalidInput(format!(
                    "sampler binding {} is out of sampler index range",
                    table.binding
                ))
            })?;
            *slot = table.descriptor_index;
        }
        let mut bytes = Vec::with_capacity(indices.len() * std::mem::size_of::<u32>());
        for index in indices {
            bytes.extend_from_slice(&index.to_ne_bytes());
        }
        let buffer_desc = BufferDesc {
            label: Some("nova-gfx dx12 sampler index buffer".to_string()),
            size: u64::try_from(bytes.len()).map_err(|error| {
                GfxError::InvalidInput(format!("sampler index buffer size overflow: {error}"))
            })?,
            usage: gfx_core::BufferUsage::COPY_SRC,
            memory_location: MemoryLocation::CpuToGpu,
        };
        let buffer = create_buffer_resource(device, &buffer_desc)?;
        upload_to_mapped_buffer(&buffer, 0, &bytes)?;
        Ok(buffer)
    }

    fn layout_sampler_index_count(layout: &ResourceSetLayoutDesc) -> Result<u32> {
        let max_binding = layout
            .entries
            .iter()
            .filter(|entry| entry.binding_type == ResourceBindingType::Sampler)
            .map(|entry| entry.binding)
            .max()
            .ok_or_else(|| GfxError::InvalidInput("resource set has no samplers".to_string()))?;
        max_binding
            .checked_add(1)
            .ok_or_else(|| GfxError::InvalidInput("sampler binding count overflow".to_string()))
    }

    fn sampler_desc_to_dx12(sampler: Dx12Sampler) -> D3D12_SAMPLER_DESC {
        D3D12_SAMPLER_DESC {
            Filter: if sampler.mag_filter == FilterMode::Linear
                || sampler.min_filter == FilterMode::Linear
            {
                D3D12_FILTER_MIN_MAG_MIP_LINEAR
            } else {
                D3D12_FILTER_MIN_MAG_MIP_POINT
            },
            AddressU: address_mode_to_dx12(sampler.address_mode_u),
            AddressV: address_mode_to_dx12(sampler.address_mode_v),
            AddressW: address_mode_to_dx12(AddressMode::ClampToEdge),
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC::default(),
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        }
    }

    fn address_mode_to_dx12(
        mode: AddressMode,
    ) -> windows::Win32::Graphics::Direct3D12::D3D12_TEXTURE_ADDRESS_MODE {
        match mode {
            AddressMode::ClampToEdge => {
                windows::Win32::Graphics::Direct3D12::D3D12_TEXTURE_ADDRESS_MODE_CLAMP
            }
            AddressMode::Repeat => {
                windows::Win32::Graphics::Direct3D12::D3D12_TEXTURE_ADDRESS_MODE_WRAP
            }
        }
    }

    fn validate_resource_set_layout_count(expected: usize, actual: usize) -> Result<()> {
        if expected != actual {
            return Err(GfxError::InvalidInput(format!(
                "DX12 pipeline expects {expected} resource sets, got {actual}"
            )));
        }
        Ok(())
    }

    fn validate_resource_set_layout_id(
        set_index: usize,
        expected: ResourceSetLayoutId,
        actual: ResourceSetLayoutId,
    ) -> Result<()> {
        if expected != actual {
            return Err(GfxError::InvalidInput(format!(
                "DX12 resource set {set_index} layout does not match pipeline layout"
            )));
        }
        Ok(())
    }

    fn bind_resource_sets(
        command_list: &ID3D12GraphicsCommandList,
        device: &Dx12Device,
        expected_layouts: &[ResourceSetLayoutId],
        resource_sets: &[ResourceSetId],
    ) -> Result<()> {
        validate_resource_set_layout_count(expected_layouts.len(), resource_sets.len())?;
        let mut root_index = 0;
        for (set_index, (resource_set, expected_layout)) in
            resource_sets.iter().zip(expected_layouts).enumerate()
        {
            let set = device.resource_sets.get(*resource_set)?;
            validate_resource_set_layout_id(set_index, *expected_layout, set.layout)?;
            let layout = device.resource_set_layouts.get(*expected_layout)?;
            for entry in &layout.desc.entries {
                match entry.binding_type {
                    ResourceBindingType::UniformBuffer
                    | ResourceBindingType::StorageBuffer
                    | ResourceBindingType::SampledTexture => {
                        let table = set
                            .resource_tables
                            .iter()
                            .find(|table| table.binding == entry.binding)
                            .ok_or_else(|| {
                                GfxError::InvalidInput(format!(
                                    "DX12 resource set is missing binding {}",
                                    entry.binding
                                ))
                            })?;
                        // SAFETY: Root parameter index follows create_root_signature order.
                        unsafe {
                            command_list
                                .SetGraphicsRootDescriptorTable(root_index, table.gpu_handle);
                        }
                        root_index += 1;
                    }
                    ResourceBindingType::Sampler => {
                        let table = set
                            .sampler_tables
                            .iter()
                            .find(|table| table.binding == entry.binding)
                            .ok_or_else(|| {
                                GfxError::InvalidInput(format!(
                                    "DX12 resource set is missing sampler binding {}",
                                    entry.binding
                                ))
                            })?;
                        // SAFETY: Root parameter index follows create_root_signature order.
                        unsafe {
                            command_list
                                .SetGraphicsRootDescriptorTable(root_index, table.gpu_handle);
                        }
                        root_index += 1;
                        let index_table = set.sampler_index_table.as_ref().ok_or_else(|| {
                            GfxError::InvalidInput(
                                "DX12 resource set is missing sampler index table".to_string(),
                            )
                        })?;
                        // SAFETY: Root parameter index follows create_root_signature order.
                        unsafe {
                            command_list
                                .SetGraphicsRootDescriptorTable(root_index, index_table.gpu_handle);
                        }
                        root_index += 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn bind_draw_step_constants(
        command_list: &ID3D12GraphicsCommandList,
        root_index: u32,
        first_vertex: u32,
        first_instance: u32,
        other: u32,
    ) {
        let constants = [first_vertex, first_instance, other];
        // SAFETY: The active root signature always appends the 3-u32 Naga HLSL
        // special-constants slot at `root_index`, and the pointer remains valid for the call.
        unsafe {
            command_list.SetGraphicsRoot32BitConstants(root_index, 3, constants.as_ptr().cast(), 0);
        }
    }

    // Naga lowers WGSL vertex_index/instance_index offsets into root constants for HLSL.
    // Nova feeds vertices from shader resources instead of IA vertex buffers, so the IA
    // offsets stay zero and the shader-visible offsets carry the WebGPU/Vulkan semantics.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct NonIndexedDrawOffsets {
        shader_first_vertex: u32,
        shader_first_instance: u32,
        start_vertex_location: u32,
        start_instance_location: u32,
    }

    const fn non_indexed_draw_offsets(
        first_vertex: u32,
        first_instance: u32,
    ) -> NonIndexedDrawOffsets {
        NonIndexedDrawOffsets {
            shader_first_vertex: first_vertex,
            shader_first_instance: first_instance,
            start_vertex_location: 0,
            start_instance_location: 0,
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct IndexedDrawOffsets {
        shader_first_vertex: u32,
        shader_first_instance: u32,
        base_vertex_location: i32,
        start_instance_location: u32,
    }

    fn indexed_draw_offsets(base_vertex: i32, first_instance: u32) -> Result<IndexedDrawOffsets> {
        let shader_first_vertex = u32::try_from(base_vertex).map_err(|error| {
            GfxError::InvalidInput(format!(
                "negative DX12 indexed draw base vertex is unsupported by HLSL vertex_index constants: {error}"
            ))
        })?;
        Ok(IndexedDrawOffsets {
            shader_first_vertex,
            shader_first_instance: first_instance,
            base_vertex_location: 0,
            start_instance_location: 0,
        })
    }

    fn validate_index_buffer_range(
        usage: BufferUsage,
        buffer_size: u64,
        binding: IndexBufferBinding,
        first_index: u32,
        index_count: u32,
    ) -> Result<()> {
        if !usage.contains(BufferUsage::INDEX) {
            return Err(GfxError::InvalidInput(
                "index buffer must include INDEX usage".to_string(),
            ));
        }
        let stride = index_format_size(binding.format);
        if binding.offset % stride != 0 {
            return Err(GfxError::InvalidInput(
                "index buffer offset must be aligned to the index format size".to_string(),
            ));
        }
        let first_index_byte = u64::from(first_index).checked_mul(stride).ok_or_else(|| {
            GfxError::InvalidInput("first index byte offset overflow".to_string())
        })?;
        let index_bytes = u64::from(index_count)
            .checked_mul(stride)
            .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
        let byte_end = binding
            .offset
            .checked_add(first_index_byte)
            .and_then(|start| start.checked_add(index_bytes))
            .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
        if byte_end > buffer_size {
            return Err(GfxError::InvalidInput(
                "index buffer range is out of bounds".to_string(),
            ));
        }
        Ok(())
    }

    fn index_format_size(format: IndexFormat) -> u64 {
        match format {
            IndexFormat::Uint16 => 2,
            IndexFormat::Uint32 => 4,
        }
    }

    fn index_format_to_dxgi(
        format: IndexFormat,
    ) -> windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT {
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32_UINT};
        match format {
            IndexFormat::Uint16 => DXGI_FORMAT_R16_UINT,
            IndexFormat::Uint32 => DXGI_FORMAT_R32_UINT,
        }
    }

    pub(crate) fn format_to_dxgi(
        format: Format,
    ) -> windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_D32_FLOAT,
            DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        };
        match format {
            Format::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
            Format::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
            Format::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
            Format::Rgba8UnormSrgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
            Format::Depth32Float => DXGI_FORMAT_D32_FLOAT,
        }
    }

    #[derive(Clone)]
    struct Dx12Buffer {
        desc: BufferDesc,
        resource: Option<ID3D12Resource>,
    }

    #[derive(Clone)]
    struct Dx12Texture {
        desc: TextureDesc,
        resource: Option<ID3D12Resource>,
        state: D3D12_RESOURCE_STATES,
    }

    struct Dx12UploadPage {
        resource: ID3D12Resource,
        size: u64,
        mapped: NonNull<u8>,
    }

    impl Dx12UploadPage {
        fn new(resource: ID3D12Resource, size: u64) -> Result<Self> {
            let mut mapped = ptr::null_mut();
            // SAFETY: This is an upload-heap buffer. D3D12 permits persistent mapping, and
            // `Drop` balances the call after submissions using the page have retired.
            unsafe { resource.Map(0, None, Some(&raw mut mapped)) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            let Some(mapped) = NonNull::new(mapped.cast::<u8>()) else {
                // SAFETY: `Map` succeeded above, so balance it before returning the invalid
                // pointer reported by the driver.
                unsafe { resource.Unmap(0, None) };
                return Err(GfxError::Backend(
                    "DX12 upload page mapping returned a null pointer".to_string(),
                ));
            };
            Ok(Self {
                resource,
                size,
                mapped,
            })
        }
    }

    impl Drop for Dx12UploadPage {
        fn drop(&mut self) {
            // SAFETY: This page owns one successful persistent `Map` call from `new`.
            unsafe { self.resource.Unmap(0, None) };
        }
    }

    struct Dx12TextureUpload {
        allocation: UploadAllocation,
        row_pitch: u32,
    }

    #[derive(Clone, Copy)]
    struct Dx12TextureUploadLayout {
        source_row_pitch: usize,
        source_offset: usize,
        row_bytes: usize,
        row_pitch: u32,
        row_pitch_usize: usize,
        height: usize,
        upload_size: u64,
    }

    struct Dx12TextureCopyPlan<'a> {
        write: TextureWrite<'a>,
        texture: ID3D12Resource,
        old_state: D3D12_RESOURCE_STATES,
        format: Format,
        upload_layout: Dx12TextureUploadLayout,
    }

    struct Dx12TextureCopyOwned {
        texture_id: TextureId,
        upload: ID3D12Resource,
        texture: ID3D12Resource,
        old_state: D3D12_RESOURCE_STATES,
        desc: TextureWriteDesc,
        format: Format,
        upload_offset: u64,
        row_pitch: u32,
    }

    struct Dx12UploadCommands {
        allocator: ID3D12CommandAllocator,
        graphics_command_list: ID3D12GraphicsCommandList,
        timestamps: Option<Dx12TimestampQueries>,
    }

    struct Dx12TimestampQueries {
        heap: ID3D12QueryHeap,
        readback: ID3D12Resource,
    }

    struct Dx12SubmittedCommandList {
        commands: Dx12UploadCommands,
        _command_list: ID3D12CommandList,
    }

    /// GPU resources queued for release once their retire fence completes.
    enum DeferredDx12Release {
        Buffer(Dx12Buffer),
        Texture(Dx12Texture),
        TextureView(Dx12TextureView),
        ResourceSet(Dx12ResourceSet),
        Swapchain(Dx12Swapchain),
        CommandEncoder(Dx12CommandEncoder),
    }

    #[derive(Clone, Copy)]
    struct Dx12TextureView {
        texture: TextureId,
        format: Format,
        rtv_slot: Option<DescriptorSlot>,
        dsv_slot: Option<DescriptorSlot>,
    }

    #[derive(Clone, Copy)]
    struct Dx12Sampler {
        mag_filter: FilterMode,
        min_filter: FilterMode,
        address_mode_u: AddressMode,
        address_mode_v: AddressMode,
    }

    #[derive(Clone)]
    struct Dx12ResourceSetLayout {
        desc: ResourceSetLayoutDesc,
    }

    #[derive(Clone)]
    struct Dx12ResourceSet {
        layout: ResourceSetLayoutId,
        resource_tables: Vec<Dx12DescriptorTable>,
        sampler_tables: Vec<Dx12DescriptorTable>,
        sampler_index_table: Option<Dx12DescriptorTable>,
        owned_buffers: Vec<ID3D12Resource>,
    }

    #[derive(Clone)]
    struct Dx12PipelineLayout {
        root_signature: ID3D12RootSignature,
        resource_set_layouts: Arc<[ResourceSetLayoutId]>,
        draw_step_constants_root_index: u32,
    }

    #[derive(Clone, Copy)]
    struct Dx12DescriptorTable {
        binding: u32,
        gpu_handle: D3D12_GPU_DESCRIPTOR_HANDLE,
        descriptor_index: u32,
    }

    #[derive(Clone, Copy)]
    struct DescriptorSlot {
        cpu_handle: D3D12_CPU_DESCRIPTOR_HANDLE,
        gpu_handle: D3D12_GPU_DESCRIPTOR_HANDLE,
        index: u32,
    }

    struct DescriptorHeapAllocator {
        heap: ID3D12DescriptorHeap,
        increment: u32,
        capacity: u32,
        next: u32,
        free_indices: Vec<u32>,
    }

    impl DescriptorHeapAllocator {
        fn new(
            device: &ID3D12Device,
            heap_type: windows::Win32::Graphics::Direct3D12::D3D12_DESCRIPTOR_HEAP_TYPE,
            capacity: u32,
            shader_visible: bool,
        ) -> Result<Self> {
            let desc = D3D12_DESCRIPTOR_HEAP_DESC {
                Type: heap_type,
                NumDescriptors: capacity,
                Flags: if shader_visible {
                    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
                } else {
                    D3D12_DESCRIPTOR_HEAP_FLAG_NONE
                },
                NodeMask: 0,
            };
            // SAFETY: Descriptor heap desc is self-contained and device is valid.
            let heap = unsafe { device.CreateDescriptorHeap(&raw const desc) }
                .map_err(|error| GfxError::Backend(error.to_string()))?;
            // SAFETY: Device is valid and returns a static descriptor size for this heap type.
            let increment = unsafe { device.GetDescriptorHandleIncrementSize(heap_type) };
            Ok(Self {
                heap,
                increment,
                capacity,
                next: 0,
                free_indices: Vec::new(),
            })
        }

        fn allocate(&mut self) -> Result<DescriptorSlot> {
            let index = if let Some(index) = self.free_indices.pop() {
                index
            } else {
                if self.next >= self.capacity {
                    return Err(GfxError::Unavailable(
                        "DX12 descriptor heap capacity exhausted".to_string(),
                    ));
                }
                let index = self.next;
                self.next = self.next.saturating_add(1);
                index
            };
            // SAFETY: Heap is valid and shader-visible when GPU handles are requested.
            let cpu_start = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
            // SAFETY: Heap is valid and shader-visible.
            let gpu_start = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
            let byte_offset = usize::try_from(u64::from(self.increment) * u64::from(index))
                .map_err(|error| {
                    GfxError::InvalidInput(format!("descriptor offset overflow: {error}"))
                })?;
            Ok(DescriptorSlot {
                cpu_handle: D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu_start.ptr.checked_add(byte_offset).ok_or_else(|| {
                        GfxError::InvalidInput("descriptor CPU handle overflow".to_string())
                    })?,
                },
                gpu_handle: D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu_start
                        .ptr
                        .checked_add(u64::try_from(byte_offset).map_err(|error| {
                            GfxError::InvalidInput(format!(
                                "descriptor GPU handle offset overflow: {error}"
                            ))
                        })?)
                        .ok_or_else(|| {
                            GfxError::InvalidInput("descriptor GPU handle overflow".to_string())
                        })?,
                },
                index,
            })
        }

        fn free(&mut self, slot: DescriptorSlot) {
            self.free_index(slot.index);
        }

        fn free_index(&mut self, index: u32) {
            if index < self.capacity && !self.free_indices.contains(&index) {
                self.free_indices.push(index);
            }
        }

        fn gpu_start(&self) -> D3D12_GPU_DESCRIPTOR_HANDLE {
            // SAFETY: Heap is live for the lifetime of the allocator.
            unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() }
        }
    }

    #[derive(Clone)]
    struct Dx12ShaderModule {
        stage: ShaderStage,
        entry_point: String,
        bytecode: Vec<u8>,
    }

    #[derive(Clone, Copy)]
    struct Dx12RenderPass {
        color_format: Format,
        depth_format: Option<Format>,
    }

    #[derive(Clone)]
    struct Dx12RenderPipeline {
        color_format: Format,
        blend_mode: BlendMode,
        primitive_topology: PrimitiveTopology,
        pipeline_state: Option<ID3D12PipelineState>,
        root_signature: Option<ID3D12RootSignature>,
        resource_set_layouts: Arc<[ResourceSetLayoutId]>,
        draw_step_constants_root_index: u32,
    }

    #[derive(Clone)]
    struct Dx12CommandEncoder {
        allocator: Option<ID3D12CommandAllocator>,
        command_list: Option<ID3D12GraphicsCommandList>,
    }

    struct DeferredDx12CommandEncoder {
        fence_value: u64,
        _encoder: Dx12CommandEncoder,
    }

    struct Dx12Submission {
        fence_value: u64,
    }

    #[derive(Clone)]
    struct Dx12Composition {
        _device: IDCompositionDesktopDevice,
        _target: IDCompositionTarget,
        visual: IDCompositionVisual,
        /// Visual transform used to stretch previous-frame content over a
        /// pending window resize; identity outside live resizes.
        transform: IDCompositionMatrixTransform,
    }

    #[derive(Clone)]
    struct Dx12Surface {
        label: Option<String>,
        hwnd: HWND,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Dx12BackBufferDamage {
        Clean,
        Partial(ScissorRect),
        Full,
    }

    impl Dx12BackBufferDamage {
        fn accumulate(&mut self, damage: Option<ScissorRect>) {
            *self = match (*self, damage) {
                (_, None) | (Self::Full, Some(_)) => Self::Full,
                (Self::Clean, Some(damage)) => Self::Partial(damage),
                (Self::Partial(previous), Some(damage)) => {
                    Self::Partial(union_scissor_rects(previous, damage))
                }
            };
        }

        fn present_damage(self) -> Option<ScissorRect> {
            match self {
                Self::Partial(damage) => Some(damage),
                Self::Clean | Self::Full => None,
            }
        }
    }

    #[derive(Clone)]
    struct Dx12Swapchain {
        surface: SurfaceId,
        config: SurfaceConfig,
        swapchain: IDXGISwapChain3,
        composition: Option<Dx12Composition>,
        rtv_heap: Option<ID3D12DescriptorHeap>,
        render_targets: Vec<ID3D12Resource>,
        rtv_descriptor_size: u32,
        frame_index: u32,
        /// DXGI creation flags; ResizeBuffers must pass the same set.
        creation_flags: u32,
        /// Frame-latency waitable object; closed when the swapchain retires.
        frame_latency_waitable: Option<HANDLE>,
        /// Flip-sequential swapchains can preserve pixels outside native dirty rectangles.
        partial_presentation: bool,
        /// Damage accumulated since each rotating back buffer was last current.
        pending_damage: Vec<Dx12BackBufferDamage>,
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gfx_core::{BufferUsage, ResourceSetLayoutEntry};

        #[test]
        fn maps_bgra_format_to_dxgi() {
            let format = format_to_dxgi(Format::Bgra8Unorm);

            assert_eq!(
                format,
                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM
            );
        }

        #[test]
        fn present_damage_maps_to_dxgi_edges() {
            let rect = Dx12Device::damage_to_dxgi_rect(ScissorRect {
                x: 10,
                y: 20,
                width: 30,
                height: 40,
            })
            .expect("damage should fit DXGI coordinates");

            assert_eq!(
                (rect.left, rect.top, rect.right, rect.bottom),
                (10, 20, 40, 60)
            );
        }

        #[test]
        fn back_buffer_damage_accumulates_without_copying_previous_buffers() {
            let mut damage = Dx12BackBufferDamage::Clean;

            damage.accumulate(Some(ScissorRect {
                x: 10,
                y: 20,
                width: 30,
                height: 40,
            }));
            damage.accumulate(Some(ScissorRect {
                x: 35,
                y: 10,
                width: 20,
                height: 15,
            }));

            assert_eq!(
                damage.present_damage(),
                Some(ScissorRect {
                    x: 10,
                    y: 10,
                    width: 45,
                    height: 50,
                })
            );
            damage.accumulate(None);
            assert_eq!(damage, Dx12BackBufferDamage::Full);
        }

        #[test]
        fn fallback_adapter_rank_respects_power_preference() {
            let integrated_memory = 512 * 1024 * 1024;
            let discrete_memory = 8 * 1024 * 1024 * 1024;

            assert!(
                fallback_adapter_rank(integrated_memory, PowerPreference::LowPower)
                    < fallback_adapter_rank(discrete_memory, PowerPreference::LowPower)
            );
            assert!(
                fallback_adapter_rank(discrete_memory, PowerPreference::HighPerformance)
                    < fallback_adapter_rank(integrated_memory, PowerPreference::HighPerformance)
            );
        }

        #[test]
        fn registry_rejects_stale_handle() {
            let mut registry = ResourceRegistry::new("buffer");
            let id: BufferId = registry.insert(Dx12Buffer {
                desc: BufferDesc {
                    label: None,
                    size: 1,
                    usage: BufferUsage::VERTEX,
                    memory_location: MemoryLocation::CpuToGpu,
                },
                resource: None,
            });

            let _removed = registry.take(id).expect("handle should be live");

            let error = match registry.get(id) {
                Ok(_) => panic!("stale handle should fail"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("stale or invalid buffer handle"));
        }

        #[test]
        fn registry_replace_live_preserves_handle_generation() {
            let mut registry = ResourceRegistry::new("swapchain");
            let id: SwapchainId = registry.insert(1_u32);

            let old = registry
                .replace_live(id, 2_u32)
                .expect("live resource should be replaced");

            assert_eq!(old, 1);
            assert_eq!(
                *registry.get(id).expect("same handle should remain live"),
                2
            );
            assert_eq!(id.generation(), 1);
        }

        #[test]
        fn draw_step_constants_root_index_follows_resource_tables() {
            let layouts = [
                ResourceSetLayoutDesc {
                    label: None,
                    entries: vec![
                        ResourceSetLayoutEntry {
                            binding: 0,
                            binding_type: ResourceBindingType::UniformBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        ResourceSetLayoutEntry {
                            binding: 1,
                            binding_type: ResourceBindingType::Sampler,
                            stages: ShaderStages::FRAGMENT,
                        },
                    ],
                },
                ResourceSetLayoutDesc {
                    label: None,
                    entries: vec![ResourceSetLayoutEntry {
                        binding: 2,
                        binding_type: ResourceBindingType::StorageBuffer,
                        stages: ShaderStages::VERTEX,
                    }],
                },
            ];

            let root_index =
                draw_step_constants_root_index(&layouts).expect("root index should fit in u32");

            assert_eq!(root_index, 4);
        }

        #[test]
        fn sampler_index_buffer_register_follows_naga_group_index() {
            assert_eq!(sampler_index_buffer_register(0).expect("group 0 fits"), 0);
            assert_eq!(sampler_index_buffer_register(1).expect("group 1 fits"), 1);
            assert_eq!(
                sampler_index_buffer_register(255).expect("group 255 fits"),
                255
            );
        }

        #[test]
        fn resource_set_layout_validation_rejects_wrong_count_or_layout() {
            let first = ResourceSetLayoutId::from_parts(1, 1);
            let second = ResourceSetLayoutId::from_parts(2, 1);

            assert!(validate_resource_set_layout_count(1, 1).is_ok());
            assert!(validate_resource_set_layout_count(1, 2).is_err());
            assert!(validate_resource_set_layout_id(0, first, first).is_ok());
            assert!(validate_resource_set_layout_id(0, first, second).is_err());
        }

        #[test]
        fn dx12_non_indexed_draw_offsets_use_shader_constants_only() {
            let offsets = non_indexed_draw_offsets(7, 11);

            assert_eq!(offsets.shader_first_vertex, 7);
            assert_eq!(offsets.shader_first_instance, 11);
            assert_eq!(offsets.start_vertex_location, 0);
            assert_eq!(offsets.start_instance_location, 0);
        }

        #[test]
        fn dx12_indexed_draw_offsets_use_shader_constants_only() {
            let offsets =
                indexed_draw_offsets(7, 11).expect("positive base vertex should be supported");

            assert_eq!(offsets.shader_first_vertex, 7);
            assert_eq!(offsets.shader_first_instance, 11);
            assert_eq!(offsets.base_vertex_location, 0);
            assert_eq!(offsets.start_instance_location, 0);
        }

        #[test]
        fn dx12_indexed_draw_offsets_reject_negative_base_vertex() {
            let error = indexed_draw_offsets(-1, 0)
                .expect_err("negative base vertex cannot be represented as a WGSL vertex_index");

            assert!(error.to_string().contains("negative DX12 indexed draw"));
        }

        #[test]
        fn dx12_uniform_buffer_binding_validation_rejects_misaligned_offset() {
            let binding = BufferBinding {
                buffer: BufferId::from_parts(1, 1),
                offset: 16,
                size: 16,
                stride: None,
            };

            assert!(validate_uniform_buffer_binding(binding, 512).is_err());
        }

        #[test]
        fn dx12_uniform_buffer_binding_validation_allows_aligned_physical_range() {
            let binding = BufferBinding {
                buffer: BufferId::from_parts(1, 1),
                offset: 256,
                size: 16,
                stride: None,
            };

            assert!(validate_uniform_buffer_binding(binding, 300).is_ok());
        }

        #[test]
        fn dx12_storage_buffer_binding_validation_rejects_unaligned_range() {
            let binding = BufferBinding {
                buffer: BufferId::from_parts(1, 1),
                offset: 2,
                size: 16,
                stride: None,
            };

            assert!(validate_storage_buffer_binding(binding).is_err());
        }

        #[test]
        fn texture_upload_required_len_allows_compact_rows() {
            let row_bytes =
                texture_upload_row_bytes(8, Format::Rgba8Unorm).expect("row size should be valid");

            let required = required_texture_upload_len(0, row_bytes, row_bytes, 4)
                .expect("required len should be valid");

            assert_eq!(required, 8 * 4 * 4);
        }

        #[test]
        fn texture_upload_required_len_allows_strided_rows_and_offset() {
            let row_bytes =
                texture_upload_row_bytes(4, Format::Bgra8Unorm).expect("row size should be valid");

            let required = required_texture_upload_len(13, 64, row_bytes, 3)
                .expect("required len should be valid");

            assert_eq!(required, 13 + 64 * 2 + 4 * 4);
        }

        #[test]
        fn texture_upload_required_len_detects_short_data() {
            let row_bytes =
                texture_upload_row_bytes(4, Format::Rgba8Unorm).expect("row size should be valid");
            let required = required_texture_upload_len(0, 64, row_bytes, 2)
                .expect("required len should be valid");

            assert!(required > 64);
        }

        #[test]
        fn dx12_texture_upload_ring_uses_placement_alignment() {
            let mut ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
                alignment: DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT,
                ..UploadRingAllocatorDesc::default()
            })
            .expect("ring descriptor should be valid");

            let first = ring.allocate(1).expect("first allocation should succeed");
            let second = ring.allocate(1).expect("second allocation should succeed");

            assert_eq!(first.offset, 0);
            assert_eq!(second.offset, DX12_TEXTURE_DATA_PLACEMENT_ALIGNMENT);
        }

        #[test]
        fn dx12_texture_copy_validation_rejects_misaligned_offset() {
            let error = validate_dx12_texture_copy(256, 256)
                .expect_err("misaligned placement should be rejected");

            assert!(error.to_string().contains("not aligned"));
        }

        #[test]
        fn dx12_texture_copy_validation_rejects_misaligned_row_pitch() {
            let error = validate_dx12_texture_copy(512, 128)
                .expect_err("misaligned row pitch should be rejected");

            assert!(error.to_string().contains("row pitch"));
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use gfx_core::{
        AdapterInfo, BackendKind, BufferDesc, BufferId, ClearColor, CommandEncoderDesc,
        CommandEncoderId, DeviceDesc, DrawDesc, DrawStepDesc, GfxBackend, GfxCommandDevice,
        GfxDiagnosticsDevice, GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice,
        GfxSubmissionDevice, GfxSurfaceDevice, LoadOp, PipelineLayoutDesc, PipelineLayoutId,
        RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId, ResourceSetDesc,
        ResourceSetId, ResourceSetLayoutDesc, ResourceSetLayoutId, ResourceStats, SamplerDesc,
        SamplerId, ShaderModuleDesc, ShaderModuleId, SubmissionId, SubmissionStatus, SurfaceConfig,
        SurfaceDesc, SurfaceId, SwapchainId, TextureDesc, TextureId, TextureViewDesc,
        TextureViewId, TextureWriteDesc,
    };

    /// Stub Direct3D 12 device for non-Windows targets.
    pub struct Dx12Device;

    impl Dx12Device {
        /// Returns unavailable on non-Windows targets.
        ///
        /// # Errors
        ///
        /// Always returns [`GfxError::Unavailable`] on non-Windows targets.
        pub fn new(_desc: &DeviceDesc) -> Result<Self> {
            Err(GfxError::Unavailable(
                "Direct3D 12 backend is only available on Windows".to_string(),
            ))
        }
    }

    /// Returns no Direct3D 12 adapters on non-Windows targets.
    ///
    /// # Errors
    ///
    /// This stub does not fail.
    pub fn enumerate_adapter_info() -> Result<Vec<AdapterInfo>> {
        Ok(Vec::new())
    }

    fn unavailable<T>() -> Result<T> {
        Err(GfxError::Unavailable(
            "Direct3D 12 backend is only available on Windows".to_string(),
        ))
    }

    impl GfxBackend for Dx12Device {
        const BACKEND_KIND: BackendKind = BackendKind::Dx12;
    }

    impl GfxSurfaceDevice for Dx12Device {
        type SurfaceTarget = ();

        fn create_surface(
            &mut self,
            _target: &Self::SurfaceTarget,
            _desc: &SurfaceDesc,
        ) -> Result<SurfaceId> {
            unavailable()
        }

        fn create_swapchain(
            &mut self,
            _surface: SurfaceId,
            _config: SurfaceConfig,
        ) -> Result<SwapchainId> {
            unavailable()
        }

        fn destroy_swapchain(&mut self, _swapchain: SwapchainId) -> Result<()> {
            unavailable()
        }

        fn destroy_surface(&mut self, _surface: SurfaceId) -> Result<()> {
            unavailable()
        }
    }

    impl GfxResourceDevice for Dx12Device {
        fn create_buffer(&mut self, _desc: &BufferDesc) -> Result<BufferId> {
            unavailable()
        }

        fn write_buffer(&mut self, _buffer: BufferId, _offset: u64, _data: &[u8]) -> Result<()> {
            unavailable()
        }

        fn create_texture(&mut self, _desc: &TextureDesc) -> Result<TextureId> {
            unavailable()
        }

        fn write_texture(&mut self, _desc: TextureWriteDesc, _data: &[u8]) -> Result<()> {
            unavailable()
        }

        fn create_texture_view(&mut self, _desc: &TextureViewDesc) -> Result<TextureViewId> {
            unavailable()
        }

        fn create_sampler(&mut self, _desc: &SamplerDesc) -> Result<SamplerId> {
            unavailable()
        }

        fn create_resource_set_layout(
            &mut self,
            _desc: &ResourceSetLayoutDesc,
        ) -> Result<ResourceSetLayoutId> {
            unavailable()
        }

        fn create_resource_set(&mut self, _desc: &ResourceSetDesc) -> Result<ResourceSetId> {
            unavailable()
        }

        fn destroy_buffer(&mut self, _buffer: BufferId) -> Result<()> {
            unavailable()
        }

        fn destroy_texture(&mut self, _texture: TextureId) -> Result<()> {
            unavailable()
        }

        fn destroy_texture_view(&mut self, _view: TextureViewId) -> Result<()> {
            unavailable()
        }

        fn destroy_sampler(&mut self, _sampler: SamplerId) -> Result<()> {
            unavailable()
        }

        fn destroy_resource_set_layout(&mut self, _layout: ResourceSetLayoutId) -> Result<()> {
            unavailable()
        }

        fn destroy_resource_set(&mut self, _resource_set: ResourceSetId) -> Result<()> {
            unavailable()
        }
    }

    impl GfxPipelineDevice for Dx12Device {
        fn create_pipeline_layout(
            &mut self,
            _desc: &PipelineLayoutDesc,
        ) -> Result<PipelineLayoutId> {
            unavailable()
        }

        fn create_shader_module(&mut self, _desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
            unavailable()
        }

        fn create_render_pass(&mut self, _desc: &RenderPassDesc) -> Result<RenderPassId> {
            unavailable()
        }

        fn create_render_pipeline(
            &mut self,
            _desc: &RenderPipelineDesc,
            _viewport_extent: gfx_core::Extent2d,
        ) -> Result<RenderPipelineId> {
            unavailable()
        }

        fn destroy_pipeline_layout(&mut self, _layout: PipelineLayoutId) -> Result<()> {
            unavailable()
        }

        fn destroy_shader_module(&mut self, _shader: ShaderModuleId) -> Result<()> {
            unavailable()
        }

        fn destroy_render_pass(&mut self, _render_pass: RenderPassId) -> Result<()> {
            unavailable()
        }

        fn destroy_render_pipeline(&mut self, _pipeline: RenderPipelineId) -> Result<()> {
            unavailable()
        }
    }

    impl GfxCommandDevice for Dx12Device {
        fn create_command_encoder(
            &mut self,
            _desc: &CommandEncoderDesc,
        ) -> Result<CommandEncoderId> {
            unavailable()
        }

        fn record_draw_desc(&mut self, _encoder: CommandEncoderId, _draw: DrawDesc) -> Result<()> {
            unavailable()
        }

        fn submit(&mut self, _encoder: CommandEncoderId) -> Result<()> {
            unavailable()
        }

        fn destroy_command_encoder(&mut self, _encoder: CommandEncoderId) -> Result<()> {
            unavailable()
        }
    }

    impl GfxSubmissionDevice for Dx12Device {
        fn submit_deferred(&mut self, _encoder: CommandEncoderId) -> Result<SubmissionId> {
            unavailable()
        }

        fn poll_submission(&mut self, _submission: SubmissionId) -> Result<SubmissionStatus> {
            unavailable()
        }

        fn wait_submission(&mut self, _submission: SubmissionId) -> Result<()> {
            unavailable()
        }
    }

    impl GfxPresentationDevice for Dx12Device {
        fn draw_steps_and_present(
            &mut self,
            _swapchain: SwapchainId,
            _render_pass: RenderPassId,
            _steps: &[DrawStepDesc],
            _clear_color: ClearColor,
        ) -> Result<()> {
            unavailable()
        }

        fn draw_steps_to_texture(
            &mut self,
            _texture_view: TextureViewId,
            _render_pass: RenderPassId,
            _steps: &[DrawStepDesc],
            _color_load_op: LoadOp<ClearColor>,
        ) -> Result<()> {
            unavailable()
        }
    }

    impl GfxDiagnosticsDevice for Dx12Device {
        fn resource_stats(&self) -> ResourceStats {
            ResourceStats::default()
        }
    }
}

pub use platform::*;
