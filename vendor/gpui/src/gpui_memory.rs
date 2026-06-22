use crate::{
    AppCell, ForegroundExecutor, GlobalImageAssetCacheSnapshot, ImagePipelineConfig,
    PerformanceMetricsSnapshot, SharedString, performance_metrics_snapshot,
};
use serde::{Deserialize, Serialize};
use std::{
    rc::{Rc, Weak},
    time::Duration,
};

const GPUI_IMAGE_USAGE_SCOPE_RELEASE_RETRIES: u8 = 8;

/// How aggressively GPUI should release framework-owned memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuiMemoryTrimLevel {
    /// Release short-lived idle buffers and cache entries that are already outside policy.
    #[default]
    Light,
    /// Release decoded image assets and idle renderer resources that can be rebuilt cheaply.
    Moderate,
    /// Release all non-essential decoded image assets and renderer scratch resources.
    Aggressive,
}

/// Classification for GPUI-owned image resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageUsageKind {
    /// Framework or application icon imagery.
    Icon,
    /// General user-interface image resources.
    #[default]
    UiImage,
    /// Short-lived preview imagery.
    PreviewImage,
    /// Background imagery.
    Background,
    /// Images produced by plugins.
    PluginImage,
    /// Images currently represented by platform atlas tiles.
    AtlasImage,
}

/// Application-defined scope for GPUI image resources.
pub type GpuiImageUsageScope = SharedString;

/// Runtime retention metadata for a bounds-decoded target-size image asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GpuiImageTargetAssetUsage {
    pub(crate) decoded_bytes: usize,
    pub(crate) usage: ImageUsageKind,
    pub(crate) scope: Option<GpuiImageUsageScope>,
    pub(crate) last_used: u64,
}

/// A framework-owned image resource scope.
///
/// Clone this handle for every owner that can still render images tagged with the same scope.
/// When the final handle is dropped, GPUI schedules a scope release on the foreground executor
/// and applies the configured trim level without exposing image or icon cache internals to the
/// application.
#[derive(Clone)]
pub struct GpuiImageUsageScopeHandle {
    inner: Rc<GpuiImageUsageScopeState>,
}

impl GpuiImageUsageScopeHandle {
    pub(crate) fn new(
        app: Weak<AppCell>,
        foreground_executor: ForegroundExecutor,
        scope: GpuiImageUsageScope,
        release_level: GpuiMemoryTrimLevel,
    ) -> Self {
        Self {
            inner: Rc::new(GpuiImageUsageScopeState {
                app,
                foreground_executor,
                scope,
                release_level,
            }),
        }
    }

    /// Returns the string key used by `img(...).usage_scope(...)`.
    pub fn scope(&self) -> &GpuiImageUsageScope {
        &self.inner.scope
    }
}

struct GpuiImageUsageScopeState {
    app: Weak<AppCell>,
    foreground_executor: ForegroundExecutor,
    scope: GpuiImageUsageScope,
    release_level: GpuiMemoryTrimLevel,
}

impl Drop for GpuiImageUsageScopeState {
    fn drop(&mut self) {
        schedule_image_usage_scope_release(
            self.app.clone(),
            self.foreground_executor.clone(),
            self.scope.clone(),
            self.release_level,
            GPUI_IMAGE_USAGE_SCOPE_RELEASE_RETRIES,
        );
    }
}

fn schedule_image_usage_scope_release(
    app: Weak<AppCell>,
    foreground_executor: ForegroundExecutor,
    scope: GpuiImageUsageScope,
    release_level: GpuiMemoryTrimLevel,
    retries: u8,
) {
    let retry_executor = foreground_executor.clone();
    foreground_executor
        .spawn(async move {
            let Some(app_cell) = app.upgrade() else {
                return;
            };
            if let Ok(mut app) = app_cell.try_borrow_mut() {
                app.end_gpui_image_usage_scope(&scope, release_level);
                return;
            }
            if retries == 0 {
                log::debug!("failed to release GPUI image usage scope {scope:?}: app is borrowed");
                return;
            }
            smol::Timer::after(Duration::from_millis(1)).await;
            schedule_image_usage_scope_release(
                Weak::clone(&app),
                retry_executor,
                scope,
                release_level,
                retries - 1,
            );
        })
        .detach();
}

/// Framework-wide memory policy for image and renderer resource retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuiMemoryPolicy {
    /// Initial uniform upload arena capacity.
    pub upload_initial_uniform_capacity: usize,
    /// Initial storage upload arena capacity.
    pub upload_initial_storage_capacity: usize,
    /// Maximum uniform upload arena capacity before aggressive trim should release it.
    pub upload_max_uniform_capacity: usize,
    /// Maximum storage upload arena capacity before aggressive trim should release it.
    pub upload_max_storage_capacity: usize,
    /// Idle frame count before retained upload buffers can be released.
    pub idle_shrink_frames: u8,
    /// Divisor used to decide whether upload arena use is low enough to shrink.
    pub low_usage_divisor: usize,
    /// Maximum atlas bytes GPUI should retain before a strong trim is recommended.
    pub atlas_max_bytes: usize,
    /// Maximum decoded bytes retained by the default image cache.
    pub image_cache_max_bytes: usize,
    /// Maximum decoded bytes retained by preview image scopes.
    pub preview_cache_max_bytes: usize,
    /// Whether hidden windows should request renderer memory trim.
    pub trim_on_window_hidden: bool,
    /// Whether application memory pressure should request renderer memory trim.
    pub trim_on_memory_pressure: bool,
}

impl Default for GpuiMemoryPolicy {
    fn default() -> Self {
        Self {
            upload_initial_uniform_capacity: 0,
            upload_initial_storage_capacity: 0,
            upload_max_uniform_capacity: 8 * 1024 * 1024,
            upload_max_storage_capacity: 32 * 1024 * 1024,
            idle_shrink_frames: 120,
            low_usage_divisor: 4,
            atlas_max_bytes: 128 * 1024 * 1024,
            image_cache_max_bytes: ImagePipelineConfig::default().max_decoded_bytes,
            preview_cache_max_bytes: 64 * 1024 * 1024,
            trim_on_window_hidden: true,
            trim_on_memory_pressure: true,
        }
    }
}

/// Aggregated GPUI memory diagnostics, combining image cache, asset, atlas, and GPU counters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuiMemorySnapshot {
    /// Number of entries retained by bounded GPUI image caches.
    pub gpui_image_asset_cache_entries: usize,
    /// Encoded bytes retained by compressed image assets for target-size decodes.
    pub gpui_image_asset_retained_compressed_bytes: usize,
    /// Decoded bytes retained by global image assets and bounded image caches.
    pub gpui_image_asset_retained_decoded_bytes: usize,
    /// Decoded bytes retained by render images currently visible through GPUI cache metrics.
    pub gpui_render_image_cpu_bytes: usize,
    /// Estimated GPU texture bytes retained for render images through atlas/HAL accounting.
    pub gpui_render_image_gpu_texture_bytes: usize,
    /// Number of entries retained by framework icon caches.
    pub gpui_icon_cache_entries: usize,
    /// Estimated decoded bytes retained by framework icon caches.
    pub gpui_icon_cache_decoded_bytes: usize,
    /// Estimated bytes retained by monochrome atlas textures.
    pub gpui_atlas_monochrome_bytes: usize,
    /// Estimated bytes retained by color atlas textures.
    pub gpui_atlas_polychrome_bytes: usize,
    /// Number of live atlas keys known to renderer metrics.
    pub gpui_atlas_live_keys: usize,
    /// Estimated unused bytes inside retained atlas textures.
    pub gpui_atlas_unused_bytes: usize,
    /// Current uniform upload arena capacity.
    pub gpui_gpu_upload_uniform_capacity: usize,
    /// Current storage upload arena capacity.
    pub gpui_gpu_upload_storage_capacity: usize,
    /// Estimated bytes retained by window surface and retained-frame resources.
    pub gpui_gpu_surface_texture_bytes: usize,
    /// Aggregate GPUI-owned retained bytes visible to diagnostics.
    pub gpui_gpu_estimated_total_retained_bytes: usize,
}

impl GpuiMemorySnapshot {
    pub(crate) fn from_metrics(
        metrics: &PerformanceMetricsSnapshot,
        global_assets: GlobalImageAssetCacheSnapshot,
    ) -> Self {
        let target_decoded_bytes = global_assets
            .target_decoded_bytes
            .max(metrics.image_asset_retained_decoded_bytes);
        let global_decoded_bytes = global_assets
            .resource_decoded_bytes
            .saturating_add(global_assets.inline_decoded_bytes)
            .saturating_add(target_decoded_bytes);
        let target_count = global_assets
            .target_count
            .max(metrics.image_asset_retained_count);
        let bounded_image_asset_entries = metrics
            .gpui_image_asset_cache_entries
            .saturating_sub(metrics.image_asset_retained_count);
        let global_asset_entries = global_assets
            .resource_count
            .saturating_add(global_assets.inline_count)
            .saturating_add(global_assets.compressed_count)
            .saturating_add(target_count);
        let image_asset_retained_decoded_bytes = metrics
            .image_cache_bytes
            .saturating_add(global_decoded_bytes);
        let gpu_surface_texture_bytes = metrics
            .gpu_retained_bytes
            .saturating_sub(metrics.atlas_retained_bytes)
            .saturating_sub(metrics.upload_arena_uniform_capacity)
            .saturating_sub(metrics.upload_arena_storage_capacity);
        let render_image_cpu_bytes = metrics
            .gpui_render_image_cpu_bytes
            .max(image_asset_retained_decoded_bytes);
        let estimated_total = render_image_cpu_bytes
            .saturating_add(global_assets.compressed_bytes)
            .saturating_add(metrics.atlas_retained_bytes)
            .saturating_add(gpu_surface_texture_bytes)
            .saturating_add(metrics.upload_arena_uniform_capacity)
            .saturating_add(metrics.upload_arena_storage_capacity);

        Self {
            gpui_image_asset_cache_entries: bounded_image_asset_entries
                .saturating_add(global_asset_entries),
            gpui_image_asset_retained_compressed_bytes: global_assets.compressed_bytes,
            gpui_image_asset_retained_decoded_bytes: image_asset_retained_decoded_bytes,
            gpui_render_image_cpu_bytes: render_image_cpu_bytes,
            gpui_render_image_gpu_texture_bytes: metrics.gpui_render_image_gpu_texture_bytes,
            gpui_icon_cache_entries: metrics.gpui_icon_cache_entries,
            gpui_icon_cache_decoded_bytes: metrics.gpui_icon_cache_decoded_bytes,
            gpui_atlas_monochrome_bytes: metrics.gpui_atlas_monochrome_bytes,
            gpui_atlas_polychrome_bytes: metrics.gpui_atlas_polychrome_bytes,
            gpui_atlas_live_keys: metrics.gpui_atlas_live_keys,
            gpui_atlas_unused_bytes: metrics.gpui_atlas_unused_bytes,
            gpui_gpu_upload_uniform_capacity: metrics.upload_arena_uniform_capacity,
            gpui_gpu_upload_storage_capacity: metrics.upload_arena_storage_capacity,
            gpui_gpu_surface_texture_bytes: gpu_surface_texture_bytes,
            gpui_gpu_estimated_total_retained_bytes: estimated_total,
        }
    }
}

/// Returns a process-wide GPUI memory snapshot that only includes globally reported metrics.
pub fn gpui_memory_snapshot() -> GpuiMemorySnapshot {
    GpuiMemorySnapshot::from_metrics(
        &performance_metrics_snapshot(),
        GlobalImageAssetCacheSnapshot::default(),
    )
}
