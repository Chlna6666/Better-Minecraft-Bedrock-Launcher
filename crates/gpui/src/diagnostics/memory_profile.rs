//! Cached GPU-owner observations. Reading a snapshot issues no graphics commands.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Used payload and retained capacity in bytes, excluding allocator overhead.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryUsage {
    /// Valid payload bytes.
    pub used_bytes: u64,
    /// Retained backing capacity.
    pub capacity_bytes: u64,
}

/// One packed CPU stream and its GPU capacity across all frame slots.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BufferMemoryProfile {
    /// Stream name, such as quads or animation values.
    pub name: String,
    /// CPU writable packed payload and Vec capacity. Shared retained Quad payloads are reported
    /// once in `RendererMemoryProfile::cpu_retained_chunks`, rather than again in this stream.
    pub cpu: MemoryUsage,
    /// Requested bytes of unique resident GPU Buffers across all slots; excludes native padding.
    /// A static version shared by two slots is counted once; in-flight old versions still count.
    pub gpu_capacity_bytes: u64,
    /// Bound capacity in each frame slot, in slot order. Shared versions appear in both entries,
    /// so their sum can exceed `gpu_capacity_bytes`.
    pub gpu_slot_capacity_bytes: Vec<u64>,
}

/// Per-window resources owned by a Nova renderer; excludes swapchain images,
/// driver overhead, extension resources and CPU scene/map metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RendererMemoryProfile {
    /// Diagnostic identity, stable until this renderer is destroyed.
    pub renderer_id: u64,
    /// Shared device identity; device totals are counted once per identity.
    pub device_id: usize,
    /// Age of this owner observation. Idle windows are not woken to refresh it.
    pub sample_age_ms: u64,
    /// Number of allocated frame-resource slots.
    pub frame_slots: usize,
    /// Successfully submitted frames when sampled. Zero does not establish
    /// rendered-workload usage; inspect the render error and sample age.
    pub submitted_frames: u64,
    /// Render failure observed at sampling time. Its allocations still count,
    /// but they cannot establish successful GPU execution of the workload.
    pub render_error: Option<String>,
    /// Packed streams and separately allocated GPU buffers.
    pub buffers: Vec<BufferMemoryProfile>,
    /// Shared packed Quad payloads held by the cache or current frame, deduplicated by backing.
    /// Cache trimming does not remove bytes still referenced by the current presentation.
    pub cpu_retained_chunks: MemoryUsage,
    /// Retained path cache payload bytes; Arc payloads counted once within this cache.
    pub cpu_path_cache_bytes: u64,
    /// Known CPU backing capacity including streams, unique retained chunks, segment metadata and scratch;
    /// excludes path cache, atlas queue, maps and draw-step scratch.
    pub cpu_frame_capacity_bytes: u64,
    /// CPU atlas upload queue payload and retained byte capacity.
    pub cpu_atlas_upload: MemoryUsage,
    /// Actual live native atlas page texel sizes by kind.
    pub atlas_monochrome_bytes: u64,
    /// Color image atlas page texel sizes.
    pub atlas_color_bytes: u64,
    /// Subpixel glyph atlas page texel sizes.
    pub atlas_subpixel_bytes: u64,
    /// Live atlas cache keys; excludes fallback tiles and pending removals.
    pub atlas_live_keys: usize,
    /// Number of native atlas pages currently owned by this renderer.
    pub atlas_pages: usize,
    /// Tile rectangles plus padding, including fallback and pending removals.
    /// This is not allocator free space: packing fragmentation is excluded.
    pub atlas_tile_bytes: u64,
    /// Depth target logical texel bytes.
    pub depth_bytes: u64,
    /// Path mask target logical texel bytes.
    pub path_target_bytes: u64,
    /// Blur sources, isolated sources and filter/composite targets' logical texel bytes.
    pub blur_target_bytes: u64,
}

impl RendererMemoryProfile {
    /// Atlas logical texel bytes across all page kinds.
    pub fn atlas_bytes(&self) -> u64 {
        self.atlas_monochrome_bytes
            .saturating_add(self.atlas_color_bytes)
            .saturating_add(self.atlas_subpixel_bytes)
    }

    /// Logical GPU sizes owned by this window. Do not add to device totals:
    /// these are a breakdown of allocations already represented there.
    pub fn gpu_bytes(&self) -> u64 {
        self.buffers
            .iter()
            .fold(self.atlas_bytes(), |total, buffer| {
                total.saturating_add(buffer.gpu_capacity_bytes)
            })
            .saturating_add(self.depth_bytes)
            .saturating_add(self.path_target_bytes)
            .saturating_add(self.blur_target_bytes)
    }
}

/// Current driver memory usage and budget for one memory segment.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryBudgetProfile {
    /// Driver-reported usage, which includes resources outside this profile's categories.
    pub usage_bytes: u64,
    /// Driver-reported budget, including a reported zero.
    pub budget_bytes: u64,
    /// Remaining budget, clamped at zero.
    pub headroom_bytes: u64,
    /// Usage exceeding the budget.
    pub over_budget_bytes: u64,
    /// Whole percentage, possibly above 100; None for a zero budget.
    pub utilization_percent: Option<u64>,
}

/// Shared device observations. Resource sizes, allocator counts and residency
/// budgets have different scopes and must not be added together.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceMemoryProfile {
    /// Native buffers allocated for binding alignment, already included in GPU bytes.
    pub binding_mirror_bytes: u64,
    /// Shared device diagnostic identity.
    pub device_id: usize,
    /// Graphics backend label.
    pub backend: String,
    /// Selected physical adapter name.
    pub adapter_name: String,
    /// Age of the last attempt to sample this device.
    pub sample_age_ms: u64,
    /// unavailable, resource-sizes, or allocator; describes the two byte counts below.
    pub accounting: String,
    /// Known allocation bytes; None when the backend has no accounting.
    pub allocated_bytes: Option<u64>,
    /// Known reserved bytes; None when unavailable. Includes live allocations.
    pub reserved_bytes: Option<u64>,
    /// Backend CPU mirror Vec capacity, separate from GPU allocations.
    pub cpu_shadow_bytes: u64,
    /// Upload pages currently occupied by pending data and allocated capacity.
    /// Capacity is already included in device allocation/reservation counts.
    pub upload_ring: MemoryUsage,
    /// Device-local budget; local can be shared system RAM on UMA.
    pub local_budget: Option<MemoryBudgetProfile>,
    /// Non-local budget when the driver reports that segment.
    pub non_local_budget: Option<MemoryBudgetProfile>,
    /// Failed budget query. No budget is reported for a failed attempt.
    pub budget_error: Option<String>,
}

/// Process-wide cached renderer profile. CPU image caches and OS process memory
/// remain separate observations; this is not a complete heap/RSS census.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryProfileSnapshot {
    /// Per-window resource breakdowns.
    pub renderers: Vec<RendererMemoryProfile>,
    /// Each shared device appears exactly once.
    pub devices: Vec<DeviceMemoryProfile>,
}

#[derive(Default)]
struct Registry {
    renderers: BTreeMap<u64, (Instant, RendererMemoryProfile)>,
    devices: BTreeMap<usize, (Instant, DeviceMemoryProfile)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// Reads the last observations published by GPU owners, without querying a
/// device or waking idle windows. Sample ages expose observations' freshness.
pub fn memory_profile_snapshot() -> MemoryProfileSnapshot {
    let registry = registry().lock();
    let now = Instant::now();
    MemoryProfileSnapshot {
        renderers: registry
            .renderers
            .values()
            .map(|(time, sample)| {
                let mut sample = sample.clone();
                sample.sample_age_ms = now
                    .saturating_duration_since(*time)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                sample
            })
            .collect(),
        devices: registry
            .devices
            .values()
            .map(|(time, sample)| {
                let mut sample = sample.clone();
                sample.sample_age_ms = now
                    .saturating_duration_since(*time)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                sample
            })
            .collect(),
    }
}

#[derive(Default)]
pub(crate) struct MemoryProfileTotals {
    pub(crate) atlas_pages: usize,
    pub(crate) windows: usize,
    pub(crate) gpu_bytes: u64,
    pub(crate) mono_bytes: u64,
    pub(crate) color_bytes: u64,
    pub(crate) subpixel_bytes: u64,
    pub(crate) tile_bytes: u64,
    pub(crate) live_keys: usize,
    pub(crate) target_bytes: u64,
}

pub(crate) fn memory_profile_totals() -> MemoryProfileTotals {
    let registry = registry().lock();
    totals_for_registry(&registry)
}

fn totals_for_registry(registry: &Registry) -> MemoryProfileTotals {
    let mut totals = MemoryProfileTotals::default();
    for (_, window) in registry.renderers.values() {
        totals.windows += 1;
        totals.atlas_pages = totals.atlas_pages.saturating_add(window.atlas_pages);
        totals.mono_bytes = totals
            .mono_bytes
            .saturating_add(window.atlas_monochrome_bytes);
        totals.color_bytes = totals.color_bytes.saturating_add(window.atlas_color_bytes);
        totals.subpixel_bytes = totals
            .subpixel_bytes
            .saturating_add(window.atlas_subpixel_bytes);
        totals.tile_bytes = totals.tile_bytes.saturating_add(window.atlas_tile_bytes);
        totals.live_keys = totals.live_keys.saturating_add(window.atlas_live_keys);
        totals.target_bytes = totals
            .target_bytes
            .saturating_add(window.depth_bytes)
            .saturating_add(window.path_target_bytes)
            .saturating_add(window.blur_target_bytes);
    }
    for (_, device) in registry.devices.values() {
        let bytes = device.allocated_bytes.unwrap_or_else(|| {
            registry
                .renderers
                .values()
                .filter(|(_, window)| window.device_id == device.device_id)
                .map(|(_, window)| window.gpu_bytes())
                .sum()
        });
        totals.gpu_bytes = totals.gpu_bytes.saturating_add(bytes);
    }
    totals
}

impl Registry {
    fn remove(&mut self, renderer_id: u64, device_id: usize) {
        self.renderers.remove(&renderer_id);
        if !self
            .renderers
            .values()
            .any(|(_, window)| window.device_id == device_id)
        {
            self.devices.remove(&device_id);
        }
    }
}

pub(crate) struct MemoryProfileRegistration {
    pub(crate) renderer_id: u64,
    pub(crate) device_id: usize,
    last_sample: Option<Instant>,
}

impl MemoryProfileRegistration {
    pub(crate) fn new(device_id: usize) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            renderer_id: NEXT.fetch_add(1, Ordering::Relaxed),
            device_id,
            last_sample: None,
        }
    }

    pub(crate) fn due(&self) -> bool {
        self.last_sample
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
    }

    pub(crate) fn device_due(&self) -> bool {
        registry()
            .lock()
            .devices
            .get(&self.device_id)
            .is_none_or(|(time, _)| time.elapsed() >= Duration::from_secs(1))
    }

    pub(crate) fn publish(
        &mut self,
        mut window: RendererMemoryProfile,
        device: Option<DeviceMemoryProfile>,
    ) {
        let now = Instant::now();
        window.renderer_id = self.renderer_id;
        window.device_id = self.device_id;
        let mut registry = registry().lock();
        registry.renderers.insert(self.renderer_id, (now, window));
        if let Some(mut device) = device {
            device.device_id = self.device_id;
            registry.devices.insert(self.device_id, (now, device));
        }
        self.last_sample = Some(now);
    }

    pub(crate) fn remove(&self) {
        registry().lock().remove(self.renderer_id, self.device_id);
    }
}

impl Drop for MemoryProfileRegistration {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_profile_counts_shared_device_once_and_removes_last_window() {
        let mut registry = Registry::default();
        let now = Instant::now();
        for renderer_id in [1, 2] {
            registry.renderers.insert(
                renderer_id,
                (
                    now,
                    RendererMemoryProfile {
                        renderer_id,
                        device_id: 7,
                        depth_bytes: 10,
                        cpu_frame_capacity_bytes: 50_000,
                        ..Default::default()
                    },
                ),
            );
        }
        registry.devices.insert(
            7,
            (
                now,
                DeviceMemoryProfile {
                    device_id: 7,
                    allocated_bytes: Some(100),
                    cpu_shadow_bytes: 10_000,
                    ..Default::default()
                },
            ),
        );
        assert_eq!(totals_for_registry(&registry).gpu_bytes, 100);
        assert_eq!(totals_for_registry(&registry).target_bytes, 20);
        registry.remove(1, 7);
        assert_eq!(totals_for_registry(&registry).gpu_bytes, 100);
        assert_eq!(registry.devices.len(), 1);
        registry.remove(2, 7);
        assert!(registry.devices.is_empty());
        assert_eq!(totals_for_registry(&registry).gpu_bytes, 0);
    }

    #[test]
    fn memory_profile_unavailable_accounting_uses_only_window_gpu_sizes() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.renderers.insert(
            1,
            (
                now,
                RendererMemoryProfile {
                    device_id: 7,
                    atlas_monochrome_bytes: 512 * 512 * 4,
                    atlas_color_bytes: 4096 * 4096 * 4,
                    cpu_frame_capacity_bytes: 90_000,
                    cpu_path_cache_bytes: 30_000,
                    depth_bytes: 100,
                    path_target_bytes: 200,
                    blur_target_bytes: 300,
                    buffers: vec![BufferMemoryProfile {
                        gpu_capacity_bytes: 400,
                        cpu: MemoryUsage {
                            used_bytes: 1,
                            capacity_bytes: 8000,
                        },
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
        );
        registry.devices.insert(
            7,
            (
                now,
                DeviceMemoryProfile {
                    device_id: 7,
                    ..Default::default()
                },
            ),
        );
        assert_eq!(
            totals_for_registry(&registry).gpu_bytes,
            512 * 512 * 4 + 4096 * 4096 * 4 + 1000
        );
    }
}
