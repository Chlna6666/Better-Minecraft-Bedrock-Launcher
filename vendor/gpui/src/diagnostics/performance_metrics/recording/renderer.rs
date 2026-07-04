use crate::RendererBackend;
use std::sync::atomic::Ordering;

use super::super::state::shared_metrics;

/// Records the selected renderer backend for diagnostics.
pub fn record_renderer_backend(backend: RendererBackend) {
    if let Ok(mut current_backend) = shared_metrics().renderer_backend.lock() {
        *current_backend = backend;
    }
}

/// Records the resolved GPU adapter diagnostics for the active renderer.
pub fn record_gpu_adapter_diagnostics(name: &str, adapter_type: &str) {
    if let Ok(mut current_name) = shared_metrics().gpu_adapter_name.lock() {
        current_name.clear();
        current_name.push_str(name);
    }
    if let Ok(mut current_type) = shared_metrics().gpu_adapter_type.lock() {
        current_type.clear();
        current_type.push_str(adapter_type);
    }
}

/// Records the number of live atlas textures known to the active renderer.
pub fn record_atlas_texture_count(count: usize) {
    shared_metrics()
        .atlas_textures
        .store(count as u64, Ordering::Relaxed);
}

/// Records the number of prepared draw items submitted for the latest frame.
pub fn record_prepared_command_count(count: usize) {
    shared_metrics()
        .prepared_command_count
        .store(count as u64, Ordering::Relaxed);
}

/// Legacy compatibility shim retained during the renderer migration.
pub fn record_backdrop_blur_primitive_count(_count: usize) {}

/// Records estimated bytes retained by GPU resources.
pub fn record_gpu_retained_bytes(bytes: usize) {
    shared_metrics()
        .gpu_retained_bytes
        .store(bytes as u64, Ordering::Relaxed);
}

/// Records a startup-facing breakdown of retained GPUI-owned GPU resources.
pub fn record_gpu_resource_breakdown(
    atlas_retained_bytes: usize,
    has_retained_frame_target: bool,
    has_path_textures: bool,
    has_backdrop_texture: bool,
    has_depth_texture: bool,
    backdrop_blur_target_groups: usize,
    gpu_mesh_buffers: usize,
) {
    let metrics = shared_metrics();
    metrics
        .atlas_retained_bytes
        .store(atlas_retained_bytes as u64, Ordering::Relaxed);
    metrics
        .has_retained_frame_target
        .store(u64::from(has_retained_frame_target), Ordering::Relaxed);
    metrics
        .has_path_textures
        .store(u64::from(has_path_textures), Ordering::Relaxed);
    metrics
        .has_backdrop_texture
        .store(u64::from(has_backdrop_texture), Ordering::Relaxed);
    metrics
        .has_depth_texture
        .store(u64::from(has_depth_texture), Ordering::Relaxed);
    metrics
        .backdrop_blur_target_groups
        .store(backdrop_blur_target_groups as u64, Ordering::Relaxed);
    metrics
        .gpu_mesh_buffers
        .store(gpu_mesh_buffers as u64, Ordering::Relaxed);
}

/// Records GPU pass diagnostics for the latest frame.
pub fn record_gpu_pass_metrics(
    mask_pass_count: usize,
    main_pass_count: usize,
    composite_pass_count: usize,
) {
    let metrics = shared_metrics();
    metrics
        .mask_pass_count
        .store(mask_pass_count as u64, Ordering::Relaxed);
    metrics
        .main_pass_count
        .store(main_pass_count as u64, Ordering::Relaxed);
    metrics
        .composite_pass_count
        .store(composite_pass_count as u64, Ordering::Relaxed);
}

/// Records gpu surface diagnostics.
pub fn record_gpu_surface_metrics(
    surface_format: &str,
    surface_alpha_mode: &str,
    surface_present_mode: &str,
    surface_reconfigure_count: usize,
    surface_error_count: usize,
) {
    let metrics = shared_metrics();
    if let Ok(mut format) = metrics.gpu_surface_format.lock() {
        *format = surface_format.to_string();
    }
    if let Ok(mut alpha_mode) = metrics.gpu_surface_alpha_mode.lock() {
        *alpha_mode = surface_alpha_mode.to_string();
    }
    if let Ok(mut present_mode) = metrics.gpu_surface_present_mode.lock() {
        *present_mode = surface_present_mode.to_string();
    }
    metrics
        .gpu_surface_reconfigure_count
        .store(surface_reconfigure_count as u64, Ordering::Relaxed);
    metrics
        .gpu_surface_error_count
        .store(surface_error_count as u64, Ordering::Relaxed);
}

/// Records the number of retained presents that reused the retained target.
pub fn record_retained_present_count(count: usize) {
    shared_metrics()
        .retained_present_count
        .store(count as u64, Ordering::Relaxed);
}
