#[cfg(feature = "profiler")]
pub(crate) mod foreground_profiler;
pub(crate) mod gpu_owner;
mod inspector;
mod memory_profile;
pub(crate) mod performance_metrics;
mod process_memory;
pub(crate) mod text;

#[cfg(feature = "profiler")]
pub use foreground_profiler::*;
pub use gpu_owner::*;
pub use inspector::*;
pub use memory_profile::*;
pub use performance_metrics::*;
pub use process_memory::*;
pub use text::{
    TextBackend, TextDurationDistribution, TextOperation, TextOperationMetricsSnapshot,
    text_metrics_snapshot,
};
