use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::tile_render::*;

pub(super) fn render_io_error(message: impl Into<String>) -> bedrock_render::BedrockRenderError {
    let message = message.into();
    bedrock_render::BedrockRenderError::io(message.clone(), std::io::Error::other(message))
}

pub(super) fn world_cache_id(world_path: &std::path::Path) -> String {
    let mut hasher = RenderFingerprint::new();
    world_path.to_string_lossy().hash(&mut hasher);
    let hash = hasher.value();
    let folder = world_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("world")
        .chars()
        .map(|value| match value {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => value,
            _ => '_',
        })
        .collect::<String>();
    format!("{folder}-{hash:016x}")
}

pub(super) fn world_cache_signature(world_path: &std::path::Path) -> String {
    let mut hasher = RenderFingerprint::new();
    hash_path_metadata(world_path, "level.dat", &mut hasher);
    hash_leveldb_current_state(world_path, &mut hasher);
    hasher.hex()
}

fn hash_leveldb_current_state(world_path: &std::path::Path, hasher: &mut RenderFingerprint) {
    hash_path_metadata(world_path, "db/CURRENT", hasher);
    let db_path = world_path.join("db");
    let current_path = db_path.join("CURRENT");
    let Ok(current) = std::fs::read_to_string(&current_path) else {
        return;
    };
    let manifest_name = current.trim();
    manifest_name.hash(hasher);
    if !manifest_name.is_empty() {
        let manifest_relative = format!("db/{manifest_name}");
        hash_path_metadata(world_path, &manifest_relative, hasher);
    }

    let Ok(entries) = std::fs::read_dir(&db_path) else {
        return;
    };
    let mut storage_file_names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?;
            matches!(extension, "log" | "ldb" | "sst")
                .then(|| path.file_name()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect::<Vec<_>>();
    storage_file_names.sort();
    for file_name in storage_file_names {
        hash_path_metadata(world_path, &format!("db/{file_name}"), hasher);
    }
}

fn hash_path_metadata(
    world_path: &std::path::Path,
    relative: &str,
    hasher: &mut RenderFingerprint,
) {
    let path = world_path.join(relative);
    relative.hash(hasher);
    if let Ok(metadata) = std::fs::metadata(&path) {
        metadata.len().hash(hasher);
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            duration.as_secs().hash(hasher);
            duration.subsec_nanos().hash(hasher);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RenderCacheIdentity {
    pub(super) world_id: String,
    pub(super) renderer_signature: String,
    pub(super) validation_seed: u64,
}

pub(super) fn decoded_cache_identity(
    world_path: &std::path::Path,
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> RenderCacheIdentity {
    let renderer_signature = decoded_cache_signature(backend, gpu_backend);
    RenderCacheIdentity {
        world_id: world_cache_id(world_path),
        validation_seed: render_cache_validation_seed_from_signature(&renderer_signature),
        renderer_signature,
    }
}

pub(super) fn decoded_cache_signature(
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> String {
    let mut hasher = RenderFingerprint::new();
    RENDER_PRESET_CACHE_VERSION.hash(&mut hasher);
    backend.cache_slug().hash(&mut hasher);
    gpu_backend.cache_slug().hash(&mut hasher);
    "brtile".hash(&mut hasher);
    UI_DECODED_TILE_CACHE_EXTENSION.hash(&mut hasher);
    "gpui-render-image-rgba-v1".hash(&mut hasher);
    "dynamic-memory-budget-v1".hash(&mut hasher);
    RENDER_PIPELINE_DEPTH.hash(&mut hasher);

    let layout = web_relief_render_layout();
    layout.chunks_per_tile.hash(&mut hasher);
    layout.blocks_per_pixel.hash(&mut hasher);
    layout.pixels_per_block.hash(&mut hasher);

    let region_layout = web_relief_region_layout();
    region_layout.chunks_per_region.hash(&mut hasher);

    hash_surface_options(web_relief_surface_options(), &mut hasher);
    hasher.hex()
}

pub(super) fn render_preset_cache_signature(
    world_path: &std::path::Path,
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> String {
    let mut hasher = RenderFingerprint::new();
    world_cache_signature(world_path).hash(&mut hasher);
    decoded_cache_signature(backend, gpu_backend).hash(&mut hasher);
    hasher.hex()
}

#[cfg(test)]
pub(super) fn render_preset_cache_validation_seed(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
) -> u64 {
    let signature = render_preset_cache_signature(world_path, render_backend, render_gpu_backend);
    render_cache_validation_seed_from_signature(&signature)
}

pub(super) fn render_cache_validation_seed_from_signature(signature: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in signature.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if hash == 0 {
        0xcbf2_9ce4_8422_2325
    } else {
        hash
    }
}

pub(super) fn tile_manifest_cache_path(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
) -> PathBuf {
    file_ops::cache_subdir("bedrock-render")
        .join("ui-manifest")
        .join(world_cache_id(world_path))
        .join(render_preset_cache_signature(
            world_path,
            render_backend,
            render_gpu_backend,
        ))
        .join(format!("dimension-{}", dimension.id()))
        .join(render_mode_cache_slug(mode))
        .join(format!(
            "{}c-{}bpp-{}ppb.brmanifest.zst",
            layout.chunks_per_tile, layout.blocks_per_pixel, layout.pixels_per_block
        ))
}

pub(super) fn render_mode_cache_slug(mode: RenderMode) -> String {
    match mode {
        RenderMode::Biome { y } => format!("biome-y{y}"),
        RenderMode::RawBiomeLayer { y } => format!("raw-biome-y{y}"),
        RenderMode::SurfaceBlocks => "surface".to_string(),
        RenderMode::LayerBlocks { y } => format!("layer-y{y}"),
        RenderMode::HeightMap => "heightmap".to_string(),
        RenderMode::RawHeightMap => "raw-heightmap".to_string(),
        RenderMode::CaveSlice { y } => format!("cave-y{y}"),
    }
}

pub(super) fn hash_surface_options<H: Hasher>(surface: SurfaceRenderOptions, hasher: &mut H) {
    surface.transparent_water.hash(hasher);
    surface.biome_tint.hash(hasher);
    surface.height_shading.hash(hasher);
    surface.skip_air.hash(hasher);
    surface.render_unknown_blocks.hash(hasher);
    hash_lighting_options(surface.lighting, hasher);
    hash_block_boundary_options(surface.block_boundaries, hasher);
    hash_block_volume_options(surface.block_volume, hasher);
    hash_atlas_options(surface.atlas, hasher);
}

pub(super) fn hash_block_boundary_options<H: Hasher>(
    block_boundaries: BlockBoundaryRenderOptions,
    hasher: &mut H,
) {
    block_boundaries.enabled.hash(hasher);
    hash_f32(block_boundaries.strength, hasher);
    hash_f32(block_boundaries.flat_strength, hasher);
    hash_f32(block_boundaries.height_threshold, hasher);
    hash_f32(block_boundaries.max_shadow, hasher);
    hash_f32(block_boundaries.highlight_strength, hasher);
    hash_f32(block_boundaries.softness, hasher);
    hash_f32(block_boundaries.line_width_pixels, hasher);
}

pub(super) fn hash_block_volume_options<H: Hasher>(
    block_volume: BlockVolumeRenderOptions,
    hasher: &mut H,
) {
    block_volume.enabled.hash(hasher);
    hash_f32(block_volume.face_width_pixels, hasher);
    hash_f32(block_volume.face_shadow_strength, hasher);
    hash_f32(block_volume.contact_shadow_strength, hasher);
    hash_f32(block_volume.cast_shadow_strength, hasher);
    block_volume.cast_shadow_max_blocks.hash(hasher);
    hash_f32(block_volume.cast_shadow_height_scale, hasher);
    hash_f32(block_volume.highlight_strength, hasher);
    hash_f32(block_volume.max_shadow, hasher);
    hash_f32(block_volume.max_highlight, hasher);
    hash_f32(block_volume.height_threshold, hasher);
    hash_f32(block_volume.softness, hasher);
}

pub(super) fn hash_atlas_options<H: Hasher>(atlas: AtlasRenderOptions, hasher: &mut H) {
    atlas.enabled.hash(hasher);
    hash_f32(atlas.texture_detail_strength, hasher);
    atlas.height_contour_interval.hash(hasher);
    hash_f32(atlas.height_contour_strength, hasher);
    hash_f32(atlas.slope_hatching_strength, hasher);
    hash_f32(atlas.forest_canopy_strength, hasher);
    hash_f32(atlas.snow_ridge_strength, hasher);
    hash_f32(atlas.water_grid_strength, hasher);
    hash_f32(atlas.shoreline_shadow_strength, hasher);
    hash_f32(atlas.chunk_grid_strength, hasher);
    hash_f32(atlas.material_edge_strength, hasher);
    hash_f32(atlas.cast_shadow_strength, hasher);
    hash_f32(atlas.ambient_occlusion_strength, hasher);
}

pub(super) fn hash_lighting_options<H: Hasher>(lighting: TerrainLightingOptions, hasher: &mut H) {
    lighting.enabled.hash(hasher);
    hash_f32(lighting.light_azimuth_degrees, hasher);
    hash_f32(lighting.light_elevation_degrees, hasher);
    hash_f32(lighting.normal_strength, hasher);
    hash_f32(lighting.shadow_strength, hasher);
    hash_f32(lighting.highlight_strength, hasher);
    hash_f32(lighting.ambient_occlusion, hasher);
    hash_f32(lighting.max_shadow, hasher);
    hash_f32(lighting.land_slope_softness, hasher);
    hash_f32(lighting.edge_relief_strength, hasher);
    hash_f32(lighting.edge_relief_threshold, hasher);
    hash_f32(lighting.edge_relief_max_shadow, hasher);
    hash_f32(lighting.edge_relief_highlight, hasher);
    lighting.underwater_relief_enabled.hash(hasher);
    hash_f32(lighting.underwater_relief_strength, hasher);
    hash_f32(lighting.underwater_depth_fade, hasher);
    hash_f32(lighting.underwater_min_light, hasher);
}

pub(super) fn hash_f32<H: Hasher>(value: f32, hasher: &mut H) {
    value.to_bits().hash(hasher);
}

pub(super) fn render_backend_label(
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> &'static str {
    match backend {
        RenderBackend::Cpu => "CPU",
        RenderBackend::Auto => "Auto GPU",
        RenderBackend::Wgpu => match gpu_backend {
            RenderGpuBackend::Auto => "Nova Auto",
            RenderGpuBackend::Dx11 => "DX11",
            RenderGpuBackend::Dx12 => "DX12",
            RenderGpuBackend::Vulkan => "Vulkan",
        },
    }
}

pub(super) const fn default_interactive_render_backend() -> RenderBackend {
    RenderBackend::Auto
}

pub(super) const fn default_interactive_render_gpu_backend() -> RenderGpuBackend {
    RenderGpuBackend::Auto
}

pub(super) fn available_system_memory_bytes() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.available_memory()
}

pub(super) fn render_cpu_chunk_batch_size(worker_count: usize) -> usize {
    worker_count.saturating_mul(4).clamp(4, 32)
}

pub(super) fn tile_cache_memory_limit(cpu_budget: RenderCpuBudget) -> usize {
    cpu_budget.thread_count().saturating_mul(64).clamp(64, 512)
}

pub(super) fn ui_tile_memory_budget_bytes(viewport: MapViewport) -> usize {
    let visible_tiles = tile_count_for_viewport(viewport, RETAIN_RADIUS).unwrap_or(16);
    let visible_budget = visible_tiles
        .saturating_mul(DEFAULT_TILE_SIZE as usize)
        .saturating_mul(DEFAULT_TILE_SIZE as usize)
        .saturating_mul(4);
    let available_budget = usize::try_from(available_system_memory_bytes() / 32)
        .unwrap_or(MIN_UI_TILE_MEMORY_BUDGET_BYTES);

    visible_budget.max(available_budget).clamp(
        MIN_UI_TILE_MEMORY_BUDGET_BYTES,
        MAX_UI_TILE_MEMORY_BUDGET_BYTES,
    )
}

pub(super) fn tile_count_for_viewport(viewport: MapViewport, radius: i32) -> Option<usize> {
    let width_tiles = (viewport.width / DEFAULT_TILE_SIZE / viewport.scale.max(0.01))
        .ceil()
        .max(1.0) as usize;
    let height_tiles = (viewport.height / DEFAULT_TILE_SIZE / viewport.scale.max(0.01))
        .ceil()
        .max(1.0) as usize;
    let radius_tiles = usize::try_from(radius.max(0)).ok()?;
    Some(
        width_tiles
            .saturating_add(radius_tiles.saturating_mul(2))
            .saturating_mul(height_tiles.saturating_add(radius_tiles.saturating_mul(2))),
    )
}

pub(super) fn render_memory_budget_bytes(work_items: usize, cpu_budget: RenderCpuBudget) -> u64 {
    let per_tile_budget = (DEFAULT_TILE_SIZE as u64)
        .saturating_mul(DEFAULT_TILE_SIZE as u64)
        .saturating_mul(8);
    let active_tiles = work_items
        .max(cpu_budget.thread_count())
        .min(cpu_budget.tile_batch_size().max(1));
    let work_budget = per_tile_budget.saturating_mul(active_tiles as u64);
    let available_budget = available_system_memory_bytes() / 8;

    work_budget
        .max(available_budget.min(MAX_RENDER_MEMORY_BUDGET_BYTES))
        .clamp(
            MIN_RENDER_MEMORY_BUDGET_BYTES,
            MAX_RENDER_MEMORY_BUDGET_BYTES,
        )
}

pub(super) fn render_staging_pool_bytes(work_items: usize, cpu_budget: RenderCpuBudget) -> usize {
    let per_tile_budget = (DEFAULT_TILE_SIZE as usize)
        .saturating_mul(DEFAULT_TILE_SIZE as usize)
        .saturating_mul(4);
    let active_tiles = work_items
        .max(cpu_budget.thread_count())
        .min(cpu_budget.thread_count().saturating_mul(2).max(1));

    per_tile_budget
        .saturating_mul(active_tiles)
        .clamp(MIN_RENDER_STAGING_POOL_BYTES, MAX_RENDER_STAGING_POOL_BYTES)
}

pub(super) fn gpu_status_text(stats: &RenderPipelineStats) -> String {
    if stats.gpu_tiles == 0 {
        if let Some(reason) = stats.gpu_fallback_reason.as_ref() {
            return format!("GPU 已回退 CPU：{}", localize_gpu_reason(reason));
        }
        if matches!(
            stats.resolved_backend,
            ResolvedRenderBackend::WgpuDx12
                | ResolvedRenderBackend::WgpuVulkan
                | ResolvedRenderBackend::Dx11
        ) {
            return format!(
                "GPU 合成已启用，等待可提交批次 · 后端 {} · 回读 {}ms",
                resolved_backend_label_zh(stats.resolved_backend),
                stats.gpu_readback_ms
            );
        }
        return "GPU 未启用或未使用 · 交互默认 CPU；手动切换 GPU 可查看 DX11/Vulkan 诊断"
            .to_string();
    }
    let adapter = stats.gpu_adapter_name.as_deref().unwrap_or("未知适配器");
    let device = stats.gpu_device_name.as_deref().unwrap_or("未知设备");
    format!(
        "GPU 合成已启用 · 后端 {} · {} · {} · {} 瓦片 · 派发 {}ms · 回读 {}ms",
        resolved_backend_label_zh(stats.resolved_backend),
        compact_gpu_name(adapter),
        compact_gpu_name(device),
        stats.gpu_tiles,
        stats.gpu_dispatch_ms,
        stats.gpu_readback_ms
    )
}

pub(super) fn render_gpu_backend_label_zh(backend: RenderGpuBackend) -> &'static str {
    match backend {
        RenderGpuBackend::Auto => "自动",
        RenderGpuBackend::Dx11 => "DX11",
        RenderGpuBackend::Dx12 => "DX12",
        RenderGpuBackend::Vulkan => "Vulkan",
    }
}

pub(super) fn render_gpu_backend_status_zh(stats: &RenderPipelineStats) -> String {
    if stats.gpu_tiles == 0 && stats.gpu_fallback_reason.is_none() {
        return "GPU 未启用".to_string();
    }
    format!(
        "请求 {} · 实际 {}",
        render_gpu_backend_label_zh(stats.gpu_requested_backend),
        render_gpu_backend_label_zh(stats.gpu_actual_backend)
    )
}

pub(super) fn resolved_backend_label_zh(backend: ResolvedRenderBackend) -> &'static str {
    match backend {
        ResolvedRenderBackend::Cpu => "CPU",
        ResolvedRenderBackend::Dx11 => "DX11",
        ResolvedRenderBackend::WgpuDx12 => "DX12",
        ResolvedRenderBackend::WgpuVulkan => "Vulkan",
        ResolvedRenderBackend::Mixed => "混合",
    }
}

pub(super) fn localize_gpu_reason(reason: &str) -> String {
    let lower = reason.to_ascii_lowercase();
    let prefix = if lower.contains("not compiled") || lower.contains("not enabled") {
        "GPU 后端未编译"
    } else if lower.contains("unavailable") {
        "GPU 后端不可用"
    } else if lower.contains("device") {
        "GPU 设备不可用"
    } else if lower.contains("adapter") {
        "GPU 适配器不可用"
    } else if lower.contains("dx12") {
        "DX12 不可用"
    } else if lower.contains("vulkan") {
        "Vulkan 不可用"
    } else {
        "GPU 错误"
    };
    format!("{prefix}（{reason}）")
}

pub(super) fn compact_gpu_name(name: &str) -> String {
    const MAX_LEN: usize = 48;
    let trimmed = name.trim();
    if trimmed.chars().count() <= MAX_LEN {
        return trimmed.to_string();
    }
    let mut value = trimmed
        .chars()
        .take(MAX_LEN.saturating_sub(1))
        .collect::<String>();
    value.push('…');
    value
}

pub(super) fn spawn_block_center(world_path: &std::path::Path) -> Option<(i32, i32)> {
    let document = bedrock_world::read_level_dat_document(&world_path.join("level.dat")).ok()?;
    let root = match &document.root {
        NbtTag::Compound(root) => root,
        _ => return None,
    };
    let spawn_x = nbt_i32(root.get("SpawnX")?)?;
    let spawn_z = nbt_i32(root.get("SpawnZ")?)?;
    Some((spawn_x, spawn_z))
}

pub(super) fn nbt_i32(tag: &NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

pub(super) fn viewer_mode_from_render_mode(mode: RenderMode) -> ViewerMode {
    match mode {
        RenderMode::SurfaceBlocks => ViewerMode::Surface,
        RenderMode::Biome { .. } | RenderMode::RawBiomeLayer { .. } => ViewerMode::Biome,
        RenderMode::HeightMap | RenderMode::RawHeightMap => ViewerMode::Height,
        RenderMode::LayerBlocks { .. } => ViewerMode::Layer,
        RenderMode::CaveSlice { .. } => ViewerMode::Cave,
    }
}

pub(super) fn map_input_state(
    window: &mut Window,
    cx: &mut Context<MapViewerWindowView>,
    placeholder: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_placeholder(SharedString::from(placeholder), window, cx);
        input
    })
}

pub(super) fn map_input_subscriptions(
    input_fields: &MapInputFields,
    cx: &mut Context<MapViewerWindowView>,
) -> Vec<Subscription> {
    [
        (MapInputField::CenterX, input_fields.center_x.clone()),
        (MapInputField::CenterZ, input_fields.center_z.clone()),
        (
            MapInputField::ZoomPercent,
            input_fields.zoom_percent.clone(),
        ),
        (
            MapInputField::DimensionId,
            input_fields.dimension_id.clone(),
        ),
    ]
    .into_iter()
    .map(|(field, input)| {
        cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
            this.handle_map_input_event(field, input, event, cx);
        })
    })
    .collect()
}

pub(super) fn parse_i32_input(value: &str, label: &'static str) -> Result<i32, SharedString> {
    if value.is_empty() {
        return Err(SharedString::from(format!("{label} 不能为空")));
    }
    value
        .parse::<i32>()
        .map_err(|_| SharedString::from(format!("{label} 必须是整数")))
}

pub(super) fn parse_zoom_scale(value: &str) -> Result<f32, SharedString> {
    let value = value.trim_end_matches('%').trim();
    if value.is_empty() {
        return Err(SharedString::from("缩放不能为空"));
    }
    let percent = value
        .parse::<f32>()
        .map_err(|_| SharedString::from("缩放必须是数字或百分比"))?;
    if !percent.is_finite() || percent <= 0.0 {
        return Err(SharedString::from("缩放必须大于 0"));
    }
    Ok((percent / 100.0).clamp(MIN_VIEWPORT_SCALE, MAX_VIEWPORT_SCALE))
}

pub(super) fn dimension_buttons(
    active: Dimension,
    custom_id: i32,
    colors: &ThemeColors,
    cx: &mut Context<MapViewerWindowView>,
) -> Vec<AnyElement> {
    [
        (Dimension::Overworld, "主世界"),
        (Dimension::Nether, "下界"),
        (Dimension::End, "末地"),
        (Dimension::Unknown(custom_id), "自定义"),
    ]
    .into_iter()
    .map(|(dimension, label)| {
        mode_button(colors, label, active == dimension)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| this.set_dimension(dimension, cx)),
            )
            .into_any_element()
    })
    .collect()
}

pub(super) fn slime_query_window_buttons(
    active: SlimeQueryWindowSize,
    colors: &ThemeColors,
    cx: &mut Context<MapViewerWindowView>,
) -> Vec<AnyElement> {
    [
        SlimeQueryWindowSize::Three,
        SlimeQueryWindowSize::Five,
        SlimeQueryWindowSize::Seven,
    ]
    .into_iter()
    .map(|size| {
        mode_button(colors, size.label(), active == size)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.set_slime_query_window_size(size, cx)
                }),
            )
            .into_any_element()
    })
    .collect()
}

pub(super) fn context_menu_chunk(menu: ContextMenuState, dimension: Dimension) -> ChunkPos {
    chunk_from_block(menu.block_x, menu.block_z, dimension)
}

pub(super) fn clamp_stage_position(
    position: Point<Pixels>,
    viewport_width: f32,
    viewport_height: f32,
) -> Point<Pixels> {
    point(
        px((position.x / px(1.0)).clamp(0.0, viewport_width.max(1.0))),
        px((position.y / px(1.0)).clamp(0.0, viewport_height.max(1.0))),
    )
}
