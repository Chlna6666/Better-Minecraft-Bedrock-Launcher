use super::*;

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
                let frame_resource_count = frame_resources.len();
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
                    present_copy_sprite_buffer: current_frame_resources
                        .buffers
                        .present_copy_sprite_buffer,
                    present_copy_sprite_upload_cache: PresentCopySpriteUploadCache::new(
                        frame_resource_count,
                    ),
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
                    present_cache_resource_set: current_frame_resources.present_cache_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
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
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    present_cache_texture: resources.present_cache_texture,
                    present_cache_texture_view: resources.present_cache_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    needs_full_redraw_after_resize: true,
                    present_cache_valid: false,
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
                let frame_resource_count = frame_resources.len();
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
                    present_copy_sprite_buffer: current_frame_resources
                        .buffers
                        .present_copy_sprite_buffer,
                    present_copy_sprite_upload_cache: PresentCopySpriteUploadCache::new(
                        frame_resource_count,
                    ),
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
                    present_cache_resource_set: current_frame_resources.present_cache_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
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
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    present_cache_texture: resources.present_cache_texture,
                    present_cache_texture_view: resources.present_cache_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    needs_full_redraw_after_resize: true,
                    present_cache_valid: false,
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
                let mut device = VulkanDevice::new(&DeviceDescriptor {
                    application_name: "gpui nova vulkan".to_string(),
                    adapter_name: renderer_options.adapter_name.clone(),
                    power_preference: nova_power_preference(renderer_options),
                })
                .context("creating nova Vulkan device")?;
                let surface = device
                    .create_surface(window, &SurfaceDescriptor { label: None })
                    .context("creating nova Vulkan surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Vulkan swapchain")?;
                let resources = create_renderer_resources(
                    &mut device,
                    surface_config,
                    "gpui nova vulkan",
                    shader_binaries,
                )
                .context("creating GPUI nova Vulkan render resources")?;
                let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
                let frame_resources = resources.frame_resources;
                let current_frame_resources = frame_resources
                    .first()
                    .copied()
                    .context("nova renderer resources should include at least one frame slot")?;
                let frame_resource_count = frame_resources.len();
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
                    present_copy_sprite_buffer: current_frame_resources
                        .buffers
                        .present_copy_sprite_buffer,
                    present_copy_sprite_upload_cache: PresentCopySpriteUploadCache::new(
                        frame_resource_count,
                    ),
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
                    present_cache_resource_set: current_frame_resources.present_cache_resource_set,
                    mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
                    poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
                    gpu_atlas_textures,
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
                    custom_mesh_3d_mesh_cache: FxHashMap::default(),
                    custom_mesh_3d_vertex_cursor: 0,
                    custom_mesh_3d_index_cursor: 0,
                    custom_mesh_3d_uploaded_bytes_this_frame: 0,
                    custom_mesh_3d_vertex_upload_scratch: Vec::new(),
                    custom_mesh_3d_index_upload_scratch: Vec::new(),
                    custom_mesh_3d_pipelines: FxHashMap::default(),
                    custom_mesh_3d_pipeline_failures: FxHashSet::default(),
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    present_cache_texture: resources.present_cache_texture,
                    present_cache_texture_view: resources.present_cache_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    draw_step_scratch: NovaDrawStepScratch::default(),
                    current_size,
                    atlas: atlas.0.clone(),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                    needs_full_redraw_after_resize: true,
                    present_cache_valid: false,
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
