use std::{sync::Arc, time::Instant};

use crate::{AnimatedFrame, RenderImage, Task};

use super::loader::ImageRenderRequest;

/// Image values retained between element frames.
pub(crate) struct ImageElementState {
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(crate) current_frame: Option<AnimatedFrame>,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) started_loading: Option<(Instant, Task<()>)>,
    pub(super) sized_image_request: Option<ImageRenderRequest>,
    pub(super) pending_sized_image_drop: Option<ImageRenderRequest>,
}
