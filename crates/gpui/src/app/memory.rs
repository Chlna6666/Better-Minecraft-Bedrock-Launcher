use std::{any::TypeId, sync::Arc};

use anyhow::Result;
use futures::{FutureExt, future::Shared};

use crate::{
    AnimationQueueSnapshot, App, BitmapPoolSnapshot, ImageCacheError, RenderImage, Task,
    compressed_cache_snapshot, performance_metrics_snapshot,
};

/// Retained image asset totals in GPUI's global asset cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalImageAssetCacheSnapshot {
    /// Resident image bytes retained by uncached resource images.
    pub resource_resident_bytes: usize,
    /// Number of completed uncached resource image assets.
    pub resource_count: usize,
    /// Resident image bytes retained by inline image assets.
    pub inline_resident_bytes: usize,
    /// Number of completed inline image assets.
    pub inline_count: usize,
    /// Compressed image bytes retained for target-size decodes.
    pub compressed_bytes: usize,
    /// Number of completed compressed image assets.
    pub compressed_count: usize,
    /// Resident image bytes retained by target-size image assets.
    pub sized_resident_bytes: usize,
    /// Number of completed target-size image assets.
    pub sized_count: usize,
}

/// Aggregated GPUI memory diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuiMemorySnapshot {
    /// Number of entries retained by GPUI image caches.
    pub image_asset_cache_entries: usize,
    /// Encoded bytes retained by compressed image assets.
    pub image_asset_compressed_bytes: usize,
    /// Number of live entries visible through the shared compressed byte cache.
    pub compressed_cache_entries: usize,
    /// Resident image bytes retained by global image assets and image caches.
    pub image_asset_resident_bytes: usize,
    /// Bytes retained by reusable bitmap backing buffers.
    pub bitmap_pool_bytes: usize,
    /// Number of reusable bitmap buffers retained by the pool.
    pub bitmap_pool_buffers: usize,
    /// Decoded animation bytes currently queued ahead of playback.
    pub animation_prefetch_bytes: usize,
    /// Resident image bytes retained by render images currently visible through GPUI cache metrics.
    pub render_image_cpu_bytes: usize,
    /// Estimated GPU texture bytes retained for render images.
    pub render_image_gpu_texture_bytes: usize,
    /// Number of entries retained by framework icon caches.
    pub icon_cache_entries: usize,
    /// Estimated decoded bytes retained by framework icon caches.
    pub icon_cache_resident_bytes: usize,
    /// Estimated bytes retained by monochrome atlas textures.
    pub atlas_monochrome_bytes: usize,
    /// Estimated bytes retained by color atlas textures.
    pub atlas_polychrome_bytes: usize,
    /// Number of live atlas keys known to renderer metrics.
    pub atlas_live_keys: usize,
    /// Estimated unused bytes inside retained atlas textures.
    pub atlas_unused_bytes: usize,
    /// Estimated bytes retained by window surface and retained-frame resources.
    pub gpu_surface_texture_bytes: usize,
    /// Aggregate GPUI-owned retained bytes visible to diagnostics.
    pub gpu_estimated_total_retained_bytes: usize,
}

impl GpuiMemorySnapshot {
    fn from_metrics(global_assets: GlobalImageAssetCacheSnapshot) -> Self {
        let metrics = performance_metrics_snapshot();
        let BitmapPoolSnapshot {
            retained_bytes: bitmap_pool_retained_bytes,
            free_buffers: bitmap_pool_buffers,
            ..
        } = crate::assets::global_bitmap_pool().snapshot();
        let AnimationQueueSnapshot {
            queued_bytes: animation_prefetch_bytes,
        } = crate::assets::animation_queue_snapshot();
        let (compressed_cache_entries, compressed_cache_bytes) = compressed_cache_snapshot();
        let global_decoded_bytes = global_assets
            .resource_resident_bytes
            .saturating_add(global_assets.inline_resident_bytes)
            .saturating_add(global_assets.sized_resident_bytes);
        let global_entries = global_assets
            .resource_count
            .saturating_add(global_assets.inline_count)
            .saturating_add(global_assets.compressed_count)
            .saturating_add(global_assets.sized_count);
        let resident_bytes = metrics
            .image_cache_bytes
            .saturating_add(global_decoded_bytes)
            .max(metrics.image_asset_total_resident_bytes);

        Self {
            image_asset_cache_entries: metrics
                .image_asset_cache_entries
                .saturating_add(global_entries),
            image_asset_compressed_bytes: metrics
                .image_asset_compressed_bytes
                .saturating_add(global_assets.compressed_bytes.max(compressed_cache_bytes)),
            compressed_cache_entries,
            image_asset_resident_bytes: resident_bytes,
            bitmap_pool_bytes: bitmap_pool_retained_bytes,
            bitmap_pool_buffers,
            animation_prefetch_bytes,
            render_image_cpu_bytes: metrics.render_image_cpu_bytes.max(resident_bytes),
            render_image_gpu_texture_bytes: metrics.render_image_gpu_texture_bytes,
            icon_cache_entries: metrics.icon_cache_entries,
            icon_cache_resident_bytes: metrics.icon_cache_resident_bytes,
            atlas_monochrome_bytes: metrics.atlas_monochrome_bytes,
            atlas_polychrome_bytes: metrics.atlas_polychrome_bytes,
            atlas_live_keys: metrics.atlas_live_keys,
            atlas_unused_bytes: metrics.atlas_unused_bytes,
            gpu_surface_texture_bytes: metrics.gpu_surface_texture_bytes,
            gpu_estimated_total_retained_bytes: metrics.gpu_estimated_total_retained_bytes.max(
                resident_bytes
                    .saturating_add(bitmap_pool_retained_bytes)
                    .saturating_add(compressed_cache_bytes)
                    .saturating_add(metrics.gpu_retained_bytes),
            ),
        }
    }
}

impl App {
    /// Returns retained image asset totals from GPUI's global asset cache.
    pub fn global_image_asset_cache_snapshot(&self) -> GlobalImageAssetCacheSnapshot {
        let mut snapshot = GlobalImageAssetCacheSnapshot::default();
        let resource_type = TypeId::of::<crate::ResourceImageLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<crate::ClipboardImageLoader>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<crate::EncodedImageLoader>>();
        let compressed_type = TypeId::of::<crate::CompressedImageLoader>();
        let target_type = TypeId::of::<crate::SizedImageLoader>();

        for ((type_id, _), task) in &self.loading_assets {
            if *type_id == resource_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.resource_count = snapshot.resource_count.saturating_add(1);
                    snapshot.resource_resident_bytes = snapshot
                        .resource_resident_bytes
                        .saturating_add(image.resident_byte_len());
                }
            } else if *type_id == inline_type || *type_id == inline_bytes_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.inline_count = snapshot.inline_count.saturating_add(1);
                    snapshot.inline_resident_bytes = snapshot
                        .inline_resident_bytes
                        .saturating_add(image.resident_byte_len());
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
                snapshot.sized_count = snapshot.sized_count.saturating_add(1);
                snapshot.sized_resident_bytes = snapshot
                    .sized_resident_bytes
                    .saturating_add(image.resident_byte_len());
            }
        }

        snapshot
    }

    /// Returns a unified memory snapshot for GPUI-owned image and renderer resources.
    pub fn gpui_memory_snapshot(&self) -> GpuiMemorySnapshot {
        GpuiMemorySnapshot::from_metrics(self.global_image_asset_cache_snapshot())
    }
}
