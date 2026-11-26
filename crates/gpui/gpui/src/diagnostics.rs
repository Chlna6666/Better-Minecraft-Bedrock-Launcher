#[cfg(feature = "profiler")]
/// Foreground work timing and hang diagnostics.
pub mod foreground_profiler;
/// GPU-owner job and extension-phase diagnostics.
pub mod gpu_owner;
/// Inspector identifiers and reflection support.
pub mod inspector;
/// Memory usage snapshots for renderer resources.
pub mod memory_profile;
/// Window, frame, renderer, and upload performance metrics.
pub mod performance_metrics;
/// Process memory snapshots.
pub mod process_memory;
/// Text operation timing and backend metrics.
pub mod text;

#[cfg(feature = "profiler")]
pub(crate) use foreground_profiler::*;
pub(crate) use inspector::*;
pub(crate) use memory_profile::{
    BufferMemoryProfile, DeviceMemoryProfile, MemoryBudgetProfile, MemoryProfileRegistration,
    MemoryUsage, RendererMemoryProfile, memory_profile_totals,
};
pub(crate) use performance_metrics::*;
