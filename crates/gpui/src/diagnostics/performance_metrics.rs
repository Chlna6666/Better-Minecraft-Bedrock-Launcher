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
/// Per-window metrics and presentation samples.
pub mod window;

pub use allocator::AllocatorBucketMetricsSnapshot;
pub use animation::AnimationMetricsSnapshot;
pub(crate) use animation::{
    record_animation_loop_restart, record_animation_queue_backpressure,
    record_animation_stale_frame_count,
};
pub use collect::snapshot;
pub use frame::*;
pub use image::*;
pub use layout::*;
pub(crate) use renderer::record_presentation_animation_sample;
pub use renderer::*;
pub use scene::*;
pub use snapshot::*;
pub use upload::*;
pub(crate) use window::{
    FrameDisposition, record_present_attempt, record_present_preflight_deferred,
    record_active_presentation_retry, record_backend_ready_wake,
    record_dirty_to_present, record_extension_present,
    record_frame_disposition,
    record_layout_recompute, record_native_vsync_wake,
    record_path_mask, record_redraw_request, record_runtime_state,
    record_visibility,
};
