use super::model::*;
use super::panels::*;
use super::prelude::*;

pub(super) fn render_io_error(message: impl Into<String>) -> bedrock_render::BedrockRenderError {
    let message = message.into();
    bedrock_render::BedrockRenderError::io(message.clone(), std::io::Error::other(message))
}

pub(super) fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
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

const SYSTEM_MEMORY_CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default)]
struct SystemMemoryCache {
    last_refresh: Option<Instant>,
    available_bytes: u64,
}

impl SystemMemoryCache {
    fn available_bytes(&mut self) -> u64 {
        let now = Instant::now();
        if self.last_refresh.is_none_or(|last_refresh| {
            now.saturating_duration_since(last_refresh) >= SYSTEM_MEMORY_CACHE_TTL
        }) {
            self.available_bytes = refresh_available_system_memory_bytes();
            self.last_refresh = Some(now);
        }
        self.available_bytes
    }
}

pub(super) fn available_system_memory_bytes() -> u64 {
    static CACHE: OnceLock<Mutex<SystemMemoryCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(SystemMemoryCache::default()));
    match cache.lock() {
        Ok(mut cache) => cache.available_bytes(),
        Err(poisoned) => poisoned.into_inner().available_bytes(),
    }
}

fn refresh_available_system_memory_bytes() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.available_memory()
}

pub(super) fn render_cpu_chunk_batch_size(worker_count: usize) -> usize {
    worker_count.saturating_mul(4).clamp(4, 32)
}

pub(super) fn manifest_probe_worker_count(cpu_budget: RenderCpuBudget) -> usize {
    cpu_budget
        .thread_count()
        .clamp(1, TILE_MANIFEST_PROBE_MAX_WORKERS)
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

    visible_budget
        .max(available_budget)
        .max(MIN_UI_TILE_MEMORY_BUDGET_BYTES)
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

pub(super) fn visible_render_batch_size(
    base_batch_size: usize,
    visible_tile_count: usize,
    is_dragging: bool,
    is_initial_load: bool,
) -> usize {
    let mut batch_size = base_batch_size.max(1);
    if visible_tile_count >= OVERVIEW_VISIBLE_TILE_THRESHOLD {
        batch_size = batch_size.max(OVERVIEW_VISIBLE_BATCH_LIMIT);
    }
    if is_dragging {
        batch_size = batch_size.min(DRAG_VISIBLE_BATCH_LIMIT.max(1));
    }
    if is_initial_load {
        let first_floor = if visible_tile_count >= OVERVIEW_VISIBLE_TILE_THRESHOLD {
            OVERVIEW_FIRST_VISIBLE_BATCH_LIMIT
        } else {
            FIRST_VISIBLE_BATCH_LIMIT
        };
        batch_size = batch_size.max(first_floor.max(1));
    }
    batch_size
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
