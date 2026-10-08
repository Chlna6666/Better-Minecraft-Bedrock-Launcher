#![expect(
    unsafe_code,
    reason = "renderer initialization creates native surfaces and queries window handles"
)]

use super::*;

#[cfg(target_os = "windows")]
fn native_windows_hwnd<W>(window: &W) -> Option<isize>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    use ::winit::raw_window_handle::RawWindowHandle;

    let raw_window_handle = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::Win32(handle) = raw_window_handle else {
        return None;
    };
    Some(handle.hwnd.get())
}

#[cfg(target_os = "windows")]
fn native_windows_drawable_size<W>(window: &W) -> Option<Size<DevicePixels>>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    use windows::Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::GetClientRect,
    };

    let hwnd = HWND(native_windows_hwnd(window)? as *mut _);
    let mut client_rect = RECT::default();
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
    /// Prepares the shared device and BGRA renderer core on their GPU owner without a window.
    ///
    /// # Errors
    ///
    /// Returns device or pipeline initialization errors. Only successful cache entries are
    /// retained, so regular window initialization can retry an unfinished preparation.
    #[cfg(target_os = "windows")]
    pub(crate) fn prepare_renderer(options: &RendererOptions) -> Result<()> {
        let started_at = Instant::now();
        let backend = shared_device(options.backend, options)?;
        let device_elapsed = started_at.elapsed();
        let mut backend = lock_backend(&backend);
        // Core pipelines use dynamic viewport/scissor; only the attachment format is keyed.
        // Window-sized textures and buffers are deliberately left to window initialization.
        let config = SurfaceConfig::new(1, 1, Format::Bgra8Unorm)?;
        let key = DeviceKey {
            backend: options.backend,
            adapter_name: options.adapter_name.clone(),
            power_preference: nova_power_preference(options),
            pipeline_cache_dir: options.pipeline_cache_dir.clone(),
        };
        let core_started_at = Instant::now();
        match &mut *backend {
            #[cfg(feature = "nova-gfx-dx12")]
            NovaBackend::Dx12(device) => {
                shared_renderer_core(key, config.format, || {
                    create_renderer_core(
                        device,
                        config,
                        "gpui nova dx12",
                        cached_nova_dx12_shader_binaries()?,
                    )
                })?;
            }
            #[cfg(feature = "nova-gfx-vulkan")]
            NovaBackend::Vulkan(device) => {
                shared_renderer_core(key, config.format, || {
                    create_renderer_core(
                        device,
                        config,
                        "gpui nova vulkan",
                        cached_nova_vulkan_shader_binaries()?,
                    )
                })?;
            }
            _ => anyhow::bail!("{} cannot prepare a Windows renderer", options.backend),
        }
        log::info!(
            "GPUI renderer preparation: backend={} total_ms={} device_ms={} core_ms={}",
            options.backend,
            started_at.elapsed().as_millis(),
            device_elapsed.as_millis(),
            core_started_at.elapsed().as_millis(),
        );
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
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
        Self::with_atlas(
            window,
            backend,
            renderer_options,
            submission_mode,
            drawable_size,
            transparent,
            NovaRendererAtlas::new(),
        )
    }

    pub(crate) fn with_atlas<W>(
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
        crate::diagnostics::performance_metrics::record_gpu_surface_metrics(
            &format!("{:?}", surface_config.format),
            &format!("{:?}", surface_config.alpha_mode),
            &format!("{present_mode:?}"),
            0,
            0,
        );
        let current_size = DrawableSize { width, height };
        #[cfg(target_os = "windows")]
        let rendering_parameters = native_windows_hwnd(window)
            .map(RenderingParameters::from_env_for_window)
            .unwrap_or_else(RenderingParameters::from_env);
        #[cfg(not(target_os = "windows"))]
        let rendering_parameters = RenderingParameters::from_env();

        match backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            RendererBackend::NovaDx12 => {
                let device_started_at = Instant::now();
                let backend = shared_device(RendererBackend::NovaDx12, renderer_options)?;
                let mut backend_guard = lock_backend(&backend);
                let device = match &mut *backend_guard {
                    NovaBackend::Dx12(device) => device,
                    _ => anyhow::bail!("shared nova backend is not a DX12 device"),
                };
                let device_elapsed = device_started_at.elapsed();
                let surface_started_at = Instant::now();
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova DX12 surface")?;
                let native_surface_elapsed = surface_started_at.elapsed();
                let swapchain_started_at = Instant::now();
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova DX12 swapchain")?;
                let swapchain_elapsed = swapchain_started_at.elapsed();
                let surface_elapsed = surface_started_at.elapsed();
                let core_started_at = Instant::now();
                let core = shared_renderer_core(
                    DeviceKey {
                        backend: RendererBackend::NovaDx12,
                        adapter_name: renderer_options.adapter_name.clone(),
                        power_preference: nova_power_preference(renderer_options),
                        pipeline_cache_dir: renderer_options.pipeline_cache_dir.clone(),
                    },
                    surface_config.format,
                    || {
                        create_renderer_core(
                            device,
                            surface_config,
                            "gpui nova dx12",
                            cached_nova_dx12_shader_binaries()?,
                        )
                    },
                )?;
                let core_elapsed = core_started_at.elapsed();
                let resources_started_at = Instant::now();
                let resources =
                    create_renderer_resources(device, surface_config, "gpui nova dx12", &core)
                        .context("creating GPUI nova DX12 render resources")?;
                log::info!(
                    "GPUI nova-gfx DX12 startup: total_ms={} device_ms={} surface_swapchain_ms={} surface_ms={} swapchain_ms={} core_ms={} resources_ms={}",
                    metrics_started_at.elapsed().as_millis(),
                    device_elapsed.as_millis(),
                    surface_elapsed.as_millis(),
                    native_surface_elapsed.as_millis(),
                    swapchain_elapsed.as_millis(),
                    core_elapsed.as_millis(),
                    resources_started_at.elapsed().as_millis(),
                );
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Ok(Self {
                    backend,
                    backend_info,
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
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    quad_resource_set_layout: resources.quad_resource_set_layout,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_rasterization_resource_set_layout: resources
                        .path_rasterization_resource_set_layout,
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
                    filters: filters::FilterRegistry::new(resources.backdrop_blur_targets),
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    path_texture_size: resources.path_texture_size,
                    frame_upload: FrameUpload::default(),
                    renderer_registry: extensions::RendererRegistry::default(),
                    retained_upload: retained_upload::RetainedUpload::default(),
                    draw_step_scratch: DrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0,
                    rendering_parameters,
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                    active_presentation_packet: None,
                    pending_animation_completions: SmallVec::new(),
                    destroyed: false,
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
                let backend = shared_device(RendererBackend::NovaMetal, renderer_options)?;
                let mut backend_guard = lock_backend(&backend);
                let device = match &mut *backend_guard {
                    NovaBackend::Metal(device) => device,
                    _ => anyhow::bail!("shared nova backend is not a Metal device"),
                };
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova Metal surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Metal swapchain")?;
                let core = shared_renderer_core(
                    DeviceKey {
                        backend: RendererBackend::NovaMetal,
                        adapter_name: renderer_options.adapter_name.clone(),
                        power_preference: nova_power_preference(renderer_options),
                        pipeline_cache_dir: renderer_options.pipeline_cache_dir.clone(),
                    },
                    surface_config.format,
                    || {
                        create_renderer_core(
                            device,
                            surface_config,
                            "gpui nova metal",
                            cached_nova_metal_shader_binaries()?,
                        )
                    },
                )?;
                let resources =
                    create_renderer_resources(device, surface_config, "gpui nova metal", &core)
                        .context("creating GPUI nova Metal render resources")?;
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Ok(Self {
                    backend,
                    backend_info,
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
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    quad_resource_set_layout: resources.quad_resource_set_layout,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_rasterization_resource_set_layout: resources
                        .path_rasterization_resource_set_layout,
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
                    filters: filters::FilterRegistry::new(resources.backdrop_blur_targets),
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    path_texture_size: resources.path_texture_size,
                    frame_upload: FrameUpload::default(),
                    renderer_registry: extensions::RendererRegistry::default(),
                    retained_upload: retained_upload::RetainedUpload::default(),
                    draw_step_scratch: DrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0,
                    rendering_parameters,
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                    active_presentation_packet: None,
                    pending_animation_completions: SmallVec::new(),
                    destroyed: false,
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
                let device_started_at = Instant::now();
                let backend = shared_device(RendererBackend::NovaVulkan, renderer_options)?;
                let mut backend_guard = lock_backend(&backend);
                let device = match &mut *backend_guard {
                    NovaBackend::Vulkan(device) => device,
                    _ => anyhow::bail!("shared nova backend is not a Vulkan device"),
                };
                let device_elapsed = device_started_at.elapsed();
                let surface_started_at = Instant::now();
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova Vulkan surface")?;
                let native_surface_elapsed = surface_started_at.elapsed();
                let swapchain_started_at = Instant::now();
                let surface_alpha = SurfaceAlphaState::new(
                    device
                        .resolve_surface_alpha_mode(surface, surface_alpha.swapchain_mode)
                        .context("resolving nova Vulkan surface alpha mode")?,
                );
                let alpha_elapsed = swapchain_started_at.elapsed();
                let surface_config = SurfaceConfig {
                    alpha_mode: surface_alpha.swapchain_mode,
                    ..surface_config
                };
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Vulkan swapchain")?;
                let swapchain_elapsed = swapchain_started_at.elapsed();
                let surface_elapsed = surface_started_at.elapsed();
                let core_started_at = Instant::now();
                let core = shared_renderer_core(
                    DeviceKey {
                        backend: RendererBackend::NovaVulkan,
                        adapter_name: renderer_options.adapter_name.clone(),
                        power_preference: nova_power_preference(renderer_options),
                        pipeline_cache_dir: renderer_options.pipeline_cache_dir.clone(),
                    },
                    surface_config.format,
                    || {
                        create_renderer_core(
                            device,
                            surface_config,
                            "gpui nova vulkan",
                            cached_nova_vulkan_shader_binaries()?,
                        )
                    },
                )?;
                let core_elapsed = core_started_at.elapsed();
                let resources_started_at = Instant::now();
                let resources =
                    create_renderer_resources(device, surface_config, "gpui nova vulkan", &core)
                        .context("creating GPUI nova Vulkan render resources")?;
                log::info!(
                    "GPUI nova-gfx Vulkan startup: total_ms={} device_ms={} surface_swapchain_ms={} surface_ms={} swapchain_ms={} alpha_ms={} core_ms={} resources_ms={}",
                    metrics_started_at.elapsed().as_millis(),
                    device_elapsed.as_millis(),
                    surface_elapsed.as_millis(),
                    native_surface_elapsed.as_millis(),
                    swapchain_elapsed.as_millis(),
                    alpha_elapsed.as_millis(),
                    core_elapsed.as_millis(),
                    resources_started_at.elapsed().as_millis(),
                );
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Ok(Self {
                    backend,
                    backend_info,
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
                    animation_value_buffer: current_frame_resources.buffers.animation_value_buffer,
                    quad_resource_set: current_frame_resources.resource_sets.quad_resource_set,
                    quad_resource_set_layout: resources.quad_resource_set_layout,
                    shadow_resource_set: current_frame_resources.resource_sets.shadow_resource_set,
                    path_rasterization_resource_set: current_frame_resources
                        .resource_sets
                        .path_rasterization_resource_set,
                    path_rasterization_resource_set_layout: resources
                        .path_rasterization_resource_set_layout,
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
                    filters: filters::FilterRegistry::new(resources.backdrop_blur_targets),
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    path_texture_size: resources.path_texture_size,
                    frame_upload: FrameUpload::default(),
                    renderer_registry: extensions::RendererRegistry::default(),
                    retained_upload: retained_upload::RetainedUpload::default(),
                    draw_step_scratch: DrawStepScratch::default(),
                    current_size,
                    pending_drawable_size: None,
                    atlas: atlas.0,
                    rendering_parameters,
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    swapchain_warmup_frames: SWAPCHAIN_WARMUP_FRAME_COUNT,
                    active_presentation_packet: None,
                    pending_animation_completions: SmallVec::new(),
                    destroyed: false,
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
