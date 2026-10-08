use super::*;

impl NovaRenderer {
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
        let mut failures = Vec::new();
        for candidate in renderer_options.candidates(backend) {
            match Self::with_atlas(
                window,
                candidate,
                renderer_options,
                submission_mode,
                drawable_size,
                transparent,
                NovaRendererAtlas::new(),
            ) {
                Ok(renderer) => {
                    crate::diagnostics::performance_metrics::record_renderer_backend(candidate);
                    return Ok(renderer);
                }
                Err(error) => {
                    log::warn!("Nova {candidate} initialization failed: {error:#}");
                    failures.push(format!("{candidate}: {error:#}"));
                }
            }
        }
        anyhow::bail!("no usable renderer: {}", failures.join("; "))
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
        let initialization = Self::initialize(
            window,
            backend,
            renderer_options,
            submission_mode,
            drawable_size,
            transparent,
            atlas,
        );
        if initialization.is_err() {
            discard_unused_device(
                backend,
                renderer_options,
                window.display_handle().ok().map(|handle| handle.as_raw()),
            );
        }
        initialization
    }

    fn initialize<W>(
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
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            RendererBackend::NovaOpenGl => {
                let device_started_at = Instant::now();
                let backend = shared_opengl_device(
                    renderer_options,
                    window.display_handle()?.as_raw(),
                    window.window_handle()?.as_raw(),
                )?;
                let mut backend_guard = lock_backend(&backend);
                let device = match &mut *backend_guard {
                    NovaBackend::OpenGl(device) => device,
                    _ => anyhow::bail!("shared nova backend is not a OpenGL device"),
                };
                let device_elapsed = device_started_at.elapsed();
                let surface_started_at = Instant::now();
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova OpenGL surface")?;
                let native_surface_elapsed = surface_started_at.elapsed();
                let swapchain_started_at = Instant::now();
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova OpenGL swapchain")?;
                let swapchain_elapsed = swapchain_started_at.elapsed();
                let surface_elapsed = surface_started_at.elapsed();
                let core_started_at = Instant::now();
                let core = shared_renderer_core(
                    DeviceKey {
                        backend: RendererBackend::NovaOpenGl,
                        adapter_name: renderer_options.adapter_name.clone(),
                        power_preference: nova_power_preference(renderer_options),
                        pipeline_cache_dir: renderer_options.pipeline_cache_dir.clone(),
                        native_display: Some(window.display_handle()?.as_raw()),
                    },
                    surface_config.format,
                    || {
                        create_renderer_core(
                            device,
                            surface_config,
                            "gpui nova opengl",
                            cached_nova_opengl_shader_binaries()?,
                        )
                    },
                )?;
                let core_elapsed = core_started_at.elapsed();
                let resources_started_at = Instant::now();
                let resources =
                    create_renderer_resources(device, surface_config, "gpui nova opengl", &core)
                        .context("creating GPUI nova OpenGL render resources")?;
                log::info!(
                    "GPUI nova-gfx OpenGL startup: total_ms={} device_ms={} surface_swapchain_ms={} surface_ms={} swapchain_ms={} core_ms={} resources_ms={}",
                    metrics_started_at.elapsed().as_millis(),
                    device_elapsed.as_millis(),
                    surface_elapsed.as_millis(),
                    native_surface_elapsed.as_millis(),
                    swapchain_elapsed.as_millis(),
                    core_elapsed.as_millis(),
                    resources_started_at.elapsed().as_millis(),
                );
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Self::from_initialized(InitializedRenderer {
                    backend,
                    backend_info,
                    surface,
                    swapchain,
                    surface_config,
                    surface_alpha,
                    resources,
                    current_size,
                    atlas,
                    rendering_parameters,
                    submission_mode,
                    metrics_started_at,
                })
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            RendererBackend::NovaDx11 => {
                let device_started_at = Instant::now();
                let backend = shared_device(RendererBackend::NovaDx11, renderer_options)?;
                let mut backend_guard = lock_backend(&backend);
                let device = match &mut *backend_guard {
                    NovaBackend::Dx11(device) => device,
                    _ => anyhow::bail!("shared nova backend is not a DX11 device"),
                };
                let device_elapsed = device_started_at.elapsed();
                let surface_started_at = Instant::now();
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova DX11 surface")?;
                let native_surface_elapsed = surface_started_at.elapsed();
                let swapchain_started_at = Instant::now();
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova DX11 swapchain")?;
                let swapchain_elapsed = swapchain_started_at.elapsed();
                let surface_elapsed = surface_started_at.elapsed();
                let core_started_at = Instant::now();
                let core = shared_renderer_core(
                    DeviceKey {
                        backend: RendererBackend::NovaDx11,
                        adapter_name: renderer_options.adapter_name.clone(),
                        power_preference: nova_power_preference(renderer_options),
                        pipeline_cache_dir: renderer_options.pipeline_cache_dir.clone(),
                        native_display: None,
                    },
                    surface_config.format,
                    || {
                        create_renderer_core(
                            device,
                            surface_config,
                            "gpui nova dx11",
                            cached_nova_dx11_shader_binaries()?,
                        )
                    },
                )?;
                let core_elapsed = core_started_at.elapsed();
                let resources_started_at = Instant::now();
                let resources =
                    create_renderer_resources(device, surface_config, "gpui nova dx11", &core)
                        .context("creating GPUI nova DX11 render resources")?;
                log::info!(
                    "GPUI nova-gfx DX11 startup: total_ms={} device_ms={} surface_swapchain_ms={} surface_ms={} swapchain_ms={} core_ms={} resources_ms={}",
                    metrics_started_at.elapsed().as_millis(),
                    device_elapsed.as_millis(),
                    surface_elapsed.as_millis(),
                    native_surface_elapsed.as_millis(),
                    swapchain_elapsed.as_millis(),
                    core_elapsed.as_millis(),
                    resources_started_at.elapsed().as_millis(),
                );
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Self::from_initialized(InitializedRenderer {
                    backend,
                    backend_info,
                    surface,
                    swapchain,
                    surface_config,
                    surface_alpha,
                    resources,
                    current_size,
                    atlas,
                    rendering_parameters,
                    submission_mode,
                    metrics_started_at,
                })
            }
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
                        native_display: None,
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
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Self::from_initialized(InitializedRenderer {
                    backend,
                    backend_info,
                    surface,
                    swapchain,
                    surface_config,
                    surface_alpha,
                    resources,
                    current_size,
                    atlas,
                    rendering_parameters,
                    submission_mode,
                    metrics_started_at,
                })
            }
            #[cfg(not(all(feature = "nova-gfx-dx12", target_os = "windows")))]
            RendererBackend::NovaDx12 => {
                anyhow::bail!(
                    "nova-gfx DX12 renderer requires the nova-gfx-dx12 feature on Windows"
                )
            }
            #[cfg(not(all(feature = "nova-gfx-dx11", target_os = "windows")))]
            RendererBackend::NovaDx11 => {
                anyhow::bail!("nova-gfx DX11 renderer requires nova-gfx-dx11 on Windows")
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
                        native_display: None,
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
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Self::from_initialized(InitializedRenderer {
                    backend,
                    backend_info,
                    surface,
                    swapchain,
                    surface_config,
                    surface_alpha,
                    resources,
                    current_size,
                    atlas,
                    rendering_parameters,
                    submission_mode,
                    metrics_started_at,
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
                        native_display: None,
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
                let backend_info = backend_guard.info();
                drop(backend_guard);
                Self::from_initialized(InitializedRenderer {
                    backend,
                    backend_info,
                    surface,
                    swapchain,
                    surface_config,
                    surface_alpha,
                    resources,
                    current_size,
                    atlas,
                    rendering_parameters,
                    submission_mode,
                    metrics_started_at,
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
            #[cfg(not(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            )))]
            RendererBackend::NovaOpenGl => {
                anyhow::bail!("OpenGL requires nova-gfx-opengl on Windows or Linux")
            }
            RendererBackend::Auto | RendererBackend::HeadlessTest => {
                anyhow::bail!("{backend} is not a concrete nova-gfx renderer")
            }
        }
    }
}
