use std::{any::TypeId, sync::Arc};

use anyhow::Result;
use futures::{FutureExt, future::Shared};

use crate::{App, ImageCacheError, RenderImage, Task, performance_metrics_snapshot};

/// Retained image asset totals in GPUI's global asset cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalImageAssetCacheSnapshot {
    /// Decoded bytes retained by uncached resource images.
    pub resource_decoded_bytes: usize,
    /// Number of completed uncached resource image assets.
    pub resource_count: usize,
    /// Decoded bytes retained by inline image assets.
    pub inline_decoded_bytes: usize,
    /// Number of completed inline image assets.
    pub inline_count: usize,
    /// Compressed image bytes retained for target-size decodes.
    pub compressed_bytes: usize,
    /// Number of completed compressed image assets.
    pub compressed_count: usize,
    /// Decoded bytes retained by target-size image assets.
    pub target_decoded_bytes: usize,
    /// Number of completed target-size image assets.
    pub target_count: usize,
}

/// Aggregated GPUI memory diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuiMemorySnapshot {
    /// Number of entries retained by bounded GPUI image caches.
    pub gpui_image_asset_cache_entries: usize,
    /// Encoded bytes retained by compressed image assets.
    pub gpui_image_asset_retained_compressed_bytes: usize,
    /// Decoded bytes retained by global image assets and bounded image caches.
    pub gpui_image_asset_retained_decoded_bytes: usize,
    /// Decoded bytes retained by render images currently visible through GPUI cache metrics.
    pub gpui_render_image_cpu_bytes: usize,
    /// Estimated GPU texture bytes retained for render images.
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
    /// Estimated bytes retained by window surface and retained-frame resources.
    pub gpui_gpu_surface_texture_bytes: usize,
    /// Aggregate GPUI-owned retained bytes visible to diagnostics.
    pub gpui_gpu_estimated_total_retained_bytes: usize,
}

impl GpuiMemorySnapshot {
    fn from_metrics(global_assets: GlobalImageAssetCacheSnapshot) -> Self {
        let metrics = performance_metrics_snapshot();
        let global_decoded_bytes = global_assets
            .resource_decoded_bytes
            .saturating_add(global_assets.inline_decoded_bytes)
            .saturating_add(global_assets.target_decoded_bytes);
        let global_entries = global_assets
            .resource_count
            .saturating_add(global_assets.inline_count)
            .saturating_add(global_assets.compressed_count)
            .saturating_add(global_assets.target_count);
        let retained_decoded_bytes = metrics
            .image_cache_bytes
            .saturating_add(global_decoded_bytes)
            .max(metrics.gpui_image_asset_total_retained_decoded_bytes);

        Self {
            gpui_image_asset_cache_entries: metrics
                .gpui_image_asset_cache_entries
                .saturating_add(global_entries),
            gpui_image_asset_retained_compressed_bytes: metrics
                .gpui_image_asset_retained_compressed_bytes
                .saturating_add(global_assets.compressed_bytes),
            gpui_image_asset_retained_decoded_bytes: retained_decoded_bytes,
            gpui_render_image_cpu_bytes: metrics
                .gpui_render_image_cpu_bytes
                .max(retained_decoded_bytes),
            gpui_render_image_gpu_texture_bytes: metrics.gpui_render_image_gpu_texture_bytes,
            gpui_icon_cache_entries: metrics.gpui_icon_cache_entries,
            gpui_icon_cache_decoded_bytes: metrics.gpui_icon_cache_decoded_bytes,
            gpui_atlas_monochrome_bytes: metrics.gpui_atlas_monochrome_bytes,
            gpui_atlas_polychrome_bytes: metrics.gpui_atlas_polychrome_bytes,
            gpui_atlas_live_keys: metrics.gpui_atlas_live_keys,
            gpui_atlas_unused_bytes: metrics.gpui_atlas_unused_bytes,
            gpui_gpu_surface_texture_bytes: metrics.gpui_gpu_surface_texture_bytes,
            gpui_gpu_estimated_total_retained_bytes: metrics
                .gpui_gpu_estimated_total_retained_bytes
                .max(retained_decoded_bytes.saturating_add(metrics.gpu_retained_bytes)),
        }
    }
}

impl App {
    /// Returns retained image asset totals from GPUI's global asset cache.
    pub fn global_image_asset_cache_snapshot(&self) -> GlobalImageAssetCacheSnapshot {
        let mut snapshot = GlobalImageAssetCacheSnapshot::default();
        let resource_type = TypeId::of::<crate::ImgResourceLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<crate::ImageDecoder>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<crate::EncodedImageDecoder>>();
        let compressed_type = TypeId::of::<crate::CompressedImgResourceLoader>();
        let target_type = TypeId::of::<crate::TargetSizeImgResourceLoader>();

        for ((type_id, _), task) in &self.loading_assets {
            if *type_id == resource_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.resource_count = snapshot.resource_count.saturating_add(1);
                    snapshot.resource_decoded_bytes = snapshot
                        .resource_decoded_bytes
                        .saturating_add(image.decoded_byte_len());
                }
            } else if *type_id == inline_type || *type_id == inline_bytes_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.inline_count = snapshot.inline_count.saturating_add(1);
                    snapshot.inline_decoded_bytes = snapshot
                        .inline_decoded_bytes
                        .saturating_add(image.decoded_byte_len());
                }
            } else if *type_id == compressed_type {
                if let Some(task) = task.downcast_ref::<
                    Shared<Task<Result<crate::CompressedImageBytes, ImageCacheError>>>,
                >() && let Some(Ok(bytes)) = task.clone().now_or_never()
                {
                    snapshot.compressed_count = snapshot.compressed_count.saturating_add(1);
                    snapshot.compressed_bytes =
                        snapshot.compressed_bytes.saturating_add(bytes.len());
                }
            } else if *type_id == target_type
                && let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                && let Some(Ok(image)) = task.clone().now_or_never()
            {
                snapshot.target_count = snapshot.target_count.saturating_add(1);
                snapshot.target_decoded_bytes = snapshot
                    .target_decoded_bytes
                    .saturating_add(image.decoded_byte_len());
            }
        }

        snapshot
    }

    /// Returns a unified memory snapshot for GPUI-owned image and renderer resources.
    pub fn gpui_memory_snapshot(&self) -> GpuiMemorySnapshot {
        GpuiMemorySnapshot::from_metrics(self.global_image_asset_cache_snapshot())
    }
}
