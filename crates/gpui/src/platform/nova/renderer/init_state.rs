use super::*;

pub(super) struct InitializedRenderer {
    pub(super) backend: SharedBackend,
    pub(super) backend_info: NovaBackendInfo,
    pub(super) surface: SurfaceId,
    pub(super) swapchain: SwapchainId,
    pub(super) surface_config: SurfaceConfig,
    pub(super) surface_alpha: SurfaceAlphaState,
    pub(super) resources: RendererResources,
    pub(super) current_size: DrawableSize,
    pub(super) atlas: NovaRendererAtlas,
    pub(super) rendering_parameters: RenderingParameters,
    pub(super) submission_mode: GpuSubmissionMode,
    pub(super) metrics_started_at: Instant,
}

impl NovaRenderer {
    pub(super) fn from_initialized(state: InitializedRenderer) -> Result<Self> {
        let InitializedRenderer {
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
        } = state;
        let present_mode = surface_config.present_mode;
        let gpu_atlas_textures = initial_gpu_atlas_textures(&resources);
        let frame_resources = resources.frame_resources;
        let current_frame_resources = frame_resources
            .first()
            .copied()
            .context("nova renderer resources should include at least one frame slot")?;
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
            backdrop_blur_pass_buffer: current_frame_resources.buffers.backdrop_blur_pass_buffer,
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
            underline_resource_set: current_frame_resources.resource_sets.underline_resource_set,
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
}
