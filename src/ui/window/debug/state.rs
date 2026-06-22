use crate::plugins::runtime::PluginMemoryReport;
use crate::utils::memory_diagnostics::BmcblMemorySnapshot;
use gpui::{Global, GpuSpecs, SharedString, WindowMetricsSnapshot, performance_metrics_snapshot};
use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebugInspectorSnapshot {
    pub enabled: bool,
    pub picking: bool,
    pub selected_label: SharedString,
    pub source_location: SharedString,
    pub bounds_label: SharedString,
    pub content_size_label: SharedString,
    pub background_hex: SharedString,
    pub border_hex: SharedString,
    pub opacity: Option<f32>,
    #[cfg(debug_assertions)]
    pub selected_id: Option<gpui::InspectorElementId>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebugRuntimeSnapshot {
    pub main_window_width_px: f32,
    pub main_window_height_px: f32,
    pub debug_window_width_px: f32,
    pub debug_window_height_px: f32,
    pub main_fps: f32,
    pub main_frame_time_ms: f32,
    pub main_render_time_ms: f32,
    pub main_render_time_avg_ms: f32,
    pub process_id: Option<u32>,
    pub process_task_count: Option<usize>,
    pub process_cpu_percent: f32,
    pub process_cpu_normalized_percent: f32,
    pub process_memory_bytes: u64,
    pub process_virtual_memory_bytes: u64,
    pub process_working_set_kb: u64,
    pub process_private_kb: u64,
    pub process_peak_working_set_kb: u64,
    pub system_memory_total_bytes: u64,
    pub system_memory_used_bytes: u64,
    pub system_memory_available_bytes: u64,
    pub system_memory_used_percent: f32,
    pub gpu_device_name: SharedString,
    pub gpu_driver_name: SharedString,
    pub gpu_driver_info: SharedString,
    pub gpu_software_emulated: bool,
    pub gpui_renderer_backend: SharedString,
    pub gpui_present_fps: f32,
    pub gpui_image_cache_items: usize,
    pub gpui_image_cache_bytes: usize,
    pub gpui_atlas_textures: usize,
    pub gpui_backdrop_blur_primitives: usize,
    pub gpui_draw_time_ms: f32,
    pub gpui_prepared_command_count: usize,
    pub gpui_gpu_surface_format: SharedString,
    pub gpui_gpu_surface_alpha_mode: SharedString,
    pub gpui_gpu_surface_present_mode: SharedString,
    pub gpui_upload_bytes: usize,
    pub gpui_pod_upload_bytes: usize,
    pub gpui_mask_pass_count: usize,
    pub gpui_main_pass_count: usize,
    pub gpui_composite_pass_count: usize,
    pub gpui_gpu_surface_reconfigure_count: usize,
    pub gpui_gpu_surface_error_count: usize,
    pub gpui_retained_present_count: usize,
    pub gpui_image_decode_compressed_bytes: usize,
    pub gpui_image_decode_decoded_bytes: usize,
    pub gpui_image_decode_frames: usize,
    pub gpui_image_decode_time_ms: f32,
    pub gpui_image_decode_count: usize,
    pub gpui_image_decode_total_compressed_bytes: usize,
    pub gpui_image_decode_total_decoded_bytes: usize,
    pub gpui_image_decode_total_frames: usize,
    pub gpui_image_decode_total_time_ms: f32,
    pub gpui_image_decode_max_time_ms: f32,
    pub gpui_image_decode_slow_count: usize,
    pub gpui_image_asset_retained_decoded_bytes: usize,
    pub gpui_image_asset_retained_count: usize,
    pub gpui_image_asset_largest_retained_decoded_bytes: usize,
    pub gpui_image_asset_cache_entries: usize,
    pub gpui_image_asset_retained_compressed_bytes: usize,
    pub gpui_image_asset_total_retained_decoded_bytes: usize,
    pub gpui_render_image_cpu_bytes: usize,
    pub gpui_render_image_gpu_texture_bytes: usize,
    pub gpui_icon_cache_entries: usize,
    pub gpui_icon_cache_decoded_bytes: usize,
    pub gpui_atlas_monochrome_bytes: usize,
    pub gpui_atlas_polychrome_bytes: usize,
    pub gpui_atlas_live_keys: usize,
    pub gpui_atlas_unused_bytes: usize,
    pub gpui_gpu_surface_texture_bytes: usize,
    pub gpui_gpu_estimated_total_retained_bytes: usize,
    pub gpui_global_image_resource_decoded_bytes: usize,
    pub gpui_global_image_resource_count: usize,
    pub gpui_global_image_inline_decoded_bytes: usize,
    pub gpui_global_image_inline_count: usize,
    pub gpui_global_image_compressed_bytes: usize,
    pub gpui_global_image_compressed_count: usize,
    pub gpui_global_image_target_decoded_bytes: usize,
    pub gpui_global_image_target_count: usize,
    pub gpui_global_image_assets_sampled_at: Option<Instant>,
    pub gpui_atlas_upload_bytes: usize,
    pub gpui_atlas_upload_tiles: usize,
    pub gpui_atlas_upload_time_ms: f32,
    pub gpui_first_frame_build_time_ms: f32,
    pub gpui_first_frame_layout_time_ms: f32,
    pub gpui_first_frame_prepaint_time_ms: f32,
    pub gpui_first_frame_paint_time_ms: f32,
    pub gpui_first_frame_scene_finish_time_ms: f32,
    pub gpui_first_frame_backend_draw_time_ms: f32,
    pub gpui_frame_build_time_ms: f32,
    pub gpui_frame_layout_time_ms: f32,
    pub gpui_frame_prepaint_time_ms: f32,
    pub gpui_frame_paint_time_ms: f32,
    pub gpui_frame_scene_finish_time_ms: f32,
    pub gpui_frame_backend_draw_time_ms: f32,
    pub gpui_layout_nodes: usize,
    pub gpui_measured_layout_nodes: usize,
    pub gpui_layout_roots: usize,
    pub gpui_layout_bounds_cache_hits: usize,
    pub gpui_layout_bounds_cache_misses: usize,
    pub gpui_layout_cache_hits: usize,
    pub gpui_layout_cache_misses: usize,
    pub gpui_layout_cache_reused_roots: usize,
    pub gpui_layout_cache_saved_roots: usize,
    pub gpui_text_layout_hits: usize,
    pub gpui_text_layout_reuses: usize,
    pub gpui_text_layout_misses: usize,
    pub gpui_scene_primitives: usize,
    pub gpui_scene_batches: usize,
    pub gpui_scene_segments: usize,
    pub gpui_scene_replayed_primitives: usize,
    pub gpui_scene_segment_rebuild_count: usize,
    pub gpui_scene_segment_reuse_count: usize,
    pub gpui_dirty_transform_count: usize,
    pub gpui_retained_frame_skips: usize,
    pub gpui_skipped_pointer_frame_count: usize,
    pub gpui_dirty_rect_count: usize,
    pub gpui_dirty_rect_area: usize,
    pub gpui_partial_redraw_count: usize,
    pub gpui_full_redraw_fallback_count: usize,
    pub gpui_gpu_retained_bytes: usize,
    pub gpui_atlas_retained_bytes: usize,
    pub gpui_has_retained_frame_target: bool,
    pub gpui_has_path_textures: bool,
    pub gpui_has_backdrop_texture: bool,
    pub gpui_has_depth_texture: bool,
    pub gpui_backdrop_blur_target_groups: usize,
    pub gpui_gpu_mesh_buffers: usize,
    pub gpui_coalesced_refresh_count: usize,
    pub gpui_coalesced_refresh_effect_count: usize,
    pub gpui_inactive_present_skip_count: usize,
    pub gpui_image_cache_evictions: usize,
    pub gpui_image_drop_count: usize,
    pub gpui_atlas_remove_count: usize,
    pub gpui_scheduler_wakeups: usize,
    pub gpui_idle_sleep_time_ms: f32,
    pub gpui_frame_request_count: usize,
    pub gpui_draw_count: usize,
    pub gpui_present_count: usize,
    pub gpui_skip_count: usize,
    pub gpui_bind_group_creations: usize,
    pub gpui_bind_group_cache_hits: usize,
    pub gpui_bind_group_cache_misses: usize,
    pub gpui_upload_arena_uniform_capacity: usize,
    pub gpui_upload_arena_storage_capacity: usize,
    pub gpui_upload_arena_uniform_used: usize,
    pub gpui_upload_arena_storage_used: usize,
    pub gpui_gpu_cache_hits: usize,
    pub gpui_gpu_cache_misses: usize,
    pub gpui_scene_retained_capacity: usize,
    pub gpui_frame_retained_capacity: usize,
    pub gpui_allocator_allocated_bytes: usize,
    pub gpui_allocator_reserved_bytes: usize,
    pub gpui_allocator_block_count: usize,
    pub gpui_allocator_allocation_count: usize,
    pub gpui_allocator_gpu_only_allocated_bytes: usize,
    pub gpui_allocator_gpu_only_reserved_bytes: usize,
    pub gpui_allocator_gpu_only_block_count: usize,
    pub gpui_allocator_cpu_to_gpu_allocated_bytes: usize,
    pub gpui_allocator_cpu_to_gpu_reserved_bytes: usize,
    pub gpui_allocator_cpu_to_gpu_block_count: usize,
    pub gpui_allocator_gpu_to_cpu_allocated_bytes: usize,
    pub gpui_allocator_gpu_to_cpu_reserved_bytes: usize,
    pub gpui_allocator_gpu_to_cpu_block_count: usize,
    pub gpui_hal_buffer_memory_bytes: usize,
    pub gpui_hal_texture_memory_bytes: usize,
    pub gpui_hal_acceleration_structure_memory_bytes: usize,
    pub gpui_hal_memory_allocation_count: usize,
    pub gpui_core_staging_buffer_live_bytes: usize,
    pub gpui_core_staging_buffer_peak_live_bytes: usize,
    pub gpui_core_staging_buffer_created_bytes: usize,
    pub gpui_core_staging_buffer_pending_bytes: usize,
    pub gpui_core_staging_buffer_peak_pending_bytes: usize,
    pub gpui_core_staging_buffer_live_count: usize,
    pub gpui_core_staging_buffer_peak_live_count: usize,
    pub gpui_core_staging_buffer_pending_count: usize,
    pub gpui_core_staging_buffer_peak_pending_count: usize,
    pub gpui_window_metrics: Vec<DebugWindowMetrics>,
    pub bmcbl_memory: BmcblMemorySnapshot,
    pub plugin_memory: PluginMemoryReport,
    pub frame_time_history_ms: VecDeque<f32>,
    pub render_time_history_ms: VecDeque<f32>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DebugWindowMetrics {
    pub window_id: u64,
    pub request_redraw_count: usize,
    pub draw_count: usize,
    pub present_count: usize,
    pub skip_count: usize,
    pub skipped_frame_count: usize,
    pub surface_reconfigure_count: usize,
    pub present_error_count: usize,
    pub layout_recompute_count: usize,
    pub upload_bytes: usize,
}

impl From<WindowMetricsSnapshot> for DebugWindowMetrics {
    fn from(metrics: WindowMetricsSnapshot) -> Self {
        Self {
            window_id: metrics.window_id,
            request_redraw_count: metrics.request_redraw_count,
            draw_count: metrics.draw_count,
            present_count: metrics.present_count,
            skip_count: metrics.skip_count,
            skipped_frame_count: metrics.skipped_frame_count,
            surface_reconfigure_count: metrics.gpu_surface_reconfigure_count,
            present_error_count: metrics.gpu_surface_error_count,
            layout_recompute_count: metrics.layout_recompute_count,
            upload_bytes: metrics.upload_bytes,
        }
    }
}

#[derive(Debug, Default)]
struct RuntimeMetricsState {
    snapshot: DebugRuntimeSnapshot,
    last_main_frame_at: Option<Instant>,
    main_fps_ema: f32,
    render_time_ema: f32,
}

#[derive(Debug)]
struct RuntimeSampler {
    system: System,
    pid: Option<sysinfo::Pid>,
    cpu_count: usize,
    gpu_specs: Option<GpuSpecs>,
}

static RUNTIME_METRICS: Lazy<Mutex<RuntimeMetricsState>> =
    Lazy::new(|| Mutex::new(RuntimeMetricsState::default()));
static RUNTIME_SAMPLER: Lazy<Mutex<RuntimeSampler>> =
    Lazy::new(|| Mutex::new(RuntimeSampler::new()));

impl RuntimeMetricsState {
    const HISTORY_LIMIT: usize = 180;

    fn push_history(history: &mut VecDeque<f32>, value: f32) {
        history.push_back(value);
        while history.len() > Self::HISTORY_LIMIT {
            let _ = history.pop_front();
        }
    }
}

impl RuntimeSampler {
    fn new() -> Self {
        let pid = get_current_pid().ok();
        let cpu_count = num_cpus::get().max(1);
        let mut system = System::new();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        if let Some(pid) = pid {
            let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
            let _ = system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                refresh_kind,
            );
        }
        Self {
            system,
            pid,
            cpu_count,
            gpu_specs: None,
        }
    }

    fn sample(&mut self, snapshot: &mut DebugRuntimeSnapshot) {
        snapshot.process_id = self.pid.map(sysinfo::Pid::as_u32);

        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());

        if let Some(pid) = self.pid {
            let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
            let _ = self.system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                refresh_kind,
            );

            if let Some(process) = self.system.process(pid) {
                snapshot.process_task_count = process.tasks().map(std::collections::HashSet::len);
                snapshot.process_cpu_percent = process.cpu_usage();
                snapshot.process_cpu_normalized_percent =
                    process.cpu_usage() / self.cpu_count as f32;
                snapshot.process_memory_bytes = process.memory();
                snapshot.process_virtual_memory_bytes = process.virtual_memory();
            } else {
                snapshot.process_task_count = None;
                snapshot.process_cpu_percent = 0.0;
                snapshot.process_cpu_normalized_percent = 0.0;
                snapshot.process_memory_bytes = 0;
                snapshot.process_virtual_memory_bytes = 0;
            }
        }

        let total_bytes = self.system.total_memory();
        let used_bytes = self.system.used_memory();
        let available_bytes = self.system.available_memory();
        snapshot.system_memory_total_bytes = total_bytes;
        snapshot.system_memory_used_bytes = used_bytes;
        snapshot.system_memory_available_bytes = available_bytes;
        snapshot.system_memory_used_percent = if total_bytes > 0 {
            used_bytes as f32 / total_bytes as f32 * 100.0
        } else {
            0.0
        };

        let mut memory_stats = crate::utils::memory::MemoryStats::new();
        memory_stats.refresh();
        snapshot.process_working_set_kb = memory_stats.working_set_kb;
        snapshot.process_private_kb = memory_stats.private_kb;
        snapshot.process_peak_working_set_kb = memory_stats.peak_working_set_kb;

        if let Some(gpu_specs) = self.gpu_specs.as_ref() {
            snapshot.gpu_device_name = SharedString::from(gpu_specs.device_name.clone());
            snapshot.gpu_driver_name = SharedString::from(gpu_specs.driver_name.clone());
            snapshot.gpu_driver_info = SharedString::from(gpu_specs.driver_info.clone());
            snapshot.gpu_software_emulated = gpu_specs.is_software_emulated;
        } else {
            snapshot.gpu_device_name = SharedString::from("");
            snapshot.gpu_driver_name = SharedString::from("");
            snapshot.gpu_driver_info = SharedString::from("");
            snapshot.gpu_software_emulated = false;
        }
    }
}

fn duration_to_ms(duration: Option<Duration>) -> f32 {
    duration.map_or(0.0, |duration| duration.as_secs_f32() * 1000.0)
}

pub fn record_main_window_frame(now: Instant, width_px: f32, height_px: f32) {
    let mut metrics = RUNTIME_METRICS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    metrics.snapshot.main_window_width_px = width_px;
    metrics.snapshot.main_window_height_px = height_px;

    if let Some(previous_frame) = metrics.last_main_frame_at.replace(now) {
        let delta = now.saturating_duration_since(previous_frame);
        if delta >= Duration::from_millis(1) && delta <= Duration::from_secs(1) {
            let frame_time_ms = delta.as_secs_f32() * 1000.0;
            metrics.snapshot.main_frame_time_ms = frame_time_ms;
            let instant_fps = 1000.0 / frame_time_ms;
            if metrics.main_fps_ema <= 0.0 {
                metrics.main_fps_ema = instant_fps;
            } else {
                metrics.main_fps_ema = metrics.main_fps_ema * 0.9 + instant_fps * 0.1;
            }
            metrics.snapshot.main_fps = metrics.main_fps_ema;
            RuntimeMetricsState::push_history(
                &mut metrics.snapshot.frame_time_history_ms,
                frame_time_ms,
            );
        }
    }
}

pub fn record_main_window_render_finished(render_time: Duration) {
    let mut metrics = RUNTIME_METRICS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let render_time_ms = render_time.as_secs_f32() * 1000.0;
    metrics.snapshot.main_render_time_ms = render_time_ms;
    RuntimeMetricsState::push_history(&mut metrics.snapshot.render_time_history_ms, render_time_ms);
    if metrics.render_time_ema <= 0.0 {
        metrics.render_time_ema = render_time_ms;
    } else {
        metrics.render_time_ema = metrics.render_time_ema * 0.9 + render_time_ms * 0.1;
    }
    metrics.snapshot.main_render_time_avg_ms = metrics.render_time_ema;
}

pub fn record_debug_window_frame(width_px: f32, height_px: f32) {
    let mut metrics = RUNTIME_METRICS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    metrics.snapshot.debug_window_width_px = width_px;
    metrics.snapshot.debug_window_height_px = height_px;
}

pub fn record_debug_gpu_specs(gpu_specs: Option<GpuSpecs>) {
    let mut sampler = RUNTIME_SAMPLER
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    sampler.gpu_specs = gpu_specs;
}

pub fn snapshot_runtime_metrics() -> DebugRuntimeSnapshot {
    let mut snapshot = RUNTIME_METRICS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .snapshot
        .clone();
    {
        let mut sampler = RUNTIME_SAMPLER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        sampler.sample(&mut snapshot);
    }
    let gpui_metrics = performance_metrics_snapshot();
    snapshot.gpui_renderer_backend =
        SharedString::from(gpui_metrics.renderer_backend.as_str().to_string());
    snapshot.gpui_present_fps = gpui_metrics.present_fps;
    snapshot.gpui_image_cache_items = gpui_metrics.image_cache_items;
    snapshot.gpui_image_cache_bytes = gpui_metrics.image_cache_bytes;
    snapshot.gpui_atlas_textures = gpui_metrics.atlas_textures;
    snapshot.gpui_backdrop_blur_primitives = 0;
    snapshot.gpui_draw_time_ms = duration_to_ms(gpui_metrics.last_draw_time);
    snapshot.gpui_prepared_command_count = gpui_metrics.prepared_command_count;
    snapshot.gpui_gpu_surface_format = SharedString::from(gpui_metrics.gpu_surface_format);
    snapshot.gpui_gpu_surface_alpha_mode = SharedString::from(gpui_metrics.gpu_surface_alpha_mode);
    snapshot.gpui_gpu_surface_present_mode =
        SharedString::from(gpui_metrics.gpu_surface_present_mode);
    snapshot.gpui_upload_bytes = gpui_metrics.upload_bytes;
    snapshot.gpui_pod_upload_bytes = gpui_metrics.pod_upload_bytes;
    snapshot.gpui_mask_pass_count = gpui_metrics.mask_pass_count;
    snapshot.gpui_main_pass_count = gpui_metrics.main_pass_count;
    snapshot.gpui_composite_pass_count = gpui_metrics.composite_pass_count;
    snapshot.gpui_gpu_surface_reconfigure_count = gpui_metrics.gpu_surface_reconfigure_count;
    snapshot.gpui_gpu_surface_error_count = gpui_metrics.gpu_surface_error_count;
    snapshot.gpui_retained_present_count = gpui_metrics.retained_present_count;
    snapshot.gpui_image_decode_compressed_bytes = gpui_metrics.last_image_decode_compressed_bytes;
    snapshot.gpui_image_decode_decoded_bytes = gpui_metrics.last_image_decode_decoded_bytes;
    snapshot.gpui_image_decode_frames = gpui_metrics.last_image_decode_frames;
    snapshot.gpui_image_decode_time_ms = duration_to_ms(gpui_metrics.last_image_decode_time);
    snapshot.gpui_image_decode_count = gpui_metrics.image_decode_count;
    snapshot.gpui_image_decode_total_compressed_bytes = gpui_metrics.image_decode_compressed_bytes;
    snapshot.gpui_image_decode_total_decoded_bytes = gpui_metrics.image_decode_decoded_bytes;
    snapshot.gpui_image_decode_total_frames = gpui_metrics.image_decode_frames;
    snapshot.gpui_image_decode_total_time_ms = duration_to_ms(gpui_metrics.image_decode_total_time);
    snapshot.gpui_image_decode_max_time_ms = duration_to_ms(gpui_metrics.image_decode_max_time);
    snapshot.gpui_image_decode_slow_count = gpui_metrics.image_decode_slow_count;
    snapshot.gpui_image_asset_retained_decoded_bytes =
        gpui_metrics.image_asset_retained_decoded_bytes;
    snapshot.gpui_image_asset_retained_count = gpui_metrics.image_asset_retained_count;
    snapshot.gpui_image_asset_largest_retained_decoded_bytes =
        gpui_metrics.image_asset_largest_retained_decoded_bytes;
    snapshot.gpui_image_asset_cache_entries = gpui_metrics.gpui_image_asset_cache_entries;
    snapshot.gpui_image_asset_retained_compressed_bytes =
        gpui_metrics.gpui_image_asset_retained_compressed_bytes;
    snapshot.gpui_image_asset_total_retained_decoded_bytes =
        gpui_metrics.gpui_image_asset_total_retained_decoded_bytes;
    snapshot.gpui_render_image_cpu_bytes = gpui_metrics.gpui_render_image_cpu_bytes;
    snapshot.gpui_render_image_gpu_texture_bytes = gpui_metrics.gpui_render_image_gpu_texture_bytes;
    snapshot.gpui_icon_cache_entries = gpui_metrics.gpui_icon_cache_entries;
    snapshot.gpui_icon_cache_decoded_bytes = gpui_metrics.gpui_icon_cache_decoded_bytes;
    snapshot.gpui_atlas_monochrome_bytes = gpui_metrics.gpui_atlas_monochrome_bytes;
    snapshot.gpui_atlas_polychrome_bytes = gpui_metrics.gpui_atlas_polychrome_bytes;
    snapshot.gpui_atlas_live_keys = gpui_metrics.gpui_atlas_live_keys;
    snapshot.gpui_atlas_unused_bytes = gpui_metrics.gpui_atlas_unused_bytes;
    snapshot.gpui_gpu_surface_texture_bytes = gpui_metrics.gpui_gpu_surface_texture_bytes;
    snapshot.gpui_gpu_estimated_total_retained_bytes =
        gpui_metrics.gpui_gpu_estimated_total_retained_bytes;
    snapshot.gpui_atlas_upload_bytes = gpui_metrics.atlas_upload_bytes;
    snapshot.gpui_atlas_upload_tiles = gpui_metrics.atlas_upload_tiles;
    snapshot.gpui_atlas_upload_time_ms = duration_to_ms(gpui_metrics.atlas_upload_time);
    snapshot.gpui_first_frame_build_time_ms = duration_to_ms(gpui_metrics.first_frame_build_time);
    snapshot.gpui_first_frame_layout_time_ms = duration_to_ms(gpui_metrics.first_frame_layout_time);
    snapshot.gpui_first_frame_prepaint_time_ms =
        duration_to_ms(gpui_metrics.first_frame_prepaint_time);
    snapshot.gpui_first_frame_paint_time_ms = duration_to_ms(gpui_metrics.first_frame_paint_time);
    snapshot.gpui_first_frame_scene_finish_time_ms =
        duration_to_ms(gpui_metrics.first_frame_scene_finish_time);
    snapshot.gpui_first_frame_backend_draw_time_ms =
        duration_to_ms(gpui_metrics.first_frame_backend_draw_time);
    snapshot.gpui_frame_build_time_ms = duration_to_ms(gpui_metrics.frame_build_time);
    snapshot.gpui_frame_layout_time_ms = duration_to_ms(gpui_metrics.frame_layout_time);
    snapshot.gpui_frame_prepaint_time_ms = duration_to_ms(gpui_metrics.frame_prepaint_time);
    snapshot.gpui_frame_paint_time_ms = duration_to_ms(gpui_metrics.frame_paint_time);
    snapshot.gpui_frame_scene_finish_time_ms = duration_to_ms(gpui_metrics.frame_scene_finish_time);
    snapshot.gpui_frame_backend_draw_time_ms = duration_to_ms(gpui_metrics.frame_backend_draw_time);
    snapshot.gpui_layout_nodes = gpui_metrics.layout_nodes;
    snapshot.gpui_measured_layout_nodes = gpui_metrics.measured_layout_nodes;
    snapshot.gpui_layout_roots = gpui_metrics.layout_roots;
    snapshot.gpui_layout_bounds_cache_hits = gpui_metrics.layout_bounds_cache_hits;
    snapshot.gpui_layout_bounds_cache_misses = gpui_metrics.layout_bounds_cache_misses;
    snapshot.gpui_layout_cache_hits = gpui_metrics.layout_cache_hits;
    snapshot.gpui_layout_cache_misses = gpui_metrics.layout_cache_misses;
    snapshot.gpui_layout_cache_reused_roots = gpui_metrics.layout_cache_reused_roots;
    snapshot.gpui_layout_cache_saved_roots = gpui_metrics.layout_cache_saved_roots;
    snapshot.gpui_text_layout_hits = gpui_metrics.text_layout_hits;
    snapshot.gpui_text_layout_reuses = gpui_metrics.text_layout_reuses;
    snapshot.gpui_text_layout_misses = gpui_metrics.text_layout_misses;
    snapshot.gpui_scene_primitives = gpui_metrics.scene_primitives;
    snapshot.gpui_scene_batches = gpui_metrics.scene_batches;
    snapshot.gpui_scene_segments = gpui_metrics.scene_segments;
    snapshot.gpui_scene_replayed_primitives = gpui_metrics.scene_replayed_primitives;
    snapshot.gpui_scene_segment_rebuild_count = gpui_metrics.scene_segment_rebuild_count;
    snapshot.gpui_scene_segment_reuse_count = gpui_metrics.scene_segment_reuse_count;
    snapshot.gpui_dirty_transform_count = gpui_metrics.dirty_transform_count;
    snapshot.gpui_retained_frame_skips = gpui_metrics.retained_frame_skips;
    snapshot.gpui_skipped_pointer_frame_count = gpui_metrics.skipped_pointer_frame_count;
    snapshot.gpui_dirty_rect_count = gpui_metrics.dirty_rect_count;
    snapshot.gpui_dirty_rect_area = gpui_metrics.dirty_rect_area;
    snapshot.gpui_partial_redraw_count = gpui_metrics.partial_redraw_count;
    snapshot.gpui_full_redraw_fallback_count = gpui_metrics.full_redraw_fallback_count;
    snapshot.gpui_gpu_retained_bytes = gpui_metrics.gpu_retained_bytes;
    snapshot.gpui_atlas_retained_bytes = gpui_metrics.atlas_retained_bytes;
    snapshot.gpui_has_retained_frame_target = gpui_metrics.has_retained_frame_target;
    snapshot.gpui_has_path_textures = gpui_metrics.has_path_textures;
    snapshot.gpui_has_backdrop_texture = gpui_metrics.has_backdrop_texture;
    snapshot.gpui_has_depth_texture = gpui_metrics.has_depth_texture;
    snapshot.gpui_backdrop_blur_target_groups = gpui_metrics.backdrop_blur_target_groups;
    snapshot.gpui_gpu_mesh_buffers = gpui_metrics.gpu_mesh_buffers;
    snapshot.gpui_coalesced_refresh_count = gpui_metrics.coalesced_refresh_count;
    snapshot.gpui_coalesced_refresh_effect_count = gpui_metrics.coalesced_refresh_effect_count;
    snapshot.gpui_inactive_present_skip_count = gpui_metrics.inactive_present_skip_count;
    snapshot.gpui_image_cache_evictions = gpui_metrics.image_cache_evictions;
    snapshot.gpui_image_drop_count = gpui_metrics.image_drop_count;
    snapshot.gpui_atlas_remove_count = gpui_metrics.atlas_remove_count;
    snapshot.gpui_scheduler_wakeups = gpui_metrics.scheduler_wakeups;
    snapshot.gpui_idle_sleep_time_ms = duration_to_ms(gpui_metrics.idle_sleep_time);
    snapshot.gpui_frame_request_count = gpui_metrics.frame_request_count;
    snapshot.gpui_draw_count = gpui_metrics.draw_count;
    snapshot.gpui_present_count = gpui_metrics.present_count;
    snapshot.gpui_skip_count = gpui_metrics.skip_count;
    snapshot.gpui_bind_group_creations = gpui_metrics.bind_group_creations;
    snapshot.gpui_bind_group_cache_hits = gpui_metrics.bind_group_cache_hits;
    snapshot.gpui_bind_group_cache_misses = gpui_metrics.bind_group_cache_misses;
    snapshot.gpui_upload_arena_uniform_capacity = gpui_metrics.upload_arena_uniform_capacity;
    snapshot.gpui_upload_arena_storage_capacity = gpui_metrics.upload_arena_storage_capacity;
    snapshot.gpui_upload_arena_uniform_used = gpui_metrics.upload_arena_uniform_used;
    snapshot.gpui_upload_arena_storage_used = gpui_metrics.upload_arena_storage_used;
    snapshot.gpui_gpu_cache_hits = gpui_metrics.gpu_cache_hits;
    snapshot.gpui_gpu_cache_misses = gpui_metrics.gpu_cache_misses;
    snapshot.gpui_scene_retained_capacity = gpui_metrics.scene_retained_capacity;
    snapshot.gpui_frame_retained_capacity = gpui_metrics.frame_retained_capacity;
    snapshot.gpui_allocator_allocated_bytes = gpui_metrics.allocator_allocated_bytes;
    snapshot.gpui_allocator_reserved_bytes = gpui_metrics.allocator_reserved_bytes;
    snapshot.gpui_allocator_block_count = gpui_metrics.allocator_block_count;
    snapshot.gpui_allocator_allocation_count = gpui_metrics.allocator_allocation_count;
    snapshot.gpui_allocator_gpu_only_allocated_bytes =
        gpui_metrics.allocator_gpu_only.allocated_bytes;
    snapshot.gpui_allocator_gpu_only_reserved_bytes =
        gpui_metrics.allocator_gpu_only.reserved_bytes;
    snapshot.gpui_allocator_gpu_only_block_count = gpui_metrics.allocator_gpu_only.block_count;
    snapshot.gpui_allocator_cpu_to_gpu_allocated_bytes =
        gpui_metrics.allocator_cpu_to_gpu.allocated_bytes;
    snapshot.gpui_allocator_cpu_to_gpu_reserved_bytes =
        gpui_metrics.allocator_cpu_to_gpu.reserved_bytes;
    snapshot.gpui_allocator_cpu_to_gpu_block_count = gpui_metrics.allocator_cpu_to_gpu.block_count;
    snapshot.gpui_allocator_gpu_to_cpu_allocated_bytes =
        gpui_metrics.allocator_gpu_to_cpu.allocated_bytes;
    snapshot.gpui_allocator_gpu_to_cpu_reserved_bytes =
        gpui_metrics.allocator_gpu_to_cpu.reserved_bytes;
    snapshot.gpui_allocator_gpu_to_cpu_block_count = gpui_metrics.allocator_gpu_to_cpu.block_count;
    snapshot.gpui_hal_buffer_memory_bytes = gpui_metrics.hal_buffer_memory_bytes;
    snapshot.gpui_hal_texture_memory_bytes = gpui_metrics.hal_texture_memory_bytes;
    snapshot.gpui_hal_acceleration_structure_memory_bytes =
        gpui_metrics.hal_acceleration_structure_memory_bytes;
    snapshot.gpui_hal_memory_allocation_count = gpui_metrics.hal_memory_allocation_count;
    snapshot.gpui_core_staging_buffer_live_bytes = gpui_metrics.core_staging_buffer_live_bytes;
    snapshot.gpui_core_staging_buffer_peak_live_bytes =
        gpui_metrics.core_staging_buffer_peak_live_bytes;
    snapshot.gpui_core_staging_buffer_created_bytes =
        gpui_metrics.core_staging_buffer_created_bytes;
    snapshot.gpui_core_staging_buffer_pending_bytes =
        gpui_metrics.core_staging_buffer_pending_bytes;
    snapshot.gpui_core_staging_buffer_peak_pending_bytes =
        gpui_metrics.core_staging_buffer_peak_pending_bytes;
    snapshot.gpui_core_staging_buffer_live_count = gpui_metrics.core_staging_buffer_live_count;
    snapshot.gpui_core_staging_buffer_peak_live_count =
        gpui_metrics.core_staging_buffer_peak_live_count;
    snapshot.gpui_core_staging_buffer_pending_count =
        gpui_metrics.core_staging_buffer_pending_count;
    snapshot.gpui_core_staging_buffer_peak_pending_count =
        gpui_metrics.core_staging_buffer_peak_pending_count;
    snapshot.gpui_window_metrics = gpui_metrics
        .window_metrics
        .into_iter()
        .map(DebugWindowMetrics::from)
        .collect();
    snapshot.bmcbl_memory = crate::utils::memory_diagnostics::snapshot_bmcbl_memory();
    snapshot
}

pub fn reset_runtime_metrics() {
    let mut metrics = RUNTIME_METRICS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *metrics = RuntimeMetricsState::default();
}

#[derive(Debug, Clone)]
pub struct DebugState {
    pub enabled: bool,
    pub main_window_id: Option<u64>,
    pub debug_window_id: Option<u64>,
    pub exe_path: SharedString,
    pub exe_size_bytes: u64,
    pub inspector: DebugInspectorSnapshot,
    pub inspector_history: VecDeque<DebugInspectorSnapshot>,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            enabled: false,
            main_window_id: None,
            debug_window_id: None,
            exe_path: SharedString::from(""),
            exe_size_bytes: 0,
            inspector: DebugInspectorSnapshot::default(),
            inspector_history: VecDeque::new(),
        }
    }
}

impl Global for DebugState {}

impl DebugState {
    pub fn reset_runtime_state(&mut self) {
        reset_runtime_metrics();
        self.inspector = DebugInspectorSnapshot::default();
        self.inspector_history.clear();
    }

    pub fn clear_inspector_history(&mut self) {
        self.inspector_history.clear();
    }

    pub fn sync_inspector(&mut self, inspector: DebugInspectorSnapshot) {
        let should_record = !inspector.selected_label.is_empty()
            && self
                .inspector_history
                .front()
                .map(|entry| {
                    entry.selected_label != inspector.selected_label
                        || entry.source_location != inspector.source_location
                })
                .unwrap_or(true);
        if should_record {
            self.inspector_history.push_front(inspector.clone());
            while self.inspector_history.len() > 24 {
                let _ = self.inspector_history.pop_back();
            }
        }
        self.inspector = inspector;
    }
}
