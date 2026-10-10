use std::{any::TypeId, sync::Arc};

use crate::{
    AnimationQueueSnapshot, App, BitmapPoolSnapshot, ImageCacheError, RenderImage,
    compressed_cache_snapshot, performance_metrics_snapshot,
};
use anyhow::Result;

use super::asset_loading::cached_asset_output;

/// Retained image asset totals in GPUI's global asset cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalImageAssetCacheSnapshot {
    /// App-wide soft retention budget; active images and explicit leases may exceed it.
    pub cache_budget_bytes: usize,
    /// Unique decoded cache cost, including compressed sources retained by animated streams.
    pub cache_cost_bytes: usize,
    /// Cache cost above the soft budget, including protected active allocations.
    pub over_budget_bytes: usize,
    /// Decoded backing bytes counted once per RenderImage identity across
    /// all decoded asset categories. Category byte counters can overlap.
    pub unique_resident_bytes: usize,
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GpuiMemorySnapshot {
    /// App-wide decoded image cache soft budget.
    #[serde(default)]
    pub image_cache_budget_bytes: usize,
    /// Unique cache cost including active stream source data; not additive with CPU image totals.
    #[serde(default)]
    pub image_cache_cost_bytes: usize,
    /// Protected cache cost above the soft budget; not an allocation failure.
    #[serde(default)]
    pub image_cache_over_budget_bytes: usize,
    /// Cached per-window and shared-device renderer observations, with sample ages.
    /// Image CPU counters below are overlapping cache views, not additive heap totals.
    pub renderer: crate::MemoryProfileSnapshot,
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
    /// Atlas page bytes minus padded tile rectangles; not allocator free space.
    pub atlas_unused_bytes: usize,
    /// Logical depth/path/blur target bytes; excludes swapchain images and buffers.
    pub gpu_surface_texture_bytes: usize,
    /// Shared-device known GPU bytes, excluding CPU caches and driver overhead.
    pub gpu_estimated_total_retained_bytes: usize,
}

impl GpuiMemorySnapshot {
    fn from_metrics(
        global_assets: GlobalImageAssetCacheSnapshot,
        metrics: crate::PerformanceMetricsSnapshot,
    ) -> Self {
        let BitmapPoolSnapshot {
            retained_bytes: bitmap_pool_retained_bytes,
            free_buffers: bitmap_pool_buffers,
            ..
        } = crate::assets::global_bitmap_pool().snapshot();
        let AnimationQueueSnapshot {
            queued_bytes: animation_prefetch_bytes,
        } = crate::assets::animation_queue_snapshot();
        let (compressed_cache_entries, compressed_cache_bytes) = compressed_cache_snapshot();
        let global_decoded_bytes = global_assets.unique_resident_bytes;
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
            image_cache_budget_bytes: global_assets.cache_budget_bytes,
            image_cache_cost_bytes: global_assets.cache_cost_bytes,
            image_cache_over_budget_bytes: global_assets.over_budget_bytes,
            renderer: crate::memory_profile_snapshot(),
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
            gpu_estimated_total_retained_bytes: metrics.gpu_estimated_total_retained_bytes,
        }
    }
}

impl App {
    /// Returns retained image asset totals from GPUI's global asset cache.
    pub fn global_image_asset_cache_snapshot(&self) -> GlobalImageAssetCacheSnapshot {
        let mut snapshot = GlobalImageAssetCacheSnapshot::default();
        snapshot.cache_budget_bytes = self.image_pipeline_config.idle_image_bytes;
        snapshot.cache_cost_bytes = self.image_cache_cost_bytes();
        snapshot.over_budget_bytes = snapshot
            .cache_cost_bytes
            .saturating_sub(snapshot.cache_budget_bytes);
        let mut decoded_images = collections::FxHashSet::default();
        let resource_type = TypeId::of::<crate::ResourceImageLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<crate::ClipboardImageLoader>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<crate::EncodedImageLoader>>();
        let compressed_type = TypeId::of::<crate::CompressedImageLoader>();
        let target_type = TypeId::of::<crate::SizedImageLoader>();

        for ((type_id, _), entry) in &self.asset_entries {
            if *type_id == resource_type {
                if let Some(Ok(image)) =
                    cached_asset_output::<Result<Arc<RenderImage>, ImageCacheError>>(entry.as_ref())
                {
                    snapshot.resource_count = snapshot.resource_count.saturating_add(1);
                    if decoded_images.insert(Arc::as_ptr(&image) as usize) {
                        snapshot.unique_resident_bytes = snapshot
                            .unique_resident_bytes
                            .saturating_add(image.resident_capacity());
                    }
                    snapshot.resource_resident_bytes = snapshot
                        .resource_resident_bytes
                        .saturating_add(image.resident_capacity());
                }
            } else if *type_id == inline_type || *type_id == inline_bytes_type {
                if let Some(Ok(image)) =
                    cached_asset_output::<Result<Arc<RenderImage>, ImageCacheError>>(entry.as_ref())
                {
                    snapshot.inline_count = snapshot.inline_count.saturating_add(1);
                    if decoded_images.insert(Arc::as_ptr(&image) as usize) {
                        snapshot.unique_resident_bytes = snapshot
                            .unique_resident_bytes
                            .saturating_add(image.resident_capacity());
                    }
                    snapshot.inline_resident_bytes = snapshot
                        .inline_resident_bytes
                        .saturating_add(image.resident_capacity());
                }
            } else if *type_id == compressed_type {
                if let Some(Ok(bytes)) = cached_asset_output::<
                    Result<crate::CompressedImageBytes, ImageCacheError>,
                >(entry.as_ref())
                {
                    snapshot.compressed_count = snapshot.compressed_count.saturating_add(1);
                    snapshot.compressed_bytes = snapshot
                        .compressed_bytes
                        .saturating_add(bytes.retained_capacity());
                }
            } else if *type_id == target_type
                && let Some(Ok(image)) =
                    cached_asset_output::<Result<Arc<RenderImage>, ImageCacheError>>(entry.as_ref())
            {
                snapshot.sized_count = snapshot.sized_count.saturating_add(1);
                if decoded_images.insert(Arc::as_ptr(&image) as usize) {
                    snapshot.unique_resident_bytes = snapshot
                        .unique_resident_bytes
                        .saturating_add(image.resident_capacity());
                }
                snapshot.sized_resident_bytes = snapshot
                    .sized_resident_bytes
                    .saturating_add(image.resident_capacity());
            }
        }

        snapshot
    }

    /// Returns a unified memory snapshot for GPUI-owned image and renderer resources.
    pub fn gpui_memory_snapshot(&self) -> GpuiMemorySnapshot {
        GpuiMemorySnapshot::from_metrics(
            self.global_image_asset_cache_snapshot(),
            performance_metrics_snapshot(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_image_capacity_does_not_inflate_gpu_memory_profile() {
        let snapshot = GpuiMemorySnapshot::from_metrics(
            GlobalImageAssetCacheSnapshot {
                unique_resident_bytes: 4_000_000,
                ..Default::default()
            },
            crate::PerformanceMetricsSnapshot {
                image_cache_bytes: 5_000_000,
                gpu_estimated_total_retained_bytes: 1024,
                gpu_retained_bytes: 1024,
                ..Default::default()
            },
        );
        assert_eq!(snapshot.gpu_estimated_total_retained_bytes, 1024);
        assert!(snapshot.image_asset_resident_bytes >= 4_000_000);
    }
}
