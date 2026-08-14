use super::*;

#[cfg(target_os = "windows")]
fn native_windows_drawable_size<W>(window: &W) -> Option<Size<DevicePixels>>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    use ::winit::raw_window_handle::RawWindowHandle;
    use windows::Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::GetClientRect,
    };

    let raw_window_handle = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::Win32(handle) = raw_window_handle else {
        return None;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let mut client_rect = RECT::default();
    // SAFETY: `hwnd` comes from the live window borrowed for renderer initialization.
    unsafe { GetClientRect(hwnd, &mut client_rect).ok()? };
    let width = client_rect.right.saturating_sub(client_rect.left);
    let height = client_rect.bottom.saturating_sub(client_rect.top);
    if width <= 0 || height <= 0 {
        return None;
    }

    Some(Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    })
}

fn resolve_initial_drawable_size<W>(window: &W, requested: Size<DevicePixels>) -> Size<DevicePixels>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    #[cfg(target_os = "windows")]
    if let Some(native) = native_windows_drawable_size(window) {
        if native != requested {
            log::debug!(
                "Nova renderer initial drawable size corrected from requested={}x{} to native-client={}x{}",
                requested.width.0,
                requested.height.0,
                native.width.0,
                native.height.0,
            );
        }
        return native;
    }

    #[cfg(not(target_os = "windows"))]
    let _ = window;

    requested
}

impl NovaRenderer {
    pub(crate) fn new<W>(
        window: &W,
        backend: RendererBackend,
        renderer_options: &RendererOptions,
        submission_mode: GpuSubmissionMode,
        drawable_size: Size<DevicePixels>,
        transparent: bool,
    ) -> Result<Self>
    where
        W: ::winit::raw_window_handle::HasDisplayHandle
            + ::winit::raw_window_handle::HasWindowHandle
            + 'static,
    {
        Self::new_with_atlas(
            window,
            backend,
            renderer_options,
            submission_mode,
            drawable_size,
            transparent,
            NovaRendererAtlas::new(),
        )
    }

    pub(crate) fn new_with_atlas<W>(
        window: &W,
        backend: RendererBackend,
        renderer_options: &RendererOptions,
        submission_mode: GpuSubmissionMode,
        drawable_size: Size<DevicePixels>,
        transparent: bool,
        atlas: NovaRendererAtlas,
    ) -> Result<Self>
    where
        W: ::winit::raw_window_handle::HasDisplayHandle
            + ::winit::raw_window_handle::HasWindowHandle
            + 'static,
    {
        let metrics_started_at = Instant::now();
        // Windows can report a fractional logical size during hidden-window startup. Rebuilding
        // physical pixels from that logical size may differ from the real HWND client area by one
        // pixel, which makes the whole scene (most visibly glyph atlas sprites) be resampled until
        // the first native resize. Match gpui-ce's native-pixel contract by treating the actual
        // client rect as authoritative before the swapchain and all size-dependent resources exist.
        let drawable_size = resolve_initial_drawable_size(window, drawable_size);
        let width = drawable_size.width.0.max(1) as u32;
        let height = drawable_size.height.0.max(1) as u32;
        log::info!("renderer_path=nova-gfx backend={backend}");
        let mut surface_config = SurfaceConfig::new(width, height, Format::Bgra8Unorm)
            .context("creating nova-gfx surface config")?;
        surface_config.present_mode = nova_present_mode_for_backend(backend, renderer_options);
        let surface_alpha =
            Self::alpha_state_for_window_transparency_on_backend(backend, transparent);
        surface_config.alpha_mode = surface_alpha.swapchain_mode;
        let present_mode = surface_config.present_mode;
        let current_size = DrawableSize { width, height };
        match backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            RendererBackend::NovaDx12 => {
                let shader_binaries = cached_nova_dx12_shader_binaries()?;
                let mut device = Dx12Device::new(&DeviceDescriptor {
                    application_name: "gpui nova dx12".to_string(),
                    adapter_name: renderer_options.adapter_name.clone(),
                    power_preference: nova_power_preference(renderer_options),
                })
                .context("creating nova DX12 device")?;
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova DX12 surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova DX12 swapchain")?;
                let resources = create_renderer_resources(
                    &mut device,
                    surface_config,
                    "gpui nova dx12",
                    shader_binaries,
                )
                .context("creating GPUI nova DX12 render resources")?;
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                Ok(Self {
                    backend: NovaBackend::Dx12(device),
                    surface,
                    swapchain,
                    surface_config,
                    surface_format: surface_config.format,
                    present_mode,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    depth_texture: resources.depth_texture,
                    depth_texture_view: resources.depth_texture_view,
                    frame_resources,
                    current_frame_resource_index: 0,
                    global_buffer: current_frame_resources.buffers.global_buffer,
                    text_raster_buffer: current_frame_resources.buffers.text_raster_buffer,
                    quad_buffer: current_frame_resources.buffers.quad_buffer,
                    shadow_buffer: current_frame_resources.buffers.shadow_buffer,
                    path_rasterization_vertex_buffer: current_frame_resources
                        .buffers
                        .path_rasterization_vertex_buffer,
                    path_sprite_buffer: current_frame_resources.buffers.path_sprite_buffer,
                    mono_sprite_buffer: current_frame_resources.buffers.mono_sprite_buffer,
                    poly_sprite_buffer: current_frame_resources.buffers.poly_sprite_buffer,
                    underline_buffer: current_frame_resources.buffers.underline_buffer,
                    backdrop_blur_pass_buffer: current_frame_resources
                        .buffers
                        .backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: current_frame_resources.buffers.backdrop_blur_buffer,
                    animation_binding_buffer: current_frame_resources
                        .buffers
                        .animation_binding_buffer,
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    custom_mesh_3d_parameters_buffer: current_frame_resources
                        .buffers
                        .custom_mesh_3d_parameters_buffer,
                    custom_mesh_3d_vertices_buffer: resources.custom_mesh_3d_vertices_buffer,
                    custom_mesh_3d_indices_buffer: resources.custom_mesh_3d_indices_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: current_frame_resources.path_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
                    synced_atlas_texture_generation: None,
                    underline_resource_set: current_frame_resources
                        .resource_sets
                        .underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    custom_mesh_3d_pipeline_layout: resources.custom_mesh_3d_pipeline_layout,
                    custom_mesh_3d_resource_set: current_frame_resources
                        .resource_sets
                        .custom_mesh_3d_resource_set,
                    custom_mesh_3d_resource_set_layout: resources
                        .custom_mesh_3d_resource_set_layout,
                    custom_mesh_3d_buffers_ready: false,
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    backdrop_blur_cache_valid: false,
                    backdrop_blur_cache_atlas_generation: 0,
                    backdrop_blur_cache_quality: None,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                })
            }
            #[cfg(not(all(feature = "nova-gfx-dx12", target_os = "windows")))]
            RendererBackend::NovaDx12 => {
                anyhow::bail!(
                    "nova-gfx DX12 renderer requires the nova-gfx-dx12 feature on Windows"
                )
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            RendererBackend::NovaMetal => {
                let shader_binaries = cached_nova_metal_shader_binaries()?;
                let mut device = MetalDevice::new(&DeviceDescriptor {
                    application_name: "gpui nova metal".to_string(),
                    adapter_name: renderer_options.adapter_name.clone(),
                    power_preference: nova_power_preference(renderer_options),
                })
                .context("creating nova Metal device")?;
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova Metal surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Metal swapchain")?;
                let resources = create_renderer_resources(
                    &mut device,
                    surface_config,
                    "gpui nova metal",
                    shader_binaries,
                )
                .context("creating GPUI nova Metal render resources")?;
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                Ok(Self {
                    backend: NovaBackend::Metal(device),
                    surface,
                    swapchain,
                    surface_config,
                    surface_format: surface_config.format,
                    present_mode,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    depth_texture: resources.depth_texture,
                    depth_texture_view: resources.depth_texture_view,
                    frame_resources,
                    current_frame_resource_index: 0,
                    global_buffer: current_frame_resources.buffers.global_buffer,
                    text_raster_buffer: current_frame_resources.buffers.text_raster_buffer,
                    quad_buffer: current_frame_resources.buffers.quad_buffer,
                    shadow_buffer: current_frame_resources.buffers.shadow_buffer,
                    path_rasterization_vertex_buffer: current_frame_resources
                        .buffers
                        .path_rasterization_vertex_buffer,
                    path_sprite_buffer: current_frame_resources.buffers.path_sprite_buffer,
                    mono_sprite_buffer: current_frame_resources.buffers.mono_sprite_buffer,
                    poly_sprite_buffer: current_frame_resources.buffers.poly_sprite_buffer,
                    underline_buffer: current_frame_resources.buffers.underline_buffer,
                    backdrop_blur_pass_buffer: current_frame_resources
                        .buffers
                        .backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: current_frame_resources.buffers.backdrop_blur_buffer,
                    animation_binding_buffer: current_frame_resources
                        .buffers
                        .animation_binding_buffer,
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    custom_mesh_3d_parameters_buffer: current_frame_resources
                        .buffers
                        .custom_mesh_3d_parameters_buffer,
                    custom_mesh_3d_vertices_buffer: resources.custom_mesh_3d_vertices_buffer,
                    custom_mesh_3d_indices_buffer: resources.custom_mesh_3d_indices_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: current_frame_resources.path_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
                    synced_atlas_texture_generation: None,
                    underline_resource_set: current_frame_resources
                        .resource_sets
                        .underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    custom_mesh_3d_pipeline_layout: resources.custom_mesh_3d_pipeline_layout,
                    custom_mesh_3d_resource_set: current_frame_resources
                        .resource_sets
                        .custom_mesh_3d_resource_set,
                    custom_mesh_3d_resource_set_layout: resources
                        .custom_mesh_3d_resource_set_layout,
                    custom_mesh_3d_buffers_ready: false,
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    backdrop_blur_cache_valid: false,
                    backdrop_blur_cache_atlas_generation: 0,
                    backdrop_blur_cache_quality: None,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                })
            }
            #[cfg(not(all(feature = "nova-gfx-metal", target_os = "macos")))]
            RendererBackend::NovaMetal => {
                anyhow::bail!(
                    "nova-gfx Metal renderer requires the nova-gfx-metal feature on macOS"
                )
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            RendererBackend::NovaVulkan => {
                let shader_binaries = cached_nova_vulkan_shader_binaries()?;
                let device_started_at = Instant::now();
                let mut device = VulkanDevice::new(&DeviceDescriptor {
                    application_name: "gpui nova vulkan".to_string(),
                    adapter_name: renderer_options.adapter_name.clone(),
                    power_preference: nova_power_preference(renderer_options),
                })
                .context("creating nova Vulkan device")?;
                let device_elapsed = device_started_at.elapsed();
                let surface_started_at = Instant::now();
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova Vulkan surface")?;
                let surface_alpha = NovaSurfaceAlphaState::new(
                    device
                        .resolve_surface_alpha_mode(surface, surface_alpha.swapchain_mode)
                        .context("resolving nova Vulkan surface alpha mode")?,
                );
                let surface_config = SurfaceConfig {
                    alpha_mode: surface_alpha.swapchain_mode,
                    ..surface_config
                };
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Vulkan swapchain")?;
                let surface_elapsed = surface_started_at.elapsed();
                let resources_started_at = Instant::now();
                let resources = create_renderer_resources(
                    &mut device,
                    surface_config,
                    "gpui nova vulkan",
                    shader_binaries,
                )
                .context("creating GPUI nova Vulkan render resources")?;
                log::info!(
                    "GPUI nova-gfx Vulkan startup: total_ms={} device_ms={} surface_swapchain_ms={} resources_ms={}",
                    metrics_started_at.elapsed().as_millis(),
                    device_elapsed.as_millis(),
                    surface_elapsed.as_millis(),
                    resources_started_at.elapsed().as_millis(),
                );
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                Ok(Self {
                    backend: NovaBackend::Vulkan(device),
                    surface,
                    swapchain,
                    surface_config,
                    surface_format: surface_config.format,
                    present_mode,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    depth_texture: resources.depth_texture,
                    depth_texture_view: resources.depth_texture_view,
                    frame_resources,
                    current_frame_resource_index: 0,
                    global_buffer: current_frame_resources.buffers.global_buffer,
                    text_raster_buffer: current_frame_resources.buffers.text_raster_buffer,
                    quad_buffer: current_frame_resources.buffers.quad_buffer,
                    shadow_buffer: current_frame_resources.buffers.shadow_buffer,
                    path_rasterization_vertex_buffer: current_frame_resources
                        .buffers
                        .path_rasterization_vertex_buffer,
                    path_sprite_buffer: current_frame_resources.buffers.path_sprite_buffer,
                    mono_sprite_buffer: current_frame_resources.buffers.mono_sprite_buffer,
                    poly_sprite_buffer: current_frame_resources.buffers.poly_sprite_buffer,
                    underline_buffer: current_frame_resources.buffers.underline_buffer,
                    backdrop_blur_pass_buffer: current_frame_resources
                        .buffers
                        .backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: current_frame_resources.buffers.backdrop_blur_buffer,
                    animation_binding_buffer: current_frame_resources
                        .buffers
                        .animation_binding_buffer,
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    custom_mesh_3d_parameters_buffer: current_frame_resources
                        .buffers
                        .custom_mesh_3d_parameters_buffer,
                    custom_mesh_3d_vertices_buffer: resources.custom_mesh_3d_vertices_buffer,
                    custom_mesh_3d_indices_buffer: resources.custom_mesh_3d_indices_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: current_frame_resources.path_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
                    synced_atlas_texture_generation: None,
                    underline_resource_set: current_frame_resources
                        .resource_sets
                        .underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    custom_mesh_3d_pipeline_layout: resources.custom_mesh_3d_pipeline_layout,
                    custom_mesh_3d_resource_set: current_frame_resources
                        .resource_sets
                        .custom_mesh_3d_resource_set,
                    custom_mesh_3d_resource_set_layout: resources
                        .custom_mesh_3d_resource_set_layout,
                    custom_mesh_3d_buffers_ready: false,
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    backdrop_blur_cache_valid: false,
                    backdrop_blur_cache_atlas_generation: 0,
                    backdrop_blur_cache_quality: None,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                })
            }
            #[cfg(not(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            )))]
            RendererBackend::NovaVulkan => {
                anyhow::bail!(
                    "nova-gfx Vulkan renderer requires the nova-gfx-vulkan feature on Windows/Linux"
                )
            }
            RendererBackend::Auto | RendererBackend::HeadlessTest => {
                anyhow::bail!("{backend} is not a concrete nova-gfx renderer")
            }
        }
    }
}
