use std::{sync::Arc, time::Instant};

use crate::{AnimatedFrame, App, RenderImage, Task};

use super::loader::ImageRenderRequest;

/// Playback and loading values retained by ordinary image elements between frames.
pub(crate) struct ImageElementState {
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(crate) current_frame: Option<AnimatedFrame>,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) started_loading: Option<(Instant, Task<()>)>,
}

/// One element-owned reference to a concrete bounds-aware image request.
///
/// The token itself is intentionally not `Clone`: moving it between pending/current slots transfers
/// ownership without changing the application-wide owner count. Frame retirement performs the final
/// release while it still has access to `App` and the current `Window`.
pub(super) struct SizedImageRequestLease {
    request: ImageRenderRequest,
}

impl SizedImageRequestLease {
    pub(super) fn acquire(request: &ImageRenderRequest, cx: &mut App) -> Self {
        cx.retain_sized_image_element_request(request);
        Self {
            request: request.clone(),
        }
    }

    pub(super) fn request(&self) -> &ImageRenderRequest {
        &self.request
    }

    pub(crate) fn into_request(self) -> ImageRenderRequest {
        self.request
    }
}

/// Deferred release produced when a bounds-aware element state leaves the rendered frame.
pub(crate) struct SizedImageElementRelease {
    pub(crate) request: ImageRenderRequest,
    pub(crate) image: Option<Arc<RenderImage>>,
}

/// Bounds-aware image state is kept separate from ordinary image playback state so switching
/// rendering modes naturally retires the old state at the frame boundary.
pub(crate) struct SizedImageElementState {
    pub(crate) playback: ImageElementState,
    pub(crate) current_image: Option<Arc<RenderImage>>,
    pub(super) sized_image_request: Option<SizedImageRequestLease>,
    pub(super) pending_sized_image_drop: Option<SizedImageRequestLease>,
}

impl SizedImageElementState {
    pub(crate) fn new(current_frame: Option<AnimatedFrame>) -> Self {
        Self {
            playback: ImageElementState {
                current_image: None,
                current_frame,
                next_frame_at: None,
                started_loading: None,
            },
            current_image: None,
            sized_image_request: None,
            pending_sized_image_drop: None,
        }
    }

    /// Moves every live request lease out of this state before the frame storage is cleared.
    pub(crate) fn drain_sized_image_releases(
        &mut self,
        releases: &mut Vec<SizedImageElementRelease>,
    ) {
        if let Some(current) = self.sized_image_request.take() {
            releases.push(SizedImageElementRelease {
                request: current.into_request(),
                image: self.current_image.take(),
            });
        } else {
            self.current_image = None;
        }

        if let Some(pending) = self.pending_sized_image_drop.take() {
            releases.push(SizedImageElementRelease {
                request: pending.into_request(),
                image: None,
            });
        }
    }
}
