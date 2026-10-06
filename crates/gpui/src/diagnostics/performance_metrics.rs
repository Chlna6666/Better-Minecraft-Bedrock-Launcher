mod allocator;
mod animation;
mod collect;
mod frame;
mod image;
mod layout;
mod renderer;
mod scene;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;
mod timing;
mod upload;
mod window;

pub use allocator::AllocatorBucketMetricsSnapshot;
pub use animation::AnimationMetricsSnapshot;
pub(crate) use animation::{
    record_animation_loop_restart, record_animation_queue_backpressure,
    record_animation_stale_frame_count,
};
pub use collect::performance_metrics_snapshot;
pub use frame::*;
pub use image::*;
pub use layout::*;
pub(crate) use renderer::record_presentation_animation_sample;
pub use renderer::*;
pub use scene::*;
pub use snapshot::*;
pub use upload::*;
pub use window::*;
pub(crate) use window::{
    record_window_active_presentation_attempt,
    record_window_active_presentation_preflight_not_ready, record_window_active_presentation_retry,
    record_window_backend_ready_wake, record_window_native_vsync_wake,
};
